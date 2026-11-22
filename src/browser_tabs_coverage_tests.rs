use super::*;
use crate::cdp::fake::{self, Browser, Reply, Request};

const IO_TIMEOUT: Duration = Duration::from_millis(500);
const CONVERSATION_URL: &str = "https://chatgpt.com/c/public-fixture";

fn endpoint(browser: &Browser) -> AttachmentEndpoint {
    AttachmentEndpoint {
        host: "127.0.0.1".into(),
        port: browser.port,
        browser_ws_endpoint: browser.browser_ws(),
    }
}

async fn within_budget<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(4), future)
        .await
        .expect("scripted CDP test exceeded its time budget")
}

#[test]
fn snippets_truncate_on_utf16_boundaries_and_trim_the_prefix() {
    let prefix = format!("PUBLIC-{}", "a".repeat(130));
    for (input, expected) in [
        (format!("\t{prefix}😀x\u{feff}"), format!("{prefix}😀x")),
        (format!("{prefix}😀xy"), format!("{prefix}😀…")),
        (format!("{prefix}yz😀"), format!("{prefix}yz…")),
        (format!("{prefix}x\t\u{00a0}yz"), format!("{prefix}x…")),
    ] {
        assert_eq!(snippet(&input), expected, "input: {input:?}");
    }
}

#[test]
fn classification_distinguishes_confirmed_completion_from_idle_and_running_tabs() {
    for (tab, expected) in [
        (json!({"authenticated": true}), "detached"),
        (
            json!({"authenticated": true, "assistantCount": 0, "completionVisible": true}),
            "detached",
        ),
        (
            json!({"authenticated": true, "promptReady": true, "completionConfirmed": true}),
            "completed",
        ),
        (
            json!({"authenticated": true, "stopExists": true, "completionConfirmed": true}),
            "running",
        ),
        (
            json!({"authenticated": false, "completionConfirmed": true}),
            "detached",
        ),
    ] {
        assert_eq!(classify(&tab), expected, "tab: {tab}");
    }
}

#[derive(Clone, Copy)]
enum CopyCase {
    Success,
    PrepareRefused,
    RunFailed { cancelled: bool, settled: bool },
}

