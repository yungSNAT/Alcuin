use super::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

enum Canned {
    /// Write the bytes, then close the connection.
    Close(Vec<u8>),
    /// Write the bytes, then keep the connection open without writing more.
    Hang(Vec<u8>),
}

struct Server {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

async fn read_request(stream: &mut TcpStream) -> String {
    let mut data = vec![];
    let mut buf = [0u8; 4096];
    loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&data[..end]).to_ascii_lowercase();
            let len = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if data.len() >= end + 4 + len {
                break;
            }
        }
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

async fn serve(replies: Vec<Canned>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(vec![]));
    let seen = requests.clone();
    tokio::spawn(async move {
        for canned in replies {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let request = read_request(&mut stream).await;
            seen.lock().unwrap().push(request);
            match canned {
                Canned::Close(bytes) => {
                    let _ = stream.write_all(&bytes).await;
                    let _ = stream.shutdown().await;
                }
                Canned::Hang(bytes) => {
                    let _ = stream.write_all(&bytes).await;
                    tokio::spawn(async move {
                        let _keep = stream;
                        std::future::pending::<()>().await;
                    });
                }
            }
        }
    });
    Server { base, requests }
}

fn http(status: &str, headers: &[(&str, &str)], body: &str) -> Vec<u8> {
    let extra: String = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn sse(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nx-request-id: PUBLIC-req\r\nConnection: close\r\n\r\n{body}"
    )
    .into_bytes()
}

fn route(base: &str, model: &str, transport: ApiTransport) -> Route {
    Route {
        model: model.into(),
        provider_label: "PUBLIC".into(),
        base_url: base.into(),
        key_source: "OPENROUTER_API_KEY".into(),
        azure_deployment_name: None,
        transport,
    }
}

fn provider(base: &str, transport: ApiTransport, timeout: Duration) -> HttpProvider {
    HttpProvider::new(
        route(base, "gpt-6-astra", transport),
        Secret::new("PUBLIC-key".into()),
        timeout,
    )
    .unwrap()
}

fn provider_error(error: &anyhow::Error) -> &ProviderError {
    error
        .downcast_ref::<ProviderError>()
        .unwrap_or_else(|| panic!("not a provider error: {error:#}"))
}

async fn closed_port() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}/v1")
}

#[derive(Default)]
struct Recorder {
    responses: Mutex<Vec<Value>>,
    deltas: Mutex<Vec<String>>,
}

impl ResponseObserver for Recorder {
    fn response(&self, response: &Value) -> Result<()> {
        self.responses.lock().unwrap().push(response.clone());
        Ok(())
    }
    fn delta(&self, text: &str) -> Result<()> {
        self.deltas.lock().unwrap().push(text.to_owned());
        Ok(())
    }
}

fn chat_body() -> Value {
    json!({
        "model": "PUBLIC-model",
        "instructions": "PUBLIC-sys",
        "input": [{"role": "user", "content": [{"type": "input_text", "text": "PUBLIC-q"}]}]
    })
}

