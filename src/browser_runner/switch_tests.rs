//! The consultation's provider choice, with scripted attempt outcomes in place of browsers.
use super::*;
use crate::browser_attempt::TEST_FAIL_AFTER_PARENT_RESERVATION;

/// What a ChatGPT attempt leaves once its dispatch is uncertain and its bounded read-only
/// recovery of the original target has expired: the only failure that may switch.
fn chatgpt_expired(store: &SessionStore, id: &str) -> Result<Value> {
    store.update(id, |meta| {
        let runtime = merge_object(
            &meta["browser"]["runtime"],
            &json!({"chromeTargetId":"PUBLIC-chatgpt-target","providerReadRecovery":{"version":1,
                "state":"expired","readOnly":true,"targetId":"PUBLIC-chatgpt-target"}}),
        );
        Ok(Some(
            json!({"dispatch":{"state":"uncertain","reservationId":"PUBLIC"},
            "browser":merge_object(&meta["browser"],&json!({"runtime":runtime}))}),
        ))
    })?;
    Err(
        anyhow::Error::new(crate::browser_provider::Failure::AnswerDeadline)
            .context("PUBLIC bounded original-target observation expired"),
    )
}
fn unsent(_: &SessionStore, _: &str) -> Result<Value> {
    anyhow::bail!("PUBLIC alternate refused before Send")
}
fn sent(store: &SessionStore, id: &str) -> Result<Value> {
    store.update(id, |_| {
        Ok(Some(
            json!({"dispatch":{"state":"started","reservationId":"PUBLIC-alternate"}}),
        ))
    })?;
    anyhow::bail!("PUBLIC alternate lost after Send")
}
fn never(_: &SessionStore, id: &str) -> Result<Value> {
    panic!("PUBLIC attempt {id} must not run")
}

