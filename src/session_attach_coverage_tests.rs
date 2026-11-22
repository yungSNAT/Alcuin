use super::*;
use crate::cdp::fake;
use std::io;

const TEST_TIMEOUT: Duration = Duration::from_secs(2);
const LEGACY_BANNER: &str = "Attempting to reattach to the existing Chrome session...";
const RECOVERY_DIAGNOSTIC: &str = "Browser recovery is incomplete:";
const STREAMING_NOTICE: &str =
    "Render will apply after completion; streaming raw text meanwhile...";

fn fixture() -> (tempfile::TempDir, SessionStore, String) {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path());
    let meta = store
        .create(
            json!({
                "prompt": "PUBLIC coverage prompt",
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
    worker.pid = u32::MAX;
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

fn view(render: bool) -> AttachView<'static> {
    AttachView {
        model: None,
        hide_prompt: true,
        render,
    }
}

async fn observe(
    store: &SessionStore,
    id: &str,
    view: &AttachView<'_>,
    cancel: &CancellationToken,
    output: &mut impl Write,
) -> i32 {
    tokio::time::timeout(
        TEST_TIMEOUT,
        attach(store, id, view, &Environment::new(), cancel, output),
    )
    .await
    .expect("attachment did not return")
    .unwrap()
}

struct FlushHook<F> {
    bytes: Vec<u8>,
    hook: F,
}

impl<F: FnMut(&[u8]) -> io::Result<()>> FlushHook<F> {
    fn new(hook: F) -> Self {
        Self {
            bytes: Vec::new(),
            hook,
        }
    }
}

impl<F: FnMut(&[u8]) -> io::Result<()>> Write for FlushHook<F> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        (self.hook)(&self.bytes)
    }
}

#[test]
fn a_heading_added_between_polls_does_not_repeat_the_saved_body() {
    let mut cursor = LogCursor::default();
    let mut output = Vec::new();

    for text in [
        "PUBLIC first line\n",
        "=== PUBLIC-a ===\nPUBLIC first line\nPUBLIC second line\n",
        "=== PUBLIC-a ===\nPUBLIC first line\nPUBLIC second line\n",
        "=== PUBLIC-a ===\nPUBLIC replacement\n",
    ] {
        cursor
            .emit("PUBLIC-a", text.to_owned(), &mut output)
            .unwrap();
    }

    assert_eq!(
        String::from_utf8(output).unwrap(),
        "PUBLIC first line\nPUBLIC second line\n\n\n=== PUBLIC-a ===\nPUBLIC replacement\n"
    );
    assert_eq!(
        cursor.saved["PUBLIC-a"],
        "=== PUBLIC-a ===\nPUBLIC replacement\n"
    );
}

#[test]
fn logs_without_model_runs_use_the_saved_model_or_the_session_name() {
    for (meta, name) in [
        (json!({"model": "PUBLIC-a", "models": []}), "PUBLIC-a"),
        (json!({}), "session"),
    ] {
        let (_home, store, id) = fixture();
        store.append_log(&id, None, "PUBLIC root first\n").unwrap();
        let mut cursor = LogCursor::default();
        let mut output = Vec::new();

        cursor.drain(&store, &id, &meta, None, &mut output).unwrap();
        let first = store.log(&id).unwrap();
        assert!(first.contains("PUBLIC root first"));
        assert_eq!(output, first.as_bytes());

        store.append_log(&id, None, "PUBLIC root second\n").unwrap();
        for _ in 0..2 {
            cursor
                .drain(&store, &id, &meta, Some(" \t"), &mut output)
                .unwrap();
        }

        let saved = store.log(&id).unwrap();
        assert!(saved.contains("PUBLIC root second"));
        assert_eq!(output, saved.as_bytes());
        assert_eq!(cursor.last_model.as_deref(), Some(name));
        assert_eq!(cursor.saved[name], saved);
    }
}

#[test]
fn finished_uses_the_completion_summary_when_timing_is_available() {
    let meta = json!({
        "status": "completed",
        "mode": "api",
        "model": "PUBLIC-a",
        "startedAt": "2026-01-01T00:00:00.000Z",
        "completedAt": "2026-01-01T00:00:02.000Z",
        "elapsedMs": 2000,
        "durationMs": 2000,
        "usage": {
            "inputTokens": 12,
            "outputTokens": 3,
            "reasoningTokens": 0,
            "totalTokens": 15
        }
    });
    let summary = session_display::completion_summary(&meta).expect("timed completion summary");
    let mut output = Vec::new();

    finished(&meta, &mut output).unwrap();

    assert_eq!(String::from_utf8(output).unwrap(), format!("\n{summary}\n"));
}

