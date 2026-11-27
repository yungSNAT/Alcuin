use super::*;

fn write_fixture(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

#[cfg(target_os = "linux")]
fn assert_errno(error: &anyhow::Error, expected: i32) {
    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error),
        Some(expected),
        "{error:#}"
    );
}

#[test]
fn creation_defaults_an_empty_slug_and_keeps_api_browser_config() {
    assert_eq!(create_id("🎈 !", Some("")).unwrap(), "session");

    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let error = store
        .create(json!(["PUBLIC-not-an-object"]), home.path(), None)
        .unwrap_err();
    assert_eq!(error.to_string(), "Session options must be an object");
    assert!(!store.root().exists());

    let config = json!({"fixture":"PUBLIC-browser-config"});
    let created = store
        .create(
            json!({"prompt":"🎈 !","mode":"api","browserConfig":config}),
            home.path(),
            None,
        )
        .unwrap();
    assert_eq!(created["id"], "session");
    assert_eq!(created["mode"], "api");
    assert_eq!(created["browser"], json!({"config":config}));
    assert_eq!(
        store.require("session").unwrap()["browser"],
        created["browser"]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn creation_propagates_an_overlong_session_path_after_acquiring_the_lock() {
    let home = tempfile::tempdir().unwrap();
    let canonical_store = SessionStore::new(home.path());
    canonical_store.ensure_storage().unwrap();

    // Lock paths use the canonical root, while mkdir uses the supplied spelling.
    let mut padded_home = home.path().as_os_str().to_os_string();
    let padding = libc::PATH_MAX as usize - padded_home.len() - 32;
    padded_home.push("/".repeat(padding));
    padded_home.push(".");
    let store = SessionStore::new(PathBuf::from(padded_home));
    let id = "PUBLIC-session-path-is-too-long-for-the-padded-home";
    store.transaction(id).unwrap().release().unwrap();

    let error = store
        .create(json!({"prompt":"PUBLIC-path"}), home.path(), Some(id))
        .unwrap_err();
    assert_errno(&error, libc::ENAMETOOLONG);
    assert!(canonical_store.list(true).unwrap().is_empty());
    assert!(!canonical_store.directory(id).unwrap().exists());
}

#[cfg(target_os = "linux")]
#[test]
fn creation_does_not_publish_session_metadata_when_a_model_record_cannot_be_written() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let id = "PUBLIC-model-write";
    let too_long = format!("PUBLIC-{}", "x".repeat(256));
    let error = store
        .create(
            json!({"prompt":"PUBLIC-model-write","models":["PUBLIC-first",too_long]}),
            home.path(),
            Some(id),
        )
        .unwrap_err();
    assert_errno(&error, libc::ENAMETOOLONG);

    let first = read_optional_json(&store.model_path(id, "PUBLIC-first").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(first["status"], "pending");
    assert!(store.read(id).unwrap().is_none());
    let dir = store.directory(id).unwrap();
    assert!(!dir.join("meta.json").exists());
    assert!(!dir.join("output.log").exists());
}

#[test]
fn cancellation_marker_records_the_verified_worker_and_can_be_cleared() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let id = "PUBLIC-worker";
    // Only read our own identity. The request is a file in a synthetic session.
    let worker = ProcessIdentity::read(std::process::id()).unwrap().unwrap();
    store
        .create(json!({"prompt":"PUBLIC-worker"}), home.path(), Some(id))
        .unwrap();
    store
        .update(id, |_| {
            Ok(Some(json!({
                "status":"running",
                "lifecycle":{
                    "detached":true,
                    "workerPid":worker.pid,
                    "workerProcessStartedAtMs":worker.started_at_ms,
                    "processIdentity":worker
                }
            })))
        })
        .unwrap();

    let request = store.cancel_launched_worker(id, &worker).unwrap().unwrap();
    let path = store
        .directory(id)
        .unwrap()
        .join(format!(".cancel-requested-{}", worker.pid));
    assert_eq!(
        read_optional_json(&path).unwrap(),
        Some(json!({"sessionId":id,"processIdentity":worker}))
    );
    assert_eq!(store.stored_for_update(id).unwrap()["status"], "running");
    request.clear().unwrap();
    assert!(!path.exists());
}

#[test]
fn invalid_primary_metadata_falls_back_to_the_legacy_record_and_request() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let id = "PUBLIC-legacy";
    let dir = store.directory(id).unwrap();
    fs::create_dir_all(&dir).unwrap();
    let legacy = json!({"id":id,"status":"completed","marker":"PUBLIC-legacy"});
    write_fixture(&dir.join("session.json"), &legacy);

    for invalid in [
        json!({"status":"completed"}),
        json!({"id":7,"status":"completed"}),
        json!({"id":"PUBLIC-invalid.","status":"completed"}),
        json!({"id":id,"status":""}),
        json!({"id":id,"status":false}),
    ] {
        write_fixture(&dir.join("meta.json"), &invalid);
        assert_eq!(store.read(id).unwrap().as_ref(), Some(&legacy), "{invalid}");
    }

    let options = json!({"prompt":"PUBLIC-request"});
    let request_path = dir.join("request.json");
    for (request, expected) in [
        (
            json!({"id":id,"status":"pending","options":options}),
            Some(options.clone()),
        ),
        (options.clone(), Some(options.clone())),
        (json!({"id":id,"status":"pending"}), None),
    ] {
        write_fixture(&request_path, &request);
        assert_eq!(store.request(id).unwrap(), expected, "{request}");
    }
    fs::write(&request_path, "PUBLIC-not-json").unwrap();
    assert_eq!(store.request(id).unwrap(), None);

    write_fixture(
        &dir.join("meta.json"),
        &json!({"id":id,"status":"pending","options":options}),
    );
    write_fixture(&request_path, &json!({"prompt":"PUBLIC-obsolete-request"}));
    assert_eq!(store.request(id).unwrap(), Some(options));
}