struct Run {
    _home: tempfile::TempDir,
    store: SessionStore,
    id: String,
    error: String,
    /// Scripts left unrun: an extra attempt never started.
    left: usize,
}
async fn run_consultation(
    input: Value,
    scripts: Vec<ScriptedAttempt>,
    capture: Option<CaptureHook>,
) -> (
    tempfile::TempDir,
    SessionStore,
    String,
    Result<Value>,
    usize,
) {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join(".oracle"));
    let environment = super::alternate_support_tests::fixture_env(home.path(), &store);
    let mut request =
        json!({"prompt":"PUBLIC provider choice","engine":"browser","model":"gpt-6-pro"});
    for (key, value) in input.as_object().unwrap() {
        request[key] = value.clone();
    }
    let resolved = crate::execution::resolve(
        &request,
        &json!({"browser":{"cookieSync":false,"manualLogin":true}}),
        &environment,
        home.path(),
    )
    .unwrap();
    let id = prepare(&store, &resolved, home.path()).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let identity = ProcessIdentity::current().unwrap();
    start_owner(&store, &id, &identity).unwrap();
    TEST_SCRIPTED_ATTEMPTS.with_borrow_mut(|queue| queue.extend(scripts));
    let outcome = run_native_consultation(
        &store,
        &id,
        &environment,
        &identity,
        &CancellationToken::new(),
        capture,
        None,
    )
    .await;
    let left = TEST_SCRIPTED_ATTEMPTS.with_borrow_mut(|queue| queue.drain(..).count());
    (home, store, id, outcome, left)
}
async fn consult(input: Value, scripts: Vec<ScriptedAttempt>, capture: Option<CaptureHook>) -> Run {
    let (home, store, id, outcome, left) = run_consultation(input, scripts, capture).await;
    let error = outcome.map(|value| value.to_string()).unwrap_err();
    Run {
        _home: home,
        store,
        id,
        error: format!("{error:#}"),
        left,
    }
}
impl Run {
    fn consultation(&self) -> Value {
        self.store.require(&self.id).unwrap()["browser"]["consultation"].clone()
    }
    fn child(&self, index: usize) -> String {
        self.consultation()["attempts"][index]["childId"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    /// The alternate attempt, which a restored consultation no longer lists.
    fn alternate(&self, provider: &str) -> String {
        let id = format!("{}-attempt-2", self.id);
        let child = self.store.require(&id).unwrap();
        assert_eq!(child["browser"]["attempt"]["provider"], provider, "{child}");
        id
    }
    fn failed_for_recovery(&self) {
        let parent = self.store.require(&self.id).unwrap();
        assert_eq!(parent["status"], "error", "{parent}");
        assert_eq!(parent["recoveryRequired"], true, "{parent}");
    }
}

#[tokio::test]
async fn the_switch_fires_only_after_an_uncertain_send_and_expired_recovery() {
    for fallback in ["gemini", "grok"] {
        // The alternate never submitted: ChatGPT is restored for read-only recovery.
        let run = consult(
            json!({"browserFallback":fallback}),
            vec![Box::new(chatgpt_expired), Box::new(unsent), Box::new(never)],
            None,
        )
        .await;
        let (first, alternate) = (run.child(0), run.alternate(fallback));
        assert_eq!(
            run.error,
            format!(
                "ChatGPT failed and {fallback} did not submit (the prompt may remain in its composer): PUBLIC alternate refused before Send. Recover ChatGPT attempt {first} (PUBLIC bounded original-target observation expired: Owned answer deadline expired; recover the retained target without sending again) read-only through this consultation; {fallback} attempt {alternate} is retained"
            )
        );
        assert_eq!(run.left, 1, "a third attempt started");
        let state = run.consultation();
        assert_eq!(state["switch"]["to"], fallback, "{state}");
        assert_eq!(state["active"]["childId"], first.as_str(), "{state}");
        run.failed_for_recovery();

        // The alternate may have received the prompt: both attempts stay retained.
        let run = consult(
            json!({"browserFallback":fallback}),
            vec![Box::new(chatgpt_expired), Box::new(sent), Box::new(never)],
            None,
        )
        .await;
        let (first, alternate) = (run.child(0), run.alternate(fallback));
        assert!(
            run.error.starts_with(&format!("ChatGPT failed and the {fallback} fallback could not finish: PUBLIC alternate lost after Send. {fallback} attempt {alternate} may have received the prompt (")),
            "{}",
            run.error
        );
        assert!(
            run.error.ends_with(&format!("); it and ChatGPT attempt {first} (PUBLIC bounded original-target observation expired: Owned answer deadline expired; recover the retained target without sending again) are retained")),
            "{}",
            run.error
        );
        assert_eq!(run.left, 1, "a third attempt started");
        let state = run.consultation();
        assert_eq!(state["active"]["childId"], alternate.as_str(), "{state}");
        run.failed_for_recovery();
    }

    // Without consent the same failure keeps its own error and starts nothing else.
    let run = consult(
        json!({}),
        vec![Box::new(chatgpt_expired), Box::new(never)],
        None,
    )
    .await;
    assert!(
        run.error
            .contains("PUBLIC bounded original-target observation expired"),
        "{}",
        run.error
    );
    assert_eq!(run.left, 1, "an alternate started without consent");
    assert!(run.consultation()["switch"].is_null());
    assert_eq!(run.consultation()["attempts"].as_array().unwrap().len(), 1);
    run.failed_for_recovery();
}

#[tokio::test]
async fn a_refused_switch_fails_closed_with_chatgpt_restored() {
    let run = consult(
        json!({"browserFallback":"gemini"}),
        vec![
            Box::new(|store: &SessionStore, id: &str| {
                // The first reservation has happened: fail the alternate's.
                TEST_FAIL_AFTER_PARENT_RESERVATION.set(true);
                chatgpt_expired(store, id)
            }),
            Box::new(never),
        ],
        None,
    )
    .await;
    assert!(
        run.error.contains("; the provider switch did not start: Provider switch decided but the gemini reservation failed; the ChatGPT attempt is restored for recovery"),
        "{}",
        run.error
    );
    assert_eq!(run.left, 1, "an alternate started after a refused switch");
    let state = run.consultation();
    assert_eq!(state["active"], state["attempts"][0], "{state}");
    run.failed_for_recovery();
}

#[tokio::test]
async fn a_direct_alternate_runs_once_and_is_never_retried_or_switched() {
    for provider in ["gemini", "grok"] {
        let run = consult(
            json!({"browserProvider":provider}),
            vec![Box::new(unsent), Box::new(never)],
            None,
        )
        .await;
        assert!(
            run.error.contains("PUBLIC alternate refused before Send"),
            "{}",
            run.error
        );
        assert_eq!(
            run.left, 1,
            "a direct {provider} attempt was followed by another"
        );
        let state = run.consultation();
        assert_eq!(state["attempts"].as_array().unwrap().len(), 1, "{state}");
        assert_eq!(state["attempts"][0]["provider"], provider, "{state}");
        assert!(
            state["switch"].is_null() && state["retry"].is_null(),
            "{state}"
        );

        // A remote capture belongs to ChatGPT: a direct alternate refuses it before any attempt.
        let run = consult(
            json!({"browserProvider":provider}),
            vec![Box::new(never)],
            Some(Arc::new(|_| Ok(()))),
        )
        .await;
        assert!(
            run.error.contains(
                "Alternate providers run locally without ChatGPT cookie bootstrap or remote capture"
            ),
            "{}",
            run.error
        );
        assert_eq!(run.left, 1);
        assert_eq!(run.consultation()["attempts"], json!([]));
    }
}

fn captured_alternate(provider: &'static str, answer: &'static str) -> ScriptedAttempt {
    Box::new(move |store: &SessionStore, id: &str| {
        let req = request(store, id)?;
        let identity = ProcessIdentity::current()?;
        let journal = TargetJournal::open(store)?;
        let claim_id = Uuid::new_v4().to_string();
        let creator = journal.record_creation(
            &crate::browser_disposition::TargetCreation::allocated("TARGET-SWITCH"),
            &json!({"host":"127.0.0.1","port":9222,"targetId":"TARGET-SWITCH","claimId":claim_id}),
            &identity,
            Some(id),
            None,
        )?;
        let capture = artifacts::capture(
            store,
            id,
            "captured-answer.md",
            answer.as_bytes(),
            &CancellationToken::new(),
        )?;
        let (dispatch, runtime) = if provider == "gemini" {
            let (dispatch, _obs, mut runtime) = super::alternate_support_tests::gemini_evidence(
                &req,
                &identity,
                &claim_id,
                "acknowledged",
                answer,
                true,
            );
            runtime["capturedAnswer"] = json!(capture);
            runtime["ownedRecoveryTarget"] =
                json!({"targetId":"TARGET-SWITCH","claimId":claim_id,"creator":creator});
            (dispatch, runtime)
        } else {
            let owner = crate::browser_commit::SubmissionOwner {
                claim_id: claim_id.clone(),
                reservation_id: req.reservation_id.clone(),
            };
            let dispatch_id = Uuid::new_v4().to_string();
            let dispatch = json!({
                "state": "acknowledged",
                "reservationId": req.reservation_id,
                "submissionOwner": owner,
                "promptSha256": req.prompt_sha256,
                "providerDispatchId": dispatch_id,
            });
            let user = format!("response-{}", Uuid::new_v4());
            let assistant = format!("response-{}", Uuid::new_v4());
            let completed_receipt = json!({
                "version": 1,
                "provider": "grok",
                "kind": "grok-websocket-v1",
                "state": "completed",
                "privateChat": true,
                "mode": "Auto",
                "claimId": owner.claim_id,
                "reservationId": owner.reservation_id,
                "dispatchId": dispatch_id,
                "clientMessageId": Uuid::new_v4().to_string(),
                "userMessageId": user,
                "assistantMessageId": assistant,
                "responseId": Uuid::new_v4().to_string(),
                "promptSha256": req.prompt_sha256,
            });
            let mut committed_receipt = completed_receipt.clone();
            committed_receipt["state"] = json!("committed");
            committed_receipt["assistantMessageId"] = Value::Null;
            committed_receipt["responseId"] = Value::Null;
            let observed = json!({
                "state": "completed",
                "provider": "grok",
                "kind": "grok-private-answer",
                "privateChat": true,
                "mode": "Auto",
                "completionConfirmed": true,
                "stopExists": false,
                "strongThinkingActive": false,
                "wireReceipt": completed_receipt,
                "userMessageId": user,
                "assistantMessageId": assistant,
                "userText": req.prompt,
                "text": answer,
                "providerText": answer,
                "images": [],
                "files": [],
            });
            let runtime = json!({
                "provider": "grok",
                "controllerPid": identity.pid,
                "controllerProcessStartedAtMs": identity.started_at_ms,
                "submissionOwner": owner,
                "grokCommitReceipt": committed_receipt,
                "submittedPromptHash": req.prompt_sha256,
                "submittedPromptHashFormat": "grok-request-v1",
                "effectiveModel": "grok-web-auto",
                "modelSelection": {"verified":true,"source":"grok-mode-picker","selectedMode":"Auto","resolvedLabel":"Grok Auto"},
                "acceptedObservation": observed,
                "capturedAnswer": capture,
                "ownedRecoveryTarget": {"targetId":"TARGET-SWITCH","claimId":claim_id,"creator":creator},
            });
            (dispatch, runtime)
        };
        store.update(id, |current| {
            Ok(Some(json!({
                "dispatch": dispatch,
                "capturedAnswer": capture,
                "browser": merge_object(
                    &current["browser"],
                    &json!({"provider": provider, "capturedAnswer": capture, "runtime": runtime}),
                ),
            })))
        })?;
        store.require(id)
    })
}

#[tokio::test]
async fn switched_gemini_and_direct_or_switched_grok_adopt_their_answers() {
    for (input, provider, switched) in [
        (
            json!({"browserFallback":"gemini","notify":false}),
            "gemini",
            true,
        ),
        (
            json!({"browserProvider":"grok","notify":false}),
            "grok",
            false,
        ),
        (
            json!({"browserFallback":"grok","notify":false}),
            "grok",
            true,
        ),
    ] {
        let answer = if provider == "gemini" {
            "PUBLIC adopted Gemini answer"
        } else {
            "PUBLIC adopted Grok answer"
        };
        let mut scripts: Vec<ScriptedAttempt> = Vec::new();
        if switched {
            scripts.push(Box::new(chatgpt_expired));
        }
        scripts.push(captured_alternate(provider, answer));
        scripts.push(Box::new(never));
        let (_home, store, id, outcome, left) = run_consultation(input, scripts, None).await;
        let completed = outcome.unwrap();
        assert_eq!(left, 1, "an extra attempt ran for {provider}");
        assert_eq!(completed["status"], "completed", "{completed}");
        assert_eq!(
            String::from_utf8(captured(&store, &id).unwrap()).unwrap(),
            answer
        );
        if provider == "gemini" {
            let log =
                fs::read_to_string(store.directory(&id).unwrap().join("models/gpt-6-pro.log"))
                    .unwrap();
            assert!(
                log.contains("[browser] Answer adopted from Gemini 4 Neon with High thinking (verified Temporary Chat and picker settings)."),
                "{log}"
            );
        }
    }
}
