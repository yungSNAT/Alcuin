use super::*;

#[test]
fn answer_placeholder_detects_exact_combined_and_oversized_labels() {
    for (text, expected) in [
        // Exact single-label placeholders, after lowercasing and collapsing whitespace.
        ("Answer now", true),
        ("  ChatGPT   said:  ", true),
        ("chatgpt said", true),
        // Owner plus gate labels separated by punctuation form a placeholder.
        ("ChatGPT said: Answer now", true),
        ("Pro thinking · File upload request", true),
        ("ChatGPT said - Pro thinking, Answer now.", true),
        // Only an owner, or only a gate, is not a placeholder.
        ("Pro thinking", false),
        ("File upload request", false),
        ("Answer now File upload request", false),
        // Any non-label text means a real answer.
        ("ChatGPT said hello", false),
        ("PUBLIC answer", false),
    ] {
        assert_eq!(answer_placeholder(text), expected, "{text:?}");
    }
    // Text longer than 60 UTF-16 units is never a placeholder, even when made of labels.
    let long = "ChatGPT said: Answer now ".repeat(4);
    assert!(long.encode_utf16().count() > 60);
    assert!(!answer_placeholder(&long));
    let filler = "PUBLIC ".repeat(20);
    assert!(!answer_placeholder(&filler));
}

#[tokio::test]
async fn claim_succeeds_only_when_the_page_confirms_the_exact_claim_id() {
    use crate::cdp::fake::{Reply, peer};

    let cancel = CancellationToken::new();
    let (cdp, _peer) = peer(Duration::from_secs(1), |request| {
        if request.dom() == Some("native-reply-claim") {
            match request.arg()["claimId"].as_str() {
                Some("PUBLIC-claim-ok") => Reply::value(json!(true)),
                Some("PUBLIC-claim-other") => Reply::value(json!(false)),
                Some("PUBLIC-claim-string") => Reply::value(json!("true")),
                _ => Reply::thrown("Error: PUBLIC-claim-failure"),
            }
        } else {
            Reply::ok()
        }
    })
    .await;

    claim(&cdp, &json!("PUBLIC-claim-ok"), &cancel)
        .await
        .expect("confirmed claim must succeed");

    for refused in ["PUBLIC-claim-other", "PUBLIC-claim-string"] {
        let error = claim(&cdp, &json!(refused), &cancel).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Native recovery ownership is unverified",
            "{refused}"
        );
    }

    let thrown = claim(&cdp, &json!("PUBLIC-claim-throws"), &cancel)
        .await
        .unwrap_err();
    let message = format!("{thrown:#}");
    assert!(message.contains("PUBLIC-claim-failure"), "{message}");
    assert_ne!(
        thrown.to_string(),
        "Native recovery ownership is unverified"
    );
}