#[tokio::test]
async fn completed_render_requests_keep_the_raw_answer_and_explain_the_fallback() {
    for oversized in [false, true] {
        let (_home, store, id) = fixture();
        let answer = if oversized {
            format!("PUBLIC {}", "x".repeat(MAX_RENDER_BYTES))
        } else {
            "PUBLIC short answer".to_owned()
        };
        store
            .append_log(&id, Some("PUBLIC-a"), &format!("Answer:\n{answer}\n"))
            .unwrap();
        let meta = store
            .update(&id, |_| Ok(Some(json!({"status": "completed"}))))
            .unwrap();
        let log = session_display::display_log(&store, &id, &meta, Some("PUBLIC-a")).unwrap();
        let shown = session_display::trim_before_first_answer(&log);
        assert_eq!(shown.len() > MAX_RENDER_BYTES, oversized);

        let mut output = Vec::new();
        let attach_view = AttachView {
            model: Some("PUBLIC-a"),
            ..view(true)
        };
        assert_eq!(
            observe(
                &store,
                &id,
                &attach_view,
                &CancellationToken::new(),
                &mut output
            )
            .await,
            0
        );

        let output = String::from_utf8(output).unwrap();
        let answer_start = output.find(answer.as_str()).expect("raw answer");
        let preamble = &output[..answer_start];
        let not_a_tty = "Render requested but stdout is not a TTY; showing raw text.";
        if oversized {
            let notice = format!(
                "Render skipped (log too large: {} bytes > {MAX_RENDER_BYTES}). Showing raw text.",
                shown.len()
            );
            assert!(preamble.contains(notice.as_str()));
            assert!(!preamble.contains(not_a_tty));
        } else {
            assert!(!preamble.contains("Render skipped"));
            assert_eq!(preamble.contains(not_a_tty), !io::stdout().is_terminal());
        }
    }
}

#[tokio::test]
async fn native_recovery_reports_missing_admission_data_once_without_connecting() {
    let browser = fake::browser(json!([]), |_| fake::Reply::ok()).await;
    let socket = browser.browser_ws();
    for (runtime, message, cancel_after_report) in [
        (json!({}), "Native recovery endpoint host is missing", false),
        (
            json!({"chromeHost": "127.0.0.1", "chromePort": 65536}),
            "Native recovery endpoint port is missing",
            false,
        ),
        (
            json!({"chromeHost": "127.0.0.1", "chromePort": browser.port}),
            "Native recovery has no pinned browser socket",
            false,
        ),
        (
            json!({
                "chromeHost": "127.0.0.1",
                "chromePort": browser.port,
                "chromeBrowserWSEndpoint": socket
            }),
            "Native recovery has no recorded admission directory",
            true,
        ),
    ] {
        let (_home, store, id) = fixture();
        let worker = dead_worker();
        store
            .update(&id, |_| {
                Ok(Some(json!({
                    "mode": "browser",
                    "status": "error",
                    "errorMessage": "PUBLIC saved failure",
                    "recoveryRequired": true,
                    "lifecycle": lifecycle(&worker),
                    "browser": {"runtime": runtime}
                })))
            })
            .unwrap();
        let before = store.stored_for_update(&id).unwrap();
        let cancel = CancellationToken::new();
        let mut output = FlushHook::new(|bytes| {
            if cancel_after_report && String::from_utf8_lossy(bytes).contains(RECOVERY_DIAGNOSTIC) {
                cancel.cancel();
            }
            Ok(())
        });

        let exit = observe(&store, &id, &view(false), &cancel, &mut output).await;

        assert_eq!(exit, if cancel_after_report { 130 } else { 0 });
        let text = String::from_utf8(output.bytes).unwrap();
        assert_eq!(text.matches(RECOVERY_DIAGNOSTIC).count(), 1, "{text}");
        assert!(text.contains(message), "{text}");
        assert_eq!(store.stored_for_update(&id).unwrap(), before);
    }
    assert!(browser.methods().is_empty());
}

