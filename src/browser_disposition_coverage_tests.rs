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
