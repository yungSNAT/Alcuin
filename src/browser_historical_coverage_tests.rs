use super::*;
use sha2::{Digest, Sha256};
use std::{fmt::Write as _, fs};

const ABSENT_PID: u32 = 1 << 30; // Above Linux's maximum PID.

fn historical_target_fixture() -> HistoricalTarget {
    HistoricalTarget {
        host: "127.0.0.1".into(),
        port: 9222,
        target_id: "PUBLIC-TARGET".into(),
        browser_ws_endpoint: Some("ws://127.0.0.1:9222/devtools/browser/PUBLIC-browser".into()),
        claim_id: None,
        conversation_id: "PUBLIC-conversation".into(),
    }
}

fn create_record(store: &SessionStore, cwd: &Path, patch: &Value) -> Value {
    let initial = store
        .create(json!({"prompt": "PUBLIC prompt"}), cwd, None)
        .unwrap();
    store
        .update(initial["id"].as_str().unwrap(), |_| Ok(Some(patch.clone())))
        .unwrap()
}

#[test]
fn foreign_controller_requires_the_same_endpoint_and_an_owner_not_proven_dead() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join("PUBLIC-store"));
    let owned = historical_target_fixture();
    let live_pid = std::process::id();
    let own = create_record(
        &store,
        home.path(),
        &json!({"browser": {"runtime": {
            "chromeTargetId": owned.target_id,
            "chromePort": owned.port,
            "chromeHost": owned.host,
            "controllerPid": live_pid
        }}}),
    );
    let id = own["id"].as_str().unwrap();
    assert!(!foreign_controller(&store, id, &owned).unwrap());

    for (port, host, pid) in [
        (owned.port + 1, owned.host.as_str(), live_pid),
        (owned.port, "PUBLIC-other.invalid", live_pid),
        (owned.port, owned.host.as_str(), ABSENT_PID),
    ] {
        create_record(
            &store,
            home.path(),
            &json!({"browser": {"runtime": {
                "chromeTargetId": owned.target_id,
                "chromePort": port,
                "chromeHost": host,
                "controllerPid": pid
            }}}),
        );
        assert!(
            !foreign_controller(&store, id, &owned).unwrap(),
            "unexpected blocker for {host}:{port} with PID {pid}"
        );
    }

    let foreign = create_record(
        &store,
        home.path(),
        &json!({"browser": {"runtime": {
            "chromeTargetId": owned.target_id,
            "chromePort": owned.port,
            "controllerPid": live_pid
        }}}),
    );
    assert!(foreign_controller(&store, id, &owned).unwrap());

    let foreign_id = foreign["id"].as_str().unwrap();
    store
        .update(foreign_id, |current| {
            let mut browser = current["browser"].clone();
            browser["runtime"]["controllerPid"] = json!(ABSENT_PID);
            Ok(Some(json!({"browser": browser})))
        })
        .unwrap();
    assert!(!foreign_controller(&store, id, &owned).unwrap());
}

#[test]
fn owned_guard_accepts_an_unchanged_record_but_rejects_stale_basis_or_terminal_model() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join("PUBLIC-store"));
    let owned = historical_target_fixture();
    let saved = create_record(
        &store,
        home.path(),
        &json!({
            "status": "running",
            "lifecycle": {"workerPid": ABSENT_PID},
            "browser": {"runtime": {
                "chromeTargetId": owned.target_id,
                "chromeHost": owned.host,
                "chromePort": owned.port,
                "chromeBrowserWSEndpoint": owned.browser_ws_endpoint,
                "controllerPid": ABSENT_PID,
                "tabUrl": "https://chatgpt.com/c/PUBLIC-conversation",
                "conversationId": owned.conversation_id,
                "ownedRecoveryTarget": owned
            }}
        }),
    );
    let cancel = CancellationToken::new();
    assert!(guard(&store, &saved, None, &saved, Some(&owned), &cancel).unwrap());

    let mut changed = saved.clone();
    changed["options"]["writeOutputPath"] = json!("PUBLIC-different.md");
    assert!(!guard(&store, &changed, None, &saved, Some(&owned), &cancel).unwrap());

    let model = json!({"status": "completed"});
    assert!(!guard(&store, &saved, Some(&model), &saved, Some(&owned), &cancel).unwrap());
}

fn file_receipt(path: &Path, contents: &[u8]) -> Value {
    let mut sha256 = String::with_capacity(64);
    for byte in &Sha256::digest(contents)[..] {
        write!(sha256, "{byte:02x}").unwrap();
    }
    json!({
        "kind": "file",
        "path": path,
        "sizeBytes": contents.len(),
        "sha256": sha256
    })
}

#[test]
fn completed_outputs_require_intact_files_and_matching_receipts_in_the_export_directory() {
    let home = tempfile::tempdir().unwrap();
    let relative_source = Path::new("PUBLIC-source.txt");
    let source = home.path().join(relative_source);
    let directory = home.path().join("PUBLIC-export");
    fs::create_dir(&directory).unwrap();
    let exported = directory.join(relative_source);
    let contents = b"PUBLIC original";
    fs::write(&source, contents).unwrap();
    fs::write(&exported, contents).unwrap();

    let saved = json!({
        "cwd": home.path(),
        "options": {"writeArtifacts": true},
        "browser": {"requiredTextOutput": {"file": directory.join("PUBLIC-answer.md")}},
        "artifacts": [
            file_receipt(relative_source, contents),
            file_receipt(&exported, contents),
            {"kind": "output"}
        ]
    });
    let cancel = CancellationToken::new();
    let answer = b"PUBLIC answer";
    verify_completed_outputs(&saved, answer, &cancel).unwrap();

    for (pointer, replacement, message) in [
        (
            "/browser/requiredTextOutput/file",
            Value::Null,
            "Historical file export directory is missing",
        ),
        (
            "/artifacts/0/path",
            Value::Null,
            "Historical artifact path is missing",
        ),
        (
            "/artifacts/1/path",
            json!(source),
            "Required historical file export is missing",
        ),
        (
            "/artifacts/1/sizeBytes",
            json!(contents.len() + 1),
            "Required historical file export is missing",
        ),
        (
            "/artifacts/1/sha256",
            json!("PUBLIC-different-checksum"),
            "Required historical file export is missing",
        ),
    ] {
        let mut changed = saved.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        assert_eq!(
            verify_completed_outputs(&changed, answer, &cancel)
                .unwrap_err()
                .to_string(),
            message,
            "{pointer}"
        );
    }

    for path in [&source, &exported] {
        fs::write(path, b"PUBLIC modified").unwrap();
        assert!(
            verify_completed_outputs(&saved, answer, &cancel).is_err(),
            "accepted a same-size modification to {}",
            path.display()
        );
        fs::write(path, contents).unwrap();
    }
    verify_completed_outputs(&saved, answer, &cancel).unwrap();
}
