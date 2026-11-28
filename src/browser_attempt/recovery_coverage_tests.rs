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

#[tokio::test]
async fn gemini_recovery_acknowledgement_refuses_wrong_state_concurrent_change_and_reseal() {
    let (_dead_root, dead_store, dead_manifest) =
        recovery_store(crate::browser_runner::tests::dead_provider_attempt());
    let dead_identity: ProcessIdentity = serde_json::from_value(
        dead_store
            .stored_for_update(dead_manifest["parentId"].as_str().unwrap())
            .unwrap()["lifecycle"]["processIdentity"]
            .clone(),
    )
    .unwrap();

    let home = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let mut prepared = crate::browser_runner::alternate_support_tests::prepare_parent(
        home.path(),
        "PUBLIC Gemini recovery guard prompt",
        json!({"browserProvider":"gemini"}),
        &cancel,
    )
    .unwrap();
    let link = prepared
        .coordinator
        .reserve_first_alternate(Provider::Gemini, &prepared.environment, &cancel)
        .unwrap();
    browser_runner::start_owner(&prepared.store, &link.child_id, &prepared.identity).unwrap();
    let store = prepared.store;
    let parent_id = prepared.parent_id;
    let req = browser_runner::request(&store, &link.child_id).unwrap();
    let claim_id = Uuid::new_v4().to_string();
    let journal = crate::browser_disposition::TargetJournal::open(&store).unwrap();
    let created = journal
        .record_creation(
            &crate::browser_disposition::TargetCreation::allocated("TARGET-GEMINI-REC"),
            &json!({"host":"127.0.0.1","port":9222,"targetId":"TARGET-GEMINI-REC","claimId":claim_id}),
            &prepared.identity,
            Some(&link.child_id),
            None,
        )
        .unwrap();
    let mut record = journal.read(&created["allocationId"]).unwrap();
    record["nativeOwnerIdentity"] = json!(dead_identity);
    record["creator"]["controllerPid"] = json!(dead_identity.pid);
    record["creator"]["controllerStartedAtMs"] = json!(dead_identity.started_at_ms);
    let record_path = store.home().join("target-dispositions").join(format!(
        "{}.json",
        created["allocationId"].as_str().unwrap()
    ));
    crate::storage::atomic_json(&record_path, &record).unwrap();
    let creator = record["creator"].clone();
    let (dispatch, observed, mut runtime) =
        crate::browser_runner::alternate_support_tests::gemini_evidence(
            &req,
            &dead_identity,
            &claim_id,
            "started",
            "PUBLIC recovered Gemini answer",
            true,
        );
    runtime["ownedRecoveryTarget"] = json!({
        "targetId": "TARGET-GEMINI-REC",
        "claimId": claim_id,
        "creator": creator,
    });
    let dead_lifecycle = json!({
        "processIdentity": dead_identity,
        "workerPid": dead_identity.pid,
        "workerProcessStartedAtMs": dead_identity.started_at_ms,
    });
    let mut updated_link = json!(link);
    let mut parent_snap = store.stored_for_update(&parent_id).unwrap();
    parent_snap["lifecycle"] = merge_object(&parent_snap["lifecycle"], &dead_lifecycle);
    parent_snap["browser"]["runtime"] = merge_object(
        &parent_snap["browser"]["runtime"],
        &json!({
            "controllerPid": dead_identity.pid,
            "controllerProcessStartedAtMs": dead_identity.started_at_ms,
        }),
    );
    updated_link["parentController"] = controller_identity(&parent_snap);
    let req_path = store
        .directory(&link.child_id)
        .unwrap()
        .join("browser-request.json");
    let mut req_json: Value = serde_json::from_slice(&fs::read(&req_path).unwrap()).unwrap();
    req_json["providerAttempt"] = updated_link.clone();
    crate::storage::atomic_json(&req_path, &req_json).unwrap();
    let new_req_hash =
        request_hash(&browser_runner::request(&store, &link.child_id).unwrap()).unwrap();
    let updated_parent = store
        .update(&parent_id, |current| {
            let mut consultation = current["browser"]["consultation"].clone();
            consultation["active"] = updated_link.clone();
            consultation["attempts"][0] = updated_link.clone();
            consultation["ready"]["link"] = updated_link.clone();
            consultation["ready"]["childRequestSha256"] = json!(new_req_hash);
            Ok(Some(json!({
                "lifecycle": parent_snap["lifecycle"],
                "browser": merge_object(
                    &current["browser"],
                    &json!({
                        "runtime": parent_snap["browser"]["runtime"],
                        "consultation": consultation,
                    }),
                )
            })))
        })
        .unwrap();
    assert_eq!(
        updated_parent["browser"]["consultation"]["active"],
        updated_link
    );
    store
        .update(&link.child_id, |current| {
            let mut attempt_ready = current["browser"]["attemptReady"].clone();
            attempt_ready["link"] = updated_link.clone();
            attempt_ready["childRequestSha256"] = json!(new_req_hash);
            Ok(Some(json!({
                "lifecycle": merge_object(&current["lifecycle"], &dead_lifecycle),
                "dispatch": dispatch,
                "browser": merge_object(
                    &current["browser"],
                    &json!({
                        "provider": "gemini",
                        "attempt": updated_link,
                        "attemptReady": attempt_ready,
                        "runtime": runtime,
                    }),
                )
            })))
        })
        .unwrap();

    let guard = Guard::acquire(&store, &parent_id, &RunnerLimits::default(), &cancel)
        .await
        .unwrap();

    let before = store.stored_for_update(&link.child_id).unwrap();
    let mut stale = before.clone();
    stale["updatedAt"] = json!("PUBLIC-stale-snapshot");
    assert_eq!(
        guard
            .acknowledge_gemini(&stale, &observed)
            .unwrap_err()
            .to_string(),
        "Gemini attempt changed during read-only recovery"
    );
    assert_eq!(
        guard
            .finish_acknowledgement(stale, "PUBLIC-commitment-changed")
            .unwrap_err()
            .to_string(),
        "PUBLIC-commitment-changed"
    );

    let acked = guard.acknowledge_gemini(&before, &observed).unwrap();
    assert_eq!(acked["dispatch"]["state"], "acknowledged");
    assert_eq!(
        guard
            .acknowledge_gemini(&acked, &observed)
            .unwrap_err()
            .to_string(),
        "Gemini recovery cannot replace an existing acknowledgement"
    );
    assert_eq!(
        guard
            .finish_acknowledgement(acked, "PUBLIC-commitment-changed")
            .unwrap_err()
            .to_string(),
        "Recovery commitment was already sealed"
    );
}
