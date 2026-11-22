#![cfg(target_os = "linux")]

use super::*;
use crate::cdp::fake::{self, Reply};

fn absent_process_identity() -> ProcessIdentity {
    let mut identity = ProcessIdentity::current().unwrap();
    // This PID is above Linux's maximum; the test does not need to spawn a child.
    identity.pid = 1 << 30;
    assert_eq!(identity.liveness(), Liveness::Dead);
    identity
}

fn new_session(store: &SessionStore, cwd: &Path) -> String {
    store
        .create(
            json!({"prompt":"PUBLIC-fixture prompt","model":"gpt-6-pro","engine":"browser"}),
            cwd,
            None,
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn completed_session(
    store: &SessionStore,
    parent: &Path,
    process_identity: ProcessIdentity,
) -> (FreshOwner, Value) {
    let profile = parent.join(format!("alcuin-browser-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&profile).unwrap();
    let id = new_session(store, parent);
    let controller = ProcessIdentity::current().unwrap();
    let owner = FreshOwner {
        version: 1,
        kind: "fresh-browser-disposal".into(),
        session_id: id.clone(),
        controller_identity: controller.clone(),
        profile_identity: NativeEntry::open(&profile, true).unwrap().identity,
        parent_identity: NativeEntry::open(parent, true).unwrap().identity,
        profile,
        process_identity,
        // These tests never connect to the saved endpoint.
        endpoint: AttachmentEndpoint {
            host: "127.0.0.1".into(),
            port: 9222,
            browser_ws_endpoint: "ws://127.0.0.1:9222/devtools/browser/PUBLIC-fixture".into(),
        },
        startup_targets: BTreeSet::new(),
        launch_sha256: sha256(b"PUBLIC-fixture launch"),
    };
    let journal = crate::browser_disposition::TargetJournal::open(store).unwrap();
    let creator = journal
        .record_creation(
            &crate::browser_disposition::TargetCreation::allocated("PUBLIC-retired"),
            &json!({
                "host":owner.endpoint.host,
                "port":owner.endpoint.port,
                "browserWSEndpoint":owner.endpoint.browser_ws_endpoint,
                "targetId":"PUBLIC-retired",
                "claimId":uuid::Uuid::new_v4().to_string()
            }),
            &controller,
            Some(&id),
            Some(json!({"id":uuid::Uuid::new_v4().to_string(),"profileDir":owner.profile})),
        )
        .unwrap();
    prepare_disposal_fixture(store, id, controller, json!(owner), creator.clone());
    journal
        .record_disposition(&creator, "confirmed-absent", "PUBLIC-fixture absence")
        .unwrap();
    (owner, creator)
}

async fn finish_peer(browser: Cdp, peer: tokio::task::JoinHandle<Vec<Value>>) -> Vec<Value> {
    let _ = browser
        .disconnect_and_wait(Duration::from_millis(250))
        .await;
    tokio::time::timeout(Duration::from_millis(500), peer)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn sessions_without_an_object_owner_do_not_record_survivors_or_retire_profiles() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join("PUBLIC-state"));
    let id = new_session(&store, home.path());
    let record = json!({"creator":{"sessionId":id}});

    for owner in [None, Some(json!("PUBLIC-not-an-owner"))] {
        if let Some(owner) = owner {
            store
                .update(&id, |current| {
                    let mut changed = current.clone();
                    changed["browser"]["runtime"]["freshProfileOwner"] = owner;
                    Ok(Some(changed))
                })
                .unwrap();
        }

        let saved = store.stored_for_update(&id).unwrap();
        record_fresh_survivor(&store, &record, "PUBLIC-survivor", Instant::now()).unwrap();
        assert!(
            retire_saved_fresh(&store, &id, Duration::ZERO)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(store.stored_for_update(&id).unwrap(), saved);
    }
}

#[tokio::test]
async fn malformed_native_identity_and_changed_creator_profile_are_rejected() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join("PUBLIC-state"));
    let (mut owner, creator) = completed_session(&store, home.path(), absent_process_identity());
    let id = owner.session_id.clone();
    let journal = crate::browser_disposition::TargetJournal::open(&store).unwrap();
    let record = journal.read(&creator["allocationId"]).unwrap();
    let saved = store.stored_for_update(&id).unwrap();
    let other_profile = home
        .path()
        .join(format!("alcuin-browser-{}", uuid::Uuid::new_v4()));
    let deadline = Instant::now() + Duration::from_secs(1);

    let mut malformed = record.clone();
    malformed["nativeOwnerIdentity"] = json!("PUBLIC-invalid-identity");
    let error = record_fresh_survivor(&store, &malformed, "PUBLIC-survivor", deadline)
        .unwrap_err()
        .to_string();
    assert!(error.contains("invalid type"), "{error}");
    assert!(error.contains("PUBLIC-invalid-identity"), "{error}");

    let mut wrong_profile = record;
    wrong_profile["creator"]["lease"]["profileDir"] = json!(other_profile);
    let binding_error = "Fresh creator profile belongs to a different directory";
    assert_eq!(
        record_fresh_survivor(&store, &wrong_profile, "PUBLIC-survivor", deadline)
            .unwrap_err()
            .to_string(),
        binding_error
    );
    assert_eq!(store.stored_for_update(&id).unwrap(), saved);

    let original_profile = std::mem::replace(&mut owner.profile, other_profile);
    store
        .update(&id, |current| {
            let mut changed = current.clone();
            changed["browser"]["runtime"]["freshProfileOwner"] = json!(owner);
            Ok(Some(changed))
        })
        .unwrap();
    let saved = store.stored_for_update(&id).unwrap();
    assert_eq!(
        retire_saved_fresh(&store, &id, Duration::from_secs(1))
            .await
            .unwrap_err()
            .to_string(),
        binding_error
    );
    assert_eq!(store.stored_for_update(&id).unwrap(), saved);
    assert!(original_profile.is_dir());
}

#[tokio::test]
async fn process_qualification_retries_incomplete_inventory_without_closing_targets() {
    let home = tempfile::tempdir().unwrap();
    let identity = ProcessIdentity::current().unwrap();
    let mut first = true;
    let (browser, peer) = fake::peer(Duration::from_secs(1), move |_| {
        if std::mem::take(&mut first) {
            Reply::ok()
        } else {
            Reply::Result(json!({"processInfo":[{"id":0,"type":"browser"}]}))
        }
    })
    .await;

    let error = qualify_processes(
        &browser,
        home.path(),
        &identity,
        Instant::now() + Duration::from_secs(2),
    )
    .await
    .unwrap_err()
    .to_string();

    let trace = finish_peer(browser, peer).await;
    assert!(trace.len() >= 2, "incomplete inventory was not retried");
    assert!(
        trace
            .iter()
            .all(|entry| entry["method"] == "SystemInfo.getProcessInfo")
    );
    // The retry may reach the deadline if the executor resumes after the final backoff.
    assert!(
        error == "Fresh Chrome process identity is malformed"
            || error == "Fresh browser disposal deadline reached",
        "{error}"
    );
}

#[tokio::test]
async fn missing_profile_clears_its_marker_only_after_native_exit() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join("PUBLIC-state"));
    let timeout = Duration::from_secs(1);

    for exited in [false, true] {
        let process_identity = if exited {
            absent_process_identity()
        } else {
            ProcessIdentity::current().unwrap()
        };
        let (owner, _) = completed_session(&store, home.path(), process_identity);
        let marker_path = crate::browser_profile::fresh_disposal_marker(&owner.profile).unwrap();
        let marker = json!({"version":1,"owner":owner});
        crate::storage::atomic_json(&marker_path, &marker).unwrap();
        fs::remove_dir(&owner.profile).unwrap();

        let result = retire_saved_fresh(&store, &owner.session_id, timeout).await;
        if exited {
            let removed = Some(json!({"version":1,"disposition":"removed"}));
            assert_eq!(result.unwrap(), removed);
            assert_eq!(read_record(&marker_path).unwrap(), None);
            assert_eq!(
                retire_saved_fresh(&store, &owner.session_id, timeout)
                    .await
                    .unwrap(),
                removed
            );
        } else {
            assert_eq!(
                result.unwrap_err().to_string(),
                "Fresh Chrome remains alive without its profile"
            );
            assert_eq!(read_record(&marker_path).unwrap(), Some(marker));
        }
        assert!(!owner.profile.exists());
    }
}

