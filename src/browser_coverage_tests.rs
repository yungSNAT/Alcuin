use super::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn disconnect_causes_report_stable_identifiers() {
    for (cause, name) in [
        (
            DisconnectCause::CdpClientDisconnect,
            "cdp-client-disconnect",
        ),
        (DisconnectCause::TargetClosed, "target-closed"),
        (DisconnectCause::LivenessUnknown, "liveness-unknown"),
        (DisconnectCause::ChromeClosed, "chrome-closed"),
    ] {
        assert_eq!(cause.as_str(), name, "{cause:?}");
    }
}

#[test]
fn deep_research_reports_are_judged_by_the_report_gate() {
    let metadata = json!({});
    let observed = json!({"kind": "deep-research-report", "text": "PUBLIC-report"});
    let result = json!({});
    let direct = crate::browser_research::require_report(&metadata, &observed)
        .expect_err("an empty saved request must not satisfy the report gate");
    let error = accept_answer(&metadata, &observed, &result)
        .expect_err("accept_answer must refuse what the report gate refuses");
    assert_eq!(error.to_string(), direct.to_string());
}

/// A discovery endpoint whose `/json/version` answers but whose `/json/list` is unreadable.
async fn discovery_with_broken_list() -> (u16, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>)
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let seen = paths.clone();
    let server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0; 4096];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let path = request.split(' ').nth(1).unwrap_or_default().to_owned();
                seen.lock().unwrap().push(path.clone());
                let (status, body) = if path == "/json/version" {
                    (
                        "200 OK",
                        json!({
                            "Browser": "PUBLIC",
                            "webSocketDebuggerUrl": format!("ws://127.0.0.1:{port}/devtools/browser/PUBLIC")
                        })
                        .to_string(),
                    )
                } else {
                    ("500 Internal Server Error", "PUBLIC-not-json".to_owned())
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (port, paths, server)
}

#[tokio::test]
async fn unreadable_target_list_after_reachable_version_is_liveness_unknown() {
    let (port, paths, server) = discovery_with_broken_list().await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let cause = TEST_DISCOVERY
        .scope(
            format!("http://127.0.0.1:{port}"),
            probe_disconnect("127.0.0.1", port, "PUBLIC-target", &cancel),
        )
        .await;
    server.abort();
    assert_eq!(cause, DisconnectCause::LivenessUnknown);
    assert_eq!(cause.as_str(), "liveness-unknown");
    let paths = paths.lock().unwrap().clone();
    assert_eq!(paths.first().map(String::as_str), Some("/json/version"));
    assert!(paths.iter().any(|p| p == "/json/list"), "{paths:?}");
}