#[tokio::test]
async fn legacy_recovery_reports_a_banner_flush_failure_without_changing_the_record() {
    for cancel_after_report in [false, true] {
        let (home, store, id) = fixture();
        store
            .update(&id, |_| {
                Ok(Some(json!({
                    "mode": "browser",
                    "status": "error",
                    "lifecycle": {"workerPid": u32::MAX},
                    "response": {"incompleteReason": "chrome-disconnected"},
                    "browser": {"runtime": {"chromeProfileRoot": home.path()}}
                })))
            })
            .unwrap();
        let before = store.stored_for_update(&id).unwrap();
        let cancel = CancellationToken::new();
        let mut output = FlushHook::new(|bytes| {
            let text = String::from_utf8_lossy(bytes);
            if text.contains(RECOVERY_DIAGNOSTIC) {
                if cancel_after_report {
                    cancel.cancel();
                }
            } else if text.contains(LEGACY_BANNER) {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "PUBLIC banner flush refused",
                ));
            }
            Ok(())
        });

        let exit = observe(&store, &id, &view(false), &cancel, &mut output).await;

        assert_eq!(exit, if cancel_after_report { 130 } else { 0 });
        let text = String::from_utf8(output.bytes).unwrap();
        assert_eq!(text.matches(LEGACY_BANNER).count(), 1, "{text}");
        assert_eq!(
            text.matches("Browser recovery is incomplete: PUBLIC banner flush refused")
                .count(),
            1,
            "{text}"
        );
        assert_eq!(store.stored_for_update(&id).unwrap(), before);
    }
}

#[tokio::test]
async fn dead_worker_attachment_honors_cancellation_and_preserves_winning_commits() {
    #[derive(Clone, Copy)]
    enum Outcome {
        Cancel,
        SessionCommit,
        ModelCommit,
        LaunchedModelCommit,
        Uncommitted,
    }

    const LOG: &str = "PUBLIC last worker log";
    const PRESERVED: &str = "Model metadata is terminal while the session commit is incomplete. Session ownership was preserved.";

    for (outcome, expected_exit) in [
        (Outcome::Cancel, 130),
        (Outcome::SessionCommit, 0),
        (Outcome::ModelCommit, 0),
        (Outcome::LaunchedModelCommit, 1),
        (Outcome::Uncommitted, 0),
    ] {
        let (_home, store, id) = fixture();
        let worker = dead_worker();
        store
            .update(&id, |_| {
                Ok(Some(json!({
                    "mode": "api",
                    "status": "running",
                    "lifecycle": lifecycle(&worker)
                })))
            })
            .unwrap();
        let model_committed =
            matches!(outcome, Outcome::ModelCommit | Outcome::LaunchedModelCommit);
        if model_committed {
            store
                .update_model(&id, "PUBLIC-a", &json!({"status": "completed"}))
                .unwrap();
        }
        store
            .append_log(&id, Some("PUBLIC-a"), &format!("{LOG}\n"))
            .unwrap();
        let before = store.stored_for_update(&id).unwrap();
        let cancel = CancellationToken::new();
        let mut handled_log = false;
        let mut output = FlushHook::new(|bytes| {
            // Match the log body without depending on how the saved log ends.
            // This flush happens after the poll loads the running record but before settlement.
            if !handled_log && String::from_utf8_lossy(bytes).contains(LOG) {
                handled_log = true;
                match outcome {
                    Outcome::Cancel => cancel.cancel(),
                    Outcome::SessionCommit => {
                        store
                            .update(&id, |_| {
                                Ok(Some(json!({
                                    "status": "completed",
                                    "response": {"id": "PUBLIC competing commit"}
                                })))
                            })
                            .unwrap();
                    }
                    Outcome::ModelCommit | Outcome::LaunchedModelCommit | Outcome::Uncommitted => {}
                }
            }
            Ok(())
        });

        let launched = matches!(outcome, Outcome::LaunchedModelCommit);
        let exit = if launched {
            tokio::time::timeout(
                TEST_TIMEOUT,
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
            .expect("launched attachment did not return")
            .unwrap()
        } else {
            observe(&store, &id, &view(true), &cancel, &mut output).await
        };

        let text = String::from_utf8(output.bytes).unwrap();
        assert_eq!(exit, expected_exit, "{text}");
        assert_eq!(text.matches(LOG).count(), 1, "{text}");
        assert_eq!(text.contains(STREAMING_NOTICE), !launched, "{text}");
        assert_eq!(text.contains(PRESERVED), model_committed, "{text}");
        let current = store.stored_for_update(&id).unwrap();
        match outcome {
            Outcome::SessionCommit => {
                assert_eq!(current["status"], "completed");
                assert_eq!(current["response"]["id"], "PUBLIC competing commit");
                assert!(!text.contains("Session failed:"), "{text}");
            }
            Outcome::Uncommitted => {
                assert_eq!(current["status"], "error");
                assert_eq!(
                    current["response"]["incompleteReason"],
                    "incomplete-capture"
                );
                assert!(
                    text.contains("Session failed: Detached worker exited"),
                    "{text}"
                );
            }
            Outcome::Cancel | Outcome::ModelCommit | Outcome::LaunchedModelCommit => {
                assert_eq!(current, before);
            }
        }
    }
}