#[tokio::test]
async fn missing_profile_cannot_clear_a_marker_in_a_replaced_parent() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join("PUBLIC-state"));
    let parent = home.path().join("PUBLIC-profiles");
    fs::create_dir(&parent).unwrap();
    let (owner, _) = completed_session(&store, &parent, absent_process_identity());
    let marker_path = crate::browser_profile::fresh_disposal_marker(&owner.profile).unwrap();
    let marker = json!({"version":1,"owner":owner});

    fs::rename(&parent, home.path().join("PUBLIC-original-parent")).unwrap();
    fs::create_dir(&parent).unwrap();
    crate::storage::atomic_json(&marker_path, &marker).unwrap();

    assert_eq!(
        retire_saved_fresh(&store, &owner.session_id, Duration::from_secs(1))
            .await
            .unwrap_err()
            .to_string(),
        "Fresh profile parent changed"
    );
    assert_eq!(read_record(&marker_path).unwrap(), Some(marker));
}

#[tokio::test]
async fn saved_disposal_retains_profile_until_helper_exit_is_proven() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join("PUBLIC-state"));
    let (owner, _) = completed_session(&store, home.path(), absent_process_identity());
    let observed = [owner.process_identity.clone()];
    let marker_path = crate::browser_profile::fresh_disposal_marker(&owner.profile).unwrap();
    let marker = json!({
        "version":1,
        "owner":owner,
        "observedProcessesSha256":sha256(&serde_json::to_vec(&observed).unwrap())
    });
    crate::storage::atomic_json(&marker_path, &marker).unwrap();
    let fixture_file = owner.profile.join("PUBLIC-profile-data");
    fs::write(&fixture_file, b"PUBLIC-profile content").unwrap();

    let id = &owner.session_id;
    let timeout = Duration::from_secs(1);
    assert_eq!(
        retire_saved_fresh(&store, id, timeout).await.unwrap(),
        Some(json!({
            "version":1,
            "disposition":"browser-exited-profile-retained",
            "observedProcesses":[],
            "reason":"Native renderer/helper absence remains unproved"
        }))
    );
    assert_eq!(
        fs::read(&fixture_file).unwrap().as_slice(),
        b"PUBLIC-profile content"
    );
    assert_eq!(read_record(&marker_path).unwrap(), Some(marker));

    store
        .update(id, |current| {
            let mut changed = current.clone();
            changed["browser"]["runtime"]["freshProfileDisposal"] =
                json!({"version":1,"disposition":"closing","observedProcesses":observed});
            Ok(Some(changed))
        })
        .unwrap();
    assert_eq!(
        retire_saved_fresh(&store, id, timeout).await.unwrap(),
        Some(json!({"version":1,"disposition":"removed","observedProcesses":observed}))
    );
    assert!(!owner.profile.exists());
    assert_eq!(read_record(&marker_path).unwrap(), None);
}

