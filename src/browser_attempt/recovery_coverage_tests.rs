use super::*;
use std::{fs, io::ErrorKind};

fn recovery_store(
    (root, manifest): (tempfile::TempDir, Value),
) -> (tempfile::TempDir, SessionStore, Value) {
    let home = std::path::Path::new(manifest["home"].as_str().unwrap());
    (root, SessionStore::new(home.join(".oracle")), manifest)
}

fn write_metadata(store: &SessionStore, id: &str, meta: &Value) {
    crate::storage::atomic_json(&store.directory(id).unwrap().join("meta.json"), meta).unwrap();
}

#[test]
fn malformed_active_link_is_skipped_but_missing_grok_lookup_is_reported() {
    let (_root, store, manifest) =
        recovery_store(crate::browser_runner::tests::dead_provider_attempt());
    let id = manifest["parentId"].as_str().unwrap();
    let mut parent = store.stored_for_update(id).unwrap();
    let path = store.directory(id).unwrap().join("meta.json");

    for active in [Value::Null, json!("PUBLIC-malformed-active-link")] {
        parent["browser"]["consultation"]["active"] = active;
        write_metadata(&store, id, &parent);
        let before = fs::read(&path).unwrap();

        require_dispatched_child(&store, id).unwrap();

        assert_eq!(fs::read(&path).unwrap(), before);
    }

    let missing = "PUBLIC-missing-grok-attempt";
    parent["browser"]["consultation"]["active"] = json!({"provider": "grok", "childId": missing});
    let lookup_error = store.require(missing).unwrap_err();
    let recovery_error = unsent_grok(&store, &parent).unwrap_err();
    assert_eq!(format!("{recovery_error:#}"), format!("{lookup_error:#}"));
}

#[cfg(unix)]
#[test]
fn remote_record_preserves_io_error_for_non_directory_session_path() {
    let (_root, store, manifest) = recovery_store(
        crate::browser_runner::tests::remote_parent_attempt("remote-selected"),
    );
    let id = manifest["parentId"].as_str().unwrap();
    let parent = store.stored_for_update(id).unwrap();
    let run = parent["browser"]["remoteDelivery"]["runId"]
        .as_str()
        .unwrap();
    let directory = store.directory(id).unwrap();
    let original = directory.with_file_name("PUBLIC-original-session");
    fs::rename(&directory, &original).unwrap();
    fs::write(&directory, b"PUBLIC-not-a-directory").unwrap();

    let Err(error) = remote_record(&store, run) else {
        panic!("a non-directory session path must not yield a remote record");
    };
    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind),
        Some(ErrorKind::NotADirectory)
    );
    assert_eq!(fs::read(directory).unwrap(), b"PUBLIC-not-a-directory");
    assert!(original.join("meta.json").is_file());
}

#[tokio::test]
async fn remote_publication_promotes_capture_and_reuses_final_record_without_reissuing() {
    let (_root, store, manifest) = recovery_store(
        crate::browser_runner::tests::remote_parent_attempt("remote-selected"),
    );
    let parent_id = manifest["parentId"].as_str().unwrap();
    let child_id = manifest["childId"].as_str().unwrap();
    let cancel = CancellationToken::new();
    let guard = Guard::acquire(&store, parent_id, &RunnerLimits::default(), &cancel)
        .await
        .unwrap();
    guard.restore(parent_id).unwrap();
    guard.restore(child_id).unwrap();
    guard.admitted.store(true, Ordering::SeqCst);
    let mut parent = Parent::resume_recovery(&store, parent_id, guard.clone(), &cancel).unwrap();
    let before = store.stored_for_update(parent_id).unwrap();
    assert_eq!(before["browser"]["consultation"]["callbackIssued"], false);

    guard.publish_remote(&mut parent).unwrap();

    let after = store.stored_for_update(parent_id).unwrap();
    assert_eq!(after["browser"]["consultation"]["callbackIssued"], true);
    assert_eq!(after["browser"]["remoteDelivery"]["captureIssued"], true);
    let run = after["browser"]["remoteDelivery"]["runId"]
        .as_str()
        .unwrap();
    let record =
        crate::remote_result::read_phase(&store, run, crate::remote_result::Phase::Result).unwrap();
    assert_eq!(record.native()["phase"], "result");
    assert!(!remote_policy(&store, &after, &cancel, guard.deadline()).unwrap());

    let directory = store.directory(parent_id).unwrap();
    let capture_path = directory.join("remote-capture.json");
    let result_path = directory.join("remote-result.json");
    let result_bytes = fs::read(&result_path).unwrap();
    let result_identity = NativeEntry::open(&result_path, false).unwrap().identity;
    match fs::remove_file(&capture_path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => panic!("could not remove the capture fixture: {error}"),
    }

    guard.publish_remote(&mut parent).unwrap();

    assert!(!capture_path.exists());
    assert_eq!(fs::read(&result_path).unwrap(), result_bytes);
    assert_eq!(
        NativeEntry::open(&result_path, false).unwrap().identity,
        result_identity
    );
    assert_eq!(store.stored_for_update(parent_id).unwrap(), after);

    // Completion flags cannot substitute for a durable acknowledgement of the final record.
    let mut claimed_complete = after.clone();
    claimed_complete["status"] = json!("completed");
    claimed_complete["browser"]["remoteDelivery"]["acknowledged"] = json!(true);
    claimed_complete["browser"]["remoteDelivery"]["producerFinished"] = json!(true);
    let Err(missing_ack) = crate::remote_result::require_acknowledged(&store, &record) else {
        panic!("the final record has not been acknowledged");
    };
    let error = remote_policy(&store, &claimed_complete, &cancel, guard.deadline()).unwrap_err();
    assert_eq!(format!("{error:#}"), format!("{missing_ack:#}"));
    assert_eq!(store.stored_for_update(parent_id).unwrap(), after);
}

