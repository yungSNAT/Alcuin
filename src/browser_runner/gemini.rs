//! A Gemini text turn uses an owned target and a durable, single physical send.
use super::*;
use crate::{browser_attempt::Provider, browser_gemini as gemini};

pub(super) async fn navigate(
    page: &Cdp,
    limits: &RunnerLimits,
    cancel: &CancellationToken,
) -> Result<()> {
    let deadline = Instant::now() + limits.command;
    let navigated = page
        .call(
            "Page.navigate",
            json!({"url":gemini::URL}),
            limits.command,
            cancel,
        )
        .await?;
    anyhow::ensure!(
        navigated.get("errorText").is_none(),
        "Gemini navigation failed"
    );
    let entry = loop {
        let remaining = left(
            deadline,
            "Gemini did not offer Temporary Chat. Sign in with a personal Google account",
        )?;
        if page.evaluate(
            "location.origin === 'https://gemini.google.com' && document.readyState !== 'loading'",
            remaining, cancel,
        ).await? == true {
            let state = page.evaluate_with_arguments(gemini::DOM, &json!([{"action":"entry"}]), remaining, cancel).await?;
            if state == "active" || state == "clicked" {
                break state;
            }
        }
        pause(limits.poll.min(remaining), cancel).await?;
    };
    if entry == "active" {
        return Ok(());
    }
    loop {
        let remaining = left(deadline, "Gemini Temporary Chat did not become active")?;
        if page
            .evaluate_with_arguments(
                gemini::DOM,
                &json!([{"action":"privacy"}]),
                remaining,
                cancel,
            )
            .await?
            == true
        {
            return Ok(());
        }
        pause(limits.poll.min(remaining), cancel).await?;
    }
}

fn left(deadline: Instant, message: &str) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    anyhow::ensure!(!remaining.is_zero(), "{message}");
    Ok(remaining)
}

