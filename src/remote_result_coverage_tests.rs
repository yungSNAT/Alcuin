//! Additional coverage for durable remote records, acknowledgements and their completion paths.

use super::*;

const RUN: &str = "44444444-4444-4444-8444-444444444444";

fn store_with_session() -> (tempfile::TempDir, SessionStore, String) {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join(".oracle"));
    let id = session_id(RUN).unwrap();
    store
        .create(
            json!({"prompt":"PUBLIC coverage prompt"}),
            home.path(),
            Some(&id),
        )
        .unwrap();
    (home, store, id)
}

fn answer() -> Value {
    json!({"answerText":"PUBLIC answer","answerMarkdown":"PUBLIC answer","tookMs":1.0})
}

fn save_phase(store: &SessionStore, result: &Value, phase: Phase) -> Value {
    save(
        store,
        RUN,
        result,
        result,
        &json!([]),
        phase,
        &CancellationToken::new(),
    )
    .unwrap()
}

#[test]
fn acknowledge_rejects_invalid_identity_and_phase_with_clear_messages() {
    let (_home, store, id) = store_with_session();
    for (record, expected) in [
        (
            json!({"runId": RUN, "phase": "PUBLIC-bogus"}),
            "Remote acknowledgement phase is invalid",
        ),
        (
            json!({"runId": RUN}),
            "Remote acknowledgement phase is invalid",
        ),
        (
            json!({"runId": "PUBLIC-not-a-run", "phase": "result"}),
            "Invalid remote run identity",
        ),
    ] {
        let error = acknowledge(&store, &record).unwrap_err();
        assert!(error.to_string().contains(expected), "{record}: {error:#}");
    }
    let dir = store.directory(&id).unwrap();
    assert!(!dir.join("remote-ack.json").exists());
    assert!(!dir.join("remote-capture-ack.json").exists());
}

#[test]
fn acknowledgement_storage_failure_is_reported_instead_of_treated_as_duplicate() {
    let (_home, store, id) = store_with_session();
    let record = save_phase(&store, &answer(), Phase::Result);
    let dir = store.directory(&id).unwrap();
    fs::remove_dir_all(&dir).unwrap();
    let error = acknowledge(&store, &record).unwrap_err();
    assert!(
        !error
            .to_string()
            .contains("Existing remote acknowledgement differs"),
        "{error:#}"
    );
    assert!(!dir.join("remote-ack.json").exists());
}

#[test]
fn falsy_owned_creators_complete_like_unowned_final_results() {
    for creator in [Value::Null, json!(false), json!(""), json!(0)] {
        let (_home, store, id) = store_with_session();
        store
            .update(&id, |_| {
                Ok(Some(
                    json!({"status":"running","browser":{"remoteDelivery":{"runId":RUN}}}),
                ))
            })
            .unwrap();
        let mut result = answer();
        result["ownedRecoveryTarget"] = json!({"creator": creator.clone()});
        let record = save_phase(&store, &result, Phase::Result);
        let (session, committed) = complete_acknowledgement(&store, &record).unwrap();
        assert!(committed, "{creator}");
        assert_eq!(session["status"], "completed", "{creator}");
        assert_eq!(
            session["browser"]["remoteDelivery"],
            json!({"runId":RUN,"acknowledged":true,"producerFinished":true})
        );
        assert!(session["browser"]["capturedAnswer"].is_null(), "{creator}");
        assert!(
            store
                .directory(&id)
                .unwrap()
                .join("remote-ack.json")
                .is_file()
        );
    }
}