#[tokio::test]
async fn delivery_rejects_unsealed_turns_and_verifies_the_final_capture_before_sealing() {
    let (_root, store, manifest) =
        recovery_store(crate::browser_runner::tests::dead_provider_attempt());
    let parent_id = manifest["parentId"].as_str().unwrap();
    let child_id = manifest["childId"].as_str().unwrap();
    let cancel = CancellationToken::new();
    let guard = Guard::acquire(&store, parent_id, &RunnerLimits::default(), &cancel)
        .await
        .unwrap();
    let original = store.stored_for_update(child_id).unwrap();
    let images = json!([{"path": "PUBLIC-delivery.png"}]);
    let mut unsealed = original.clone();
    unsealed["dispatch"]["turnCapture"] = json!("PUBLIC-unsealed-capture");
    assert_eq!(
        guard
            .seal_delivery(&unsealed, &original, &images)
            .unwrap_err()
            .to_string(),
        "Recovery delivery differs from its admitted final turn"
    );

    // Model an already sealed turn with no valid capture artifact on disk. Changing only
    // its images should reach artifact verification, but must not install a delivery seal.
    let index = crate::browser_turns::index(&original).unwrap();
    let mut before = original;
    let mut rows = vec![json!({"images": []}); index + 1];
    rows[index]["sourceSha256"] = json!(sha256(b"PUBLIC-original-turn"));
    before["browser"]["runtime"]["completedBrowserTurns"] = json!(rows);
    guard.sealed_turn.set(turn(&before)).unwrap();

    rows[index]["images"] = images.clone();
    let mut changed_source = rows.clone();
    changed_source[index]["sourceSha256"] = json!(sha256(b"PUBLIC-replacement-turn"));
    let cases = [
        (Vec::new(), Some("Recovery delivery changed earlier turns")),
        (
            changed_source,
            Some("Recovery delivery changed the final source or capture"),
        ),
        (rows, None),
    ];

    for (next, expected_message) in cases {
        let mut after = before.clone();
        after["browser"]["runtime"]["completedBrowserTurns"] = json!(next);
        write_metadata(&store, child_id, &after);
        let after = store.stored_for_update(child_id).unwrap();
        let error = guard.seal_delivery(&before, &after, &images).unwrap_err();

        if let Some(message) = expected_message {
            assert_eq!(error.to_string(), message);
        } else {
            let saved = crate::browser_turns::saved(&after).unwrap();
            let verification_error =
                crate::browser_turns::verify_turn(&store, &after, &saved[index], index)
                    .unwrap_err();
            assert_eq!(format!("{error:#}"), format!("{verification_error:#}"));
        }

        assert!(guard.delivered_turn.get().is_none());
        assert_eq!(guard.turn_seal(), &turn(&before));
        assert_eq!(store.stored_for_update(child_id).unwrap(), after);
    }
}

#[tokio::test]
async fn restore_does_not_reopen_a_child_cancelled_after_lock_acquisition() {
    let (_root, store, manifest) =
        recovery_store(crate::browser_runner::tests::dead_provider_attempt());
    let parent_id = manifest["parentId"].as_str().unwrap();
    let child_id = manifest["childId"].as_str().unwrap();
    let cancel = CancellationToken::new();
    let guard = Guard::acquire(&store, parent_id, &RunnerLimits::default(), &cancel)
        .await
        .unwrap();
    let mut child = store.stored_for_update(child_id).unwrap();
    child["status"] = json!("cancelled");
    child["errorMessage"] = json!("PUBLIC-cancelled-child");
    write_metadata(&store, child_id, &child);
    let runs = store.model_runs(child_id).unwrap();

    let error = guard.restore(child_id).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Parent recovery cannot reopen this outcome"
    );
    assert_eq!(store.stored_for_update(child_id).unwrap(), child);
    assert_eq!(store.model_runs(child_id).unwrap(), runs);
}