#[test]
fn masking_missing_key_describe_and_status_messages() {
    assert_eq!(mask_api_key(""), None);
    assert_eq!(mask_api_key("PUBLICk").as_deref(), Some("P***k"));

    let message = missing_key_message(&route(
        "http://x",
        "claude-PUBLIC",
        ApiTransport::ChatCompletions,
    ));
    assert_eq!(
        message,
        "Missing OPENROUTER_API_KEY. Set it via the environment."
    );

    let error = |reason| ProviderError {
        reason,
        message: "PUBLIC-message".into(),
        request_id: None,
        status: None,
    };
    assert_eq!(
        describe(&error(FailureReason::ClientTimeout), None),
        "Client-side timeout: OpenAI streaming call exceeded the configured deadline."
    );
    assert_eq!(
        describe(&error(FailureReason::Unknown), None),
        "OpenAI streaming call ended with an unknown transport error."
    );

    for (body, expected) in [
        (r#"{"error":null}"#, "500 status code (no body)"),
        (r#"{"error":{"message":"PUBLIC-msg"}}"#, "500 PUBLIC-msg"),
        (r#"{"error":{"code":1}}"#, r#"500 {"code":1}"#),
        (r#"{"error":{"message":""}}"#, r#"500 {"message":""}"#),
        (r#"{"error":{"message":42}}"#, "500 42"),
    ] {
        assert_eq!(status_message(500, body.as_bytes()), expected, "{body}");
    }
}

#[tokio::test]
async fn cancelled_token_aborts_before_any_request_through_every_entry_point() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let base = closed_port().await;
    let expected = "The request was aborted before OpenAI finished responding.";

    let responses = provider(&base, ApiTransport::Responses, Duration::from_secs(2));
    let chat = provider(&base, ApiTransport::ChatCompletions, Duration::from_secs(2));

    let errors = [
        <HttpProvider as ResponseProvider>::create(
            &responses,
            &json!({"background": true}),
            &cancel,
        )
        .await
        .unwrap_err(),
        <HttpProvider as ResponseProvider>::retrieve(&responses, "resp_PUBLIC", &cancel)
            .await
            .unwrap_err(),
        chat.create(&chat_body(), &cancel).await.unwrap_err(),
    ];
    for error in &errors {
        let error = provider_error(error);
        assert_eq!(error.reason, FailureReason::ClientAbort);
        assert_eq!(error.message, expected);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn post_send_failures_map_to_connection_lost_and_timeout() {
    let cancel = CancellationToken::new();
    let refused = provider(
        &closed_port().await,
        ApiTransport::Responses,
        Duration::from_secs(2),
    );
    let error = refused
        .create(&json!({"background": true}), &cancel)
        .await
        .unwrap_err();
    let error = provider_error(&error);
    assert_eq!(error.reason, FailureReason::ConnectionLost);
    assert_eq!(
        error.message,
        "Connection to OpenAI dropped before the response completed."
    );

    let server = serve(vec![Canned::Hang(vec![])]).await;
    let slow = provider(
        &server.base,
        ApiTransport::Responses,
        Duration::from_millis(150),
    );
    let error = slow
        .create(&json!({"background": true}), &cancel)
        .await
        .unwrap_err();
    let error = provider_error(&error);
    assert_eq!(error.reason, FailureReason::ClientTimeout);
    assert_eq!(error.message, "OpenAI request timed out before completion.");
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn retrieve_honours_should_retry_header_true_and_false() {
    let cancel = CancellationToken::new();
    let server = serve(vec![
        Canned::Close(http(
            "200 OK",
            &[("x-should-retry", "true"), ("retry-after-ms", "0")],
            "{}",
        )),
        Canned::Close(http(
            "200 OK",
            &[("x-request-id", "PUBLIC-req")],
            r#"{"id":"resp_PUBLIC","status":"completed"}"#,
        )),
    ])
    .await;
    let http_provider = provider(
        &server.base,
        ApiTransport::Responses,
        Duration::from_secs(2),
    );
    let result = http_provider
        .retrieve("resp_PUBLIC", &cancel)
        .await
        .unwrap();
    assert_eq!(result["id"], "resp_PUBLIC");
    assert_eq!(result["_request_id"], "PUBLIC-req");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|r| r.starts_with("GET /v1/responses/resp_PUBLIC "))
    );

    let server = serve(vec![Canned::Close(http(
        "500 Internal Server Error",
        &[("x-should-retry", "false")],
        r#"{"error":{"message":"PUBLIC-fail"}}"#,
    ))])
    .await;
    let http_provider = provider(
        &server.base,
        ApiTransport::Responses,
        Duration::from_secs(2),
    );
    let error = http_provider
        .retrieve("resp_PUBLIC", &cancel)
        .await
        .unwrap_err();
    let error = provider_error(&error);
    assert_eq!(error.reason, FailureReason::ApiError);
    assert_eq!(error.message, "PUBLIC-fail");
    assert_eq!(error.status, Some(500));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn retrieve_connection_failure_backs_off_until_cancelled() {
    let cancel = CancellationToken::new();
    let http_provider = provider(
        &closed_port().await,
        ApiTransport::Responses,
        Duration::from_secs(2),
    );
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        trigger.cancel();
    });
    let started = std::time::Instant::now();
    let error = http_provider
        .retrieve("resp_PUBLIC", &cancel)
        .await
        .unwrap_err();
    assert!(
        started.elapsed() < Duration::from_millis(450),
        "{:?}",
        started.elapsed()
    );
    let error = provider_error(&error);
    assert_eq!(error.reason, FailureReason::ClientAbort);
    assert_eq!(
        error.message,
        "The request was aborted before OpenAI finished responding."
    );
}

#[tokio::test]
async fn unsupported_endpoint_and_oversized_buffered_responses_fail() {
    let cancel = CancellationToken::new();
    let mut big = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
    big.extend(std::iter::repeat_n(b'a', 9 * 1024 * 1024));
    let server = serve(vec![
        Canned::Close(http("404 Not Found", &[], "")),
        Canned::Close(
            b"HTTP/1.1 200 OK\r\nContent-Length: 9437184\r\nConnection: close\r\n\r\n{}".to_vec(),
        ),
        Canned::Close(big),
    ])
    .await;
    let http_provider = provider(
        &server.base,
        ApiTransport::Responses,
        Duration::from_secs(4),
    );
    let body = json!({"background": true});

    let error = http_provider.create(&body, &cancel).await.unwrap_err();
    let provider_err = provider_error(&error);
    assert_eq!(provider_err.reason, FailureReason::UnsupportedEndpoint);
    assert_eq!(provider_err.status, Some(404));
    assert!(
        provider_err.message.starts_with("HTTP 404/405"),
        "{}",
        provider_err.message
    );

    for _ in 0..2 {
        let error = http_provider.create(&body, &cancel).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Provider response exceeds the 8 MiB transfer limit."
        );
    }
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn body_read_timeout_maps_to_client_timeout() {
    let cancel = CancellationToken::new();
    let server = serve(vec![Canned::Hang(
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{".to_vec(),
    )])
    .await;
    let http_provider = provider(
        &server.base,
        ApiTransport::Responses,
        Duration::from_millis(200),
    );
    let error = http_provider
        .create(&json!({"background": true}), &cancel)
        .await
        .unwrap_err();
    let error = provider_error(&error);
    assert_eq!(error.reason, FailureReason::ClientTimeout);
    assert_eq!(
        error.message,
        "Provider transfer was interrupted; request evidence is retained."
    );
}

#[tokio::test]
async fn chat_stream_accumulates_deltas_usage_and_ignores_events_after_done() {
    let cancel = CancellationToken::new();
    let stream = concat!(
        "data: {\"id\":\"chatcmpl-PUBLIC\",\"choices\":[{\"delta\":{\"content\":\"PUBLIC-\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-PUBLIC\",\"choices\":[{\"delta\":{\"content\":\"answer\"}}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
        "data: [DONE]\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"ignored\"}}]}\n\n",
    );
    let server = serve(vec![Canned::Close(sse(stream))]).await;
    let http_provider = provider(
        &server.base,
        ApiTransport::ChatCompletions,
        Duration::from_secs(2),
    );
    let recorder = Recorder::default();
    let result = http_provider
        .create_observed(&chat_body(), &cancel, &recorder)
        .await
        .unwrap();
    assert_eq!(result["id"], "chatcmpl-PUBLIC");
    assert_eq!(result["output_text"][0], "PUBLIC-answer");
    assert_eq!(
        result["usage"],
        json!({"input_tokens": 3, "output_tokens": 2, "total_tokens": 5})
    );
    assert_eq!(result["_request_id"], "PUBLIC-req");
    assert_eq!(*recorder.deltas.lock().unwrap(), ["PUBLIC-", "answer"]);
    assert_eq!(recorder.responses.lock().unwrap().last(), Some(&result));

    let requests = server.requests();
    assert!(
        requests[0].starts_with("POST /v1/chat/completions "),
        "{}",
        requests[0]
    );
    assert!(requests[0].contains("\"stream\":true"), "{}", requests[0]);
    assert!(requests[0].contains("\"messages\""), "{}", requests[0]);
}

#[tokio::test]
async fn chat_stream_flushes_final_event_and_assigns_generated_id() {
    let cancel = CancellationToken::new();
    let stream = "data: {\"choices\":[{\"delta\":{\"content\":\"PUBLIC-tail\"}}]}\n";
    let server = serve(vec![Canned::Close(sse(stream))]).await;
    let http_provider = provider(
        &server.base,
        ApiTransport::ChatCompletions,
        Duration::from_secs(2),
    );
    let recorder = Recorder::default();
    let result = http_provider
        .create_observed(&chat_body(), &cancel, &recorder)
        .await
        .unwrap();
    assert!(
        result["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("openrouter-")),
        "{result}"
    );
    assert_eq!(result["output_text"][0], "PUBLIC-tail");
    assert_eq!(*recorder.deltas.lock().unwrap(), ["PUBLIC-tail"]);
    assert_eq!(recorder.responses.lock().unwrap().last(), Some(&result));
}

#[tokio::test]
async fn stream_error_events_fail_with_api_error_and_request_id() {
    let cancel = CancellationToken::new();
    for (transport, body, stream, expected) in [
        (
            ApiTransport::Responses,
            json!({}),
            "event: error\ndata: {\"message\":\"PUBLIC-stream-failed\"}\n\n",
            "PUBLIC-stream-failed",
        ),
        (
            ApiTransport::ChatCompletions,
            chat_body(),
            "data: {\"error\":{\"message\":\"PUBLIC-chat-failed\"}}\n\n",
            "PUBLIC-chat-failed",
        ),
    ] {
        let server = serve(vec![Canned::Close(sse(stream))]).await;
        let http_provider = provider(&server.base, transport, Duration::from_secs(2));
        let error = http_provider.create(&body, &cancel).await.unwrap_err();
        let error = provider_error(&error);
        assert_eq!(error.reason, FailureReason::ApiError);
        assert_eq!(error.message, expected);
        assert_eq!(error.request_id.as_deref(), Some("PUBLIC-req"));
    }
}

#[tokio::test]
async fn chat_background_request_is_buffered_and_converted() {
    let cancel = CancellationToken::new();
    let server = serve(vec![Canned::Close(http(
        "200 OK",
        &[("x-request-id", "PUBLIC-req")],
        r#"{"id":"chatcmpl-PUBLIC","choices":[{"message":{"content":"PUBLIC-done"}}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#,
    ))])
    .await;
    let http_provider = provider(
        &server.base,
        ApiTransport::ChatCompletions,
        Duration::from_secs(2),
    );
    let mut body = chat_body();
    body["background"] = json!(true);
    let result = http_provider.create(&body, &cancel).await.unwrap();
    assert_eq!(result["id"], "chatcmpl-PUBLIC");
    assert_eq!(result["output_text"], json!(["PUBLIC-done"]));
    assert_eq!(result["status"], "completed");
    assert_eq!(result["_request_id"], "PUBLIC-req");
    assert_eq!(text_output(&result), "PUBLIC-done");
    let requests = server.requests();
    assert!(requests[0].contains("\"stream\":false"), "{}", requests[0]);
}

#[tokio::test]
async fn responses_stream_is_posted_with_stream_flag_and_decoded() {
    let cancel = CancellationToken::new();
    let stream = concat!(
        "event: response.created\n",
        "data: {\"response\":{\"id\":\"resp_PUBLIC\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"delta\":\"PUBLIC-hello\",\"item_id\":\"msg_PUBLIC\",\"output_index\":0,\"content_index\":0}\n\n",
        "event: response.completed\n",
        "data: {\"response\":{\"id\":\"resp_PUBLIC\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"PUBLIC-hello\"}]}]}}\n\n",
    );
    let server = serve(vec![Canned::Close(sse(stream))]).await;
    let http_provider = provider(
        &server.base,
        ApiTransport::Responses,
        Duration::from_secs(2),
    );
    let recorder = Recorder::default();
    let outcome = http_provider
        .create_observed(&json!({"model": "gpt-6-astra"}), &cancel, &recorder)
        .await;
    match outcome {
        Ok(result) => assert_eq!(result["id"], "resp_PUBLIC"),
        Err(error) => assert!(
            error.to_string().contains("Missing streamed output"),
            "{error:#}"
        ),
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].starts_with("POST /v1/responses "),
        "{}",
        requests[0]
    );
    assert!(requests[0].contains("\"stream\":true"), "{}", requests[0]);
}
