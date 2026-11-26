//! Coverage tests for request resolution in `execution`: API model slugs, fan-out model
//! checks, browser cookie options and configuration fallbacks.

use super::*;

#[test]
fn request_models_keep_provider_prefixed_slugs_and_refuse_pro_fan_out_entries() {
    let (model, unique) = request_models(
        &json!({"models": ["openai/gpt-6-astra", "gpt-6-astra", "openai/gpt-6-astra"]}),
        "api",
        None,
    )
    .unwrap();
    assert_eq!(model, "openai/gpt-6-astra");
    assert_eq!(
        unique,
        vec![json!("openai/gpt-6-astra"), json!("gpt-6-astra")]
    );
    assert_eq!(
        api_model("openai/gpt-6-astra").unwrap(),
        "openai/gpt-6-astra"
    );

    let error = request_models(
        &json!({"models": ["gpt-6-astra", "gpt-6-pro"]}),
        "api",
        None,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("GPT-6 Pro is an API reasoning mode, not a model slug."),
        "{error:#}"
    );
}

#[test]
fn browser_settings_drop_blank_cookie_names_and_fall_back_on_invalid_cookie_waits() {
    let dir = tempfile::tempdir().unwrap();
    let env = Environment::default();
    let cases = [
        (
            json!({"browserCookieNames": " , ,", "browserCookieWait": "PUBLIC-not-a-duration"}),
            Value::Null,
        ),
        (
            json!({"browserCookieNames": "PUBLIC-a, ,PUBLIC-b", "browserCookieWait": "PUBLIC-not-a-duration"}),
            json!(["PUBLIC-a", "PUBLIC-b"]),
        ),
    ];
    for (input, expected_names) in cases {
        let (browser, _, _) = browser_settings(
            &input,
            &json!({}),
            &env,
            dir.path(),
            "api",
            false,
            "gpt-6-astra",
        )
        .unwrap();
        assert_eq!(browser["cookieNames"], expected_names, "{input}");
        assert_eq!(browser["cookieSyncWaitMs"], json!(0u64), "{input}");
    }
}

#[test]
fn resolve_applies_prompt_suffix_and_config_base_url_and_background_only_as_fallbacks() {
    let dir = tempfile::tempdir().unwrap();
    let env = Environment::default();
    let config = json!({
        "promptSuffix": "PUBLIC suffix",
        "apiBaseUrl": "https://PUBLIC.example/v1",
        "background": true
    });

    let resolved = resolve(
        &json!({"prompt": "  PUBLIC prompt about caching  ", "engine": "api"}),
        &config,
        &env,
        dir.path(),
    )
    .unwrap();
    assert_eq!(resolved.engine, "api");
    assert_eq!(resolved.options["baseUrl"], "https://PUBLIC.example/v1");
    assert_eq!(resolved.options["background"], true);
    let expected = "PUBLIC prompt about caching\nPUBLIC suffix";
    let carried = [
        resolved.options["prompt"].as_str().unwrap_or_default(),
        resolved.user_prompt.as_str(),
    ];
    assert!(
        carried.iter().any(|text| text.contains(expected)),
        "suffix missing from {carried:?}"
    );

    let explicit = resolve(
        &json!({
            "prompt": "PUBLIC prompt about caching",
            "engine": "api",
            "baseUrl": "https://PUBLIC-explicit.example/v1",
            "background": false
        }),
        &config,
        &env,
        dir.path(),
    )
    .unwrap();
    assert_eq!(
        explicit.options["baseUrl"],
        "https://PUBLIC-explicit.example/v1"
    );
    assert_eq!(explicit.options["background"], false);
}
