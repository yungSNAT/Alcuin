//! Coverage tests for Web Search selection: blocked guard cleanup and unexpected selection results.

use super::*;
use crate::cdp::fake::{Reply, Request, peer};
use std::sync::{Arc, Mutex};

/// Classify one command the selection code sent, by method or by the composer script body.
fn classify(request: &Request) -> &'static str {
    if request.method == "Input.dispatchKeyEvent" {
        return match request.params["type"].as_str() {
            Some("keyDown") => "keyDown",
            Some("keyUp") => "keyUp",
            _ => "key-other",
        };
    }
    let expression = request.expression();
    if expression.contains("const summary = { cleaned: true, sawKeyDown") {
        "cleanup"
    } else if expression.contains("window.addEventListener('keydown', onKey") {
        "prepare"
    } else if expression.contains("promptMatches:") {
        "verify"
    } else if expression.contains("const candidates = Array.from(context.form.querySelectorAll") {
        "probe"
    } else if expression.contains("const matchesLabel =") {
        "select"
    } else if expression.contains("const expected = cfg.navigation") {
        "navigation"
    } else {
        "ownership"
    }
}

/// Run `activate` against a scripted page whose select and cleanup answers are given.
async fn run_activation(select: Value, cleanup: Value) -> (Result<bool>, Vec<String>, bool) {
    let phases = Arc::new(Mutex::new(Vec::new()));
    let seen = phases.clone();
    let (page, peer_task) = peer(Duration::from_secs(2), move |request| {
        let phase = classify(request);
        seen.lock().unwrap().push(phase.to_owned());
        let value = match phase {
            "cleanup" => cleanup.clone(),
            "prepare" => json!({
                "contextMatches": true,
                "workSelected": false,
                "modeUnverified": false,
                "focused": true
            }),
            "probe" => json!({"status": "focused", "focused": true}),
            "verify" => json!({"selected": false, "promptMatches": true}),
            "navigation" => json!({
                "contextMatches": true,
                "workSelected": false,
                "modeUnverified": false
            }),
            "select" => select.clone(),
            "keyDown" | "keyUp" | "key-other" => json!({}),
            _ => json!(true),
        };
        Reply::value(value)
    })
    .await;
    let mut reservation = RendererReservation::acquire(
        page,
        "PUBLIC-coverage-owner".into(),
        Duration::from_secs(1),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let cfg = json!({
        "prompt": "PUBLIC coverage prompt",
        "navigation": {"origin": "https://chatgpt.com", "conversationId": null, "landingPath": "/"}
    });
    let result = activate(
        &mut reservation,
        cfg,
        Duration::from_millis(200),
        &CancellationToken::new(),
    )
    .await;
    let _ = reservation
        .quarantine(crate::browser_renderer::CONTROL_ACK_TIMEOUT)
        .await;
    let settled = reservation.settled_before_dispatch();
    reservation
        .page()
        .disconnect_and_wait(Duration::from_secs(1))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), peer_task)
        .await
        .unwrap()
        .unwrap();
    let trace = phases.lock().unwrap().clone();
    (result, trace, settled)
}

fn count(trace: &[String], phase: &str) -> usize {
    trace.iter().filter(|step| step.as_str() == phase).count()
}

#[test]
fn web_search_blocked_refusal_names_uncertain_cleanup() {
    let error = (WEB_SEARCH.blocked)(&json!("PUBLIC-overlay"));
    assert_eq!(
        error.to_string(),
        "Web Search tools activation or exact guard cleanup is uncertain; nothing was sent"
    );
}

#[tokio::test]
async fn blocked_guard_cleanup_refuses_before_selecting() {
    let (result, trace, settled) = run_activation(
        json!("clicked"),
        json!({
            "cleaned": true,
            "sawKeyDown": true,
            "clicked": true,
            "blocked": "PUBLIC-blocking-overlay"
        }),
    )
    .await;
    let error = result.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Web Search tools activation or exact guard cleanup is uncertain; nothing was sent"
    );
    for phase in ["keyDown", "keyUp", "cleanup"] {
        assert_eq!(count(&trace, phase), 1, "{phase}: {trace:?}");
    }
    assert_eq!(count(&trace, "select"), 0, "{trace:?}");
    assert!(!settled, "a blocked cleanup leaves input uncertain");
}

#[tokio::test]
async fn unexpected_selection_result_marks_input_uncertain_and_refuses() {
    let clean = json!({
        "cleaned": true,
        "sawKeyDown": true,
        "clicked": true,
        "blocked": null
    });
    for select in [json!("PUBLIC-unexpected"), json!({"status": "PUBLIC-odd"})] {
        let (result, trace, settled) = run_activation(select.clone(), clean.clone()).await;
        assert_eq!(result.unwrap_err().to_string(), UNVERIFIED, "{select}");
        assert_eq!(count(&trace, "cleanup"), 1, "{trace:?}");
        assert_eq!(
            count(&trace, "select"),
            1,
            "an unexpected answer is never retried: {trace:?}"
        );
        assert!(!settled, "unexpected selection must mark input uncertain");
    }
}
