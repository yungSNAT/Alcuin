//! Coverage for the failure branches of `open_on_connection`: admission, target acquisition and
//! navigation failures must each release the browser transport and report a precise error.

use super::*;
use crate::cdp::fake::{Reply, Request, browser};

const URL: &str = "https://chatgpt.com/c/public-recovery-coverage";

fn version() -> Reply {
    Reply::Result(json!({
        "product": "PUBLIC/1.0",
        "protocolVersion": "1.3",
        "userAgent": "PUBLIC-agent"
    }))
}

/// Attach to a scripted loopback browser and run one observation that is expected to fail.
/// Returns the error and every CDP method the browser received.
async fn failed_observation<F>(budget: Duration, respond: F) -> (anyhow::Error, Vec<String>)
where
    F: FnMut(&Request) -> Reply + Send + 'static,
{
    let startup = tempfile::tempdir().unwrap();
    crate::browser_chrome::TEST_STARTUP_LOCK
        .scope(startup.path().join("startup.lock"), async move {
            let fake = browser(json!([]), respond).await;
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
            let deadline = Instant::now() + budget;
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
        .await
}

#[tokio::test]
async fn admission_deadline_failure_leaves_connection_and_reports_deadline() {
    let (error, methods) =
        failed_observation(Duration::ZERO, |request| match request.method.as_str() {
            "Target.getTargets" => Reply::Result(json!({"targetInfos": []})),
            "Browser.getVersion" => version(),
            _ => Reply::ok(),
        })
        .await;
    assert_eq!(error.to_string(), "Conversation reopening deadline reached");
    assert!(
        !methods.iter().any(|m| m == "Target.createTarget"),
        "no observation target may be created without admission: {methods:?}"
    );
}

#[tokio::test]
async fn target_acquisition_failure_is_returned_after_releasing_admission() {
    let (error, methods) = failed_observation(Duration::from_secs(2), |request| {
        match request.method.as_str() {
            "Target.getTargets" => Reply::Result(json!({"targetInfos": []})),
            "Browser.getVersion" => version(),
            _ => Reply::Error("PUBLIC browser refused".into()),
        }
    })
    .await;
    let chain = format!("{error:#}");
    assert!(chain.contains("Target.createTarget was refused"), "{chain}");
    assert!(
        !methods.iter().any(|m| m == "Page.navigate"),
        "no navigation may follow a failed acquisition: {methods:?}"
    );
}
