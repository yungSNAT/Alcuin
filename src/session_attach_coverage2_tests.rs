use super::*;
use std::{collections::BTreeSet, io, path::PathBuf};

const TEST_LIMIT: Duration = Duration::from_secs(2);
const LEGACY_BANNER: &str = "Attempting to reattach to the existing Chrome session...";
const RECOVERY_DIAGNOSTIC: &str = "Browser recovery is incomplete:";
const OUTPUT_ERROR: &str = "PUBLIC-output refused";

fn fixture() -> (tempfile::TempDir, SessionStore, String) {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let meta = store
        .create(
            json!({
                "prompt": "PUBLIC-coverage prompt",
                "model": "PUBLIC-a",
                "models": ["PUBLIC-a", "PUBLIC-b"]
            }),
            home.path(),
            None,
        )
        .unwrap();
    let id = meta["id"].as_str().unwrap().to_owned();
    (home, store, id)
}

fn dead_worker() -> ProcessIdentity {
    let mut worker = ProcessIdentity::current().unwrap();
    // A positive PID outside Linux's allocatable range.
    worker.pid = 2_147_483_647;
    worker.started_at_ms = 1.0;
    assert_eq!(worker.liveness(), Liveness::Dead);
    worker
}

fn lifecycle(worker: &ProcessIdentity) -> Value {
    json!({
        "detached": true,
        "processIdentity": worker,
        "workerPid": worker.pid,
        "workerProcessStartedAtMs": worker.started_at_ms
    })
}

struct RefuseText<'a> {
    needle: &'a str,
    attempted: Vec<u8>,
}

impl<'a> RefuseText<'a> {
    fn new(needle: &'a str) -> Self {
        Self {
            needle,
            attempted: Vec::new(),
        }
    }
}

impl Write for RefuseText<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.attempted.extend_from_slice(bytes);
        if String::from_utf8_lossy(&self.attempted).contains(self.needle) {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, OUTPUT_ERROR));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn fallback_usage_line_propagates_an_output_error() {
    let mut output = RefuseText::new("Finished (tok i/o/r/t:");
    let error = finished(
        &json!({"status": "completed", "usage": {"outputTokens": 3}}),
        &mut output,
    )
    .unwrap_err();

    assert_eq!(error.to_string(), OUTPUT_ERROR);
    assert!(String::from_utf8_lossy(&output.attempted).contains(output.needle));
}

#[tokio::test]
async fn refusing_a_render_notice_stops_attachment_before_the_saved_answer() {
    for (status, extra_bytes, notice) in [
        (
            "completed",
            MAX_RENDER_BYTES,
            "Render skipped (log too large:",
        ),
        ("completed", 0, "Render requested but stdout is not a TTY"),
        ("running", 0, "Render will apply after completion"),
    ] {
        if notice.contains("not a TTY") && io::stdout().is_terminal() {
            continue;
        }

        let (_home, store, id) = fixture();
        store
            .append_log(
                &id,
                Some("PUBLIC-a"),
                &format!("Answer:\nPUBLIC-raw answer{}\n", "x".repeat(extra_bytes)),
            )
            .unwrap();
        store
            .update(&id, |_| Ok(Some(json!({"status": status}))))
            .unwrap();
        let before = store.stored_for_update(&id).unwrap();
        let mut output = RefuseText::new(notice);
        let error = tokio::time::timeout(
            TEST_LIMIT,
            attach(
                &store,
                &id,
                &AttachView {
                    model: Some("PUBLIC-a"),
                    hide_prompt: true,
                    render: true,
                },
                &Environment::new(),
                &CancellationToken::new(),
                &mut output,
            ),
        )
        .await
        .expect("attachment did not stop after the write error")
        .unwrap_err();

        assert_eq!(error.to_string(), OUTPUT_ERROR);
        let attempted = String::from_utf8(output.attempted).unwrap();
        assert!(attempted.contains(notice), "{attempted}");
        assert!(!attempted.contains("PUBLIC-raw answer"), "{attempted}");
        assert_eq!(store.stored_for_update(&id).unwrap(), before);
    }
}

fn files_below(root: &Path) -> BTreeSet<PathBuf> {
    let mut files = BTreeSet::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            files.extend(files_below(&entry.path()));
        } else {
            files.insert(entry.path());
        }
    }
    files
}

#[tokio::test]
async fn launched_cancellation_rejects_a_stale_witness_without_changing_the_session() {
    let (home, store, id) = fixture();
    let mut worker = dead_worker();
    store
        .update(&id, |_| {
            Ok(Some(json!({
                "mode": "api",
                "status": "running",
                "lifecycle": lifecycle(&worker)
            })))
        })
        .unwrap();
    store
        .update_model(
            &id,
            "PUBLIC-a",
            &json!({"status": "completed", "response": {"id": "PUBLIC-model-winner"}}),
        )
        .unwrap();
    let before = store.stored_for_update(&id).unwrap();
    assert_eq!(before["status"], "running");
    let files_before = files_below(home.path());
    worker.started_at_ms += 1.0;

    let cancel = CancellationToken::new();
    cancel.cancel();
    let mut output = Vec::new();
    let exit = tokio::time::timeout(
        TEST_LIMIT,
        attach_launched(
            &store,
            &id,
            &worker,
            &Environment::new(),
            &cancel,
            &mut output,
        ),
    )
    .await
    .expect("cancelled launched attachment did not return")
    .unwrap();

    assert_eq!(exit, 130);
    let text = String::from_utf8(output).unwrap();
    assert!(
        text.contains("Unable to request cancellation from launched worker:"),
        "{text}"
    );
    assert!(!text.contains("Session ownership was preserved."), "{text}");
    assert_eq!(store.stored_for_update(&id).unwrap(), before);
    assert_eq!(files_below(home.path()), files_before);
}

