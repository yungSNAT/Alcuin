use super::*;

// Linux's maximum PID limit is exclusive.
const ABSENT_LINUX_PID: u32 = 4_194_304;

fn test_journal(home: &Path) -> TargetJournal {
    TargetJournal::open(&SessionStore::new(home.join(".oracle"))).unwrap()
}

#[test]
fn legacy_creator_death_requires_positive_process_evidence() {
    let current = ProcessIdentity::current().unwrap();
    let cases = [
        (json!(current.pid), Value::Null, false),
        (json!(current.pid), json!(current.started_at_ms), false),
        (json!(current.pid), json!(current.started_at_ms - 1.0), true),
        (json!(current.pid), json!("PUBLIC-not-a-start-time"), false),
        (json!(ABSENT_LINUX_PID), json!(current.started_at_ms), true),
        (json!(u64::MAX), json!(current.started_at_ms), false),
        (
            json!("PUBLIC-not-a-pid"),
            json!(current.started_at_ms),
            false,
        ),
    ];

    for (pid, start, expected_dead) in cases {
        let record = json!({
            "creator": {
                "controllerPid": pid,
                "controllerStartedAtMs": start
            }
        });
        assert_eq!(creator_dead(&record), expected_dead, "{record}");
    }
}

#[tokio::test]
async fn journal_scans_skip_bad_json_and_reconciliation_advances_then_wraps() {
    let home = tempfile::tempdir().unwrap();
    let journal = test_journal(home.path());
    let id = Uuid::new_v4().to_string();
    let name = format!("{id}.json");
    let record_path = journal.file(&json!(id)).unwrap();
    let cursor_path = journal.path.join("reconcile-cursor.json");
    let bad_bytes = b"PUBLIC-invalid-json";
    fs::write(&record_path, bad_bytes).unwrap();

    for deadline in [Instant::now(), Instant::now() + Duration::from_secs(2)] {
        assert!(journal.uncommitted_records(deadline).unwrap().is_empty());
        assert!(
            !cursor_path.exists(),
            "the uncommitted scan must not write a cursor"
        );
        assert_eq!(fs::read(&record_path).unwrap(), bad_bytes);
    }

    for expected_cursor in [name.as_str(), ""] {
        let rows = journal
            .reconcile_until(
                Instant::now() + Duration::from_secs(2),
                &mut |_, _| async { panic!("Unreadable records must not grant a close") },
                &mut |_, _, _| panic!("Unreadable records must not release an admission"),
            )
            .await
            .unwrap();

        assert!(rows.is_empty());
        assert_eq!(
            read_bounded_json(&cursor_path),
            Some(json!({ "after": expected_cursor }))
        );
        assert_eq!(fs::read(&record_path).unwrap(), bad_bytes);
    }
}

#[test]
fn grok_commit_propagates_the_missing_saved_request_error() {
    let home = tempfile::tempdir().unwrap();
    let journal = test_journal(home.path());
    let id = Uuid::new_v4().to_string();
    let session = json!({
        "id": id,
        "browser": {
            "runtime": {
                "provider": "grok",
                "acceptedObservation": { "text": "PUBLIC-private-answer" }
            }
        }
    });
    let Err(request_error) = crate::browser_runner::request(&journal.store, &id) else {
        panic!("The fresh store unexpectedly contains a request for {id}");
    };

    let error = journal.session_commit(&json!({}), &session).unwrap_err();
    assert_eq!(format!("{error:#}"), format!("{request_error:#}"));
}

#[test]
fn gemini_commit_and_verification_refuse_missing_request_and_tampered_evidence() {
    let home = tempfile::tempdir().unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let prepared = crate::browser_runner::alternate_support_tests::prepare_parent(
        home.path(),
        "PUBLIC Gemini disposition source",
        json!({"browserProvider":"gemini","writeOutputPath":null}),
        &cancel,
    )
    .unwrap();
    let journal = TargetJournal::open(&prepared.store).unwrap();
    let missing_id = Uuid::new_v4().to_string();
    let missing = json!({
        "id": missing_id,
        "browser": {
            "runtime": {
                "provider": "gemini",
                "acceptedObservation": { "text": "PUBLIC-temp-answer" }
            }
        }
    });
    let req_err = crate::browser_runner::request(&journal.store, &missing_id).unwrap_err();
    assert_eq!(
        format!(
            "{:#}",
            journal.session_commit(&json!({}), &missing).unwrap_err()
        ),
        format!("{req_err:#}")
    );

    let id = prepared.parent_id.as_str();
    let req = crate::browser_runner::request(&journal.store, id).unwrap();
    let identity = prepared.identity;
    let claim_id = Uuid::new_v4().to_string();
    let creator = journal
        .record_creation(
            &TargetCreation::allocated("TARGET-GEMINI"),
            &json!({"host":"127.0.0.1","port":9222,"targetId":"TARGET-GEMINI","claimId":claim_id}),
            &identity,
            Some(id),
            None,
        )
        .unwrap();
    let answer = "PUBLIC complete Gemini disposition answer";
    let (dispatch, _observed, mut runtime) =
        crate::browser_runner::alternate_support_tests::gemini_evidence(
            &req,
            &identity,
            &claim_id,
            "completed",
            answer,
            true,
        );
    let capture = crate::artifacts::capture(
        &journal.store,
        id,
        "captured-gemini.md",
        answer.as_bytes(),
        &cancel,
    )
    .unwrap();
    runtime["capturedAnswer"] = json!(capture);
    runtime["ownedRecoveryTarget"] = json!({
        "targetId": "TARGET-GEMINI",
        "claimId": claim_id,
        "creator": creator,
    });
    journal
        .store
        .update(id, |_| {
            Ok(Some(json!({
                "status": "completed",
                "completedAt": "PUBLIC completion",
                "dispatch": dispatch,
                "capturedAnswer": capture,
                "browser": {
                    "provider": "gemini",
                    "capturedAnswer": capture,
                    "runtime": runtime,
                }
            })))
        })
        .unwrap();
    let saved = journal.store.require(id).unwrap();

    let mut bad_obs = saved.clone();
    bad_obs["browser"]["runtime"]["acceptedObservation"]["temporaryChat"] = json!(false);
    assert!(
        journal
            .session_commit(&creator, &bad_obs)
            .unwrap_err()
            .to_string()
            .contains("Gemini answer lacks evidence")
    );

    let commit = journal.session_commit(&creator, &saved).unwrap();
    assert_eq!(commit["kind"], "gemini-session");
    assert!(commit.get("conversationId").is_none());
    journal.commit(&creator, &commit).unwrap();
    journal.finalise(&commit).unwrap();
    let record = journal.read(&creator["allocationId"]).unwrap();
    journal.verify_proposed(&record, &saved).unwrap();

    assert!(
        journal
            .verify_proposed(&record, &bad_obs)
            .unwrap_err()
            .to_string()
            .contains("Gemini answer lacks evidence")
    );

    let mut bad_hash = record.clone();
    bad_hash["commit"]["assistantContent"]["sha256"] = json!("0".repeat(64));
    assert_eq!(
        journal
            .verify_proposed(&bad_hash, &saved)
            .unwrap_err()
            .to_string(),
        "Gemini commitment or captured text changed"
    );

    let mut wrong_kind = record;
    wrong_kind["commit"]["kind"] = json!("session");
    wrong_kind["commit"]["conversationId"] = json!("PUBLIC-fake");
    assert_eq!(
        journal
            .verify_proposed(&wrong_kind, &saved)
            .unwrap_err()
            .to_string(),
        "Gemini evidence cannot use another provider commitment kind"
    );
}