#[test]
fn rejected_session_and_model_updates_leave_the_records_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let id = "PUBLIC-stable";
    let model = "PUBLIC-model";
    store
        .create(
            json!({"prompt":"PUBLIC-stable","model":model}),
            home.path(),
            Some(id),
        )
        .unwrap();
    let meta_path = store.directory(id).unwrap().join("meta.json");
    let model_path = store.model_path(id, model).unwrap();
    let original_meta = fs::read(&meta_path).unwrap();
    let original_model = fs::read(&model_path).unwrap();
    let wrong_id = json!({"id":"PUBLIC-other-id"});

    let error = store
        .update(id, |_| Ok(Some(wrong_id.clone())))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Invalid stored metadata for session {id}.")
    );
    let error = store
        .update_session_and_model(id, model, |_, _| {
            Ok(Some((wrong_id, Some(json!({"status":"completed"})))))
        })
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Invalid stored metadata for session {id}.")
    );
    assert_eq!(fs::read(&meta_path).unwrap(), original_meta);
    assert_eq!(fs::read(&model_path).unwrap(), original_model);

    for invalid_run in [
        json!({"model":"PUBLIC-wrong-model","status":"pending"}),
        json!({"model":model,"status":""}),
        json!({"model":model,"status":12}),
    ] {
        write_fixture(&model_path, &invalid_run);
        let error = store
            .update_model(id, model, &json!({"status":"completed"}))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("Invalid stored model metadata for {model} in session {id}.")
        );
        assert_eq!(read_optional_json(&model_path).unwrap(), Some(invalid_run));
    }
    assert_eq!(fs::read(&meta_path).unwrap(), original_meta);
}

#[test]
fn combined_model_logs_follow_start_times_and_a_blank_filter_returns_all() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let id = "PUBLIC-logs";
    store
        .create(
            json!({"prompt":"PUBLIC-logs","models":["PUBLIC-A","PUBLIC-Z"]}),
            home.path(),
            Some(id),
        )
        .unwrap();
    for (model, start, text) in [
        ("PUBLIC-A", "2026-01-01T00:00:02Z", "PUBLIC-later\n"),
        ("PUBLIC-Z", "2026-01-01T00:00:01Z", "PUBLIC-earlier\n"),
    ] {
        store
            .update_model(id, model, &json!({"startedAt":start}))
            .unwrap();
        store.append_log(id, Some(model), text).unwrap();
    }
    let expected = "=== PUBLIC-Z ===\nPUBLIC-earlier\n\n=== PUBLIC-A ===\nPUBLIC-later";
    assert_eq!(store.log(id).unwrap(), expected);
    assert_eq!(store.log_for_model(id, " \t\n").unwrap(), expected);
}

