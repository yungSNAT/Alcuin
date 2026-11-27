use super::*;
use crate::cdp::fake::{self, Reply};

const DESTINATION: &str = "https://chatgpt.com/";
const COOKIE_PAYLOAD: &str = r#"[{"name":"PUBLIC-cookie","value":"PUBLIC-cookie-value"}]"#;
const CDP_TIMEOUT: Duration = Duration::from_millis(200);

fn isolated_environment(home: &Path) -> Environment {
    Environment::from([(
        "ORACLE_HOME_DIR".into(),
        home.to_string_lossy().into_owned(),
    )])
}

async fn finish_peer(page: Cdp, task: tokio::task::JoinHandle<Vec<Value>>) -> Vec<Value> {
    page.disconnect_and_wait(CDP_TIMEOUT).await.unwrap();
    tokio::time::timeout(CDP_TIMEOUT, task)
        .await
        .unwrap()
        .unwrap()
}

#[test]
fn blank_and_oversized_candidates_are_ignored_and_non_utf8_files_fall_back() {
    let home = tempfile::tempdir().unwrap();
    let oversized = format!(
        r#"[{{"name":"PUBLIC-oversized","value":"PUBLIC-{}"}}]"#,
        "x".repeat(MAX_COOKIE_INPUT)
    );
    for raw in [" \t\n", oversized.as_str()] {
        assert!(
            CookieBootstrap::candidate(raw, "inline-file", true, home.path())
                .unwrap()
                .is_none()
        );
    }

    // Lossy UTF-8 decoding would turn this into a valid array and select the wrong source.
    std::fs::write(
        home.path().join("PUBLIC-invalid-utf8.json"),
        b"[{\"name\":\"PUBLIC-wrong-source\",\"value\":\"PUBLIC-\xff\"}]",
    )
    .unwrap();
    let batch = CookieBootstrap::resolve_explicit(
        &json!({
            "browserInlineCookiesFile": "PUBLIC-invalid-utf8.json",
            "browserInlineCookies": COOKIE_PAYLOAD
        }),
        &json!({}),
        &isolated_environment(home.path()),
        home.path(),
    )
    .unwrap()
    .unwrap();

    assert_eq!(batch.requirement()["source"], "inline-arg");
    let parameters = batch.parameters(DESTINATION).unwrap();
    assert_eq!(parameters.len(), 1);
    assert_eq!(parameters[0]["name"], "PUBLIC-cookie");
    assert_eq!(parameters[0]["value"], "PUBLIC-cookie-value");
}

#[test]
fn configured_cookie_string_survives_the_worker_gate_and_rounds_expiry_seconds() {
    let home = tempfile::tempdir().unwrap();
    let payload = r#"[
        {"name":"PUBLIC-round-down","value":"PUBLIC-\"quoted\"","expires":1700000000.49},
        {"name":"PUBLIC-round-up","value":"PUBLIC-other-value","expires":1700000000.5}
    ]"#;
    let mut config = json!({
        "inlineCookies": STANDARD.encode(payload),
        "inlineCookiesSource": "PUBLIC-legacy-source"
    });
    let batch = prepare_config(
        &json!({}),
        &mut config,
        &isolated_environment(home.path()),
        home.path(),
    )
    .unwrap()
    .unwrap();

    let requirement =
        json!({"version": 1, "kind": "explicit-cookies", "source": "configured-inline"});
    assert_eq!(
        config,
        json!({
            "cookieBootstrapRequired": requirement,
            "cookieSync": true,
            "manualLoginCookieSync": true
        })
    );

    let identity = crate::process::ProcessIdentity::current().unwrap();
    let encoded = encode_start_gate(Some(&batch), "PUBLIC-worker-session", &identity).unwrap();
    assert!(encoded.starts_with(b"cookies-v1\n"));
    let restored = decode_start_gate(&encoded, "PUBLIC-worker-session", &identity)
        .unwrap()
        .unwrap();
    assert_eq!(restored.requirement(), requirement);

    let parameters = restored.parameters(DESTINATION).unwrap();
    assert_eq!(parameters.len(), 2);
    assert_eq!(parameters[0]["name"], "PUBLIC-round-down");
    assert_eq!(parameters[0]["value"], "PUBLIC-\"quoted\"");
    assert_eq!(parameters[0]["expires"], 1_700_000_000.0);
    assert_eq!(parameters[1]["name"], "PUBLIC-round-up");
    assert_eq!(parameters[1]["expires"], 1_700_000_001.0);
}