#[tokio::test]
async fn turn_id_allows_markdown_copy_and_failed_copies_require_cleanup_acknowledgement() {
    within_budget(async {
        let target = json!({
            "targetId": "PUBLIC-target",
            "type": "page",
            "url": CONVERSATION_URL,
        });
        let cancel = CancellationToken::new();

        for case in [
            CopyCase::Success,
            CopyCase::PrepareRefused,
            CopyCase::RunFailed {
                cancelled: false,
                settled: true,
            },
            CopyCase::RunFailed {
                cancelled: true,
                settled: false,
            },
        ] {
            let mut attach_count = 0;
            let browser = fake::browser(json!([]), move |request| match request.method.as_str() {
                "Target.attachToTarget" => {
                    attach_count += 1;
                    Reply::Result(json!({"sessionId": format!("PUBLIC-session-{attach_count}")}))
                }
                "Runtime.enable" | "DOM.enable" => Reply::ok(),
                "Runtime.evaluate" => match request.arg()["action"].as_str() {
                    None => Reply::value(json!({
                        "url": CONVERSATION_URL,
                        "assistantCount": 1,
                        "lastAssistantTurnIndex": 1,
                        "assistantSnapshot": {
                            "turnIndex": 1,
                            "turnId": "PUBLIC-turn",
                            "text": "PUBLIC-rendered answer",
                        },
                    })),
                    Some("prepare") => {
                        Reply::value(json!({"prepared": !matches!(case, CopyCase::PrepareRefused)}))
                    }
                    Some("run") if matches!(case, CopyCase::Success) => Reply::value(
                        json!({"success": true, "markdown": "**PUBLIC-copied answer**"}),
                    ),
                    Some("run") => Reply::Error("PUBLIC-copy run failed".into()),
                    Some("cancel") => {
                        let (cancelled, settled) = match case {
                            CopyCase::RunFailed { cancelled, settled } => (cancelled, settled),
                            _ => (true, true),
                        };
                        Reply::value(json!({"cancelled": cancelled, "settled": settled}))
                    }
                    Some(action) => Reply::Error(format!("Unexpected copy action: {action}")),
                },
                method => Reply::Error(format!("Unexpected CDP method: {method}")),
            })
            .await;

            let result = inspect(&endpoint(&browser), &target, true, IO_TIMEOUT, &cancel).await;
            let (expected_actions, expected_attaches): (&[Option<&str>], usize) = match case {
                CopyCase::Success => {
                    let tab = result.unwrap();
                    assert!(tab.get("lastAssistantMessageId").is_none());
                    assert_eq!(tab["lastAssistantTurnId"], "PUBLIC-turn");
                    assert_eq!(tab["lastAssistantText"], "PUBLIC-rendered answer");
                    assert_eq!(tab["lastAssistantMarkdown"], "**PUBLIC-copied answer**");
                    assert!(tab.get("captureSource").is_none());
                    (&[None, Some("prepare"), Some("run"), None], 1)
                }
                CopyCase::PrepareRefused => {
                    assert_eq!(
                        result.unwrap_err().to_string(),
                        "Historical copy preparation was refused"
                    );
                    (&[None, Some("prepare"), Some("cancel")], 2)
                }
                CopyCase::RunFailed { .. } => {
                    let error = result.unwrap_err();
                    assert_eq!(
                        error.to_string(),
                        "Historical copy timer/clipboard cleanup could not be acknowledged"
                    );
                    assert_eq!(
                        error.root_cause().to_string(),
                        "Historical copy cleanup is unsettled"
                    );
                    (&[None, Some("prepare"), Some("run"), Some("cancel")], 2)
                }
            };

            let trace = browser.trace.lock().unwrap();
            let attaches: Vec<_> = trace
                .iter()
                .filter(|entry| entry["method"] == "Target.attachToTarget")
                .collect();
            assert_eq!(attaches.len(), expected_attaches);
            assert!(
                attaches
                    .iter()
                    .all(|entry| entry["params"]["targetId"] == "PUBLIC-target")
            );

            let evaluations: Vec<_> = trace
                .iter()
                .filter(|entry| entry["method"] == "Runtime.evaluate")
                .map(|entry| {
                    let arg = Request {
                        method: "Runtime.evaluate".into(),
                        params: entry["params"].clone(),
                        session: None,
                    }
                    .arg();
                    (&entry["sessionId"], arg)
                })
                .collect();
            let actions: Vec<_> = evaluations
                .iter()
                .map(|(_, arg)| arg["action"].as_str())
                .collect();
            assert_eq!(actions.as_slice(), expected_actions);

            let token = evaluations[1].1["captureToken"].as_str().unwrap();
            assert!(!token.is_empty());
            for (session, arg) in evaluations
                .iter()
                .filter(|(_, arg)| arg["action"].is_string())
            {
                let expected_session = if arg["action"] == "cancel" {
                    "PUBLIC-session-2"
                } else {
                    "PUBLIC-session-1"
                };
                assert_eq!(*session, expected_session);
                assert_eq!(arg["captureToken"], token);
                assert_eq!(
                    arg["meta"],
                    json!({"messageId": null, "turnId": "PUBLIC-turn"})
                );
                assert_eq!(
                    arg["receipt"]["conversation"],
                    json!({"conversationId": "public-fixture", "origin": "https://chatgpt.com"})
                );
            }
        }
    })
    .await;
}

