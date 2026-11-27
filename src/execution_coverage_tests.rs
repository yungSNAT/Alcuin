//! Coverage tests for the failure display and the dry-run summaries in `execution`.

use super::*;

/// A browser or API request built directly, without resolving any configuration.
fn request(engine: &str, options: Value, browser: Value) -> ResolvedRequest {
    ResolvedRequest {
        options,
        engine: engine.to_owned(),
        files: Vec::new(),
        markdown: String::new(),
        user_prompt: String::new(),
        browser,
        browser_preview: json!({}),
        cookie_bootstrap: None,
        min_prompt_chars: None,
    }
}

fn has_line(lines: &[String], expected: &str) -> bool {
    lines.iter().any(|line| line == expected)
}

#[test]
fn logged_failure_displays_the_full_error_chain() {
    let error = anyhow::anyhow!("PUBLIC-root cause").context("PUBLIC-outer step");
    let failure = LoggedFailure(error);
    assert_eq!(failure.to_string(), "PUBLIC-outer step: PUBLIC-root cause");
    let boxed: Box<dyn std::error::Error> = Box::new(failure);
    assert_eq!(boxed.to_string(), "PUBLIC-outer step: PUBLIC-root cause");
}

#[test]
fn browser_dry_run_reports_models_images_attachments_and_bundles() {
    let resolved = request(
        "browser",
        json!({
            "model": "gpt-6-pro",
            "models": ["gpt-6-astra"],
            "generateImage": "PUBLIC-image.png",
            "browserAttachments": ["PUBLIC-a.txt"],
            "browserBundleFiles": ["PUBLIC-b.txt"],
            "browserBundleFormat": "zip",
            "browserFollowUps": ["PUBLIC follow-up", "   "]
        }),
        json!({
            "desiredModel": "PUBLIC-model",
            "chatgptUrl": "https://chatgpt.com/g/PUBLIC/project",
            "keepBrowser": true
        }),
    );
    let details = dry_run_resolved(&resolved);
    assert_eq!(details["resolvedEngine"], "browser");
    assert_eq!(details["models"], json!(["gpt-6-astra"]));
    assert_eq!(details["followUpCount"], 1);
    let browser = &details["browser"];
    assert_eq!(browser["imageOutputPath"], "PUBLIC-image.png");
    assert_eq!(browser["attachments"], json!(["PUBLIC-a.txt"]));
    assert_eq!(browser["bundleFiles"], json!(["PUBLIC-b.txt"]));
    assert_eq!(browser["bundleFormat"], "zip");
    assert_eq!(
        browser["chatgptUrl"],
        "https://chatgpt.com/g/PUBLIC/project"
    );
    assert_eq!(browser["desiredModel"], "PUBLIC-model");
    let guidance: Vec<&str> = details["guidance"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(guidance.contains(
        &"ChatGPT generated images will use the image-aware wait/download path and return saved files in structuredContent.images."
    ));
    assert!(guidance.contains(
        &"This is a multi-turn browser consult; all follow-ups stay in one ChatGPT conversation."
    ));
    assert!(
        guidance
            .iter()
            .any(|line| line.contains("project URL is persistent"))
    );

    let lines = dry_run_lines(&details);
    assert_eq!(lines[0], "[dry-run] MCP resolved request:");
    for expected in [
        "  engine: browser",
        "  model: gpt-6-pro",
        "  models: gpt-6-astra",
        "  files: 0",
        "  browser desired model: PUBLIC-model",
        "  browser keep browser: yes",
        "  browser manual login: no",
        "  ChatGPT URL: https://chatgpt.com/g/PUBLIC/project",
        "  image output: PUBLIC-image.png",
        "  follow-ups: 1",
    ] {
        assert!(
            has_line(&lines, expected),
            "missing {expected:?} in {lines:#?}"
        );
    }
}

#[test]
fn api_dry_run_lists_models_without_browser_details() {
    let resolved = request(
        "api",
        json!({
            "model": "gpt-6-astra",
            "models": ["gpt-6-astra", "openai/gpt-6-astra"],
            "generateImage": "PUBLIC-ignored.png"
        }),
        json!({}),
    );
    let details = dry_run_resolved(&resolved);
    assert_eq!(
        details["models"],
        json!(["gpt-6-astra", "openai/gpt-6-astra"])
    );
    assert!(details.get("browser").is_none());
    assert_eq!(details["followUpCount"], 0);
    let lines = dry_run_lines(&details);
    assert!(has_line(&lines, "  engine: api"));
    assert!(has_line(
        &lines,
        "  models: gpt-6-astra, openai/gpt-6-astra"
    ));
    assert!(!lines.iter().any(|line| line.starts_with("  browser")));
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("  guidance: API engine requires provider credentials."))
    );
}

#[test]
fn dry_run_lines_join_string_models_and_skip_empty_lists() {
    let cases = [
        (
            json!(["PUBLIC-a", "PUBLIC-b", 3]),
            Some("  models: PUBLIC-a, PUBLIC-b"),
        ),
        (json!(["PUBLIC-only"]), Some("  models: PUBLIC-only")),
        (json!([]), None),
        (Value::Null, None),
    ];
    for (models, expected) in cases {
        let details = json!({
            "resolvedEngine": "api",
            "model": "gpt-6-astra",
            "models": models,
            "files": ["PUBLIC-x", "PUBLIC-y"],
            "followUpCount": 0,
            "guidance": ["PUBLIC guidance"]
        });
        let lines = dry_run_lines(&details);
        let model_lines: Vec<&String> = lines
            .iter()
            .filter(|line| line.starts_with("  models:"))
            .collect();
        match expected {
            Some(line) => assert_eq!(model_lines, [line], "{models}"),
            None => assert!(model_lines.is_empty(), "{models}"),
        }
        assert!(has_line(&lines, "  files: 2"));
        assert!(has_line(&lines, "  guidance: PUBLIC guidance"));
        assert_eq!(lines.last().unwrap(), "  guidance: PUBLIC guidance");
    }
}
