use super::*;
use crate::cdp::fake::{self, Reply};
use std::fs;

const ABSENT_PID: u32 = 1 << 30; // Above Linux's maximum PID.
const CONVERSATION_URL: &str = "https://chatgpt.com/c/PUBLIC-conversation";

fn owned_target(port: u16, endpoint: String) -> HistoricalTarget {
    HistoricalTarget {
        host: "127.0.0.1".into(),
        port,
        target_id: "PUBLIC-TARGET".into(),
        browser_ws_endpoint: Some(endpoint),
        claim_id: None,
        conversation_id: "PUBLIC-conversation".into(),
    }
}

fn runtime_for(target: &HistoricalTarget) -> Value {
    json!({
        "chromeHost": target.host,
        "chromePort": target.port,
        "chromeTargetId": target.target_id,
        "chromeBrowserWSEndpoint": target.browser_ws_endpoint,
        "controllerPid": ABSENT_PID,
        "tabUrl": CONVERSATION_URL,
        "conversationId": target.conversation_id,
        "submittedPromptHash": "PUBLIC-prompt-hash",
        "ownedRecoveryTarget": target
    })
}

#[test]
fn owned_guard_rejects_a_saved_session_that_is_no_longer_accessible() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("PUBLIC-store");
    let store = SessionStore::new(directory.clone());
    let target = owned_target(9222, "ws://127.0.0.1:9222/devtools/browser/PUBLIC".into());
    let initial = store
        .create(json!({"prompt": "PUBLIC prompt"}), temp.path(), None)
        .unwrap();
    let id = initial["id"].as_str().unwrap();
    let saved = store
        .update(id, |_| {
            Ok(Some(json!({
                "mode": "browser",
                "status": "running",
                "lifecycle": {"workerPid": ABSENT_PID},
                "browser": {"runtime": runtime_for(&target)}
            })))
        })
        .unwrap();

    let cancel = CancellationToken::new();
    assert!(guard(&store, &saved, None, &saved, Some(&target), &cancel).unwrap());

    fs::remove_dir_all(&directory).unwrap();
    fs::write(&directory, "PUBLIC not a directory").unwrap();

    // The guard checks the persisted session's authority before scanning for other controllers.
    let error = guard(&store, &saved, None, &saved, Some(&target), &cancel)
        .expect_err("the in-memory record must not substitute for the persisted session");
    assert_eq!(
        error.root_cause().to_string(),
        format!("Session {id:?} not found."),
        "{error:#}"
    );
}

#[tokio::test]
async fn creatorless_retirement_resume_preserves_pending_state_until_inventory_confirms_absence() {
    tokio::time::timeout(Duration::from_secs(4), async {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path().join("PUBLIC-store"));
        let mut inventories = [
            json!({}),
            json!({"targetInfos": [{"type": "page"}]}),
            json!({"targetInfos": [{
                "targetId": "PUBLIC-TARGET",
                "type": "page",
                "url": CONVERSATION_URL
            }]}),
            json!({"targetInfos": [{"targetId": "PUBLIC-OTHER-TARGET", "type": "page"}]}),
        ]
        .into_iter();
        let fake = fake::browser(json!([]), move |request| {
            if request.method == "Target.getTargets" {
                Reply::Result(inventories.next().expect("unexpected inventory request"))
            } else {
                Reply::Error(format!("unexpected command: {}", request.method))
            }
        })
        .await;
        let mut target = owned_target(fake.port, fake.browser_ws());
        target.claim_id = Some("PUBLIC-claim".into());

        let model = "gpt-5.2-pro";
        let initial = store
            .create(
                json!({"prompt": "PUBLIC prompt", "model": model, "mode": "browser"}),
                temp.path(),
                None,
            )
            .unwrap();
        let id = initial["id"].as_str().unwrap();
        let cancel = CancellationToken::new();
        let answer = b"PUBLIC captured answer";
        let capture = artifacts::capture(&store, id, "PUBLIC-answer.md", answer, &cancel).unwrap();
        let completed_at = sessions::timestamp();
        let retirement = json!({
            "version": 1,
            "id": Uuid::new_v4(),
            "binding": {
                "target": target,
                "capture": capture,
                "completedAt": completed_at
            }
        });
        let mut runtime = runtime_for(&target);
        runtime["creatorlessRetirement"] = retirement.clone();
        runtime["targetDispositionPending"] = json!(true);
        store
            .update_session_and_model(id, model, |_, _| {
                Ok(Some((
                    json!({
                        "model": model,
                        "mode": "browser",
                        "status": "completed",
                        "completedAt": completed_at,
                        "response": {"status": "completed"},
                        "lifecycle": {"workerPid": ABSENT_PID},
                        "recoveryRequired": true,
                        "cleanupWarning": "PUBLIC retirement pending",
                        "browser": {"capturedAnswer": capture, "runtime": runtime}
                    }),
                    Some(json!({
                        "status": "completed",
                        "completedAt": completed_at,
                        "response": {"status": "completed"}
                    })),
                )))
            })
            .unwrap();
        let before = store.stored_for_update(id).unwrap();

        for expected in [
            "Retirement target inventory is incomplete",
            "Retirement target inventory has an invalid identity",
        ] {
            let Err(error) = resume_creatorless_absence(&store, id, &cancel).await else {
                panic!("invalid inventory should fail with: {expected}");
            };
            assert_eq!(error.to_string(), expected);
            assert_eq!(store.stored_for_update(id).unwrap(), before);
        }

        assert!(
            resume_creatorless_absence(&store, id, &cancel)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(store.stored_for_update(id).unwrap(), before);

        let completion = resume_creatorless_absence(&store, id, &cancel)
            .await
            .unwrap()
            .expect("the exact original target is absent");
        assert_eq!(
            completion.disposition,
            Some(json!({
                "decision": "retired",
                "absence": "confirmed-absent",
                "reason": "already-absent",
                "retirement": retirement
            }))
        );
        assert_eq!(completion.session, store.stored_for_update(id).unwrap());
        assert_eq!(completion.session["status"], "completed");
        assert_eq!(completion.session["completedAt"], before["completedAt"]);
        assert_eq!(completion.session["recoveryRequired"], false);
        assert!(completion.session["cleanupWarning"].is_null());
        assert_eq!(
            completion.session["browser"]["runtime"]["targetDispositionPending"],
            false
        );
        assert!(
            completion.session["browser"]["runtime"]["targetRetiredAt"]
                .as_str()
                .is_some_and(|timestamp| !timestamp.is_empty())
        );
        assert_eq!(
            completion.session["browser"]["capturedAnswer"],
            before["browser"]["capturedAnswer"]
        );
        assert_eq!(artifacts::verify_capture(&store, &capture).unwrap(), answer);
        assert_eq!(fake.methods(), ["Target.getTargets"; 4]);
    })
    .await
    .expect("loopback retirement recovery timed out");
}