async fn select(
    reservation: &mut RendererReservation,
    dispatch: &str,
    limits: &RunnerLimits,
    cancel: &CancellationToken,
) -> Result<()> {
    let deadline = Instant::now() + limits.command;
    // Google's model row closes the picker. Reopen it for the thinking submenu.
    for action in [
        "open-picker",
        "show-neon",
        "open-picker",
        "show-levels",
        "select-high",
        "seal",
    ] {
        let failure = format!("Gemini 4 Neon and High thinking could not be verified ({action})");
        loop {
            let remaining = left(deadline, &failure)?;
            if reservation
                .evaluate_mutation(
                    gemini::DOM,
                    json!({"action":action,"id":dispatch}),
                    remaining,
                    cancel,
                )
                .await?
                == true
            {
                break;
            }
            pause(limits.poll.min(remaining), cancel).await?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // The target and its original worker stay explicit.
pub(super) async fn deliver(
    store: &SessionStore,
    id: &str,
    witness: &ExecutionWitness,
    request: &BrowserRequest,
    target: &OwnedTarget,
    endpoint: &AttachmentEndpoint,
    reservation: &mut RendererReservation,
    bringup: DirectoryLock,
    deferred_reconciliation: bool,
    limits: &RunnerLimits,
    cancel: &CancellationToken,
) -> Result<Delivered> {
    let link = crate::browser_attempt::require_child(store, id, cancel)?;
    anyhow::ensure!(
        link.provider == Provider::Gemini,
        "Gemini attempt is unavailable"
    );
    let dispatch = Uuid::new_v4().to_string();
    select(reservation, &dispatch, limits, cancel).await?;
    automation_log(
        store,
        id,
        "[browser] Gemini Temporary Chat verified, Gemini 4 Neon with High thinking.",
    )?;
    let model = gemini::model();
    let thinking = gemini::thinking();
    runtime_patch(
        store,
        id,
        witness,
        json!({"provider":"gemini","tabUrl":gemini::URL,
        "submissionOwner":reservation.owner(),"modelSelection":model,"thinkingSelection":thinking,
        "effectiveModel":"gemini-4-neon"}),
    )?;
    bringup.release()?;
    if deferred_reconciliation {
        crate::browser_retirement::reconcile(store, target.browser(), endpoint).await?;
    }
    let prompt = request.prompt.replace("\r\n", "\n");
    let entry = reservation
        .evaluate_mutation(
            gemini::DOM,
            json!({"action":"type","id":dispatch,"prompt":prompt}),
            limits.command,
            cancel,
        )
        .await?;
    anyhow::ensure!(
        entry == "entered",
        "Gemini composer could not be prepared ({})",
        entry.as_str().unwrap_or("invalid result")
    );
    let deadline = Instant::now() + limits.command;
    loop {
        let remaining = left(
            deadline,
            "Complete Gemini prompt entry could not be verified",
        )?;
        if reservation
            .evaluate_mutation(
                gemini::DOM,
                json!({"action":"verify-draft","id":dispatch,"prompt":prompt}),
                remaining,
                cancel,
            )
            .await?
            == true
        {
            break;
        }
        pause(limits.poll.min(remaining), cancel).await?;
    }
    let started = json!({"state":"started","reservationId":request.reservation_id,
        "submissionOwner":reservation.owner(),"promptSha256":request.prompt_sha256,
        "startedAt":timestamp(),"providerDispatchId":dispatch});
    store.update(id, |current| {
        witness.require(current)?;
        crate::browser_attempt::require_if_child(store, id, cancel)?;
        anyhow::ensure!(
            !cancel.is_cancelled()
                && current.get("dispatch").is_none()
                && current["browser"]["runtime"]["chromeTargetId"] == target.id()
                && current["browser"]["runtime"]["tabLeaseId"] == target.lease().id(),
            "Gemini target or dispatch changed before Send"
        );
        Ok(Some(json!({"dispatch":started,"recoveryRequired":true})))
    })?;
    reservation.dispatch_started(limits.command, cancel).await?;
    anyhow::ensure!(
        reservation
            .evaluate_mutation(
                gemini::DOM,
                json!({"action":"send","id":dispatch,"prompt":prompt}),
                limits.command,
                cancel
            )
            .await?
            == true,
        "Gemini Send could not be verified; retain the original target without replay"
    );
    let observed = observe(reservation, &dispatch, &prompt, limits, cancel).await?;
    let provider_receipt = gemini::receipt(reservation.owner(), &started);
    reservation
        .acknowledge_gemini(provider_receipt.clone(), limits.command, cancel)
        .await?;
    target
        .lease()
        .update(&json!({"promptSubmitted":true,"tabUrl":gemini::URL}))?;
    store.update(id, |current| {
        witness.require(current)?;
        crate::browser_attempt::require_if_child(store, id, cancel)?;
        anyhow::ensure!(!cancel.is_cancelled() && current["dispatch"] == started, "Gemini dispatch changed");
        Ok(Some(json!({"dispatch":merge_object(&started,&json!({"state":"acknowledged"})),
            "browser":merge_object(&current["browser"],&json!({"runtime":merge_object(&current["browser"]["runtime"],
            &json!({"geminiCommitReceipt":provider_receipt,"submittedPromptHash":request.prompt_sha256,"submittedPromptHashFormat":"gemini-request-v1"}))}))})))
    })?;
    let saved = store.require(id)?;
    gemini::accept_saved_answer(request, &saved, &observed)?;
    let outcome = OutcomeWitness {
        recovery: recovery_identity(&saved),
        options_sha256: request.options_sha256.clone(),
        dispatch: dispatch_identity(&saved),
    };
    let text = observed["text"]
        .as_str()
        .context("Gemini answer text is missing")?
        .to_owned();
    let receipt = capture_outcome(store, id, witness, &outcome, &text, Some(&observed), cancel)?;
    publish_answer(store, id, witness, &outcome, &receipt, cancel)?;
    Ok(Delivered {
        model,
        thinking,
        receipt,
        text,
        outcome,
    })
}

async fn observe(
    reservation: &mut RendererReservation,
    dispatch: &str,
    prompt: &str,
    limits: &RunnerLimits,
    cancel: &CancellationToken,
) -> Result<Value> {
    let deadline = Instant::now() + limits.answer;
    let mut previous = Value::Null;
    loop {
        let remaining = left(
            deadline,
            "Gemini did not complete its original Temporary Chat response",
        )?;
        let observed = reservation
            .evaluate_mutation(
                gemini::DOM,
                json!({"action":"observe","id":dispatch,"prompt":prompt}),
                remaining.min(limits.command),
                cancel,
            )
            .await?;
        if !observed.is_null() && observed == previous {
            return Ok(observed);
        }
        previous = observed;
        pause(remaining.min(Duration::from_millis(750)), cancel).await?;
    }
}

#[allow(clippy::too_many_arguments)] // Recovery is bound to the original request, target and worker.
pub(super) async fn recover(
    store: &SessionStore,
    id: &str,
    witness: &ExecutionWitness,
    saved: &Value,
    request: &BrowserRequest,
    target: &OwnedTarget,
    guard: &crate::browser_attempt::RecoveryGuard,
    limits: &RunnerLimits,
    cancel: &CancellationToken,
) -> Result<()> {
    let runtime = &saved["browser"]["runtime"];
    if runtime["answerSourceCapture"].is_object() {
        let observed = &runtime["acceptedObservation"];
        gemini::accept_saved_answer(request, saved, observed)?;
        let capture: CaptureReceipt =
            serde_json::from_value(runtime["answerSourceCapture"].clone())?;
        anyhow::ensure!(
            capture.session_id == id
                && saved["capturedAnswer"] == json!(capture)
                && saved["browser"]["capturedAnswer"] == json!(capture)
                && runtime["capturedAnswer"] == json!(capture)
                && capture.sha256
                    == artifacts::sha256(
                        observed["text"]
                            .as_str()
                            .context("Gemini capture text missing")?
                            .as_bytes()
                    ),
            "Recovered Gemini capture differs from the original answer"
        );
        artifacts::verify_capture(store, &capture)?;
        let outcome = OutcomeWitness {
            recovery: recovery_identity(saved),
            options_sha256: request.options_sha256.clone(),
            dispatch: dispatch_identity(saved),
        };
        return publish_answer(store, id, witness, &outcome, &capture, cancel);
    }
    let deadline = Instant::from_std(guard.deadline());
    let arguments = json!([{"action":"observe","recovery":true,"id":saved["dispatch"]["providerDispatchId"],
        "owner":runtime["submissionOwner"],"prompt":request.prompt.replace("\r\n", "\n")}]);
    let mut previous = Value::Null;
    let observed = loop {
        guard.require_session(id, &store.stored_for_update(id)?)?;
        let remaining = left(
            deadline,
            "Recovery did not find a completed original Gemini response",
        )?;
        let observed = target
            .page()
            .evaluate_with_arguments(
                gemini::DOM,
                &arguments,
                remaining.min(limits.command),
                cancel,
            )
            .await?;
        if !observed.is_null() && observed == previous {
            break observed;
        }
        previous = observed;
        pause(remaining.min(Duration::from_millis(750)), cancel).await?;
    };
    let acknowledged = if saved["dispatch"]["state"] == "started" {
        guard.acknowledge_gemini(saved, &observed)?
    } else {
        anyhow::ensure!(
            saved["dispatch"]["state"] == "acknowledged",
            "Gemini recovery has no dispatched request"
        );
        saved.clone()
    };
    gemini::accept_saved_answer(request, &acknowledged, &observed)?;
    let outcome = OutcomeWitness {
        recovery: recovery_identity(&acknowledged),
        options_sha256: request.options_sha256.clone(),
        dispatch: dispatch_identity(&acknowledged),
    };
    let text = observed["text"]
        .as_str()
        .context("Recovered Gemini answer text is missing")?;
    let receipt = capture_outcome(store, id, witness, &outcome, text, Some(&observed), cancel)?;
    publish_answer(store, id, witness, &outcome, &receipt, cancel)
}
