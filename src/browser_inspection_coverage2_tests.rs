use super::*;
use crate::cdp::fake::{Browser, Reply, browser};

const TARGET_ID: &str = "PUBLIC-inspection-target";
const SETUP_TIMEOUT: Duration = Duration::from_secs(1);

async fn scripted_target(
    evaluate: impl Fn() -> Reply + Copy + Send + 'static,
) -> (InspectionTarget, [Browser; 2]) {
    let page = browser(json!([]), move |request| match request.method.as_str() {
        "Runtime.evaluate" => evaluate(),
        _ => Reply::ok(),
    })
    .await;
    let info = json!({
        "id": TARGET_ID,
        "targetId": TARGET_ID,
        "type": "page",
        "title": "PUBLIC-inspection-page",
        "url": "https://chatgpt.com/c/PUBLIC-inspection-conversation",
        "attached": false,
        "canAccessOpener": false,
        "webSocketDebuggerUrl": format!("ws://127.0.0.1:{}/devtools/page/{TARGET_ID}", page.port),
    });
    let discovery = browser(json!([info.clone()]), move |request| {
        match request.method.as_str() {
            "Browser.getVersion" => Reply::Result(json!({
                "product": "PUBLIC/1.0",
                "protocolVersion": "1.3",
                "userAgent": "PUBLIC-agent",
                "jsVersion": "PUBLIC",
            })),
            "Target.getTargets" => Reply::Result(json!({"targetInfos": [info]})),
            "Target.getTargetInfo" => Reply::Result(json!({"targetInfo": info})),
            "Target.attachToTarget" => {
                Reply::Result(json!({"sessionId": "PUBLIC-inspection-session"}))
            }
            "Runtime.evaluate" => evaluate(),
            _ => Reply::ok(),
        }
    })
    .await;
    let endpoint = AttachmentEndpoint::inspect(
        "127.0.0.1",
        discovery.port,
        SETUP_TIMEOUT,
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    (
        InspectionTarget {
            endpoint,
            reference: TARGET_ID.into(),
            pinned: true,
            recovered: None,
        },
        [discovery, page],
    )
}

fn assert_renderer_was_queried(peers: &[Browser; 2]) {
    assert!(
        peers.iter().any(|peer| {
            peer.trace
                .lock()
                .unwrap()
                .iter()
                .any(|command| command["method"] == "Runtime.evaluate")
        }),
        "inspection failed before querying the renderer"
    );
}

#[tokio::test]
async fn inspect_preserves_the_renderer_exception_for_a_pinned_tab() {
    let (target, peers) =
        scripted_target(|| Reply::thrown("Error: PUBLIC-inspection-renderer-failure")).await;
    let cancel = CancellationToken::new();

    let error = tokio::time::timeout(
        Duration::from_secs(2),
        inspect(&target, false, SETUP_TIMEOUT, &cancel),
    )
    .await
    .expect("inspection did not finish after the renderer replied")
    .unwrap_err();

    assert_renderer_was_queried(&peers);
    assert!(
        format!("{error:#}").contains("PUBLIC-inspection-renderer-failure"),
        "{error:#}"
    );
}

#[tokio::test]
async fn inspect_times_out_a_silent_renderer_without_cancelling_the_caller() {
    let (target, peers) = scripted_target(|| Reply::Silent).await;
    let cancel = CancellationToken::new();

    let error = tokio::time::timeout(
        Duration::from_secs(2),
        inspect(&target, false, Duration::from_millis(180), &cancel),
    )
    .await
    .expect("inspection did not finish after its own deadline")
    .unwrap_err();

    assert_renderer_was_queried(&peers);
    // The command deadline and the enclosing observation deadline can expire on the same tick.
    let diagnostic = format!("{error:#}").to_ascii_lowercase();
    assert!(
        diagnostic.contains("timed out")
            || diagnostic.contains("timeout")
            || diagnostic.contains("deadline reached"),
        "{error:#}"
    );
    assert!(!cancel.is_cancelled());
}