#[test]
fn selected_consultation_ack_leaves_cancelled_or_replaced_sessions_untouched() {
    for (status, delivery_run) in [("cancelled", RUN), ("running", "PUBLIC-other-run")] {
        let (_home, store, id) = store_with_session();
        store
            .update(&id, |_| {
                Ok(Some(json!({"status":status,"browser":{
                    "consultation":{"callbackIssued":true},
                    "remoteDelivery":{"runId":delivery_run,"captureIssued":true}}})))
            })
            .unwrap();
        let record = save_phase(&store, &answer(), Phase::Result);
        let meta = store.directory(&id).unwrap().join("meta.json");
        let before = fs::read(&meta).unwrap();
        let (snapshot, committed) = complete_acknowledgement(&store, &record).unwrap();
        assert!(!committed, "{status}");
        assert_eq!(snapshot["status"], status);
        assert_eq!(snapshot["browser"]["remoteDelivery"]["runId"], delivery_run);
        assert_eq!(fs::read(&meta).unwrap(), before);
        assert!(
            store
                .directory(&id)
                .unwrap()
                .join("remote-ack.json")
                .is_file()
        );
    }
}

#[test]
fn promotion_fails_when_the_session_directory_is_replaced_by_a_file() {
    let (_home, store, id) = store_with_session();
    save_phase(&store, &answer(), Phase::Capture);
    let capture = read_phase(&store, RUN, Phase::Capture).unwrap();
    let dir = store.directory(&id).unwrap();
    fs::remove_dir_all(&dir).unwrap();
    fs::write(&dir, b"PUBLIC not a directory").unwrap();
    assert!(promote_capture(&store, &capture, &CancellationToken::new(), None).is_err());
    assert_eq!(fs::read(&dir).unwrap(), b"PUBLIC not a directory");
}

#[cfg(unix)]
#[test]
fn promotion_reports_a_final_record_that_cannot_be_written() {
    use std::os::unix::fs::PermissionsExt;
    let (_home, store, id) = store_with_session();
    save_phase(&store, &answer(), Phase::Capture);
    let capture = read_phase(&store, RUN, Phase::Capture).unwrap();
    let dir = store.directory(&id).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let probe = dir.join("PUBLIC-probe");
    // Privileged users ignore directory permissions; then the failure cannot be provoked.
    let writable = fs::File::create(&probe).is_ok();
    let outcome = if writable {
        None
    } else {
        Some(promote_capture(
            &store,
            &capture,
            &CancellationToken::new(),
            None,
        ))
    };
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    let _ = fs::remove_file(&probe);
    if let Some(outcome) = outcome {
        let Err(error) = outcome else {
            panic!("Promotion wrote a final record into a read-only directory");
        };
        assert!(
            error.chain().any(|cause| cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied)),
            "{error:#}"
        );
        assert!(!dir.join("remote-result.json").exists());
        assert!(dir.join("remote-capture.json").is_file());
    }
}

#[test]
fn owned_commit_refuses_a_session_without_answer_or_identity_and_leaves_journal_uncommitted() {
    use crate::{
        browser_disposition::{TargetCreation, TargetJournal},
        process::ProcessIdentity,
    };
    let (_home, store, id) = store_with_session();
    let journal = TargetJournal::open(&store).unwrap();
    let native = ProcessIdentity::current().unwrap();
    let creator = journal
        .record_creation(
            &TargetCreation::allocated("PUBLIC-target"),
            &json!({"targetId":"PUBLIC-target","host":"127.0.0.1","port":9222,
                "browserWSEndpoint":"PUBLIC invalid endpoint","claimId":Uuid::new_v4().to_string()}),
            &native,
            Some(&id),
            None,
        )
        .unwrap();
    let record = RemoteRecord {
        wire: String::new(),
        native: json!({"runId":RUN,"phase":"result",
            "capturedResult":{"conversationId":"PUBLIC-conversation"},"artifacts":[]}),
    };
    TEST_COMMIT_FAILURES.with(|left| left.set(0));
    for (session, expected) in [
        (
            json!({"id": id, "browser":{"capturedAnswer":{"version":1}}}),
            "Remote answer file is missing",
        ),
        (
            json!({"browser":{"capturedAnswer":{"version":1}}}),
            "Remote session identity is missing",
        ),
    ] {
        let Err(error) = commit_owned(&store, &record, &session, &creator) else {
            panic!("commit_owned accepted an invalid session: {expected}");
        };
        assert!(error.to_string().contains(expected), "{error:#}");
        assert!(
            journal.read(&creator["allocationId"]).unwrap()["commit"].is_null(),
            "{expected}"
        );
    }
}