#[tokio::test]
async fn legacy_recovery_stops_on_a_banner_write_error_or_a_missing_session() {
    let (_home, store, id) = fixture();
    let saved = store.stored_for_update(&id).unwrap();
    let environment = Environment::new();
    let cancel = CancellationToken::new();

    let mut refused = RefuseText::new(LEGACY_BANNER);
    let error = recover_legacy(&store, &id, &saved, &environment, &cancel, &mut refused)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), OUTPUT_ERROR);
    assert_eq!(store.stored_for_update(&id).unwrap(), saved);

    // Simulate removal after attachment took its saved ownership snapshot.
    let directory = store.directory(&id).unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
    let missing = store.require(&id).unwrap_err().to_string();
    let mut output = Vec::new();
    let error = tokio::time::timeout(
        TEST_LIMIT,
        recover_legacy(&store, &id, &saved, &environment, &cancel, &mut output),
    )
    .await
    .expect("recovery did not return for a removed session")
    .unwrap_err();

    let diagnostic = format!("{error:#}");
    assert!(
        diagnostic.contains(missing.as_str()) || diagnostic.contains(id.as_str()),
        "{diagnostic}"
    );
    assert_eq!(
        String::from_utf8(output).unwrap(),
        format!("{LEGACY_BANNER}\n")
    );
    assert!(!directory.exists());
    assert!(!cancel.is_cancelled());
}

// Unlike a command trace alone, this peer confirms the WebSocket was opened and
// then disconnected even when recovery rejects the saved state before its first command.
async fn observe_recovery_socket(listener: tokio::net::TcpListener) -> Vec<String> {
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::tungstenite::Message;

    let port = listener.local_addr().unwrap().port();
    loop {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut head = [0; 4096];
        let header_len = loop {
            let len = stream.peek(&mut head).await.unwrap();
            assert_ne!(len, 0, "client disconnected before sending HTTP headers");
            if let Some(end) = head[..len]
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
            {
                break end + 4;
            }
            assert!(len < head.len(), "HTTP headers exceeded the fixture buffer");
            tokio::task::yield_now().await;
        };
        let path = std::str::from_utf8(&head[..header_len])
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .split_ascii_whitespace()
            .nth(1)
            .unwrap();
        if path == "/devtools/browser/PUBLIC" {
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut methods = Vec::new();
            while let Some(message) = socket.next().await {
                match message {
                    Ok(Message::Text(frame)) => {
                        let command: Value = serde_json::from_str(&frame).unwrap();
                        methods.push(command["method"].as_str().unwrap().to_owned());
                        let reply = json!({
                            "id": command["id"],
                            "error": {"message": "PUBLIC-recovery command refused"}
                        });
                        socket
                            .send(Message::Text(reply.to_string().into()))
                            .await
                            .unwrap();
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            return methods;
        }

        let body = match path {
            "/json/version" => json!({
                "Browser": "PUBLIC",
                "webSocketDebuggerUrl": format!("ws://127.0.0.1:{port}/devtools/browser/PUBLIC")
            }),
            "/json/list" | "/json" => json!([]),
            _ => panic!("unexpected discovery path: {path}"),
        }
        .to_string();
        stream.read_exact(&mut head[..header_len]).await.unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    }
}

#[tokio::test]
async fn native_recovery_disconnects_after_rejecting_an_unowned_capture() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let peer = tokio::spawn(observe_recovery_socket(listener));
    let (_home, store, id) = fixture();
    let profile = tempfile::tempdir().unwrap();
    store
        .update(&id, |_| {
            Ok(Some(json!({
                "mode": "browser",
                "status": "error",
                "errorMessage": "PUBLIC-original failure",
                "recoveryRequired": true,
                "lifecycle": lifecycle(&dead_worker()),
                "browser": {
                    "config": {"inputTimeoutMs": 500},
                    "runtime": {
                        "chromeHost": "127.0.0.1",
                        "chromePort": port,
                        "chromeBrowserWSEndpoint": format!("ws://127.0.0.1:{port}/devtools/browser/PUBLIC"),
                        "profileDir": profile.path()
                    }
                }
            })))
        })
        .unwrap();
    let before = store.stored_for_update(&id).unwrap();

    let mut output = Vec::new();
    let exit = tokio::time::timeout(
        TEST_LIMIT,
        attach(
            &store,
            &id,
            &AttachView {
                model: None,
                hide_prompt: true,
                render: false,
            },
            &Environment::new(),
            &CancellationToken::new(),
            &mut output,
        ),
    )
    .await
    .expect("native recovery did not return")
    .unwrap();

    let text = String::from_utf8(output).unwrap();
    assert_eq!(exit, 0, "{text}");
    assert_eq!(text.matches(RECOVERY_DIAGNOSTIC).count(), 1, "{text}");
    assert!(
        text.contains("Session failed: PUBLIC-original failure"),
        "{text}"
    );
    assert!(
        !text.contains("Recovery connection cleanup is unconfirmed"),
        "{text}"
    );
    assert_eq!(store.stored_for_update(&id).unwrap(), before);

    let methods = tokio::time::timeout(TEST_LIMIT, peer)
        .await
        .unwrap_or_else(|_| panic!("pinned socket was not opened and disconnected: {text}"))
        .unwrap();
    assert!(
        methods.iter().all(|method| {
            !method.starts_with("Input.")
                && !matches!(
                    method.as_str(),
                    "Target.createTarget"
                        | "Target.closeTarget"
                        | "Page.navigate"
                        | "Browser.close"
                )
        }),
        "{methods:?}"
    );
}
