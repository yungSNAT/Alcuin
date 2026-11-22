use super::*;
use crate::cdp::fake::{Reply, browser};

#[test]
fn recoverable_accepts_only_connection_level_io_failures() {
    use std::io::{Error, ErrorKind};
    for (kind, expected) in [
        (ErrorKind::ConnectionRefused, true),
        (ErrorKind::ConnectionReset, true),
        (ErrorKind::NotConnected, true),
        (ErrorKind::PermissionDenied, false),
        (ErrorKind::TimedOut, false),
        (ErrorKind::InvalidData, false),
    ] {
        let error =
            anyhow::Error::new(Error::new(kind, "PUBLIC-io-failure")).context("PUBLIC-wrapper");
        assert_eq!(recoverable(&error), expected, "{kind:?}");
    }
}

#[tokio::test]
async fn finish_without_recovered_tab_passes_the_harvest_error_through_unchanged() {
    let cancel = CancellationToken::new();
    let fake = browser(json!([]), |request| match request.method.as_str() {
        "Browser.getVersion" => Reply::Result(json!({
            "product": "PUBLIC/1.0",
            "protocolVersion": "1.3",
            "userAgent": "PUBLIC-agent",
            "jsVersion": "PUBLIC"
        })),
        "Target.getTargets" => Reply::Result(json!({"targetInfos": []})),
        _ => Reply::ok(),
    })
    .await;
    let endpoint =
        AttachmentEndpoint::inspect("127.0.0.1", fake.port, Duration::from_secs(2), &cancel)
            .await
            .unwrap();
    let target = InspectionTarget {
        endpoint,
        reference: "PUBLIC-target".into(),
        pinned: false,
        recovered: None,
    };
    assert!(!target.pinned);
    assert_eq!(target.reference, "PUBLIC-target");
    let error = target
        .finish(true, Err(anyhow::anyhow!("PUBLIC-harvest-failure")))
        .await
        .unwrap_err();
    let rendered = format!("{error:#}");
    assert_eq!(rendered, "PUBLIC-harvest-failure");
    assert!(
        !rendered.contains("Recovered observation disposition is unconfirmed"),
        "{rendered}"
    );
}
