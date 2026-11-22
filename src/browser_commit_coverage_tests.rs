use super::*;
use crate::cdp::fake::{self, Reply};

const PROMPT: &str = "PUBLIC-prompt";

fn owner() -> SubmissionOwner {
    SubmissionOwner {
        claim_id: "PUBLIC-claim".into(),
        reservation_id: "PUBLIC-reservation".into(),
    }
}

fn landing_probe(prompt: &str) -> Value {
    json!({
        "currentUrl": "https://chatgpt.com/",
        "workSelected": false,
        "modeUnverified": false,
        "invalid": false,
        "turnIds": [],
        "messages": [],
        "canonicalPrompt": prompt
    })
}

#[test]
fn escaped_autolink_restores_parentheses_at_javascript_whitespace_boundaries() {
    const URL: &str = "https://example.invalid/PUBLIC-path(a)";
    const LINK: &str =
        r"[https://example.invalid/PUBLIC-path\(a\)](https://example.invalid/PUBLIC-path\(a\))";

    for space in [
        ' ', '\t', '\n', '\u{a0}', '\u{1680}', '\u{2000}', '\u{2005}', '\u{200a}',
    ] {
        let source = format!("PUBLIC-prefix{space}{LINK}{space}PUBLIC-suffix");
        let expected = format!("PUBLIC-prefix{space}{URL}{space}PUBLIC-suffix");
        assert_eq!(markdown_source_text(&source), Some(expected), "{source:?}");
    }

    for non_space in ['\u{85}', '\u{200b}'] {
        let source = format!("PUBLIC-prefix{non_space}{LINK}");
        assert_eq!(markdown_source_text(&source), None, "{source:?}");
    }
}

#[test]
fn escaped_literal_link_requires_a_complete_non_image_label() {
    for (source, expected) in [
        (
            r"PUBLIC-prefix [PUBLIC-label]\(PUBLIC-target)",
            Some("PUBLIC-prefix [PUBLIC-label](PUBLIC-target)"),
        ),
        (r"PUBLIC-prefix ![PUBLIC-label]\(PUBLIC-target)", None),
        (r"PUBLIC-prefix ]\( [PUBLIC-unclosed-label", None),
    ] {
        assert_eq!(
            markdown_source_text(source).as_deref(),
            expected,
            "{source:?}"
        );
    }
}

#[test]
fn landing_observation_stays_pending_only_in_the_unchanged_context() {
    let owner = owner();
    let landing = landing_probe(PROMPT);
    let before = snapshot(&owner, landing.clone()).unwrap();
    let mut latch = None;

    assert!(matches!(
        observe(&owner, &before, landing.clone(), &mut latch),
        CommitObservation::Pending
    ));
    assert!(latch.is_none());

    for (field, replacement, reason) in [
        ("invalid", json!(true), "user-identity-unavailable"),
        ("workSelected", json!(true), "conversation-context-changed"),
        (
            "modeUnverified",
            json!(true),
            "conversation-context-changed",
        ),
        (
            "canonicalPrompt",
            json!("PUBLIC-different-prompt"),
            "conversation-context-changed",
        ),
        (
            "currentUrl",
            json!("https://chat.openai.com/"),
            "conversation-context-changed",
        ),
        (
            "currentUrl",
            json!("https://chatgpt.com/g/g-p-PUBLIC-project/project"),
            "conversation-context-changed",
        ),
    ] {
        let mut current = landing.clone();
        current[field] = replacement.clone();
        let observed = observe(&owner, &before, current, &mut latch);
        assert!(
            matches!(observed, CommitObservation::Ambiguous(actual) if actual == reason),
            "{field}={replacement}: expected {reason}, got {observed:?}"
        );
        assert!(latch.is_none());
    }

    let empty_landing = landing_probe("");
    let attachment_only = snapshot(&owner, empty_landing.clone()).unwrap();
    let observed = observe(&owner, &attachment_only, empty_landing, &mut latch);
    assert!(
        matches!(
            observed,
            CommitObservation::Ambiguous("attachment-only-commit-unqualified")
        ),
        "{observed:?}"
    );
    assert!(latch.is_none());
}

