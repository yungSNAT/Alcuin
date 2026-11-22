use super::*;
use crate::cdp::fake::{self, Reply};

const LANDING: &str = "https://chatgpt.com/?temporary-chat=true";

fn signed_in() -> Value {
    json!({"ok":false,"pageUrl":"https://chatgpt.com/","domLoginCta":false,"cfBlocked":false,"sessionResolved":true,"sessionAuthenticated":true})
}

fn signed_out() -> Value {
    json!({"ok":false,"pageUrl":"https://chatgpt.com/","domLoginCta":true,"cfBlocked":false,"sessionResolved":true,"sessionAuthenticated":false})
}

fn guard_ok() -> Value {
    json!({"throttled":false,"modelsLoadFailed":false})
}

fn observed_model() -> Value {
    json!({"status":"already-selected","label":"GPT-6","checkedCount":1,"ownerKey":"PUBLIC-model"})
}

fn ready_effort() -> Value {
    json!({"status":"ready","selectedLevel":"pro","ownerKey":"PUBLIC-effort","basis":"slider-label-and-index","capabilityKey":null})
}

struct Script {
    login: Vec<Value>,
    guard: Vec<Value>,
    model: Vec<Value>,
    effort: Vec<Value>,
}

fn step(seq: &[Value], n: &mut usize) -> Value {
    let value = seq[(*n).min(seq.len() - 1)].clone();
    *n += 1;
    value
}

fn dom_calls(trace: &[Value], name: &str) -> usize {
    let marker = format!("alcuin-dom:{name}");
    trace
        .iter()
        .filter(|m| {
            m["params"]["expression"]
                .as_str()
                .and_then(|e| e.split_once(marker.as_str()))
                .is_some_and(|(_, rest)| {
                    !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-')
                })
        })
        .count()
}

