//! Coverage for the unconfirmed-cleanup branch of `open_on_connection`: when navigation of a fresh
//! observation target fails and the target cannot be closed, the error must state that the exact
//! admission is retained.

use super::*;
use crate::cdp::fake::{Reply, browser};

const URL: &str = "https://chatgpt.com/c/public-recovery-coverage2";
const TARGET: &str = "PUBLIC-observation-target";
const SESSION: &str = "PUBLIC-observation-session";

fn target_info() -> Value {
    json!({
        "targetId": TARGET,
        "type": "page",
        "title": "",
        "url": "about:blank",
        "attached": true,
        "canAccessOpener": false,
        "browserContextId": "PUBLIC-context"
    })
}

#[tokio::test]
async fn navigation_failure_with_refused_close_reports_unconfirmed_cleanup() {
    let startup = tempfile::tempdir().unwrap();
    let (error, methods) = crate::browser_chrome::TEST_STARTUP_LOCK
        .scope(startup.path().join("startup.lock"), async move {
            let mut created = false;
            let fake = browser(json!([]), move |request| match request.method.as_str() {
                "Browser.getVersion" => Reply::Result(json!({
                    "product": "PUBLIC/1.0",
                    "protocolVersion": "1.3",
                    "userAgent": "PUBLIC-agent"
                })),
                "Target.getTargets" => {
                    let infos = if created { vec![target_info()] } else { vec![] };
                    Reply::Result(json!({ "targetInfos": infos }))
                }
                "Target.createTarget" => {
                    created = true;
                    Reply::WithEvents(
                        json!({ "targetId": TARGET }),
                        vec![json!({
                            "method": "Target.targetCreated",
                            "params": { "targetInfo": target_info() }
                        })],
                    )
                }
                "Target.attachToTarget" => Reply::WithEvents(
                    json!({ "sessionId": SESSION }),
                    vec![json!({
                        "method": "Target.attachedToTarget",
                        "params": {
                            "sessionId": SESSION,
                            "targetInfo": target_info(),
                            "waitingForDebugger": false
                        }
                    })],
                ),
                "Target.getTargetInfo" => Reply::Result(json!({ "targetInfo": target_info() })),
                "Page.enable" => Reply::Error("PUBLIC page refused".into()),
                "Target.closeTarget" | "Page.close" => Reply::Error("PUBLIC close refused".into()),
                _ => Reply::ok(),
            })
            .await;
            let cancel = CancellationToken::new();
            let endpoint = AttachmentEndpoint {
                host: "127.0.0.1".into(),
                port: fake.port,
                browser_ws_endpoint: fake.browser_ws(),
            };
            let connection = ChromeConnection::attach(endpoint, Duration::from_secs(2), &cancel)
                .await
                .expect("attach to the scripted browser");
            let profiles = tempfile::tempdir().unwrap();
            let profile_path = profiles.path().join("PUBLIC-profile");
            let profile = ProfileRegistry::open(&profile_path).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            let outcome = open_on_connection(
                connection,
                profile,
                "PUBLIC-session",
                URL,
                &json!({}),
                deadline,
                &cancel,
            )
            .await;
            let Err(error) = outcome else {
                panic!("observation unexpectedly opened");
            };
            (error, fake.methods())
        })
        .await;

    let top = error.to_string();
    assert!(
        top.starts_with("Observation cleanup unconfirmed; exact admission retained: "),
        "{error:#}"
    );
    assert!(
        methods.iter().any(|m| m == "Target.createTarget"),
        "a fresh observation target must have been created: {methods:?}"
    );
    assert!(
        methods.iter().any(|m| m == "Page.enable"),
        "navigation must have started with Page.enable: {methods:?}"
    );
    assert!(
        !methods.iter().any(|m| m == "Page.navigate"),
        "no navigation may follow a refused Page.enable: {methods:?}"
    );
}