#[tokio::test]
async fn completed_page_fixture_fulfills_only_fetch_events_for_its_session() {
    use base64::{Engine, engine::general_purpose::STANDARD};

    let mut fulfilled = false;
    let (browser, peer) = fake::peer(Duration::from_secs(1), move |request| {
        match request.method.as_str() {
            "Fetch.enable" | "Fetch.disable" => Reply::ok(),
            "Page.navigate" => Reply::WithEvents(
                json!({}),
                vec![
                    json!({"method":"Page.loadEventFired","sessionId":"PUBLIC-page"}),
                    json!({"method":"Fetch.requestPaused","sessionId":"PUBLIC-other",
                        "params":{"requestId":"PUBLIC-foreign-request"}}),
                    json!({"method":"Fetch.requestPaused","sessionId":"PUBLIC-page",
                        "params":{"requestId":"PUBLIC-owned-request"}}),
                ],
            ),
            "Fetch.fulfillRequest" => {
                fulfilled = request.params["requestId"] == "PUBLIC-owned-request";
                Reply::ok()
            }
            "Runtime.evaluate" if request.expression().contains("document.readyState") => {
                Reply::value(json!(fulfilled))
            }
            "Runtime.evaluate" if request.arg() == json!({"claimId":"PUBLIC-claim"}) => {
                Reply::value(json!(true))
            }
            _ => Reply::Error("PUBLIC-unexpected fixture command or argument".into()),
        }
    })
    .await;

    {
        let page = browser.with_session("PUBLIC-page".to_owned());
        tokio::time::timeout(
            Duration::from_secs(2),
            mount_completed_page(&browser, &page, &json!("PUBLIC-claim")),
        )
        .await
        .unwrap()
        .unwrap();
    }

    let trace = finish_peer(browser, peer).await;
    assert_eq!(trace.first().unwrap()["method"], "Fetch.enable");
    assert_eq!(trace.last().unwrap()["method"], "Fetch.disable");
    assert!(
        trace
            .iter()
            .all(|entry| entry["sessionId"] == "PUBLIC-page")
    );

    let navigation = trace
        .iter()
        .find(|entry| entry["method"] == "Page.navigate")
        .unwrap();
    assert_eq!(
        navigation["params"]["url"],
        "https://chatgpt.com/c/PUBLIC-conversation"
    );

    let mut replies = trace
        .iter()
        .filter(|entry| entry["method"] == "Fetch.fulfillRequest");
    let params = &replies.next().unwrap()["params"];
    assert!(replies.next().is_none());
    assert_eq!(params["requestId"], "PUBLIC-owned-request");
    assert_eq!(params["responseCode"], 200);
    assert_eq!(
        params["responseHeaders"],
        json!([{"name":"Content-Type","value":"text/html"}])
    );
    let body =
        String::from_utf8(STANDARD.decode(params["body"].as_str().unwrap()).unwrap()).unwrap();
    assert!(body.contains("PUBLIC answer"));
    assert!(body.contains("connect-src 'none'"));
}
