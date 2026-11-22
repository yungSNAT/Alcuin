use super::*;
use bytes::Bytes;
use http_body_util::Full;
use hyper::{body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use std::{convert::Infallible, future::Future, sync::Mutex};
use tokio::{net::TcpListener, task::JoinHandle};

const RUN: &str = "12345678-1234-1234-1234-123456789abc";
const ARTIFACT: &str = "12345678-1234-1234-1234-123456789abd";

struct Host {
    address: String,
    seen: Arc<Mutex<Vec<String>>>,
    worker: JoinHandle<()>,
}

impl Host {
    async fn start(respond: impl Fn(&str) -> (u16, Vec<u8>) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        let respond = Arc::new(respond);
        let worker = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let recorded = recorded.clone();
                let respond = respond.clone();
                let service = service_fn(move |request: hyper::Request<Incoming>| {
                    let line = format!("{} {}", request.method(), request.uri().path());
                    let (status, body) = respond(&line);
                    recorded.lock().unwrap().push(line);
                    std::future::ready(Ok::<_, Infallible>(
                        hyper::Response::builder()
                            .status(status)
                            .body(Full::new(Bytes::from(body)))
                            .unwrap(),
                    ))
                });
                let _ = http1::Builder::new()
                    .keep_alive(false)
                    .serve_connection(TokioIo::new(socket), service)
                    .await;
            }
        });
        Self {
            address,
            seen,
            worker,
        }
    }

    fn client(&self, home: &tempfile::TempDir) -> RemoteClient {
        RemoteClient::new(&self.address, None, SessionStore::new(home.path())).unwrap()
    }

    fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

async fn bounded<T>(operation: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(1), operation)
        .await
        .expect("loopback fixture did not finish")
}

fn request(recovery: bool) -> Request {
    Request {
        payload: json!({"prompt": "PUBLIC-fixture", "options": {"model": "gpt-6-pro"}}),
        recover_run_id: recovery.then(|| RUN.to_owned()),
        session_id: Some("public-receiver".into()),
        image_destination: None,
        cancel: CancellationToken::new(),
        require_cancellation: false,
        verbose: false,
    }
}

fn event_body(events: &[Value]) -> Vec<u8> {
    let mut body = Vec::new();
    for event in events {
        serde_json::to_writer(&mut body, event).unwrap();
        body.push(b'\n');
    }
    body
}

fn delivery_event(result: &Value, wrong_digest: bool) -> Value {
    let wire = json!({"runId": RUN, "result": result, "artifacts": []}).to_string();
    let digest = if wrong_digest {
        "0".repeat(64)
    } else {
        crate::artifacts::sha256(&remote_json::stringify(&wire).unwrap())
    };
    json!({
        "type": "result",
        "result": result,
        "delivery": {"version": 1, "runId": RUN, "sha256": digest}
    })
}