#[tokio::test]
async fn selected_finish_skips_uncommitted_or_capture_and_warns_on_unsettled_child() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join(".oracle"));
    let record = |phase: &str, captured: Value| RemoteRecord {
        wire: String::new(),
        native: json!({"runId":RUN,"phase":phase,"capturedResult":captured}),
    };
    let consultation =
        json!({"id":"PUBLIC invalid id","browser":{"consultation":{"callbackIssued":true}}});
    for (session, committed, phase, captured) in [
        (consultation.clone(), false, "result", json!({})),
        (consultation.clone(), true, "capture", json!({})),
        (
            json!({"id":"PUBLIC invalid id"}),
            false,
            "result",
            json!({"selectedAttempt":{"index":1}}),
        ),
    ] {
        let acknowledgement = CommittedAcknowledgement {
            session: session.clone(),
            committed,
            owned: None,
        };
        let warning = acknowledgement
            .finish(&store, &record(phase, captured))
            .await
            .unwrap();
        assert!(warning.is_none(), "{session} {committed} {phase}");
    }
    let acknowledgement = CommittedAcknowledgement {
        session: consultation,
        committed: true,
        owned: None,
    };
    let warning = acknowledgement
        .finish(&store, &record("result", json!({})))
        .await
        .unwrap()
        .expect("unsettled selected child is reported");
    assert!(
        warning.starts_with(
            "Remote delivery is saved; selected child retirement remains unconfirmed: "
        ),
        "{warning}"
    );
}

#[tokio::test]
async fn owned_finish_warns_on_out_of_range_port_and_unreachable_endpoint() {
    let (_home, store, id) = store_with_session();
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let cases = [
        (
            json!({}),
            json!({"target":{"host":"127.0.0.1","port":70000,"browserWSEndpoint":"ws://127.0.0.1:1/PUBLIC"}}),
            Some("too large"),
        ),
        (
            json!({"id": id}),
            json!({"target":{"host":"127.0.0.1","port":closed}}),
            None,
        ),
    ];
    for (session, creator, detail) in cases {
        let journal = crate::browser_disposition::TargetJournal::open(&store).unwrap();
        let warning = finish_owned(&store, &session, Ok((journal, creator.clone())))
            .await
            .unwrap()
            .expect("unconfirmed retirement is reported");
        assert!(
            warning
                .starts_with("Remote delivery is saved; target retirement remains unconfirmed: "),
            "{warning}"
        );
        if let Some(detail) = detail {
            assert!(warning.contains(detail), "{warning}");
        }
    }
    assert!(store.require(&id).is_ok());
}

#[cfg(target_os = "linux")]
#[test]
fn native_ack_fixture_stops_when_a_durable_record_already_exists() {
    use crate::{
        browser_disposition::{TargetCreation, TargetJournal},
        process::ProcessIdentity,
    };
    for existing in ["remote-capture.json", "remote-result.json"] {
        let (_home, store, id) = store_with_session();
        let journal = TargetJournal::open(&store).unwrap();
        let native = ProcessIdentity::current().unwrap();
        let creator = journal
            .record_creation(
                &TargetCreation::allocated("PUBLIC-target"),
                &json!({"targetId":"PUBLIC-target","host":"127.0.0.1","port":9222,
                    "browserWSEndpoint":"PUBLIC invalid endpoint","claimId":Uuid::new_v4().to_string()}),
                &native,
                Some(&id),
                None,
            )
            .unwrap();
        let dir = store.directory(&id).unwrap();
        fs::write(dir.join(existing), b"PUBLIC existing record").unwrap();
        assert!(
            native_ack_fixture(&store, RUN, &creator).is_err(),
            "{existing}"
        );
        assert_eq!(
            fs::read(dir.join(existing)).unwrap(),
            b"PUBLIC existing record"
        );
        let capture_acknowledged = dir.join("remote-capture-ack.json").is_file();
        assert_eq!(
            capture_acknowledged,
            existing == "remote-result.json",
            "{existing}"
        );
        if capture_acknowledged {
            assert_eq!(store.require(&id).unwrap()["status"], "partial");
        }
    }
}
