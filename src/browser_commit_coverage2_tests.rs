use super::*;

#[test]
fn autolinks_reject_javascript_whitespace_inside_urls_even_when_escaped() {
    const URL_PREFIX: &str = "https://example.invalid/PUBLIC-path";

    for space in [
        '\t', '\n', '\u{b}', '\u{c}', '\r', ' ', '\u{a0}', '\u{1680}',
    ]
    .into_iter()
    .chain('\u{2000}'..='\u{200a}')
    {
        for escape in ["", "\\"] {
            let url = format!("{URL_PREFIX}{escape}{space}PUBLIC-end");
            let source = format!("PUBLIC-prefix [{url}]({url})");
            assert_eq!(
                markdown_source_text(&source),
                None,
                "U+{:04X}, escape {escape:?}: {source:?}",
                u32::from(space)
            );
        }
    }

    // These characters are not JavaScript whitespace and must not split the URL.
    for non_space in ['\u{85}', '\u{180e}', '\u{200b}'] {
        let url = format!("{URL_PREFIX}{non_space}PUBLIC-end");
        let source = format!("PUBLIC-prefix [{url}]({url})");
        assert_eq!(
            markdown_source_text(&source),
            Some(format!("PUBLIC-prefix {url}")),
            "{source:?}"
        );
    }
}

#[test]
fn existing_conversation_recovery_requires_a_matching_witness_after_binding_intent() {
    const PROMPT: &str = "PUBLIC-prompt";
    const MESSAGE_ID: &str = "PUBLIC-new-message";
    const CONVERSATION_ID: &str = "PUBLIC-conversation";
    const ORIGIN: &str = "https://chatgpt.com";

    let owner = SubmissionOwner {
        claim_id: "PUBLIC-claim".into(),
        reservation_id: "PUBLIC-reservation".into(),
    };
    let mut before = CommitSnapshot {
        owner: owner.clone(),
        navigation: Navigation {
            origin: ORIGIN.into(),
            conversation_id: Some(CONVERSATION_ID.into()),
            landing_path: None,
        },
        message_ids: vec!["PUBLIC-old-message".into()],
        turn_ids: vec!["PUBLIC-old-turn".into()],
        canonical_prompt: PROMPT.into(),
        submission_intent: None,
        submission_witness: None,
    };
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
    let mut receipt = CommitReceipt {
        owner,
        version: 1,
        kind: "new-user-message".into(),
        message_id: MESSAGE_ID.into(),
        turn_id: "PUBLIC-new-turn".into(),
        message_fingerprint: prompt_fingerprint(PROMPT, MESSAGE_ID),
        message_fingerprint_format: "original-source-v1".into(),
        prompt_text_fingerprint: intent.prompt_sha256.clone(),
        conversation: Conversation {
            origin: ORIGIN.into(),
            conversation_id: CONVERSATION_ID.into(),
        },
        turns_count: 2,
        submission_witness: None,
    };

    before.require_receipt(&receipt).unwrap();

    before.bind_submission(intent).unwrap();
    assert_eq!(
        before.require_receipt(&receipt).unwrap_err().to_string(),
        "Recovered submission witness missing"
    );

    receipt.submission_witness = Some(witness);
    before.require_receipt(&receipt).unwrap();
    assert!(before.submission_witness.is_none());

    receipt.submission_witness.as_mut().unwrap().message_id = "PUBLIC-other-message".into();
    assert_eq!(
        before.require_receipt(&receipt).unwrap_err().to_string(),
        "Recovered receipt differs from the original outbound submission witness"
    );
}