#[tokio::test]
async fn health_reports_missing_endpoint_and_run_errors_preserve_response_bodies() {
    let home = tempfile::tempdir().unwrap();
    for (body, expected) in [
        (b"{\"error\":17}".as_slice(), "{\"error\":17}"),
        (
            b"PUBLIC-rejected \xff\n".as_slice(),
            "PUBLIC-rejected \u{fffd}\n",
        ),
        (
            b"".as_slice(),
            "Remote host responded with status 503 Service Unavailable",
        ),
    ] {
        let host = Host::start(move |line| {
            if line == "GET /health" {
                (404, br#"{"error":"PUBLIC-ignored"}"#.to_vec())
            } else {
                (503, body.to_vec())
            }
        })
        .await;
        let client = host.client(&home);
        let request = request(true);
        if body.is_empty() {
            assert_eq!(
                bounded(client.health(&request.cancel)).await.unwrap(),
                json!({
                    "ok": false,
                    "statusCode": 404,
                    "error": "remote host does not expose /health (upgrade oracle on the host and retry)"
                })
            );
        }
        let error = bounded(client.run(request, Hooks::default()))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
        let mut expected_requests = Vec::new();
        if body.is_empty() {
            expected_requests.push("GET /health".to_owned());
        }
        expected_requests.push(format!("GET /runs/{RUN}/result"));
        assert_eq!(host.requests(), expected_requests);
    }
}

#[tokio::test]
async fn new_runs_validate_the_receiver_and_apply_model_policy_before_and_after_http() {
    let home = tempfile::tempdir().unwrap();
    let result = json!({"answerText": "PUBLIC-answer", "effectiveModel": "gpt-4o"});
    let body = event_body(&[json!({"type": "result", "result": result})]);
    let host = Host::start(move |_| (200, body.clone())).await;
    let client = host.client(&home);

    let invalid_id = "../PUBLIC-invalid";
    let validation_error = crate::sessions::assert_id(invalid_id).unwrap_err();
    let mut invalid = request(false);
    invalid.session_id = Some(invalid_id.into());
    let error = bounded(client.run(invalid, Hooks::default()))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), validation_error.to_string());
    assert!(host.requests().is_empty());

    let alternative = [
        "gpt-4o",
        "gpt-6",
        "gpt-6-mini",
        "auto",
        "PUBLIC-unknown-model",
    ]
    .into_iter()
    .find(|model| policy::assert_model_allowed(Some(*model)).is_err())
    .unwrap_or("gpt-4o");
    let mut sent = 0;
    for desired_model in ["gpt-6-pro", alternative] {
        let mut request = request(false);
        request.payload["browserConfig"] = json!({"desiredModel": desired_model});
        let expected = policy::assert_model_allowed(Some(desired_model)).and_then(|_| {
            sent += 1;
            policy::assert_browser_completion(None, None, Some("gpt-4o"))?;
            Ok(result.clone())
        });
        let actual = bounded(client.run(request, Hooks::default())).await;
        assert_eq!(
            actual.map_err(|error| error.to_string()),
            expected.map_err(|error| error.to_string())
        );
        assert_eq!(host.requests(), vec!["POST /runs".to_owned(); sent]);
    }
}

