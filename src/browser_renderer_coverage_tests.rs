//! Coverage tests for renderer reservation state guards, Grok prompt preparation and the
//! GPT-6 model and effort picker outcomes, driven by a scripted CDP peer.

use super::*;
use crate::cdp::fake::{Reply, Request, peer};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

const RESERVATION: &str = "PUBLIC-reservation";
const SHORT: Duration = Duration::from_secs(5);

/// Which renderer script a scripted request belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Submission,
    Health,
    Prepare,
    Finish,
    Model,
    Thinking,
    Other,
}

fn classify(request: &Request, arg: &Value) -> Kind {
    if arg.get("provider").is_some() {
        Kind::Submission
    } else if *arg == json!({}) {
        Kind::Health
    } else if arg.get("token").is_some() && arg.get("rendererId").is_none() {
        Kind::Prepare
    } else if request.expression().contains(FINISH.trim()) {
        Kind::Finish
    } else if arg.get("level").is_some() {
        Kind::Thinking
    } else if arg.get("model").is_some() {
        Kind::Model
    } else {
        Kind::Other
    }
}

/// An inspection acknowledgement that matches the reservation's token and owner.
fn owned(arg: &Value) -> Value {
    json!({
        "token": arg["token"],
        "parentReservationId": RESERVATION,
        "rendererId": "PUBLIC-renderer",
        "acknowledged": true
    })
}

fn default_reply(kind: Kind, arg: &Value) -> Reply {
    match kind {
        Kind::Submission => Reply::value(json!(true)),
        Kind::Health => Reply::value(json!({})),
        Kind::Prepare => Reply::value(owned(arg)),
        Kind::Finish => {
            let mut value = owned(arg);
            value["cleanupComplete"] = json!(true);
            Reply::value(value)
        }
        _ => Reply::Error("PUBLIC-unscripted".into()),
    }
}

async fn scripted<F>(
    provider: RendererProvider,
    state: DispatchState,
    mut answer: F,
) -> RendererReservation
where
    F: FnMut(Kind, &Value) -> Option<Reply> + Send + 'static,
{
    let (page, _trace) = peer(Duration::from_secs(5), move |request| {
        let arg = request.arg();
        let kind = classify(request, &arg);
        answer(kind, &arg).unwrap_or_else(|| default_reply(kind, &arg))
    })
    .await;
    RendererReservation {
        page,
        provider,
        grok_intent: None,
        owner: SubmissionOwner {
            claim_id: "PUBLIC-claim".into(),
            reservation_id: RESERVATION.into(),
        },
        state,
        input_send_issued: false,
        inspection: None,
        epoch: 0,
        dispatch_not_started: true,
        selection_sealed: false,
        retirement_barrier: false,
        pending_work: 0,
        unconfirmed_work: false,
        settlement_confirmed: false,
        notes: Vec::new(),
    }
}

fn unverified_picker(reason: &str) -> String {
    format!("Picker selection is unverified ({reason}); refusing to submit.")
}

