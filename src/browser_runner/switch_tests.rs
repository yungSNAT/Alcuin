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
async fn consult(input: Value, scripts: Vec<ScriptedAttempt>, capture: Option<CaptureHook>) -> Run {
    let home = tempfile::tempdir().unwrap();
    let store = SessionStore::new(home.path().join(".oracle"));
    let environment = crate::config::Environment::from([
        (
            "ORACLE_HOME_DIR".into(),
            store.home().to_string_lossy().into(),
        ),
        (
            (if cfg!(windows) { "USERPROFILE" } else { "HOME" }).into(),
            home.path().to_string_lossy().into(),
        ),
    ]);
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
    let error = run_native_consultation(
        &store,
        &id,
        &environment,
        &identity,
        &CancellationToken::new(),
        capture,
        None,
    )
    .await
    .map(|value| value.to_string())
    .unwrap_err();
    let left = TEST_SCRIPTED_ATTEMPTS.with_borrow_mut(|queue| queue.drain(..).count());
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
                "ChatGPT failed and {fallback} did not submit: PUBLIC alternate refused before Send. Recover ChatGPT attempt {first} (PUBLIC bounded original-target observation expired: Owned answer deadline expired; recover the retained target without sending again) read-only through this consultation; {fallback} attempt {alternate} is retained"
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