#[test]
fn session_scans_report_a_non_directory_root_but_skip_an_orphan_folder() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let options = json!({"prompt":"PUBLIC-scan"});
    fs::write(store.root(), "PUBLIC-not-a-directory").unwrap();
    for error in [
        store.list_created_since(true, None).unwrap_err(),
        store.list_same_run(&options).unwrap_err(),
    ] {
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::NotADirectory
        );
    }

    fs::remove_file(store.root()).unwrap();
    let id = "PUBLIC-indexed";
    let created = store
        .create(options.clone(), home.path(), Some(id))
        .unwrap();
    let orphan = "PUBLIC-orphan";
    fs::create_dir(store.directory(orphan).unwrap()).unwrap();
    assert_eq!(store.list_same_run(&options).unwrap(), [created]);
    assert!(!store.run_index().join("covered").join(orphan).exists());
}

#[test]
fn target_scan_leaves_corrupt_records_uncovered_and_falls_back_for_unindexable_targets() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let id = "PUBLIC-target-legacy";
    let target = "PUBLIC-TARGET";
    let dir = store.directory(id).unwrap();
    fs::create_dir_all(&dir).unwrap();
    fs::create_dir(store.root().join("PUBLIC-invalid.")).unwrap();
    let legacy = json!({
        "id":id,
        "status":"completed",
        "browser":{"runtime":{"chromeTargetId":target}}
    });
    write_fixture(&dir.join("session.json"), &legacy);
    fs::write(dir.join("meta.json"), "PUBLIC-DAMAGED not JSON").unwrap();

    assert!(
        store
            .mentions_uncovered(id, "PUBLIC-DAMAGED", false)
            .unwrap()
    );
    assert!(store.mentions_uncovered(id, target, true).unwrap());
    let index = store.target_index();
    assert!(!index.join("covered").join(id).exists());
    assert!(!index.join("targets").join(target).join(id).exists());
    assert_eq!(
        store.list_naming_target(target).unwrap().as_slice(),
        std::slice::from_ref(&legacy)
    );
    assert_eq!(
        store.list_naming_target("PUBLIC/not-indexable").unwrap(),
        [legacy]
    );
}

#[tokio::test(start_paused = true)]
async fn wait_refreshes_pending_metadata_and_returns_the_new_terminal_state() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let id = "PUBLIC-wait";
    store
        .create(json!({"prompt":"PUBLIC-wait"}), home.path(), Some(id))
        .unwrap();
    let cancel = CancellationToken::new();
    let mut waiting = std::pin::pin!(store.wait(id, Some(Duration::from_millis(150)), &cancel));
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());

    store
        .update(id, |_| {
            Ok(Some(
                json!({"status":"completed","marker":"PUBLIC-refreshed"}),
            ))
        })
        .unwrap();
    let (observed, timed_out) = tokio::time::timeout(Duration::from_millis(200), waiting)
        .await
        .unwrap()
        .unwrap();
    assert!(!timed_out);
    assert_eq!(observed["status"], "completed");
    assert_eq!(observed["marker"], "PUBLIC-refreshed");
}

