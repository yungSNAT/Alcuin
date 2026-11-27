#![cfg(target_os = "linux")]

use super::*;
use crate::cdp::fake::{self, Reply};

fn absent_process_identity() -> ProcessIdentity {
    let mut identity = ProcessIdentity::current().unwrap();
    // This PID is above Linux's maximum, so the fixture needs no child process.
    identity.pid = 1 << 30;
    assert_eq!(identity.liveness(), Liveness::Dead);
    identity
}

fn completed_session(store: &SessionStore, parent: &Path) -> FreshOwner {
    let profile = parent.join(format!("alcuin-browser-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&profile).unwrap();
    let meta = store
        .create(
            json!({"prompt":"PUBLIC retirement fixture","model":"gpt-6-pro","engine":"browser"}),
            parent,
            None,
        )
        .unwrap();
    let id = meta["id"].as_str().unwrap();
    let controller = ProcessIdentity::current().unwrap();
    let owner = FreshOwner {
        version: 1,
        kind: "fresh-browser-disposal".into(),
        session_id: id.into(),
        controller_identity: controller.clone(),
        profile_identity: NativeEntry::open(&profile, true).unwrap().identity,
        parent_identity: NativeEntry::open(parent, true).unwrap().identity,
        profile,
        process_identity: absent_process_identity(),
        // The saved browser is already dead; nothing connects to this endpoint.
        endpoint: AttachmentEndpoint {
            host: "127.0.0.1".into(),
            port: 9222,
            browser_ws_endpoint: "ws://127.0.0.1:9222/devtools/browser/PUBLIC-retirement".into(),
        },
        startup_targets: BTreeSet::new(),
        launch_sha256: sha256(b"PUBLIC launch fixture"),
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
            Some(id),
            Some(json!({"id":uuid::Uuid::new_v4().to_string(),"profileDir":owner.profile})),
        )
        .unwrap();
    prepare_disposal_fixture(store, id.into(), controller, json!(owner), creator.clone());
    journal
        .record_disposition(
            &creator,
            "confirmed-absent",
            "PUBLIC retirement fixture absence",
        )
        .unwrap();
    owner
}

fn save_observed_processes(store: &SessionStore, id: &str, observed: &[ProcessIdentity]) {
    store
        .update(id, |current| {
            let mut changed = current.clone();
            changed["browser"]["runtime"]["freshProfileDisposal"] =
                json!({"version":1,"disposition":"closing","observedProcesses":observed});
            Ok(Some(changed))
        })
        .unwrap();
}

async fn finish_peer(browser: Cdp, peer: tokio::task::JoinHandle<Vec<Value>>) -> Vec<Value> {
    let _ = browser
        .disconnect_and_wait(Duration::from_millis(200))
        .await;
    tokio::time::timeout(Duration::from_millis(500), peer)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn blank_closure_requires_native_exit_when_post_close_inventory_fails() {
    for exited in [true, false] {
        let identity = if exited {
            absent_process_identity()
        } else {
            ProcessIdentity::current().unwrap()
        };
        let mut inventory_reads = 0;
        let (browser, peer) = fake::peer(Duration::from_secs(1), move |request| {
            match request.method.as_str() {
                "Target.getTargets" => {
                    inventory_reads += 1;
                    if inventory_reads <= 2 {
                        Reply::Result(json!({"targetInfos":[{
                            "targetId":"PUBLIC-blank","type":"page","url":"about:blank"
                        }]}))
                    } else {
                        Reply::Error("PUBLIC post-close inventory denied".into())
                    }
                }
                "Target.closeTarget" => Reply::Result(json!({"success":true})),
                method => panic!("unexpected blank disposal command: {method}"),
            }
        })
        .await;

        let result = close_proven_blanks(
            &browser,
            &BTreeSet::from(["PUBLIC-blank".into()]),
            None,
            &identity,
            Instant::now() + Duration::from_millis(1500),
        )
        .await;
        let trace = finish_peer(browser, peer).await;
        let methods: Vec<_> = trace
            .iter()
            .map(|entry| entry["method"].as_str().unwrap())
            .collect();
        let mut expected = vec![
            "Target.getTargets",
            "Target.getTargets",
            "Target.closeTarget",
        ];

        if exited {
            result.unwrap();
        } else {
            expected.push("Target.getTargets");
            let error = result.unwrap_err();
            // CDP exposes the command refusal, not the endpoint's raw error text.
            assert_eq!(
                error.to_string(),
                "Post-close inventory was unavailable: CDP command Target.getTargets was refused by the endpoint."
            );
            assert!(
                error.chain().any(|cause| {
                    cause.to_string()
                        == "Disposable blank tabs closed, but natural Chrome exit remains unconfirmed"
                }),
                "{error:#}"
            );
            assert_eq!(
                error.root_cause().to_string(),
                "Fresh browser disposal deadline reached"
            );
            assert_eq!(identity.liveness(), Liveness::Alive);
        }
        assert_eq!(methods, expected);
        assert_eq!(trace[2]["params"]["targetId"], "PUBLIC-blank");
    }
}

#[tokio::test]
async fn missing_profile_parent_prevents_disposal_from_reporting_removal() {
    let home = tempfile::tempdir().unwrap();
    let parent = home.path().join("PUBLIC-profiles");
    fs::create_dir(&parent).unwrap();
    let store = SessionStore::new(home.path().join("PUBLIC-state"));
    let owner = completed_session(&store, &parent);
    let saved = store.stored_for_update(&owner.session_id).unwrap();

    fs::remove_dir(&owner.profile).unwrap();
    fs::remove_dir(&parent).unwrap();
    let Err(parent_error) = NativeEntry::open(&parent, true) else {
        panic!("the removed profile parent was opened");
    };

    let error = retire_saved_fresh(&store, &owner.session_id, Duration::from_secs(1))
        .await
        .unwrap_err();
    assert_eq!(format!("{error:#}"), format!("{parent_error:#}"));
    assert!(!parent.exists());
    assert_eq!(store.stored_for_update(&owner.session_id).unwrap(), saved);
}

#[tokio::test]
async fn disposal_waits_for_helper_exit_and_resumes_without_the_launch_record() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join("PUBLIC-state"));
    let owner = completed_session(&store, home.path());
    let id = &owner.session_id;
    let observed = [owner.process_identity.clone()];
    let marker_path = crate::browser_profile::fresh_disposal_marker(&owner.profile).unwrap();
    let marker = json!({
        "version":1,
        "owner":owner,
        "observedProcessesSha256":sha256(&serde_json::to_vec(&observed).unwrap())
    });
    crate::storage::atomic_json(&marker_path, &marker).unwrap();
    let launch = owner.profile.join("oracle-chrome-native.json");
    let launch_content = b"PUBLIC pre-existing launch record";
    fs::write(&launch, launch_content).unwrap();

    let live_helper = ProcessIdentity::current().unwrap();
    save_observed_processes(&store, id, &[observed[0].clone(), live_helper.clone()]);
    let budget = Duration::from_secs(2);
    // A recorded helper that outlives the budget leaves the profile retained, not removed.
    let retained = retire_saved_fresh(&store, id, Duration::from_secs(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained["disposition"], "browser-exited-profile-retained");
    assert!(owner.profile.is_dir());
    assert_eq!(live_helper.liveness(), Liveness::Alive);
    assert_eq!(fs::read(&launch).unwrap().as_slice(), launch_content);
    assert_eq!(read_record(&marker_path).unwrap(), Some(marker.clone()));

    save_observed_processes(&store, id, &observed);
    let interrupted = TEST_DISPOSAL_STOP
        .scope(
            "after-launch-record-removal",
            retire_saved_fresh(&store, id, budget),
        )
        .await
        .unwrap_err();
    assert_eq!(
        interrupted.to_string(),
        "PUBLIC injected interruption after launch record removal"
    );
    assert!(!launch.exists());
    assert!(owner.profile.is_dir());
    assert_eq!(read_record(&marker_path).unwrap(), Some(marker));

    assert_eq!(
        retire_saved_fresh(&store, id, budget).await.unwrap(),
        Some(json!({"version":1,"disposition":"removed","observedProcesses":observed}))
    );
    assert!(!owner.profile.exists());
    assert_eq!(read_record(&marker_path).unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn completed_page_mount_retries_readiness_before_installing_the_claim() {
    for becomes_ready in [true, false] {
        let mut previous_probe = None;
        let (browser, peer) = fake::peer(Duration::from_secs(1), move |request| {
            match request.method.as_str() {
                "Fetch.enable" | "Fetch.disable" | "Page.navigate" => Reply::ok(),
                "Runtime.evaluate" if request.expression().contains("document.readyState") => {
                    let now = tokio::time::Instant::now();
                    let previous = previous_probe.replace(now);
                    if let Some(previous) = previous {
                        assert!(now.duration_since(previous) >= Duration::from_millis(20));
                    }
                    Reply::value(json!(becomes_ready && previous.is_some()))
                }
                "Runtime.evaluate" if request.arg() == json!({"claimId":"PUBLIC-claim"}) => {
                    Reply::value(json!(true))
                }
                _ => panic!("unexpected page fixture command: {}", request.method),
            }
        })
        .await;

        let page = browser.with_session("PUBLIC-page".into());
        let result = tokio::time::timeout(
            Duration::from_secs(6),
            mount_completed_page(&browser, &page, &json!("PUBLIC-claim")),
        )
        .await
        .expect("page mounting did not respect its readiness deadline");
        drop(page);
        let trace = finish_peer(browser, peer).await;
        assert_eq!(trace.first().unwrap()["method"], "Fetch.enable");
        assert_eq!(trace.last().unwrap()["method"], "Fetch.disable");

        let evaluations: Vec<_> = trace
            .iter()
            .filter(|entry| entry["method"] == "Runtime.evaluate")
            .map(|entry| entry["params"]["expression"].as_str().unwrap())
            .collect();
        if becomes_ready {
            result.unwrap();
            let [first, second, install] = evaluations.as_slice() else {
                panic!("expected two readiness probes followed by claim installation");
            };
            assert!(first.contains("document.readyState"));
            assert_eq!(first, second);
            assert!(install.contains("oracle:target-claim"));
            assert!(install.contains("PUBLIC-claim"));
        } else {
            assert_eq!(
                result.unwrap_err().to_string(),
                "Synthetic completed page did not load"
            );
            assert!(evaluations.len() >= 2);
            assert!(
                evaluations
                    .iter()
                    .all(|expression| expression.contains("document.readyState"))
            );
        }
    }
}
