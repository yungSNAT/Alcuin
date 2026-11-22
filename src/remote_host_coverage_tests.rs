//! Additional coverage for the native remote host: legacy admission, result route edges,
//! receive timeouts on acknowledgements, chunked bodies and small helpers.

use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TOKEN: &str = "PUBLIC coverage token";

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

/// A driver that waits for `gate` when the payload asks for it and captures early on request.
fn driver(gate: Arc<tokio::sync::Notify>) -> RunDriver {
    Arc::new(
        move |context: RunContext| -> BoxFuture<'static, Result<ProducedRun>> {
            let gate = gate.clone();
            Box::pin(async move {
                if context.payload["wait"] == true {
                    gate.notified().await;
                }
                let id = remote_result::session_id(&context.run_id)?;
                context.store.create(
                    json!({"prompt":"PUBLIC remote fixture"}),
                    context.store.home(),
                    Some(&id),
                )?;
                let result = json!({"answerText":"PUBLIC reply","answerMarkdown":"PUBLIC reply"});
                let produced = ProducedRun {
                    captured: result.clone(),
                    result,
                    artifacts: json!([]),
                };
                if context.payload["capture"] == true {
                    context.capture(&produced)?;
                }
                Ok(produced)
            })
        },
    )
}

async fn start_host(
    queued: bool,
    driver: RunDriver,
    receive_timeout: Duration,
) -> (tempfile::TempDir, RemoteHost) {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join(".oracle"));
    let (max_concurrent_runs, max_queued_runs) = if queued {
        (Some(1), Some(1))
    } else {
        (None, None)
    };
    let host = RemoteHost::start_with_timeouts(
        store,
        HostOptions {
            address: "127.0.0.1:0".parse().unwrap(),
            token: TOKEN.into(),
            max_concurrent_runs,
            max_queued_runs,
            max_browser_tabs: 1,
        },
        driver,
        Duration::from_secs(60),
        receive_timeout,
        MAX_RUN_BODY_BYTES,
    )
    .await
    .unwrap();
    (home, host)
}

async fn post(client: &reqwest::Client, url: &str, body: Value) -> reqwest::Response {
    client
        .post(format!("{url}/runs"))
        .bearer_auth(TOKEN)
        .json(&body)
        .send()
        .await
        .unwrap()
}

fn lines(body: &str) -> Vec<Value> {
    body.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn run_id(rows: &[Value]) -> String {
    rows.iter()
        .find(|row| row["type"] == "run-started")
        .and_then(|row| row["runId"].as_str())
        .unwrap_or_else(|| panic!("no run-started row in {rows:?}"))
        .to_owned()
}

/// Reads from `socket` until `needle` appears, the peer closes, or three seconds pass.
async fn read_until(socket: &mut tokio::net::TcpStream, needle: &str) -> String {
    let mut buffer = vec![];
    let mut chunk = [0u8; 4096];
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let read = socket.read(&mut chunk).await.unwrap_or(0);
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if String::from_utf8_lossy(&buffer).contains(needle) {
                break;
            }
        }
    })
    .await;
    String::from_utf8_lossy(&buffer).into_owned()
}

#[test]
fn refusal_errors_display_their_messages_and_survive_downcasting() {
    assert_eq!(
        BodyTooLarge.to_string(),
        "Remote request body exceeds its limit."
    );
    let error: anyhow::Error = BodyTooLarge.into();
    assert!(error.downcast_ref::<BodyTooLarge>().is_some());
    for (status, code) in [(404, "artifact_not_found"), (410, "artifact_unavailable")] {
        let refusal = ArtifactRefusal(status, code);
        assert_eq!(refusal.to_string(), code);
        let error: anyhow::Error = refusal.into();
        let ArtifactRefusal(seen_status, seen_code) =
            error.downcast_ref::<ArtifactRefusal>().copied().unwrap();
        assert_eq!((seen_status, seen_code), (status, code));
    }
}

#[test]
fn artifact_mime_normalises_valid_types_and_rejects_malformed_ones() {
    let cases: &[(Option<&str>, Option<&str>)] = &[
        (Some("Text/Plain; charset=utf-8"), Some("text/plain")),
        (Some("image/svg+xml"), Some("image/svg+xml")),
        (Some("text"), None),
        (Some("text/plain/extra"), None),
        (Some("/plain"), None),
        (Some("text/"), None),
        (None, None),
    ];
    for (input, expected) in cases {
        assert_eq!(
            artifact_mime(*input).as_deref(),
            *expected,
            "input {input:?}"
        );
    }
}