#[tokio::test]
async fn event_stream_logs_messages_and_retains_invalid_artifact_warnings() {
    let home = tempfile::tempdir().unwrap();
    let filename = "PUBLIC-unregistered.zip";
    let body = event_body(&[
        json!({"type": "log", "message": "PUBLIC-host-log"}),
        json!({"type": "log", "message": 17}),
        json!({"type": "PUBLIC-unknown", "message": "PUBLIC-do-not-log"}),
        json!({
            "type": "artifact-progress", "artifactId": ARTIFACT, "phase": "PUBLIC-copying",
            "receivedBytes": null, "totalBytes": "PUBLIC-total"
        }),
        json!({"type": "artifact-progress", "artifactId": ARTIFACT, "phase": "PUBLIC-done"}),
        json!({"type": "artifact-ready", "artifact": {"filename": filename}}),
        json!({
            "type": "result",
            "result": {"answerText": "PUBLIC-answer", "warnings": [{"code": "PUBLIC-original"}]}
        }),
    ]);
    let host = Host::start(move |_| (200, body.clone())).await;
    let client = host.client(&home);
    let display_filename = crate::browser_output::filename(filename);
    for verbose in [true, false] {
        let mut request = request(true);
        request.verbose = verbose;
        let messages = Arc::new(Mutex::new(Vec::new()));
        let recorded = messages.clone();
        let hooks = Hooks {
            log: Some(Arc::new(move |message| {
                recorded.lock().unwrap().push(message.to_owned());
                Ok(())
            })),
            ..Hooks::default()
        };
        let result = bounded(client.run(request, hooks)).await.unwrap();
        assert_eq!(result["answerText"], "PUBLIC-answer");
        assert!(result.get("artifacts").is_none());
        let warnings = result["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 2);
        assert_eq!(warnings[0]["code"], "PUBLIC-original");
        let warning = &warnings[1];
        assert_eq!(warning["code"], "remote-artifact-transfer-failed");
        assert_eq!(warning["severity"], "warning");
        assert!(warning.get("details").is_none());
        let message = warning["message"].as_str().unwrap();
        assert!(
            message.contains(&format!("transfer failed for {display_filename}.")),
            "{message}"
        );
        assert!(!message.split_once("Reason: ").unwrap().1.is_empty());

        let mut expected = vec!["PUBLIC-host-log".to_owned(), String::new()];
        if verbose {
            expected.extend([
                format!("[browser] Artifact {ARTIFACT} PUBLIC-copying null/PUBLIC-total bytes"),
                format!("[browser] Artifact {ARTIFACT} PUBLIC-done"),
            ]);
        }
        expected.push(format!("[browser] {message}"));
        assert_eq!(*messages.lock().unwrap(), expected);
    }
    assert_eq!(host.requests(), vec![format!("GET /runs/{RUN}/result"); 2]);
}

#[tokio::test]
async fn delivery_must_verify_before_capture_and_registration_failures_prevent_ack() {
    let home = tempfile::tempdir().unwrap();
    for (code, wrong_digest, acknowledged) in [
        ("PUBLIC-unrelated-warning", false, true),
        ("remote-artifact-registration-failed", false, false),
        ("remote-image-registration-failed", false, false),
        ("PUBLIC-unrelated-warning", true, false),
    ] {
        let result = json!({"answerText": "PUBLIC-answer", "warnings": [{"code": code}]});
        let body = event_body(&[delivery_event(&result, wrong_digest)]);
        let ack = format!("POST /runs/{RUN}/ack");
        let ack_route = ack.clone();
        let host = Host::start(move |line| {
            if line == ack_route {
                (200, Vec::new())
            } else {
                (200, body.clone())
            }
        })
        .await;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let recorded = captured.clone();
        let hooks = Hooks {
            result_captured: Some(Arc::new(move |value| {
                let recorded = recorded.clone();
                Box::pin(async move {
                    recorded.lock().unwrap().push(value);
                    Ok(())
                })
            })),
            ..Hooks::default()
        };

        let actual = bounded(host.client(&home).run(request(true), hooks)).await;
        if wrong_digest {
            assert_eq!(
                actual.unwrap_err().to_string(),
                "Remote delivery identity or digest does not match its result and artifacts"
            );
            assert!(captured.lock().unwrap().is_empty());
        } else {
            assert_eq!(actual.unwrap(), result);
            assert_eq!(captured.lock().unwrap().as_slice(), &[result]);
        }
        let mut expected = vec![format!("GET /runs/{RUN}/result")];
        if acknowledged {
            expected.push(ack);
        }
        assert_eq!(host.requests(), expected);
    }
}

#[tokio::test]
async fn cancellation_waits_for_result_persistence_and_preserves_its_error_without_ack() {
    let home = tempfile::tempdir().unwrap();
    let result = json!({"answerText": "PUBLIC-captured-answer"});
    let body = event_body(&[delivery_event(&result, false)]);
    let host = Host::start(move |_| (200, body.clone())).await;
    let client = host.client(&home);
    for fail in [false, true] {
        let request = request(true);
        let cancel = request.cancel.clone();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let recorded = captured.clone();
        let hooks = Hooks {
            result_captured: Some(Arc::new(move |value| {
                let cancel = cancel.clone();
                let recorded = recorded.clone();
                Box::pin(async move {
                    cancel.cancel();
                    tokio::task::yield_now().await;
                    recorded.lock().unwrap().push(value);
                    if fail {
                        bail!("PUBLIC-result-persistence-failure");
                    }
                    Ok(())
                })
            })),
            ..Hooks::default()
        };

        let error = bounded(client.run(request, hooks)).await.unwrap_err();
        let expected = if fail {
            "Browser run cancelled; result persistence also failed: PUBLIC-result-persistence-failure"
        } else {
            "Browser run cancelled"
        };
        assert_eq!(format!("{error:#}"), expected);
        assert_eq!(
            captured.lock().unwrap().as_slice(),
            std::slice::from_ref(&result)
        );
    }
    assert_eq!(host.requests(), vec![format!("GET /runs/{RUN}/result"); 2]);
}

#[tokio::test]
async fn invalid_transfer_warns_but_a_transfer_cancelled_at_start_does_not() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::start(|_| (404, Vec::new())).await;
    let client = host.client(&home);
    let mut request = request(true);
    request.image_destination = Some(home.path().join("PUBLIC-output.png"));

    let invalid_filename = "PUBLIC-invalid.zip";
    let (sender, receiver) = mpsc::channel(1);
    assert!(
        sender
            .try_send(Transfer {
                descriptor: json!({"kind": "file", "filename": invalid_filename}),
                image_index: 0,
            })
            .is_ok()
    );
    drop(sender);
    let output = bounded(client.transfers(receiver, &request, &Hooks::default(), &request.cancel))
        .await
        .unwrap();
    assert!(output.artifacts.is_empty());
    assert_eq!(output.failures, 1);
    assert_eq!(output.warnings.len(), 1);
    assert_eq!(
        output.warnings[0]["code"],
        "remote-artifact-transfer-failed"
    );
    assert_eq!(
        output.warnings[0]["details"],
        json!({"artifactKind": "file"})
    );
    let message = output.warnings[0]["message"].as_str().unwrap();
    let display_filename = crate::browser_output::filename(invalid_filename);
    assert!(
        message.contains(&format!("transfer failed for {display_filename}.")),
        "{message}"
    );
    assert!(!message.split_once("Reason: ").unwrap().1.is_empty());
    assert!(host.requests().is_empty());

    let image_filename = "PUBLIC-image.png";
    let image_bytes = b"PUBLIC-image";
    let (sender, receiver) = mpsc::channel(1);
    assert!(
        sender
            .try_send(Transfer {
                descriptor: json!({
                    "runId": RUN,
                    "artifactId": ARTIFACT,
                    "kind": "image",
                    "filename": image_filename,
                    "sizeBytes": image_bytes.len(),
                    "sha256": crate::artifacts::sha256(image_bytes)
                }),
                image_index: 0,
            })
            .is_ok()
    );
    drop(sender);
    let cancel = request.cancel.clone();
    let messages = Arc::new(Mutex::new(Vec::new()));
    let recorded = messages.clone();
    let hooks = Hooks {
        log: Some(Arc::new(move |message| {
            recorded.lock().unwrap().push(message.to_owned());
            cancel.cancel();
            Ok(())
        })),
        ..Hooks::default()
    };
    let output = bounded(client.transfers(receiver, &request, &hooks, &request.cancel))
        .await
        .unwrap();
    assert!(request.cancel.is_cancelled());
    let display_filename = crate::browser_output::filename(image_filename);
    assert_eq!(
        messages.lock().unwrap().as_slice(),
        &[format!(
            "[browser] Transferring artifact {display_filename} from bridge host..."
        )]
    );
    assert!(output.artifacts.is_empty());
    assert!(output.warnings.is_empty());
    assert_eq!(output.failures, 0);
    assert!(host.requests().is_empty());
    assert!(!request.image_destination.as_ref().unwrap().exists());
}

#[test]
fn merge_replaces_matching_paths_in_place_and_only_adds_the_requested_kind() {
    let old = json!({"path": "PUBLIC-shared.zip", "kind": "file", "source": "PUBLIC-host"});
    let replacement =
        json!({"path": "PUBLIC-shared.zip", "kind": "file", "source": "PUBLIC-receiver"});
    let retained = json!({"path": "PUBLIC-retained.zip", "kind": "file"});
    let added = json!({"path": "PUBLIC-added.zip", "kind": "file"});
    let image = json!({"path": "PUBLIC-image.png", "kind": "image"});
    let mut result = json!({"savedFiles": [old, retained]});

    merge(
        &mut result,
        "savedFiles",
        &[replacement.clone(), image, added.clone()],
        Some("file"),
    );

    assert_eq!(result["savedFiles"], json!([replacement, retained, added]));
}