async fn run_page(script: Script) -> (Value, Vec<Value>) {
    let mut counts = [0usize; 4];
    let (page, peer) = fake::peer(Duration::from_secs(1), move |req| {
        match req.method.as_str() {
            "Page.enable" | "Runtime.enable" => Reply::ok(),
            "Page.navigate" => Reply::Result(json!({"frameId":"PUBLIC-frame"})),
            "Runtime.evaluate" if req.expression() == "location.href" => {
                Reply::value(json!(LANDING))
            }
            "Runtime.evaluate" => {
                let cfg = req.arg();
                match req.dom() {
                    Some("health-login") => Reply::value(step(&script.login, &mut counts[0])),
                    Some("health-guard") => Reply::value(step(&script.guard, &mut counts[1])),
                    Some("model") => Reply::value(step(&script.model, &mut counts[2])),
                    Some("thinking") => Reply::value(step(&script.effort, &mut counts[3])),
                    Some("navigation") => Reply::value(
                        json!({"currentUrl":LANDING,"workSelected":false,"modeUnverified":false}),
                    ),
                    Some("submission") => Reply::value(json!(true)),
                    Some("inspection-prepare") => Reply::value(json!({
                        "token":cfg["token"],
                        "parentReservationId":cfg["owner"]["reservationId"],
                        "rendererId":"PUBLIC-renderer",
                        "acknowledged":true
                    })),
                    Some("inspection-finish") => Reply::value(json!({
                        "token":cfg["token"],
                        "parentReservationId":cfg["owner"]["reservationId"],
                        "rendererId":cfg["rendererId"],
                        "acknowledged":true,
                        "cleanupComplete":true
                    })),
                    other => Reply::Error(format!("unexpected script {other:?}")),
                }
            }
            other => Reply::Error(format!("unexpected method {other}")),
        }
    })
    .await;
    let mut result = json!({"status":"controls_broken","observations":{"authenticated":false,"modelVerified":false,"effortVerified":false}});
    inspect_page(
        &page,
        &mut result,
        "pro",
        Duration::from_secs(1),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    page.disconnect_and_wait(Duration::from_secs(1))
        .await
        .unwrap();
    let trace = peer.await.unwrap();
    (result, trace)
}

#[tokio::test]
async fn inspect_page_retries_blank_page_pending_model_and_pending_effort_until_ready() {
    let (result, trace) = run_page(Script {
        login: vec![json!({"pageUrl":"about:blank"}), signed_in()],
        guard: vec![guard_ok()],
        model: vec![json!({"status":"pending"}), observed_model()],
        effort: vec![json!({"status":"pending"}), ready_effort()],
    })
    .await;
    assert_eq!(result["status"], "ready");
    assert_eq!(
        result["observations"],
        json!({"authenticated":true,"modelVerified":true,"effortVerified":true})
    );
    assert_eq!(dom_calls(&trace, "health-login"), 3);
    assert_eq!(dom_calls(&trace, "model"), 4);
    assert_eq!(dom_calls(&trace, "thinking"), 3);
}

#[tokio::test]
async fn inspect_page_stops_when_guard_throttles_after_reservation() {
    let (result, trace) = run_page(Script {
        login: vec![signed_in()],
        guard: vec![
            guard_ok(),
            json!({"throttled":true,"modelsLoadFailed":false}),
        ],
        model: vec![observed_model()],
        effort: vec![ready_effort()],
    })
    .await;
    assert_eq!(result["status"], "throttled");
    assert_eq!(result["observations"]["authenticated"], true);
    assert_eq!(result["observations"]["modelVerified"], false);
    assert_eq!(dom_calls(&trace, "model"), 0);
    assert_eq!(dom_calls(&trace, "thinking"), 0);
}

#[tokio::test]
async fn inspect_page_reports_sign_in_when_access_recheck_fails() {
    let (result, trace) = run_page(Script {
        login: vec![signed_in(), signed_out()],
        guard: vec![guard_ok()],
        model: vec![observed_model()],
        effort: vec![ready_effort()],
    })
    .await;
    assert_eq!(result["status"], "sign_in_required");
    assert_eq!(result["observations"]["authenticated"], false);
    assert_eq!(result["observations"]["modelVerified"], false);
    assert_eq!(result["observations"]["effortVerified"], false);
    assert_eq!(dom_calls(&trace, "health-login"), 2);
    assert_eq!(dom_calls(&trace, "model"), 2);
}

#[test]
fn model_classifies_pending_and_ambiguous_evidence() {
    for (input, status) in [
        (json!({"status":"pending"}), "pending"),
        (json!({"status":"button-missing"}), "pending"),
        (
            json!({"status":"already-selected","label":"GPT-6","ownerKey":"PUBLIC-o","checkedCount":2}),
            "ambiguous",
        ),
        (
            json!({"status":"already-selected","ownerKey":"PUBLIC-o","checkedCount":1}),
            "ambiguous",
        ),
        (
            json!({"status":"already-selected","label":"GPT-6","ownerKey":"","checkedCount":1}),
            "ambiguous",
        ),
        (json!({"status":"other"}), "ambiguous"),
        (observed_model(), "observed"),
    ] {
        let out = model(&input);
        assert_eq!(out["status"], status, "{input}");
    }
    let empty = model(&json!({}));
    assert!(empty["label"].is_null());
    assert!(empty["ownerKey"].is_null());
    assert_eq!(empty["checkedCount"].as_f64(), Some(0.0));
}

#[test]
fn version_accepts_only_dotted_digit_pairs() {
    for (input, expected) in [
        (json!("5.5"), json!("5.5")),
        (json!("10.25"), json!("10.25")),
        (json!("5."), Value::Null),
        (json!(".5"), Value::Null),
        (json!("a.5"), Value::Null),
        (json!("5.a"), Value::Null),
        (json!("55"), Value::Null),
        (json!("5.5.1"), Value::Null),
        (json!(5.5), Value::Null),
    ] {
        assert_eq!(version(&input), expected, "{input}");
    }
}

#[test]
fn effort_keeps_known_statuses_and_reports_variant_kind() {
    for status in ["selection_unknown", "controls_broken", "pending"] {
        assert_eq!(effort(&json!({"status":status}))["status"], status);
    }
    assert_eq!(
        effort(&json!({"status":"bogus"}))["status"],
        "selection_unknown"
    );
    let mut input = ready_effort();
    input["variantCanonicalVersion"] = json!("5.5");
    input["canonicalVersion"] = json!("6.0");
    input["observedModelKind"] = json!("thinking");
    let out = effort(&input);
    assert_eq!(out["status"], "ready");
    assert_eq!(out["selectedLevel"], "pro");
    assert_eq!(out["variantCanonicalVersion"], "5.5");
    assert_eq!(out["canonicalVersion"], "6.0");
    assert_eq!(out["observedModelKind"], "thinking");
    input["observedModelKind"] = json!("PUBLIC-other");
    assert!(effort(&input)["observedModelKind"].is_null());
    input["variantCanonicalVersion"] = json!("bad");
    input["observedModelKind"] = json!("instant");
    assert!(effort(&input)["observedModelKind"].is_null());
}

#[tokio::test(start_paused = true)]
async fn pause_waits_or_fails_when_cancelled() {
    let cancel = CancellationToken::new();
    assert!(pause(&cancel).await.is_ok());
    cancel.cancel();
    let error = pause(&cancel).await.unwrap_err();
    assert_eq!(error.to_string(), "Browser health cancelled");
}

#[test]
fn format_adds_recovery_guidance_per_reason_and_status() {
    for (input, head, recovery) in [
        (
            json!({"status":"controls_broken","reason":"challenge_detected"}),
            "Browser health: controls_broken (challenge_detected). Advisory snapshot; cleanup unconfirmed.",
            "A security challenge was detected",
        ),
        (
            json!({"status":"controls_broken","reason":"admission_deadline_exceeded","cleanupComplete":true}),
            "Browser health: controls_broken (admission_deadline_exceeded). Advisory snapshot; cleanup confirmed.",
            "Health did not obtain browser admission",
        ),
        (
            json!({"status":"controls_broken","reason":"browser_identity_unverified"}),
            "Browser health: controls_broken (browser_identity_unverified). Advisory snapshot; cleanup unconfirmed.",
            "Browser identity or profile ownership could not be verified",
        ),
        (
            json!({"status":"sign_in_required","cleanupComplete":true}),
            "Browser health: sign_in_required. Advisory snapshot; cleanup confirmed.",
            "Sign in manually in the dedicated browser profile",
        ),
        (
            json!({"status":"browser_missing"}),
            "Browser health: browser_missing. Advisory snapshot; cleanup unconfirmed.",
            "Set --browser-chrome-path",
        ),
        (
            json!({"status":"throttled"}),
            "Browser health: throttled. Advisory snapshot; cleanup unconfirmed.",
            "Allow the account limit to clear",
        ),
        (
            json!({}),
            "Browser health: controls_broken. Advisory snapshot; cleanup unconfirmed.",
            "Health could not verify the browser state.",
        ),
    ] {
        let text = format(&input);
        let (first, rest) = text.split_once('\n').expect("recovery line");
        assert_eq!(first, head, "{input}");
        assert!(rest.starts_with(recovery), "{input}: {rest}");
    }
    assert_eq!(
        format(&json!({"status":"ready","cleanupComplete":true})),
        "Browser health: ready. Advisory snapshot; cleanup confirmed."
    );
}
