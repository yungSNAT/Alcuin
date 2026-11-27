use super::*;

const CONVERSATION_URL: &str = "https://chatgpt.com/c/00000000-0000-4000-8000-000000000001";

#[test]
fn similarity_scores_exact_prefix_and_substring_matches() {
    for (candidate, expected) in [
        ("PUBLIC-ALPHA", 1.0),
        ("PUBLIC-ALPHA-EXTENDED", 0.95),
        ("PUBLIC-ARCHIVED-PUBLIC-ALPHA", 0.8),
    ] {
        for (input, candidate) in [("PUBLIC-ALPHA", candidate), (candidate, "PUBLIC-ALPHA")] {
            let actual = similarity(input, candidate);
            assert!(
                (actual - expected).abs() <= f64::EPSILON,
                "{input:?} vs. {candidate:?}: expected {expected}, got {actual}"
            );
        }
    }
}

#[test]
fn browser_resume_does_not_reconstruct_a_url_from_a_blank_conversation_id() {
    for conversation_id in ["", " \t\n"] {
        let parent = json!({
            "browser": {
                "config": {"url": "https://chatgpt.com/"},
                "runtime": {"conversationId": conversation_id}
            }
        });
        assert_eq!(browser_resume(&parent), None, "{conversation_id:?}");
    }
}

#[test]
fn browser_parent_requires_saved_effort_for_legacy_pro_models() {
    let mut parent = json!({
        "mode": "browser",
        "browser": {
            "runtime": {"tabUrl": CONVERSATION_URL},
            "config": {"desiredModel": " PUBLIC-saved-model "}
        }
    });

    for model in ["gpt-5-pro", "gpt-5.1-pro", "gpt-5.2-pro", " GPT-5.4-PRO "] {
        let mut pro_parent = parent.clone();
        let config = &mut pro_parent["browser"]["config"];
        config["requestedModel"] = json!(model);

        let error = browser_parent("PUBLIC-parent", &pro_parent).unwrap_err();
        assert_eq!(
            error.to_string(),
            concat!(
                "Session PUBLIC-parent is missing its stored browser model or thinking effort. ",
                "Refusing to replace the saved selection with current defaults."
            ),
            "{model:?}"
        );

        pro_parent["browser"]["config"]["thinkingTime"] = json!("extended");
        let resumed = browser_parent("PUBLIC-parent", &pro_parent)
            .unwrap()
            .unwrap();
        assert_eq!(resumed["desiredModel"], "PUBLIC-saved-model");
        assert_eq!(resumed["thinkingTime"], "extended");
        assert_eq!(resumed["resumeConversationUrl"], CONVERSATION_URL);
    }

    parent["browser"]["config"]["requestedModel"] = json!("gpt-5.4");
    let resumed = browser_parent("PUBLIC-parent", &parent).unwrap().unwrap();
    assert_eq!(resumed["desiredModel"], "PUBLIC-saved-model");
    assert!(resumed.get("thinkingTime").is_none());
}

#[test]
fn browser_parent_rejects_missing_or_non_object_config_even_with_a_saved_url() {
    for config in [
        None,
        Some(Value::Null),
        Some(json!("PUBLIC-invalid-config")),
    ] {
        let mut parent = json!({
            "mode": "browser",
            "browser": {"runtime": {"tabUrl": CONVERSATION_URL}}
        });
        if let Some(config) = config {
            parent["browser"]["config"] = config;
        }

        let error = browser_parent("PUBLIC-parent", &parent).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Session PUBLIC-parent is missing its stored browser configuration."
        );
    }
}
