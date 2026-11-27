use super::*;
use crate::cdp::fake::{self, Reply};

const TIMEOUT: Duration = Duration::from_secs(1);
const PAGE_SESSION: &str = "PUBLIC-page-session";
const FRAME_SESSION: &str = "PUBLIC-frame-session";
const PAGE_TARGET: &str = "PUBLIC-page";
const BASELINE_TARGET: &str = "PUBLIC-baseline-frame";
const CONVERSATION: &str = "public-conversation";
const PAGE_URL: &str = "https://chatgpt.com/c/public-conversation";
const REPORT_URL: &str = "https://chatgpt.com/deep-research/PUBLIC-report";
const USER: &str = "PUBLIC-user";
const PROMPT: &str = "PUBLIC complete research request";
const REPORT_TEXT: &str =
    "PUBLIC completed report with more than forty characters of research findings.";
const REPORT_HTML: &str = "<p>PUBLIC completed report</p>";

fn metadata() -> Value {
    let hash = browser::prompt_fingerprint(PROMPT, USER);
    let owner = json!({"reservationId": "PUBLIC-reservation"});
    json!({
        "dispatch": {"submissionOwner": owner},
        "browser": {
            "config": {"researchMode": "deep"},
            "runtime": {
                "chromeTargetId": PAGE_TARGET,
                "conversationId": CONVERSATION,
                "tabUrl": PAGE_URL,
                "submittedPromptHash": hash,
                "submittedPromptHashFormat": "original-source-v1",
                "ownedRecoveryTarget": {"targetId": PAGE_TARGET, "conversationId": CONVERSATION},
                "userCommitReceipt": {"messageId": USER, "messageFingerprint": hash},
                "researchTargetBaseline": {
                    "version": 1,
                    "pageTargetId": PAGE_TARGET,
                    "submissionOwner": owner,
                    "targetIds": [BASELINE_TARGET]
                }
            }
        }
    })
}

fn source_snapshot() -> Value {
    json!({
        "currentUrl": PAGE_URL,
        "conversationId": CONVERSATION,
        "lastUserMessageId": USER,
        "lastUserOriginalSourceMessageId": USER,
        "lastUserOriginalSourceText": PROMPT,
        "lastUserTurnIndex": 2,
        "scopedResearchIframeCount": 0,
        "researchStatus": {
            "accountBlocked": false,
            "researchActivity": false,
            "hasActiveScopedResearch": false,
            "incompleteResult": false
        }
    })
}

fn attachment(session: &str, target: &str, url: &str) -> Value {
    json!({
        "method": "Target.attachedToTarget",
        "sessionId": PAGE_SESSION,
        "params": {
            "sessionId": session,
            "targetInfo": {"type": "iframe", "targetId": target, "url": url}
        }
    })
}

fn page_info(url: &str) -> Reply {
    Reply::Result(json!({
        "targetInfo": {"type": "page", "targetId": PAGE_TARGET, "url": url}
    }))
}

async fn finish_peer(root: &Cdp, task: tokio::task::JoinHandle<Vec<Value>>) -> Vec<Value> {
    root.disconnect_and_wait(TIMEOUT).await.unwrap();
    tokio::time::timeout(TIMEOUT, task).await.unwrap().unwrap()
}

fn methods(trace: &[Value]) -> Vec<&str> {
    trace
        .iter()
        .map(|request| request["method"].as_str().unwrap())
        .collect()
}

fn completed_frame() -> Value {
    json!({
        "completed": true,
        "researchStarted": false,
        "textLength": REPORT_TEXT.len(),
        "text": REPORT_TEXT,
        "html": REPORT_HTML
    })
}

fn planning_frame() -> Value {
    json!({
        "completed": false,
        "researchStarted": false,
        "textLength": 0,
        "planTitle": "PUBLIC research plan",
        "planSteps": ["PUBLIC compare sources"],
        "planActionText": "PUBLIC edit plan"
    })
}

