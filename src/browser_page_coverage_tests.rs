use super::*;
use crate::cdp::fake::{Reply, peer};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

const SHORT: Duration = Duration::from_secs(1);

fn recorder() -> (
    Arc<Mutex<Vec<String>>>,
    impl Fn(&str) + Send + Sync + 'static,
) {
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = lines.clone();
    (lines, move |line: &str| {
        sink.lock().unwrap().push(line.to_owned())
    })
}

#[test]
fn cloudflare_challenge_wording_depends_on_headless_mode() {
    assert_eq!(
        CloudflareChallenge { headless: true }.to_string(),
        "Cloudflare challenge detected in headless mode. Re-run with --headful so you can solve the challenge."
    );
    assert_eq!(
        CloudflareChallenge { headless: false }.to_string(),
        "Cloudflare challenge detected. Complete the “Just a moment…” check in the open browser, then rerun."
    );
}

#[tokio::test]
async fn wait_document_ready_polls_until_the_page_is_interactive() {
    let cancel = CancellationToken::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let (cdp, _peer) = peer(SHORT, move |request| {
        if request.method != "Runtime.evaluate" {
            return Reply::ok();
        }
        let n = counter.fetch_add(1, Ordering::SeqCst);
        Reply::value(json!(if n < 2 { "loading" } else { "interactive" }))
    })
    .await;
    wait_document_ready(&cdp, SHORT, &cancel).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn login_probe_status_is_normalised_like_the_reference() {
    let cancel = CancellationToken::new();
    for (status, expected) in [
        (json!(200), "200"),
        (json!("200"), "200"),
        (json!(" 401 "), "401"),
        (json!("1.5"), "1.5"),
        (json!("inf"), "0"),
        (json!("PUBLIC-not-a-number"), "0"),
        (json!(null), "0"),
        (json!(true), "0"),
    ] {
        let probe = json!({"ok": true, "status": status, "backendStatus": 204, "sessionAuthenticated": true});
        let (cdp, _peer) = peer(SHORT, move |_| Reply::value(probe.clone())).await;
        let (lines, log) = recorder();
        ensure_logged_in(
            &cdp,
            LoginHint {
                applied_cookies: 1,
                remote: false,
            },
            SHORT,
            &cancel,
            &log,
        )
        .await
        .unwrap();
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1, "{status}");
        assert_eq!(
            lines[0],
            format!(
                "Login check passed (sessionStatus={expected}, sessionAuthenticated=true, backendStatus=204, domLoginCta=false, appAuthenticated=false)"
            ),
            "{status}"
        );
    }
}

#[tokio::test]
async fn failed_login_with_applied_cookies_and_no_login_button_asks_to_refresh_the_session() {
    let cancel = CancellationToken::new();
    let (cdp, _peer) = peer(SHORT, |_| {
        Reply::value(json!({
            "ok": false,
            "status": "401",
            "pageUrl": "https://chatgpt.com/auth/login?next=PUBLIC#frag",
            "error": "PUBLIC-probe-error"
        }))
    })
    .await;
    let (lines, log) = recorder();
    let error = ensure_logged_in(
        &cdp,
        LoginHint {
            applied_cookies: 3,
            remote: false,
        },
        SHORT,
        &cancel,
        &log,
    )
    .await
    .unwrap_err();
    assert!(error.is::<SessionMissing>());
    assert_eq!(
        error.to_string(),
        "ChatGPT session not detected. ChatGPT login appears missing; open chatgpt.com in Chrome to refresh the session or provide inline cookies (--browser-inline-cookies[(-file)] / ORACLE_BROWSER_COOKIES_JSON)."
    );
    let lines = lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("sessionStatus=401"), "{}", lines[0]);
    assert!(
        lines[0].contains("url=https://chatgpt.com/auth/login,"),
        "{}",
        lines[0]
    );
    assert!(
        lines[0].contains("error=PUBLIC-probe-error"),
        "{}",
        lines[0]
    );
}

#[tokio::test]
async fn manual_login_wait_returns_errors_other_than_a_missing_session_at_once() {
    let cancel = CancellationToken::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let (cdp, _peer) = peer(SHORT, move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        Reply::thrown("Error: PUBLIC-probe-crash")
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let (lines, log) = recorder();
    let error = wait_for_login(
        &cdp,
        true,
        LoginHint {
            applied_cookies: 1,
            remote: false,
        },
        Duration::from_secs(3),
        dir.path(),
        SHORT,
        &cancel,
        &log,
    )
    .await
    .unwrap_err();
    assert!(!error.is::<SessionMissing>());
    assert!(
        format!("{error:#}").contains("PUBLIC-probe-crash"),
        "{error:#}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(lines.lock().unwrap().is_empty());
}

#[tokio::test]
async fn dialogs_are_logged_answered_and_answer_failures_are_reported() {
    let cancel = CancellationToken::new();
    let mut events: Vec<Value> = (0..5000)
        .map(|n| json!({"method": "Network.dataReceived", "params": {"n": n}}))
        .collect();
    events.push(json!({
        "method": "Page.javascriptDialogOpening",
        "sessionId": "PUBLIC-other",
        "params": {"type": "alert", "message": "PUBLIC foreign"}
    }));
    events.push(json!({
        "method": "Page.javascriptDialogOpening",
        "params": {"type": "alert", "message": ""}
    }));
    events.push(json!({
        "method": "Page.javascriptDialogOpening",
        "params": {"type": "confirm", "message": "PUBLIC question"}
    }));
    let (cdp, _peer) = peer(SHORT, move |request| match request.method.as_str() {
        "Page.enable" => Reply::WithEvents(json!({}), std::mem::take(&mut events)),
        "Page.handleJavaScriptDialog" => Reply::Error("PUBLIC-no-dialog".into()),
        _ => Reply::ok(),
    })
    .await;
    let (lines, log) = recorder();
    let guard = dismiss_dialogs(&cdp, SHORT, &cancel, log).await.unwrap();
    for _ in 0..200 {
        if lines.lock().unwrap().len() >= 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let lines = lines.lock().unwrap().clone();
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert_eq!(lines[0], "[nav] accepting JS dialog (alert)");
    assert!(
        lines[1].starts_with("[nav] failed to answer JS dialog: "),
        "{lines:?}"
    );
    assert_eq!(
        lines[2],
        "[nav] dismissing JS dialog (confirm): PUBLIC question"
    );
    assert!(
        lines[3].starts_with("[nav] failed to answer JS dialog: "),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|line| line.contains("PUBLIC foreign")));
    drop(guard);
}

#[tokio::test]
async fn challenge_ui_warning_is_labelled_authentication_challenge() {
    let cancel = CancellationToken::new();
    let (cdp, _peer) = peer(SHORT, |_| {
        Reply::value(json!([{"text": "  Please verify\n you are human "}]))
    })
    .await;
    let (lines, log) = recorder();
    let warning = ui_warning(&cdp, "PUBLIC answer", SHORT, &cancel, &log).await;
    assert_eq!(
        warning.as_deref(),
        Some(
            "ChatGPT displayed a authentication/challenge warning while waiting for PUBLIC answer: Please verify you are human"
        )
    );
    assert_eq!(
        *lines.lock().unwrap(),
        ["[browser] ChatGPT UI warning detected (auth_or_challenge): Please verify you are human"]
    );
}