#[test]
fn invalid_configured_cookies_do_not_enable_cookie_sync_or_save_a_requirement() {
    let home = tempfile::tempdir().unwrap();
    let env = isolated_environment(home.path());

    for (payload, destination, expected_error) in [
        (
            "PUBLIC-invalid-cookie-payload",
            DESTINATION,
            "Explicit browser cookie input is invalid or exceeds the supported limits",
        ),
        (
            COOKIE_PAYLOAD,
            "https://public-untrusted.example.invalid/",
            "Cookie bootstrap requires a trusted provider destination",
        ),
        (
            r#"[{"value":"PUBLIC-nameless-cookie"}]"#,
            DESTINATION,
            "Explicit browser cookie bootstrap contains no usable cookies",
        ),
    ] {
        let mut config = json!({
            "inlineCookies": payload,
            "chatgptUrl": destination,
            "cookieSync": false,
            "manualLoginCookieSync": false
        });
        let error = prepare_config(&json!({}), &mut config, &env, home.path()).unwrap_err();

        assert_eq!(error.to_string(), expected_error);
        assert!(config["cookieBootstrapRequired"].is_null());
        assert_eq!(config["cookieSync"], json!(false));
        assert_eq!(config["manualLoginCookieSync"], json!(false));
    }
}

#[tokio::test]
async fn profile_cookie_writes_report_refusal_or_the_selected_profile_name() {
    let batch = CookieBootstrap::parse(COOKIE_PAYLOAD, "chrome-profile")
        .unwrap()
        .unwrap();

    for acknowledged in [false, true] {
        let (page, task) = fake::peer(CDP_TIMEOUT, move |_| {
            Reply::Result(json!({"success": acknowledged}))
        })
        .await;
        let result = batch
            .apply(&page, DESTINATION, CDP_TIMEOUT, &CancellationToken::new())
            .await;

        if acknowledged {
            let count = result.unwrap();
            assert_eq!(count, 1);
            assert_eq!(
                batch.applied_line(count, &json!({"chromeProfile": "PUBLIC-alt-profile"})),
                "Copied 1 cookies from Chrome profile PUBLIC-alt-profile"
            );
            assert_eq!(
                batch.applied_line(count, &json!({})),
                "Copied 1 cookies from Chrome profile Default"
            );
        } else {
            assert_eq!(result.unwrap_err().to_string(), NO_PROFILE_COOKIES);
        }

        let trace = finish_peer(page, task).await;
        assert_eq!(trace.len(), 1);
        assert_eq!(trace[0]["method"], "Network.setCookie");
        assert_eq!(trace[0]["params"]["name"], "PUBLIC-cookie");
        assert_eq!(trace[0]["params"]["value"], "PUBLIC-cookie-value");
    }
}

#[tokio::test]
async fn stale_cookie_cleanup_logs_a_redacted_inspection_failure_without_deleting() {
    let (page, task) = fake::peer(CDP_TIMEOUT, |request| match request.method.as_str() {
        "Target.getTargets" => Reply::Result(json!({"targetInfos": []})),
        "Network.getAllCookies" => Reply::Error("PUBLIC-private-error-detail".into()),
        _ => Reply::ok(),
    })
    .await;

    let lines =
        clear_stale_conversation_cookies(&page, &page, &[], CDP_TIMEOUT, &CancellationToken::new())
            .await;
    let trace = finish_peer(page, task).await;

    assert_eq!(
        lines,
        [
            "[cookies] Failed to inspect ChatGPT conversation cookies: CDP command Network.getAllCookies was refused by the endpoint."
        ]
    );
    let methods: Vec<_> = trace
        .iter()
        .map(|command| command["method"].as_str().unwrap())
        .collect();
    assert_eq!(methods, ["Target.getTargets", "Network.getAllCookies"]);
}
