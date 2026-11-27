use super::*;

fn fixture_request() -> BrowserRequest {
    serde_json::from_value(json!({
        "version": 1,
        "sessionId": "PUBLIC-session",
        "reservationId": "PUBLIC-reservation",
        "prompt": "PUBLIC-prompt",
        "promptSha256": "PUBLIC-prompt-hash",
        "optionsSha256": "PUBLIC-options-hash",
        "uploadPaths": [],
        "estimatedInputTokens": 12
    }))
    .unwrap()
}

#[test]
fn control_plan_distinguishes_existing_tabs_from_new_tabs_and_reports_keep_open() {
    let cases: [(Value, &[&str]); 4] = [
        (
            json!({
                "attachRunning": true,
                "browserTabRef": "  PUBLIC-local-tab  ",
                "keepBrowser": true
            }),
            &[
                "[browser] Browser control: attach to an already-running local Chrome tab; \
                 may focus/control the browser UI.",
                "[browser] Browser guidance: Alcuin reuses the matching ChatGPT tab \
                 (PUBLIC-local-tab) and leaves the existing browser process alone.",
                "[browser] Browser guidance: The browser stays open because Alcuin did not launch it.",
            ],
        ),
        (
            json!({"attachRunning": true, "browserTabRef": " \t "}),
            &[
                "[browser] Browser control: attach to an already-running local Chrome session; \
                 may focus/control the browser UI.",
                "[browser] Browser guidance: Alcuin opens a dedicated tab and leaves the existing \
                 browser process alone.",
            ],
        ),
        (
            json!({"remoteChrome": {}, "browserTabRef": "PUBLIC-remote-tab", "headless": true}),
            &[
                "[browser] Browser control: reuse an existing remote Chrome tab; \
                 does not use a visible local browser window.",
                "[browser] Browser guidance: Alcuin reuses the matching ChatGPT tab \
                 (PUBLIC-remote-tab) in the configured remote Chrome session.",
                "[browser] Browser guidance: Local Chrome launch, cookie copy, and window hiding \
                 flags are skipped.",
            ],
        ),
        (
            json!({"keepBrowser": true}),
            &[
                "[browser] Browser control: launch visible Chrome; may focus/control the browser UI.",
                "[browser] Browser guidance: A visible automation Chrome window may take focus \
                 while Alcuin controls ChatGPT.",
                "[browser] Browser guidance: Use --browser-attach-running or --remote-chrome \
                 to reduce desktop disruption.",
                "[browser] Browser guidance: Chrome will remain open after the run because \
                 --browser-keep-browser is enabled.",
            ],
        ),
    ];

    for (config, expected) in cases {
        assert_eq!(
            control_plan(&config).as_slice(),
            expected,
            "config: {config}"
        );
    }
}

#[test]
fn verbose_header_redacts_remote_token_even_without_inline_cookies() {
    let config = json!({
        "desiredModel": "PUBLIC-target",
        "remoteToken": "PUBLIC-remote-token"
    });
    let meta = json!({"options": {"verbose": true}, "browser": {"config": config}});

    let output = header(&meta, &fixture_request()).unwrap();
    let config_line = output
        .lines()
        .next()
        .unwrap()
        .strip_prefix("[verbose] Browser config: ")
        .unwrap();
    let printed: Value = serde_json::from_str(config_line).unwrap();

    assert_eq!(printed, json!({"desiredModel": "PUBLIC-target"}));
    assert!(!output.contains("PUBLIC-remote-token"));
    assert_eq!(meta["browser"]["config"], config);
}

#[test]
fn completion_formats_non_string_evidence_and_hides_only_the_answer_when_silent() {
    let mut meta = json!({
        "model": "PUBLIC-model",
        "browser": {"modelSelection": {"status": 7, "source": false}}
    });
    let request = fixture_request();
    let warnings = [json!({"message": "PUBLIC-warning"})];
    let evidence = concat!(
        "[browser] Model selection evidence: requestedKey=PUBLIC-model; target=(none); ",
        "resolvedLabel=(unavailable); status=7; strategy=(default); verified=no; ",
        "source=false; capturedAt=undefined\n",
        "[browser] PUBLIC-warning\n",
    );
    let finish = "unknown · PUBLIC-model[browser] · ↑12 ↓0 ↻0 Δ12\n";
    let heading = crate::browser_output::answer_heading(&meta);

    assert_eq!(
        completion(&meta, &request, &warnings, ""),
        format!("{evidence}{heading}\n(no text output)\n\n{finish}")
    );

    meta["options"] = json!({"silent": true});
    assert_eq!(
        completion(&meta, &request, &warnings, ""),
        format!("{evidence}{finish}")
    );
}

#[test]
fn thinking_monitor_reports_a_subsecond_status_age_in_milliseconds() {
    let mut monitor = ThinkingMonitor::new(&json!({"heartbeatIntervalMs": 1000}));
    let thinking = json!({"strongThinkingActive": true});

    assert_eq!(
        monitor.line(1_999, &thinking).as_deref(),
        Some("[browser] ChatGPT thinking - 1s elapsed; status=active; source=inline")
    );
    assert_eq!(
        monitor.line(2_000, &thinking).as_deref(),
        Some(
            "[browser] ChatGPT thinking - 2s elapsed; status=active; last change 1ms ago; source=inline"
        )
    );
    assert_eq!(monitor.line(2_999, &thinking), None);
}