#[tokio::test]
async fn inactive_states_refuse_mutation_release_and_report_settlement() {
    let cancel = CancellationToken::new();
    for (state, assertable, releasable, settled) in [
        (DispatchState::NotStarted, true, true, false),
        (DispatchState::Started, true, false, false),
        (DispatchState::Acknowledged, true, true, false),
        (DispatchState::Quarantined, false, false, true),
        (DispatchState::Released, false, false, true),
    ] {
        let mut owner = scripted(RendererProvider::Chatgpt, state, |_, _| None).await;
        owner.settlement_confirmed = true;
        assert_eq!(owner.settled_before_dispatch(), settled, "{state:?}");

        let asserted = owner.assert_active(SHORT, &cancel).await;
        if assertable {
            asserted.unwrap();
        } else {
            assert_eq!(
                asserted.unwrap_err().to_string(),
                "Renderer reservation is inactive; refusing mutation"
            );
        }

        let released = owner.release_clean(SHORT).await;
        if releasable {
            released.unwrap();
            assert_eq!(owner.state(), DispatchState::Released);
            assert!(owner.settlement_confirmed);
        } else {
            assert_eq!(
                released.unwrap_err().to_string(),
                "Uncertain submission cannot release renderer ownership"
            );
            assert_eq!(owner.state(), state);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn grok_prompt_waits_out_pending_mode_and_lagging_inspection() {
    let actions = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = actions.clone();
    let mut choose = 0;
    let mut inspect = 0;
    let mut owner = scripted(
        RendererProvider::Grok,
        DispatchState::NotStarted,
        move |kind, arg| {
            if kind != Kind::Other {
                return None;
            }
            let action = arg["action"].as_str().unwrap_or_default().to_owned();
            seen.lock().unwrap().push(action.clone());
            Some(Reply::value(match action.as_str() {
                "open-mode" => json!(true),
                "choose-mode" => {
                    choose += 1;
                    if choose == 1 {
                        json!({"status": "pending"})
                    } else {
                        json!({"status": "selected", "mode": "Auto"})
                    }
                }
                "inspect" => {
                    inspect += 1;
                    json!({"mode": if inspect == 1 {"Fast"} else {"Auto"}})
                }
                "type" if arg["prompt"] == "PUBLIC prompt" && arg["mode"] == "Auto" => json!(true),
                _ => json!(false),
            }))
        },
    )
    .await;
    let mode = owner
        .prepare_grok_prompt("PUBLIC prompt", SHORT, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(mode, "Auto");
    assert_eq!(
        *actions.lock().unwrap(),
        [
            "open-mode",
            "choose-mode",
            "choose-mode",
            "inspect",
            "inspect",
            "type"
        ]
    );
    assert!(owner.selection_sealed);
    assert_eq!(owner.state(), DispatchState::NotStarted);
}

#[tokio::test]
async fn grok_prompt_rejects_an_unconfirmed_mode_status() {
    let mut owner = scripted(
        RendererProvider::Grok,
        DispatchState::NotStarted,
        |kind, arg| {
            if kind != Kind::Other {
                return None;
            }
            Some(Reply::value(match arg["action"].as_str() {
                Some("choose-mode") => json!({"status": "PUBLIC-closed"}),
                _ => json!(true),
            }))
        },
    )
    .await;
    let error = owner
        .prepare_grok_prompt("PUBLIC prompt", SHORT, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Grok mode selection is unconfirmed");
    assert!(owner.selection_sealed);
    assert_eq!(owner.state(), DispatchState::NotStarted);
}

#[tokio::test]
async fn unacknowledged_inspection_reservation_quarantines_the_target() {
    let mut owner = scripted(
        RendererProvider::Chatgpt,
        DispatchState::NotStarted,
        |kind, _| {
            if kind == Kind::Prepare {
                Some(Reply::value(json!({"acknowledged": false})))
            } else {
                None
            }
        },
    )
    .await;
    let error = owner
        .inspect_current_model(SHORT, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Picker inspection reservation was not acknowledged"
    );
    assert_eq!(owner.state(), DispatchState::Quarantined);
    assert!(owner.unconfirmed_work);
    assert!(owner.settlement_confirmed);
    assert_eq!(owner.epoch, 1);
}

#[tokio::test]
async fn unacknowledged_picker_cleanup_names_the_picker_and_refuses_reuse() {
    let cancel = CancellationToken::new();
    for effort in [true, false] {
        let mut owner = scripted(
            RendererProvider::Chatgpt,
            DispatchState::NotStarted,
            |kind, _| match kind {
                Kind::Model | Kind::Thinking => Some(Reply::value(json!({"status": "ready"}))),
                Kind::Finish => Some(Reply::value(json!({"cleanupState": "menu-visible"}))),
                _ => None,
            },
        )
        .await;
        let result = if effort {
            owner.inspect_current_effort("pro", SHORT, &cancel).await
        } else {
            owner.inspect_current_model(SHORT, &cancel).await
        };
        let error = result.unwrap_err();
        let picker = if effort { "effort" } else { "model" };
        let text = format!("{error:#}");
        assert!(
            text.contains(&format!(
                "Picker cleanup is unconfirmed ({picker} picker, finish not acknowledged); refusing target reuse"
            )),
            "{text}"
        );
        assert!(error.downcast_ref::<crate::cdp::NotSent>().is_some());
        assert_eq!(owner.state(), DispatchState::Quarantined);
        assert!(owner.unconfirmed_work);
        assert!(owner.inspection.is_none());
    }
}

#[tokio::test]
async fn model_picker_outcomes_name_the_unverified_selection() {
    let cancel = CancellationToken::new();
    for (outcome, health_fails, reason) in [
        (
            json!({"status": "selection-invalid", "reason": "PUBLIC-reason"}),
            false,
            "PUBLIC-reason",
        ),
        (
            json!({"status": "selection-invalid"}),
            false,
            "model selection returned unknown evidence",
        ),
        (
            json!({"status": "already-selected", "label": "  "}),
            false,
            "model selection returned unknown evidence",
        ),
        (
            json!({"status": "PUBLIC-other", "label": "GPT-6"}),
            false,
            "model selection returned unknown evidence",
        ),
        (
            json!({"status": "option-not-found"}),
            true,
            "model option is unavailable or ambiguous",
        ),
        (
            json!({"status": "option-not-found"}),
            false,
            "model option is unavailable or ambiguous",
        ),
    ] {
        let reply = outcome.clone();
        let mut owner = scripted(
            RendererProvider::Chatgpt,
            DispatchState::NotStarted,
            move |kind, _| match kind {
                Kind::Model => Some(Reply::value(reply.clone())),
                Kind::Health if health_fails => Some(Reply::Error("PUBLIC-health".into())),
                _ => None,
            },
        )
        .await;
        let Err(error) = owner
            .verify_pair("GPT-6", "extra-high", SHORT, &cancel)
            .await
        else {
            panic!("{outcome} was verified");
        };
        assert_eq!(error.to_string(), unverified_picker(reason), "{outcome}");
        assert_eq!(owner.unconfirmed_work, health_fails, "{outcome}");
        assert_eq!(owner.state(), DispatchState::NotStarted);
        assert_eq!(owner.epoch, 1);
    }

    let mut owner = scripted(
        RendererProvider::Chatgpt,
        DispatchState::NotStarted,
        |_, _| None,
    )
    .await;
    let hydration = unverified_picker("model hydration did not produce owned evidence");
    for level in ["pro", "extra-high"] {
        let Err(error) = owner
            .verify_pair("GPT-6", level, Duration::ZERO, &cancel)
            .await
        else {
            panic!("zero deadline verified {level}");
        };
        assert_eq!(error.to_string(), hydration);
    }
    let Err(error) = owner
        .select_pair("select", "Latest", "pro", Duration::ZERO, &cancel)
        .await
    else {
        panic!("zero deadline selected a pair");
    };
    assert_eq!(error.to_string(), hydration);
    assert_eq!(owner.epoch, 0);
}

#[tokio::test(start_paused = true)]
async fn model_button_that_never_mounts_fails_after_the_wait() {
    for status in ["pending", "button-missing"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let mut owner = scripted(
            RendererProvider::Chatgpt,
            DispatchState::NotStarted,
            move |kind, _| {
                if kind == Kind::Model {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Some(Reply::value(json!({"status": status})))
                } else {
                    None
                }
            },
        )
        .await;
        let started = Instant::now();
        let Err(error) = owner
            .verify_pair(
                "GPT-6",
                "pro",
                Duration::from_secs(30),
                &CancellationToken::new(),
            )
            .await
        else {
            panic!("{status} was verified");
        };
        assert_eq!(
            error.to_string(),
            unverified_picker("model hydration did not produce owned evidence")
        );
        assert!(started.elapsed() >= MODEL_BUTTON_WAIT);
        assert!(calls.load(Ordering::SeqCst) > 1);
        assert_eq!(
            owner.take_notes(),
            [
                "Model picker button not mounted yet; waiting up to 20s for the composer pill to render."
            ]
        );
        assert!(owner.take_notes().is_empty());
    }
}

#[tokio::test]
async fn effort_picker_outcomes_fail_closed_with_reference_notes() {
    let cancel = CancellationToken::new();
    let model = json!({
        "status": "already-selected",
        "label": "GPT-6",
        "checkedCount": 1,
        "ownerKey": "PUBLIC-owner"
    });
    let unverified =
        "Browser model and thinking effort could not be verified; refusing to submit.".to_owned();
    let cases: [(&str, Value, String, Option<&str>); 8] = [
        (
            "extra-high",
            json!({"status": "pending"}),
            unverified_picker("effort selection closed before verification"),
            None,
        ),
        (
            "extra-high",
            json!({"status": "selection-invalid", "reason": "PUBLIC-effort-reason"}),
            unverified_picker("PUBLIC-effort-reason"),
            None,
        ),
        (
            "pro",
            json!({"status": "selection-invalid"}),
            unverified_picker("owned effort evidence is unavailable"),
            None,
        ),
        (
            "extra-high",
            json!({"status": "selection-unverified"}),
            unverified.clone(),
            Some(
                "[browser] Thinking time: selection unverified (requested Extra-high); the effort in ChatGPT is unconfirmed.",
            ),
        ),
        (
            "extra-high",
            json!({"status": "model-kind-not-found", "modelKind": "PUBLIC-kind"}),
            unverified.clone(),
            Some(
                "[browser] Thinking time: model kind not found for PUBLIC-kind (requested Extra-high); keeping the effort already selected in ChatGPT.",
            ),
        ),
        (
            "extra-high",
            json!({"status": "chip-not-found", "modelKind": "PUBLIC-kind"}),
            unverified.clone(),
            Some(
                "[browser] Thinking time: chip not found (requested Extra-high); keeping the effort already selected in ChatGPT.",
            ),
        ),
        (
            "pro",
            json!({"status": "PUBLIC-mystery", "diagnostic": {"step": "PUBLIC"}}),
            "Thinking time: unknown outcome selecting Pro; refusing to submit without confirmed Pro."
                .to_owned(),
            Some("[browser] Model picker diagnostic: {\"step\":\"PUBLIC\"}"),
        ),
        (
            "extra-high",
            json!({"status": "PUBLIC-mystery"}),
            unverified.clone(),
            Some(
                "[browser] Thinking time: unknown outcome selecting Extra-high; continuing with ChatGPT default.",
            ),
        ),
    ];
    for (level, outcome, message, note) in cases {
        let model_reply = model.clone();
        let effort_reply = outcome.clone();
        let mut owner = scripted(
            RendererProvider::Chatgpt,
            DispatchState::NotStarted,
            move |kind, _| match kind {
                Kind::Model => Some(Reply::value(model_reply.clone())),
                Kind::Thinking => Some(Reply::value(effort_reply.clone())),
                _ => None,
            },
        )
        .await;
        let Err(error) = owner
            .select_pair("select", "GPT-6", level, SHORT, &cancel)
            .await
        else {
            panic!("{level} {outcome} was selected");
        };
        assert_eq!(error.to_string(), message, "{level} {outcome}");
        let notes = owner.take_notes();
        assert!(
            notes.iter().any(|n| n == "Model picker: GPT-6"),
            "{notes:?}"
        );
        match note {
            Some(expected) => assert!(notes.iter().any(|n| n == expected), "{notes:?}"),
            None => assert!(
                !notes
                    .iter()
                    .any(|n| n.starts_with("[browser] Thinking time")),
                "{notes:?}"
            ),
        }
        assert_eq!(owner.state(), DispatchState::NotStarted);
        assert!(!owner.unconfirmed_work);
    }
}

#[tokio::test]
async fn verify_pair_accepts_slider_evidence_and_rejects_unknown_basis() {
    let cancel = CancellationToken::new();
    for (basis, verified) in [
        ("slider-label-and-index", true),
        ("PUBLIC-unknown-basis", false),
    ] {
        let mut owner = scripted(
            RendererProvider::Chatgpt,
            DispatchState::NotStarted,
            move |kind, _| match kind {
                Kind::Model => Some(Reply::value(json!({
                    "status": "already-selected",
                    "label": "GPT-6",
                    "checkedCount": 1,
                    "ownerKey": "PUBLIC-owner"
                }))),
                Kind::Thinking => Some(Reply::value(json!({
                    "status": "ready",
                    "selectedLevel": "extra-high",
                    "ownerKey": "PUBLIC-owner",
                    "basis": basis
                }))),
                _ => None,
            },
        )
        .await;
        let result = owner
            .verify_pair("Latest", "extra-high", SHORT, &cancel)
            .await;
        if verified {
            let (model, thinking, effective) = match result {
                Ok(pair) => pair,
                Err(error) => panic!("{error:#}"),
            };
            assert_eq!(effective, "gpt-6-astra");
            assert_eq!(model.requested_model.as_deref(), Some("Latest"));
            assert_eq!(model.extra.get("verificationOnly"), Some(&json!(true)));
            assert_eq!(
                thinking.extra.get("status"),
                Some(&json!("already-selected"))
            );
            assert_eq!(owner.epoch, 4);
            assert!(owner.inspection.is_some());
        } else {
            let Err(error) = result else {
                panic!("unknown basis was verified");
            };
            assert_eq!(
                error.to_string(),
                "Independent thinking-tier observation is missing or incorrect"
            );
            assert_eq!(owner.epoch, 3);
        }
        assert!(
            owner
                .take_notes()
                .iter()
                .any(|n| n == "Model picker: GPT-6")
        );
        assert_eq!(owner.state(), DispatchState::NotStarted);
    }
}

#[tokio::test]
async fn gemini_acknowledgement_requires_started_gemini_reservation_and_live_ownership() {
    let cancel = CancellationToken::new();
    let receipt = json!({"id": "00000000-0000-4000-8000-000000000007"});
    for (provider, state) in [
        (RendererProvider::Gemini, DispatchState::NotStarted),
        (RendererProvider::Chatgpt, DispatchState::Started),
    ] {
        let mut owner = scripted(provider, state, |_, _| None).await;
        let error = owner
            .acknowledge_gemini(receipt.clone(), SHORT, &cancel)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Gemini receipt requires its original started reservation"
        );
        assert_eq!(owner.state(), state);
    }

    let mut owner = scripted(
        RendererProvider::Gemini,
        DispatchState::Started,
        |kind, arg| {
            (kind == Kind::Other && arg["action"] == "acknowledge")
                .then(|| Reply::value(json!(false)))
        },
    )
    .await;
    let error = owner
        .acknowledge_gemini(receipt, SHORT, &cancel)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Gemini's original Temporary Chat no longer owns this dispatch"
    );
    assert_eq!(owner.state(), DispatchState::Started);
}