#[tokio::test]
async fn collection_keeps_detached_errors_and_title_resolution_uses_sorted_summaries() {
    within_budget(async {
        let listing = json!({"targetInfos": [
            {"targetId": "PUBLIC-home", "type": "page",
             "url": "https://chatgpt.com/", "title": "PUBLIC-home tab"},
            {"targetId": "PUBLIC-failed", "type": "page",
             "url": "https://chatgpt.com/c/public-failed", "title": "PUBLIC-failed tab"},
            {"targetId": "PUBLIC-conversation", "type": "page",
             "url": CONVERSATION_URL, "title": "PUBLIC-conversation tab"},
            {"targetId": "PUBLIC-worker", "type": "worker", "url": CONVERSATION_URL},
            {"targetId": "PUBLIC-focused", "type": "page",
             "url": "https://chatgpt.com/", "title": "PUBLIC-focused tab"},
        ]});
        let browser = fake::browser(json!([]), move |request| match request.method.as_str() {
            "Target.getTargets" => Reply::Result(listing.clone()),
            "Target.attachToTarget" => {
                Reply::Result(json!({"sessionId": request.params["targetId"]}))
            }
            "Runtime.enable" | "DOM.enable" => Reply::ok(),
            "Runtime.evaluate" if request.session.as_deref() == Some("PUBLIC-failed") => {
                Reply::value(json!({"identityChanged": true}))
            }
            "Runtime.evaluate" => Reply::value(json!({
                "focused": request.session.as_deref() == Some("PUBLIC-focused"),
                "promptReady": true,
                "lastAssistantText": "PUBLIC-rendered answer",
            })),
            method => Reply::Error(format!("Unexpected CDP method: {method}")),
        })
        .await;
        let endpoint = endpoint(&browser);
        let cancel = CancellationToken::new();

        let rows = collect(&endpoint, IO_TIMEOUT, &cancel).await.unwrap();
        let ids: Vec<_> = rows
            .iter()
            .map(|row| row["targetId"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            ["PUBLIC-focused", "PUBLIC-failed", "PUBLIC-conversation", "PUBLIC-home"]
        );
        assert_eq!(rows[0]["focused"], json!(true));
        assert_eq!(
            rows[1],
            json!({
                "host": "127.0.0.1",
                "port": browser.port,
                "targetId": "PUBLIC-failed",
                "title": "PUBLIC-failed tab",
                "url": "https://chatgpt.com/c/public-failed",
                "conversationId": "public-failed",
                "state": "detached",
                "error": "ChatGPT conversation changed or inspection failed; refusing mixed capture.",
            })
        );
        assert_eq!(rows[2]["state"], "running");
        assert_eq!(rows[2]["lastAssistantText"], "PUBLIC-rendered answer");
        assert!(rows[2]["lastAssistantMarkdown"].is_null());

        let resolved = resolve_target(
            &endpoint,
            Some("  CoNvErSaTiOn TaB  "),
            false,
            IO_TIMEOUT,
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(resolved, rows[2]);
    })
    .await;
}

#[tokio::test]
async fn cancellation_during_inspection_stops_collection_before_the_next_tab() {
    within_budget(async {
        let cancel = CancellationToken::new();
        let cancel_on_inspection = cancel.clone();
        let browser = fake::browser(json!([]), move |request| match request.method.as_str() {
            "Target.getTargets" => Reply::Result(json!({"targetInfos": [
                {"targetId": "PUBLIC-first", "type": "page", "url": CONVERSATION_URL},
                {"targetId": "PUBLIC-next", "type": "page", "url": "https://chatgpt.com/"},
            ]})),
            "Target.attachToTarget" => {
                Reply::Result(json!({"sessionId": request.params["targetId"]}))
            }
            "Runtime.enable" | "DOM.enable" => Reply::ok(),
            "Runtime.evaluate" => {
                cancel_on_inspection.cancel();
                Reply::value(json!({"promptReady": true}))
            }
            method => Reply::Error(format!("Unexpected CDP method: {method}")),
        })
        .await;

        let error = collect(&endpoint(&browser), IO_TIMEOUT, &cancel)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Historical collection cancelled");

        let trace = browser.trace.lock().unwrap();
        let attached: Vec<_> = trace
            .iter()
            .filter(|entry| entry["method"] == "Target.attachToTarget")
            .map(|entry| entry["params"]["targetId"].as_str().unwrap())
            .collect();
        assert_eq!(attached, ["PUBLIC-first"]);
    })
    .await;
}