#[test]
fn failed_capture_does_not_mark_the_run_captured() {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join(".oracle"));
    let context = RunContext::fixture(
        "PUBLIC-not-a-run-id",
        json!({"prompt":"PUBLIC prompt"}),
        store,
    );
    let result = json!({"answerText":"PUBLIC reply"});
    let produced = ProducedRun {
        captured: result.clone(),
        result,
        artifacts: json!([]),
    };
    for _ in 0..2 {
        let error = context.capture(&produced).unwrap_err();
        assert!(
            !error.to_string().contains("already saved"),
            "a failed save must not count as a capture: {error:#}"
        );
    }
    assert!(!context.captured.load(Ordering::Acquire));
}

#[tokio::test]
async fn legacy_admission_refuses_overlapping_runs_until_the_running_one_finishes() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let (_home, host) = start_host(false, driver(gate.clone()), Duration::from_secs(300)).await;
    let client = client();
    let url = format!("http://{}", host.address);

    let health: Value = client
        .get(format!("{url}/health"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["admissionMode"], "legacy");
    assert_eq!(health["maxQueuedRuns"], 0);

    let first = post(&client, &url, json!({"prompt":"PUBLIC first","wait":true})).await;
    assert_eq!(first.status(), 200);
    assert_eq!(first.headers()["content-type"], "application/x-ndjson");

    let refused = post(&client, &url, json!({"prompt":"PUBLIC second"})).await;
    assert_eq!(refused.status(), 409);
    assert_eq!(
        refused.json::<Value>().await.unwrap(),
        json!({"error":"busy"})
    );

    gate.notify_one();
    let rows = lines(&first.text().await.unwrap());
    run_id(&rows);
    assert!(rows.iter().all(|row| row["type"] != "error"), "{rows:?}");

    // The busy flag clears once the first run's worker finishes.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let next = post(&client, &url, json!({"prompt":"PUBLIC third"})).await;
        if next.status() == 200 {
            let rows = lines(&next.text().await.unwrap());
            run_id(&rows);
            break;
        }
        assert_eq!(next.status(), 409);
        assert!(tokio::time::Instant::now() < deadline, "busy never cleared");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn result_routes_handle_early_capture_wrong_methods_bad_bodies_and_stalled_acks() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let (_home, host) = start_host(true, driver(gate), Duration::from_millis(400)).await;
    let client = client();
    let url = format!("http://{}", host.address);

    // A driver that already captured is not captured again, and the run completes.
    let response = post(
        &client,
        &url,
        json!({"prompt":"PUBLIC early","capture":true}),
    )
    .await;
    assert_eq!(response.status(), 200);
    let rows = lines(&response.text().await.unwrap());
    assert!(rows.iter().all(|row| row["type"] != "error"), "{rows:?}");
    let run = run_id(&rows);

    let recovered = client
        .get(format!("{url}/runs/{run}/result"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(recovered.status(), 200);
    assert!(recovered.text().await.unwrap().contains("PUBLIC reply"));

    // A result route with the wrong method for its action is not found.
    for (method, action) in [
        (reqwest::Method::GET, "ack"),
        (reqwest::Method::POST, "result"),
    ] {
        let refused = client
            .request(method.clone(), format!("{url}/runs/{run}/{action}"))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 404, "{method} {action}");
        assert!(refused.bytes().await.unwrap().is_empty());
    }

    // A body that is not JSON is an invalid request.
    let invalid = client
        .post(format!("{url}/runs"))
        .bearer_auth(TOKEN)
        .header("Content-Type", "application/json")
        .body("PUBLIC not json")
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), 400);
    assert_eq!(
        invalid.json::<Value>().await.unwrap(),
        json!({"error":"invalid_request"})
    );

    // An acknowledgement whose body stalls past the receive budget gets 408.
    let mut socket = tokio::net::TcpStream::connect(host.address).await.unwrap();
    socket
        .write_all(
            format!(
                "POST /runs/{run}/ack HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: 64\r\n\r\n{{",
                host.address
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let head = read_until(&mut socket, "\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 408"), "{head}");
    drop(socket);

    // A chunked run body with trailers is accepted and runs to completion.
    let body = r#"{"prompt":"PUBLIC chunked"}"#;
    let mut socket = tokio::net::TcpStream::connect(host.address).await.unwrap();
    socket
        .write_all(
            format!(
                "POST /runs HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nTrailer: X-Public-Trailer\r\nConnection: close\r\n\r\n{:x}\r\n{body}\r\n0\r\nX-Public-Trailer: PUBLIC\r\n\r\n",
                host.address,
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let reply = read_until(&mut socket, "\r\n0\r\n\r\n").await;
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.contains("application/x-ndjson"), "{reply}");
    assert!(reply.contains("run-started"), "{reply}");
    assert!(reply.contains("PUBLIC reply"), "{reply}");
    drop(socket);

    host.shutdown().await.unwrap();
}