async fn observe_fixture(
    before: Value,
    after: Value,
    frames: Vec<(&'static str, Value)>,
) -> (Result<ReportObservation>, Vec<Value>) {
    let mut snapshots = [before, after].into_iter();
    let (root, task) = fake::peer(TIMEOUT, move |request| match request.method.as_str() {
        "Runtime.evaluate" if request.params.get("contextId").is_none() => {
            assert_eq!(request.session.as_deref(), Some(PAGE_SESSION));
            assert_eq!(
                request.arg(),
                json!({"sourceOnly": true, "fixture": "PUBLIC-preserved"})
            );
            Reply::value(snapshots.next().expect("only two source observations"))
        }
        "Target.getTargetInfo" => page_info(PAGE_URL),
        "Target.setAutoAttach" if request.params["autoAttach"] == true => Reply::WithEvents(
            json!({}),
            vec![attachment(FRAME_SESSION, BASELINE_TARGET, REPORT_URL)],
        ),
        "Target.setAutoAttach" | "Runtime.releaseObject" => Reply::ok(),
        "Target.detachFromTarget" => {
            assert!(request.session.is_none());
            assert_eq!(request.params["sessionId"], FRAME_SESSION);
            Reply::ok()
        }
        "Page.getFrameTree" => {
            assert_eq!(request.session.as_deref(), Some(PAGE_SESSION));
            let children: Vec<_> = frames
                .iter()
                .map(|(id, _)| json!({"frame": {"id": id, "url": REPORT_URL}}))
                .chain(std::iter::once(
                    json!({"frame": {"id": BASELINE_TARGET, "url": REPORT_URL}}),
                ))
                .collect();
            Reply::Result(json!({
                "frameTree": {
                    "frame": {"id": "PUBLIC-page-frame", "url": PAGE_URL},
                    "childFrames": children
                }
            }))
        }
        "DOM.getFrameOwner" => {
            assert!(
                frames
                    .iter()
                    .any(|(id, _)| request.params["frameId"] == *id),
                "unexpected frame owner lookup: {}",
                request.params
            );
            Reply::Result(json!({"backendNodeId": 17}))
        }
        "DOM.resolveNode" => Reply::Result(json!({"object": {"objectId": "PUBLIC-owner-object"}})),
        "Runtime.callFunctionOn" => Reply::value(json!(2)),
        "Page.createIsolatedWorld" => {
            let index = frames
                .iter()
                .position(|(id, _)| request.params["frameId"] == *id)
                .unwrap();
            Reply::Result(json!({"executionContextId": index + 1}))
        }
        "Runtime.evaluate" => {
            assert!(request.expression().contains(REPORT_URL));
            let context = usize::try_from(request.params["contextId"].as_u64().unwrap()).unwrap();
            Reply::value(frames[context - 1].1.clone())
        }
        method => panic!("unexpected observation operation: {method}"),
    })
    .await;
    let page = root.with_session(PAGE_SESSION.into());
    let result = observe_report(
        &page,
        &metadata(),
        &json!({"sourceOnly": false, "fixture": "PUBLIC-preserved"}),
        TIMEOUT,
        &CancellationToken::new(),
    )
    .await;
    (result, finish_peer(&root, task).await)
}

#[tokio::test]
async fn report_observation_rejects_invalid_baselines_before_reading_the_page() {
    let (root, task) = fake::peer(TIMEOUT, |_| Reply::Error("PUBLIC unexpected read".into())).await;
    let page = root.with_session(PAGE_SESSION.into());
    let cancel = CancellationToken::new();
    let baseline_error = "Research pre-submission target baseline is unavailable";
    for (field, replacement, expected) in [
        ("version", json!(2), baseline_error),
        ("pageTargetId", json!("PUBLIC-other-page"), baseline_error),
        ("submissionOwner", Value::Null, baseline_error),
        (
            "submissionOwner",
            json!({"reservationId": "PUBLIC-different-reservation"}),
            baseline_error,
        ),
        (
            "targetIds",
            json!({}),
            "Research target baseline is malformed",
        ),
        (
            "targetIds",
            json!([17]),
            "Research baseline target identity is malformed",
        ),
        (
            "targetIds",
            json!([""]),
            "Research baseline target identity is malformed",
        ),
    ] {
        let mut saved = metadata();
        saved["browser"]["runtime"]["researchTargetBaseline"][field] = replacement;
        let Err(error) = observe_report(&page, &saved, &json!({}), TIMEOUT, &cancel).await else {
            panic!("accepted invalid baseline field {field}");
        };
        assert_eq!(error.to_string(), expected, "{field}");
    }

    let mut saved = metadata();
    saved["browser"]["runtime"]["chromeTargetId"] = Value::Null;
    for (timeout, expected) in [
        (Duration::MAX, "Invalid report observation deadline"),
        (TIMEOUT, "Research page target is unavailable"),
    ] {
        let Err(error) = observe_report(&page, &saved, &json!({}), timeout, &cancel).await else {
            panic!("accepted invalid deadline or target");
        };
        assert_eq!(error.to_string(), expected);
    }
    assert!(finish_peer(&root, task).await.is_empty());
}

#[tokio::test]
async fn completed_report_replaces_assistant_content_with_owned_frame_evidence() {
    let mut before = source_snapshot();
    before["researchStatus"]["researchActivity"] = json!(true);
    before["lastAssistantMessageId"] = json!("PUBLIC-stale-message");
    before["lastAssistantTurnId"] = json!("PUBLIC-stale-turn");
    before["images"] = json!(["PUBLIC-stale-image"]);
    before["files"] = json!(["PUBLIC-stale-file"]);
    let mut after = before.clone();
    after["researchStatus"]["researchActivity"] = json!(false);
    after["scopedResearchIframeCount"] = json!(1);

    let (result, trace) = observe_fixture(
        before,
        after,
        vec![("PUBLIC-report-frame", completed_frame())],
    )
    .await;
    let observed = result.unwrap();
    assert!(observed.plan.is_none());
    assert!(!observed.pending_research);
    assert!(observed.research_observed);
    let report = observed.report.unwrap();
    assert_eq!(report["kind"], "deep-research-report");
    assert_eq!(report["lastUserMessageId"], USER);
    assert_eq!(report["lastUserOriginalSourceText"], PROMPT);
    assert_eq!(report["text"], REPORT_TEXT);
    assert_eq!(report["providerText"], REPORT_TEXT);
    assert_eq!(report["html"], REPORT_HTML);
    assert_eq!(
        report["report"],
        json!({
            "version": 1,
            "scope": "page-frame",
            "pageTargetId": PAGE_TARGET,
            "targetId": PAGE_TARGET,
            "frameId": "PUBLIC-report-frame",
            "url": REPORT_URL,
            "ownerTurnIndex": 2,
            "completed": true,
            "researchStarted": false,
            "textSha256": sha256(REPORT_TEXT.as_bytes())
        })
    );
    for key in ["lastAssistantMessageId", "lastAssistantTurnId"] {
        assert_eq!(report.get(key), Some(&Value::Null), "{key}");
    }
    for key in ["images", "files"] {
        assert_eq!(report[key], json!([]), "{key}");
    }
    assert!(report["completionVisible"] == true);
    assert_eq!(trace.first().unwrap()["method"], "Runtime.evaluate");
    assert_eq!(trace.last().unwrap()["method"], "Runtime.evaluate");
    assert_eq!(
        trace
            .iter()
            .filter(|request| request["method"] == "Page.createIsolatedWorld")
            .count(),
        1
    );
}

#[tokio::test]
async fn report_observation_propagates_plans_and_distinguishes_pending_from_seen() {
    let idle = source_snapshot();
    let mut iframe = idle.clone();
    iframe["scopedResearchIframeCount"] = json!(1);
    let mut incomplete = idle.clone();
    incomplete["researchStatus"]["incompleteResult"] = json!(true);
    let mut active = idle.clone();
    active["researchStatus"]["hasActiveScopedResearch"] = json!(true);

    for (before, after, expected) in [
        (&idle, &idle, (false, false)),
        (&idle, &iframe, (true, true)),
        (&iframe, &idle, (false, true)),
        (&idle, &incomplete, (true, false)),
        (&active, &idle, (false, true)),
        (&idle, &active, (true, true)),
    ] {
        let (result, _) = observe_fixture(before.clone(), after.clone(), vec![]).await;
        let observed = result.unwrap();
        assert_eq!(
            (observed.pending_research, observed.research_observed),
            expected
        );
        assert!(observed.plan.is_none());
        assert!(observed.report.is_none());
    }

    let (result, _) = observe_fixture(
        idle.clone(),
        idle,
        vec![("PUBLIC-plan-frame", planning_frame())],
    )
    .await;
    let observed = result.unwrap();
    assert!(observed.report.is_none());
    assert!(observed.pending_research && observed.research_observed);
    assert_eq!(
        observed.plan,
        Some(json!({
            "title": "PUBLIC research plan",
            "steps": ["PUBLIC compare sources"],
            "actionText": "PUBLIC edit plan",
            "researchStarted": false
        }))
    );
}

#[tokio::test]
async fn report_capture_rejects_changed_source_evidence_and_competing_active_research() {
    let before = source_snapshot();
    let mut shifted = before.clone();
    shifted["turnsCount"] = json!(4);
    for (after, frames, expected) in [
        (
            shifted,
            vec![("PUBLIC-report-frame", completed_frame())],
            "Research original source changed during capture",
        ),
        (
            before.clone(),
            vec![
                ("PUBLIC-report-frame", completed_frame()),
                ("PUBLIC-plan-frame", planning_frame()),
            ],
            "Completed and active research reports are ambiguous",
        ),
    ] {
        let (result, trace) = observe_fixture(before.clone(), after, frames).await;
        let Err(error) = result else {
            panic!("accepted ambiguous capture: {expected}");
        };
        assert_eq!(error.to_string(), expected);
        assert!(methods(&trace).contains(&"Target.detachFromTarget"));
    }
}

#[test]
fn main_status_requires_boolean_evidence_and_rejects_a_blocked_account() {
    let idle = source_snapshot();
    assert_eq!(main_status(&idle).unwrap(), (false, false));
    for field in [
        "accountBlocked",
        "researchActivity",
        "hasActiveScopedResearch",
        "incompleteResult",
    ] {
        let mut malformed = idle.clone();
        malformed["researchStatus"][field] = json!("PUBLIC-not-a-boolean");
        assert_eq!(
            main_status(&malformed).unwrap_err().to_string(),
            "Research main-page status is unavailable",
            "{field}"
        );
        let mut flagged = idle.clone();
        flagged["researchStatus"][field] = json!(true);
        match field {
            "accountBlocked" => assert_eq!(
                main_status(&flagged).unwrap_err().to_string(),
                "chatgpt-account-blocked: ChatGPT detected suspicious activity; secure the account in Chrome before retrying"
            ),
            "incompleteResult" => assert_eq!(main_status(&flagged).unwrap(), (true, false)),
            _ => assert_eq!(main_status(&flagged).unwrap(), (true, true)),
        }
    }
}

#[test]
fn source_evidence_keeps_non_object_values_intact() {
    for raw in [
        Value::Null,
        json!("PUBLIC-source"),
        json!([{"stopExists": true}]),
    ] {
        assert_eq!(source_evidence(&raw), raw);
    }
}

#[test]
fn attachment_drain_rejects_lagged_and_closed_inventories() {
    let (sender, mut events) = tokio::sync::broadcast::channel(1);
    sender
        .send(attachment("PUBLIC-lost", "PUBLIC-lost-frame", REPORT_URL))
        .unwrap();
    sender
        .send(attachment(FRAME_SESSION, BASELINE_TARGET, REPORT_URL))
        .unwrap();
    let mut sessions = BTreeMap::new();
    let message = "Research page attachment inventory is incomplete";

    assert_eq!(
        drain(&mut events, PAGE_SESSION, &mut sessions)
            .unwrap_err()
            .to_string(),
        message
    );
    assert!(
        sessions.is_empty(),
        "a lagged drain must not succeed with a partial inventory"
    );
    drain(&mut events, PAGE_SESSION, &mut sessions).unwrap();
    assert_eq!(iframe_target_keys(&sessions).unwrap(), [BASELINE_TARGET]);
    drop(sender);
    assert_eq!(
        drain(&mut events, PAGE_SESSION, &mut sessions)
            .unwrap_err()
            .to_string(),
        message
    );
}

#[tokio::test]
async fn frame_scan_checks_the_parent_and_only_tolerates_tree_errors_for_non_report_targets() {
    for (parent_url, child_url, expected_error) in [
        (
            "https://chatgpt.com/c/public-other",
            REPORT_URL,
            Some("Research parent page or conversation changed"),
        ),
        (PAGE_URL, "about:blank", None),
        (
            PAGE_URL,
            REPORT_URL,
            Some("CDP command Page.getFrameTree was refused by the endpoint."),
        ),
    ] {
        let (root, task) = fake::peer(TIMEOUT, move |request| match request.method.as_str() {
            "Target.getTargetInfo" => page_info(parent_url),
            "Target.setAutoAttach" if request.params["autoAttach"] == true => Reply::WithEvents(
                json!({}),
                vec![attachment(FRAME_SESSION, BASELINE_TARGET, child_url)],
            ),
            "Target.setAutoAttach" | "Target.detachFromTarget" => Reply::ok(),
            "Page.getFrameTree" if request.session.as_deref() == Some(FRAME_SESSION) => {
                Reply::Error("PUBLIC frame tree unavailable".into())
            }
            "Page.getFrameTree" => Reply::Result(json!({
                "frameTree": {"frame": {"id": "PUBLIC-page-frame", "url": PAGE_URL}}
            })),
            method => panic!("unexpected frame scan operation: {method}"),
        })
        .await;
        let page = root.with_session(PAGE_SESSION.into());
        let result = scan_reports(
            &page,
            PAGE_TARGET,
            Some((CONVERSATION, 2)),
            &[],
            TIMEOUT,
            &CancellationToken::new(),
        )
        .await;
        let trace = finish_peer(&root, task).await;

        if let Some(expected) = expected_error {
            assert_eq!(result.unwrap_err().to_string(), expected);
        } else {
            let scan = result.unwrap();
            assert_eq!(scan.target_keys, [BASELINE_TARGET]);
            assert_eq!(scan.inspected_frames, 0);
            assert!(!scan.pending_research);
            assert!(scan.report.is_none());
        }
        if parent_url != PAGE_URL {
            assert_eq!(methods(&trace), ["Target.getTargetInfo"]);
            continue;
        }

        let tree_sessions: Vec<_> = trace
            .iter()
            .filter(|request| request["method"] == "Page.getFrameTree")
            .map(|request| request["sessionId"].as_str().unwrap())
            .collect();
        if expected_error.is_none() {
            assert_eq!(tree_sessions, [FRAME_SESSION, PAGE_SESSION]);
        } else {
            assert_eq!(tree_sessions, [FRAME_SESSION]);
        }
        let auto_attach: Vec<_> = trace
            .iter()
            .filter(|request| request["method"] == "Target.setAutoAttach")
            .map(|request| request["params"]["autoAttach"].as_bool().unwrap())
            .collect();
        assert_eq!(auto_attach, [true, false]);
        let detach = trace.last().unwrap();
        assert_eq!(detach["method"], "Target.detachFromTarget");
        assert_eq!(detach["params"]["sessionId"], FRAME_SESSION);
        assert!(detach.get("sessionId").is_none());
    }
}

#[tokio::test]
async fn owner_lookup_returns_none_without_evaluating_an_unresolved_node() {
    for has_backend_node in [false, true] {
        let (root, task) = fake::peer(TIMEOUT, move |request| match request.method.as_str() {
            "DOM.getFrameOwner" => {
                assert_eq!(request.params["frameId"], "PUBLIC-unowned-frame");
                if has_backend_node {
                    Reply::Result(json!({"backendNodeId": 17}))
                } else {
                    Reply::ok()
                }
            }
            "DOM.resolveNode" => {
                assert_eq!(request.params["backendNodeId"], 17);
                Reply::Result(json!({"object": {"description": "PUBLIC unresolved owner"}}))
            }
            method => panic!("unexpected owner lookup operation: {method}"),
        })
        .await;
        let page = root.with_session(PAGE_SESSION.into());
        let owner = owner_turn(
            &page,
            "PUBLIC-unowned-frame",
            Instant::now() + TIMEOUT,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(owner, None);
        let trace = finish_peer(&root, task).await;
        let expected: &[_] = if has_backend_node {
            &["DOM.getFrameOwner", "DOM.resolveNode"]
        } else {
            &["DOM.getFrameOwner"]
        };
        assert_eq!(methods(&trace), expected);
    }
}