#[test]
fn zombie_reconciliation_requires_a_parseable_start_and_defers_to_a_terminal_model() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let id = "PUBLIC-stale";
    let model = "PUBLIC-model";
    store
        .create(
            json!({"prompt":"PUBLIC-stale","model":model,"zombieTimeoutMs":1}),
            home.path(),
            Some(id),
        )
        .unwrap();
    let running = store
        .update(id, |_| {
            Ok(Some(
                json!({"status":"running","startedAt":"2000-01-01T00:00:00Z"}),
            ))
        })
        .unwrap();
    let reported = store.reported(id, running.clone()).unwrap();
    assert_eq!(reported["status"], "error");
    assert!(
        reported["errorMessage"]
            .as_str()
            .unwrap()
            .starts_with("Session marked as zombie")
    );

    let mut bad_start = running.clone();
    bad_start["startedAt"] = json!("PUBLIC-not-a-timestamp");
    assert_eq!(store.reported(id, bad_start.clone()).unwrap(), bad_start);

    store
        .update_model(id, model, &json!({"status":"completed"}))
        .unwrap();
    assert_eq!(store.reconcile_unverified_zombie(id).unwrap(), running);
    assert_eq!(store.stored_for_update(id).unwrap(), running);
    assert_eq!(
        store.model_for_update(id, model).unwrap().unwrap()["status"],
        "completed"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn hardening_updates_the_open_directory_and_skips_missing_children() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("PUBLIC-original");
    let moved = home.path().join("PUBLIC-moved");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let entry = NativeEntry::open(&path, true).unwrap();
    fs::rename(&path, &moved).unwrap();

    harden_entry(&entry, &path, true).unwrap();
    assert_eq!(
        fs::metadata(&moved).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(!path.exists());

    // The replacement can be enumerated, but its child is absent from the open directory.
    fs::create_dir(&path).unwrap();
    let child = path.join("PUBLIC-replacement-file");
    fs::write(&child, "PUBLIC-unchanged").unwrap();
    fs::set_permissions(&child, fs::Permissions::from_mode(0o644)).unwrap();
    harden_entry(&entry, &path, true).unwrap();

    assert_eq!(
        fs::metadata(&child).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert_eq!(fs::read_to_string(child).unwrap(), "PUBLIC-unchanged");
    assert!(fs::read_dir(moved).unwrap().next().is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn hardening_propagates_a_descriptor_permission_error_without_changing_the_file() {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("PUBLIC-path-only");
    fs::write(&path, "PUBLIC-fixture").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let handle = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH)
        .open(&path)
        .unwrap();
    let entry = NativeEntry::from_handle(handle, false).unwrap();

    let error = harden_entry(&entry, &path, false).unwrap_err();
    assert_errno(&error, libc::EBADF);
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[test]
fn an_invalid_controller_pid_is_not_evidence_that_the_controller_is_gone() {
    assert!(!controller_may_live(&json!({})));
    for pid in [
        Value::Null,
        json!("PUBLIC-not-a-pid"),
        json!(u64::from(u32::MAX) + 1),
    ] {
        assert!(controller_may_live(&json!({"controllerPid":pid})), "{pid}");
    }
}

#[test]
fn browser_merge_updates_warnings_in_place_and_ignores_non_object_updates() {
    let basis = json!({
        "warnings":[
            {"code":"PUBLIC-A","message":"PUBLIC-old"},
            {"code":"PUBLIC-B","message":"PUBLIC-basis"}
        ]
    });
    let current = json!({
        "producer":"PUBLIC-retained",
        "warnings":[
            {"code":"PUBLIC-A","message":"PUBLIC-old"},
            {"code":"PUBLIC-B","message":"PUBLIC-concurrent"}
        ]
    });
    let next = json!({
        "warnings":[
            {"code":"PUBLIC-A","message":"PUBLIC-updated"},
            {"code":"PUBLIC-B","message":"PUBLIC-basis"},
            {"code":"PUBLIC-C","message":"PUBLIC-added"}
        ]
    });
    assert_eq!(
        merge_browser(&current, &next, &basis),
        json!({
            "producer":"PUBLIC-retained",
            "warnings":[
                {"code":"PUBLIC-A","message":"PUBLIC-updated"},
                {"code":"PUBLIC-B","message":"PUBLIC-concurrent"},
                {"code":"PUBLIC-C","message":"PUBLIC-added"}
            ]
        })
    );
    assert_eq!(
        merge_browser(
            &current,
            &json!({"runtime":{"fixture":"PUBLIC-runtime"}}),
            &basis
        ),
        json!({
            "producer":"PUBLIC-retained",
            "warnings":current["warnings"],
            "runtime":{"fixture":"PUBLIC-runtime"}
        })
    );
    for scalar in [Value::Null, json!("PUBLIC-not-an-object"), json!([])] {
        assert_eq!(merge_browser(&current, &scalar, &basis), current);
        assert_eq!(merge_object(&current, &scalar), current);
        assert_eq!(changed_fields(&scalar, &basis), json!({}));
    }
}
