use super::*;
use crate::cdp::fake::{self, Reply, Request};

const TARGET: &str = "PUBLIC-target";
const CONVERSATION_URL: &str = "https://chatgpt.com/c/PUBLIC-conversation";
const PROMPT: &str = "PUBLIC-prompt";
const ANSWER: &str = "PUBLIC-answer";

fn target_info() -> Value {
    json!({
        "id": TARGET,
        "targetId": TARGET,
        "type": "page",
        "title": "PUBLIC-title",
        "url": CONVERSATION_URL,
        "attached": false
    })
}

fn unfinished_turn_reply(request: &Request) -> Reply {
    match request.method.as_str() {
        "Target.getTargets" => Reply::Result(json!({"targetInfos": [target_info()]})),
        "Target.getTargetInfo" => Reply::Result(json!({"targetInfo": target_info()})),
        "Target.attachToTarget" => Reply::Result(json!({"sessionId": "PUBLIC-observation"})),
        "Runtime.evaluate" => Reply::value(json!({
            "url": CONVERSATION_URL,
            "title": "PUBLIC-title",
            "userCount": 1,
            "assistantCount": 0,
            "lastUserText": PROMPT,
            "lastUserSnippet": PROMPT,
            "stopExists": true,
            "sendExists": false
        })),
        _ => Reply::ok(),
    }
}

#[tokio::test]
async fn harvest_deadline_cancels_an_unfinished_turn_without_persisting_it() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join("store"));
    let meta = store
        .create(
            json!({"prompt": PROMPT, "model": "gpt-6-astra", "mode": "browser"}),
            home.path(),
            None,
        )
        .unwrap();
    let id = meta["id"].as_str().unwrap();
    let before = store.require(id).unwrap();

    // Both browser-level attachment and the advertised page socket stay in the fixture.
    let page = fake::browser(json!([]), unfinished_turn_reply).await;
    let mut target = target_info();
    target["webSocketDebuggerUrl"] = json!(format!(
        "ws://127.0.0.1:{}/devtools/page/{TARGET}",
        page.port
    ));
    let browser = fake::browser(json!([target]), unfinished_turn_reply).await;
    let endpoint = AttachmentEndpoint {
        host: "127.0.0.1".into(),
        port: browser.port,
        browser_ws_endpoint: browser.browser_ws(),
    };
    let cancel = CancellationToken::new();
    let error = tokio::time::timeout(
        Duration::from_secs(3),
        harvest_target_within(
            &store,
            id,
            &endpoint,
            TARGET,
            true,
            &cancel,
            Duration::from_millis(190),
        ),
    )
    .await
    .expect("harvest must return after cancelling its observation")
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Browser harvest timed out after acknowledged observation cleanup"
    );
    assert!(
        error
            .chain()
            .any(|cause| cause.to_string() == "Browser harvest cancelled"),
        "{error:#}"
    );
    assert!(
        browser
            .methods()
            .into_iter()
            .chain(page.methods())
            .any(|method| method == "Runtime.evaluate"),
        "the harvest must observe the unfinished turn before timing out"
    );
    assert!(!cancel.is_cancelled(), "the caller's token must stay live");
    assert_eq!(store.require(id).unwrap(), before);
}

fn set_status<T: From<&'static str>>(status: &mut T, fixture: &'static str) {
    *status = T::from(fixture);
}

#[test]
fn harvest_summary_reports_explicit_mismatch_signals_and_missing_fields() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join("store"));
    let meta = store
        .create(
            json!({"prompt": PROMPT, "model": "gpt-6-astra", "mode": "browser"}),
            home.path(),
            None,
        )
        .unwrap();
    let id = meta["id"].as_str().unwrap();
    let mut tab = json!({
        "targetId": TARGET,
        "conversationId": "PUBLIC-conversation",
        "url": CONVERSATION_URL,
        "state": "completed",
        "currentModelLabel": "PUBLIC-model",
        "assistantCount": 2,
        "stopExists": false,
        "sendExists": true,
        "lastUserText": PROMPT,
        "lastUserSnippet": PROMPT,
        "lastAssistantSnippet": ANSWER,
        "lastAssistantText": ANSWER,
        "lastAssistantMarkdown": ANSWER,
        "assistantText": ANSWER,
        "assistantMarkdown": ANSWER
    });
    let integrity = browser_harvest::persist(&store, id, &tab, true).unwrap();

    // Vary the display fields independently of the captured answer.
    tab["state"] = json!("running");
    tab["stopExists"] = json!(true);
    tab["sendExists"] = json!(false);
    let mut snapshot = HarvestSnapshot {
        endpoint: AttachmentEndpoint {
            host: "127.0.0.1".into(),
            port: 9222,
            browser_ws_endpoint: "ws://127.0.0.1:9222/devtools/browser/PUBLIC".into(),
        },
        tab,
        integrity,
    };
    set_status(&mut snapshot.integrity.status, "mismatch");
    assert!(snapshot.integrity.explicit_target);
    assert_eq!(
        harvest_summary("PUBLIC-session", &snapshot),
        concat!(
            "Session: PUBLIC-session\n",
            "Target: PUBLIC-target\n",
            "State: running\n",
            "Model: PUBLIC-model\n",
            "URL: https://chatgpt.com/c/PUBLIC-conversation\n",
            "Assistant turns: 2\n",
            "Capture identity: mismatch (explicit target)\n",
            "Explicit harvest target differs from the saved capture; original artifacts are unchanged.\n",
            "Signals: stop=yes send=no\n",
            "Last user: PUBLIC-prompt\n",
            "---\n",
        )
    );

    snapshot.tab = json!({"sendExists": true, "lastUserSnippet": ""});
    snapshot.integrity.explicit_target = false;
    set_status(&mut snapshot.integrity.status, "matched");
    assert_eq!(
        harvest_summary("PUBLIC-session", &snapshot),
        concat!(
            "Session: PUBLIC-session\n",
            "Target: \n",
            "State: detached\n",
            "Model: (unknown)\n",
            "URL: \n",
            "Assistant turns: 0\n",
            "Capture identity: matched\n",
            "Signals: stop=no send=yes\n",
            "---\n",
        )
    );
}
