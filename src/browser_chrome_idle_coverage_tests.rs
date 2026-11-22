use super::{IdleStartup, startup_url};
use crate::cdp::fake::{self, Reply};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

const QUERY_TIMEOUT: Duration = Duration::from_millis(500);

#[tokio::test]
async fn pending_startup_binds_when_its_nonce_appears_and_keeps_that_target() {
    let url = startup_url();
    let mut startup = IdleStartup::pending(&url);
    assert_eq!(startup.url, url);
    assert!(startup.target.is_none());

    let worker = json!({
        "targetId": "PUBLIC-provider-worker",
        "type": "service_worker",
        "url": "https://chatgpt.com/PUBLIC-worker.js"
    });
    let mut inventories = [
        json!({"targetInfos": [worker]}),
        json!({"targetInfos": [
            worker,
            {"targetId": "PUBLIC-startup", "type": "page", "url": url}
        ]}),
    ]
    .into_iter();

    let queries = Arc::new(AtomicUsize::new(0));
    let recorded = Arc::clone(&queries);
    let (browser, _peer) = fake::peer(QUERY_TIMEOUT, move |request| {
        recorded.fetch_add(1, Ordering::Relaxed);
        if request.method != "Target.getTargets" {
            return Reply::Error("PUBLIC-expected an inventory query".into());
        }
        inventories.next().map_or_else(
            || Reply::Error("PUBLIC-unexpected repeat inventory query".into()),
            Reply::Result,
        )
    })
    .await;

    let expired = startup.resolve(&browser, Instant::now()).await.unwrap_err();
    assert_eq!(
        expired.to_string(),
        "Persistent browser idle cleanup deadline reached"
    );
    assert!(startup.target.is_none());
    assert_eq!(queries.load(Ordering::Relaxed), 0);

    for (expected_target, expected_queries) in [
        (None, 1),
        (Some("PUBLIC-startup"), 2),
        (Some("PUBLIC-startup"), 2),
    ] {
        startup
            .resolve(&browser, Instant::now() + QUERY_TIMEOUT)
            .await
            .unwrap();
        assert_eq!(startup.target.as_deref(), expected_target);
        assert_eq!(startup.url, url);
        assert_eq!(queries.load(Ordering::Relaxed), expected_queries);
    }
}

#[tokio::test]
async fn pending_startup_rejects_invalid_inventories_without_binding() {
    let url = startup_url();
    let cases = [
        (
            json!({"targetInfos": {}}),
            "Persistent startup inventory is unavailable",
        ),
        (
            json!({"targetInfos": [
                {"targetId": "PUBLIC-first", "type": "page", "url": url},
                {"targetId": "PUBLIC-second", "type": "page", "url": url}
            ]}),
            "Persistent startup nonce is ambiguous",
        ),
        (
            json!({"targetInfos": [{"targetId": null, "type": "page", "url": url}]}),
            "Persistent startup target is invalid",
        ),
        (
            json!({"targetInfos": [{
                "targetId": "PUBLIC-foreign",
                "type": "page",
                "url": "https://example.invalid/PUBLIC-foreign"
            }]}),
            "Persistent Chrome contains a foreign, changed or unretired target",
        ),
    ];

    for (inventory, expected) in cases {
        let (browser, _peer) = fake::peer(QUERY_TIMEOUT, move |request| {
            if request.method == "Target.getTargets" {
                Reply::Result(inventory.clone())
            } else {
                Reply::Error("PUBLIC-expected an inventory query".into())
            }
        })
        .await;
        let mut startup = IdleStartup::pending(&url);

        let error = startup
            .resolve(&browser, Instant::now() + QUERY_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert!(startup.target.is_none(), "{expected}");
        assert_eq!(startup.url, url);
    }
}
