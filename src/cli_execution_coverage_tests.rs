use super::*;
use crate::providers::ApiTransport;
use serde_json::json;
use std::path::Path;

fn resolved(input: Value, home: &Path) -> ResolvedRequest {
    let mut input = input;
    input["engine"] = json!("api");
    execution::resolve(&input, &json!({}), &Environment::new(), home).unwrap()
}

fn route(label: &str, base_url: &str) -> Route {
    Route {
        model: "gpt-6-astra".into(),
        provider_label: label.into(),
        base_url: base_url.into(),
        key_source: "PUBLIC_API_KEY".into(),
        azure_deployment_name: None,
        transport: ApiTransport::Responses,
    }
}

fn header_for(request: &ResolvedRequest, route: &Route, home: &Path) -> Vec<String> {
    let body = crate::providers::build_request(
        &route.model,
        &request.options,
        &request.user_prompt,
        crate::prompts::DEFAULT_SYSTEM,
    )
    .unwrap();
    api_header(
        request,
        route,
        &Secret::new("PUBLIC-synthetic-key".into()),
        &body,
        home,
    )
    .unwrap()
}

#[test]
fn browser_runs_with_pro_thinking_time_detach_and_others_stay_attached() {
    let home = tempfile::tempdir().unwrap();
    let env = Environment::new();
    for (thinking, wait, expected) in [
        ("pro", true, true),
        ("pro", false, true),
        ("standard", true, false),
        ("standard", false, false),
    ] {
        let mut request = resolved(json!({"prompt": "PUBLIC browser prompt"}), home.path());
        request.engine = "browser".into();
        request.options["model"] = json!("gpt-6-astra");
        request.browser = json!({"thinkingTime": thinking});
        assert_eq!(
            detach(&request, wait, &env),
            expected,
            "thinkingTime={thinking} wait={wait}"
        );
    }
}

#[test]
fn verbose_header_without_files_reports_zero_tokens_effort_and_custom_base() {
    let home = tempfile::tempdir().unwrap();
    for (label, base_url, option_base, host) in [
        (
            "PUBLIC-Provider",
            "https://public.example.test/v1",
            None,
            "public.example.test",
        ),
        (
            "OpenAI",
            "https://public-proxy.example.test/v1",
            Some("https://public-proxy.example.test/v1"),
            "public-proxy.example.test",
        ),
    ] {
        let mut input = json!({"prompt": "PUBLIC verbose prompt without files", "verbose": true});
        if let Some(base) = option_base {
            input["baseUrl"] = json!(base);
        }
        let mut request = resolved(input, home.path());
        assert!(request.files.is_empty());
        let route = route(label, base_url);
        let body = crate::providers::build_request(
            &route.model,
            &request.options,
            &request.user_prompt,
            crate::prompts::DEFAULT_SYSTEM,
        )
        .unwrap();
        request.options["reasoningEffort"] = json!("high");
        let lines = api_header(
            &request,
            &route,
            &Secret::new("PUBLIC-synthetic-key".into()),
            &body,
            home.path(),
        )
        .unwrap();
        assert!(lines[0].starts_with("[verbose] cwd: "), "{lines:?}");
        assert_eq!(lines[1], "[verbose] No files attached.", "{lines:?}");
        assert_eq!(
            lines[2], "[verbose] Attached files use 0 tokens",
            "{lines:?}"
        );
        assert!(
            lines.contains(&"Reasoning effort: high".to_owned()),
            "{lines:?}"
        );
        let provider = lines
            .iter()
            .find(|line| line.starts_with("Provider: "))
            .unwrap();
        assert!(
            provider.starts_with(&format!("Provider: {label} | base: ")),
            "{provider}"
        );
        assert!(provider.contains(host), "{provider}");
        assert!(!provider.contains("api.openai.com"), "{provider}");
        assert!(provider.ends_with("| key: PUBLIC_API_KEY"), "{provider}");
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("Calling gpt-6-astra") && line.ends_with("no files.")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("Tip: no files attached")),
            "{lines:?}"
        );
    }
}

#[test]
fn verbose_header_lists_ten_files_and_counts_the_rest() {
    let home = tempfile::tempdir().unwrap();
    let names: Vec<String> = (0..11).map(|n| format!("PUBLIC-{n:02}.txt")).collect();
    for name in &names {
        std::fs::write(home.path().join(name), "public fixture\n").unwrap();
    }
    let request = resolved(
        json!({"prompt": "PUBLIC many files prompt", "verbose": true, "files": names}),
        home.path(),
    );
    assert_eq!(request.files.len(), 11);
    let lines = header_for(
        &request,
        &route("OpenAI", "https://api.openai.com/v1"),
        home.path(),
    );
    let attached = &lines[1];
    assert!(
        attached.starts_with("[verbose] Attached files (11): "),
        "{attached}"
    );
    assert!(attached.ends_with(" (+1 more)"), "{attached}");
    assert!(attached.contains("PUBLIC-00.txt"), "{attached}");
    assert_eq!(attached.matches(".txt").count(), 10, "{attached}");
    assert!(
        lines[2].starts_with("[verbose] Attached files use "),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.ends_with(" tokens, 11 files.")),
        "{lines:?}"
    );
}

#[test]
fn completed_browser_run_with_pending_cleanup_still_prints_its_log() {
    for patch in [
        json!({"status": "completed", "cleanupWarning": "PUBLIC tab left open"}),
        json!({
            "status": "completed",
            "recoveryRequired": true,
            "browser": {"runtime": {"targetDispositionPending": true}}
        }),
    ] {
        let home = tempfile::tempdir().unwrap();
        let store = SessionStore::new(home.path());
        let meta = store
            .create(
                json!({"prompt": "PUBLIC cleanup run", "model": "gpt-6-pro"}),
                home.path(),
                None,
            )
            .unwrap();
        let id = meta["id"].as_str().unwrap();
        store.append_log(id, None, "PUBLIC saved answer\n").unwrap();
        let applied = patch.clone();
        store
            .update(id, move |_| Ok(Some(applied.clone())))
            .unwrap();
        assert_eq!(store.require(id).unwrap()["status"], "completed");
        let mut output = vec![];
        print_browser_run(&store, id, false, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.starts_with("PUBLIC saved answer\n"), "{patch}: {text}");
        assert!(!text.contains("Answer incomplete"), "{patch}: {text}");
    }
}