#[tokio::test]
async fn receipt_read_rejects_changed_owner_and_recovery_requires_the_matching_witness() {
    const MESSAGE_ID: &str = "PUBLIC-message";
    const CONVERSATION_ID: &str = "PUBLIC-conversation";

    let owner = owner();
    let mut before = snapshot(&owner, landing_probe(PROMPT)).unwrap();
    let intent = SubmissionIntent {
        witness_id: "PUBLIC-witness".into(),
        dispatch_guard_id: "PUBLIC-dispatch".into(),
        prompt_sha256: sha256(PROMPT.as_bytes()),
        web_search: false,
    };
    let witness = SubmissionWitness {
        version: 1,
        owner: owner.clone(),
        witness_id: intent.witness_id.clone(),
        dispatch_guard_id: intent.dispatch_guard_id.clone(),
        message_id: MESSAGE_ID.into(),
        prompt_sha256: intent.prompt_sha256.clone(),
        source: "chatgpt-fetch-v1".into(),
        request_path: "/backend-api/f/conversation".into(),
        history_disabled: true,
    };
    let expected_arg = json!({
        "owner": owner, "ids": [], "prompt": PROMPT, "intent": intent, "saved": null
    });
    before.bind_submission(intent).unwrap();

    let mut current = landing_probe(PROMPT);
    current["currentUrl"] = json!(format!("https://chatgpt.com/c/{CONVERSATION_ID}"));
    current["turnIds"] = json!(["PUBLIC-turn"]);
    current["messages"] = json!([{
        "id": MESSAGE_ID, "turnId": "PUBLIC-turn", "index": 0,
        "matches": true, "fingerprintText": PROMPT
    }]);
    let response = json!({
        "probe": current,
        "submission": {"state": "captured", "witness": witness}
    });
    let timeout = Duration::from_secs(1);
    let (cdp, peer) = fake::peer(timeout, move |request| {
        assert_eq!(request.method, "Runtime.evaluate");
        assert!(
            request
                .expression()
                .contains("/* alcuin-dom:submission-receipt */")
        );
        assert_eq!(request.arg(), expected_arg);
        Reply::value(response.clone())
    })
    .await;

    let cancel = CancellationToken::new();
    let mut latch = None;
    let changed_owner = SubmissionOwner {
        reservation_id: "PUBLIC-other-reservation".into(),
        ..owner.clone()
    };
    let error = read_receipt(
        &cdp,
        &changed_owner,
        &before,
        PROMPT,
        timeout,
        &cancel,
        &mut latch,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Pre-dispatch submission ownership changed"
    );
    assert!(latch.is_none());

    let observed = read_receipt(&cdp, &owner, &before, PROMPT, timeout, &cancel, &mut latch)
        .await
        .unwrap();
    let CommitObservation::Committed(mut receipt) = observed else {
        panic!("Expected a receipt for the original owner, got {observed:?}");
    };
    assert_eq!(latch.as_deref(), Some(CONVERSATION_ID));
    assert_eq!(receipt.message_id, MESSAGE_ID);
    assert_eq!(receipt.submission_witness.as_ref(), Some(&witness));

    before.seal_witness(&receipt).unwrap();
    assert_eq!(before.submission_witness.as_ref(), Some(&witness));
    before.require_receipt(&receipt).unwrap();

    receipt
        .submission_witness
        .as_mut()
        .unwrap()
        .dispatch_guard_id = "PUBLIC-other-dispatch".into();
    assert_eq!(
        before.require_receipt(&receipt).unwrap_err().to_string(),
        "Recovered receipt differs from the original outbound submission witness"
    );

    cdp.disconnect_and_wait(timeout).await.unwrap();
    let commands = tokio::time::timeout(timeout, peer).await.unwrap().unwrap();
    assert_eq!(commands.len(), 1, "The changed owner must not reach CDP");
}
