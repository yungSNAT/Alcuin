use super::*;
use crate::cdp::fake::{self, Reply, Request};
use tokio::task::JoinHandle;

const IO_TIMEOUT: Duration = Duration::from_secs(1);
const PAGE_URL: &str = "https://chatgpt.com/";
const TRANSFER_NAME: &str = "PUBLIC-transfer.txt";
const TRANSFER_BYTES: &[u8] = b"PUBLIC-transfer contents\n";

async fn owned_peer(
    cancel: &CancellationToken,
    mut respond: impl FnMut(&Request) -> Option<Reply> + Send + 'static,
) -> (RendererReservation, JoinHandle<Vec<Value>>) {
    let (page, peer) = fake::peer(IO_TIMEOUT, move |request| {
        if request.method != "Runtime.evaluate" {
            return Reply::ok();
        }
        if request.arg()["action"] == "acquire" {
            return Reply::value(json!(true));
        }
        respond(request).unwrap_or_else(|| Reply::value(json!(true)))
    })
    .await;
    let reservation =
        RendererReservation::acquire(page, "PUBLIC-coverage-claim".into(), IO_TIMEOUT, cancel)
            .await
            .unwrap();
    (reservation, peer)
}

async fn stop_peer(reservation: &RendererReservation, peer: JoinHandle<Vec<Value>>) -> Vec<Value> {
    reservation
        .page()
        .disconnect_and_wait(IO_TIMEOUT)
        .await
        .unwrap();
    tokio::time::timeout(IO_TIMEOUT, peer)
        .await
        .expect("scripted peer did not disconnect")
        .unwrap()
}

fn evaluations(trace: &[Value]) -> impl Iterator<Item = Request> + '_ {
    trace
        .iter()
        .filter(|message| message["method"] == "Runtime.evaluate")
        .map(|message| Request {
            method: "Runtime.evaluate".into(),
            params: message["params"].clone(),
            session: None,
        })
}

fn script_args(trace: &[Value], script: &str) -> Vec<Value> {
    evaluations(trace)
        .filter(|request| request.expression().contains(script))
        .map(|request| request.arg())
        .collect()
}

fn script_source(expression: &str) -> &str {
    expression
        .rsplit_once(")(...")
        .map_or(expression, |(source, _)| source)
}

fn assert_no_input(trace: &[Value]) {
    assert!(
        trace.iter().all(|message| {
            !message["method"]
                .as_str()
                .is_some_and(|method| method.starts_with("Input."))
        }),
        "unexpected input dispatch: {trace:?}"
    );
}

fn prepared_prompt(expectations: Value) -> PreparedPrompt {
    let prompt = "PUBLIC-staged prompt";
    PreparedPrompt {
        cfg: json!({
            "prompt": prompt,
            "promptSha256": crate::artifacts::sha256(prompt.as_bytes()),
            "guardId": "PUBLIC-prompt-guard",
            "witnessId": "PUBLIC-witness",
            "dispatchGuardId": "PUBLIC-dispatch-guard",
            "navigation": navigation(PAGE_URL).unwrap(),
            "expectations": expectations,
            "sendSelectors": SEND_SELECTORS,
            "admit": false
        }),
    }
}

#[tokio::test]
async fn attachment_menu_checks_navigation_before_declining_to_press_plus() {
    for work_selected in [false, true] {
        let cancel = CancellationToken::new();
        let (mut reservation, peer) = owned_peer(&cancel, move |request| {
            (request.arg()["plusGuardId"] == "PUBLIC-plus-guard").then(|| {
                Reply::value(json!({
                    "status": "context-changed",
                    "navigation": {
                        "workSelected": work_selected,
                        "contextMatches": !work_selected,
                        "modeUnverified": false
                    }
                }))
            })
        })
        .await;
        let nav = navigation(PAGE_URL).unwrap();
        let cfg = json!({"plusGuardId":"PUBLIC-plus-guard","navigation":nav});
        let result = open_attachment_menu(&mut reservation, &cfg, IO_TIMEOUT, &cancel).await;
        if work_selected {
            assert_eq!(
                result.unwrap_err().to_string(),
                "ChatGPT navigated to Work or another ChatGPT context during attachment preparation; upload/send was stopped before prompt submission."
            );
        } else {
            assert!(
                !result.unwrap(),
                "a non-focused probe must not open the menu"
            );
        }

        let trace = stop_peer(&reservation, peer).await;
        let probes: Vec<_> = evaluations(&trace)
            .map(|request| request.arg())
            .filter(|arg| arg["plusGuardId"] == "PUBLIC-plus-guard")
            .collect();
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0]["navigation"], nav);
        assert_no_input(&trace);
    }
}

#[tokio::test(start_paused = true)]
async fn research_checks_the_staged_prompt_and_propagates_activation_failure() {
    for check_ready in [false, true] {
        let cancel = CancellationToken::new();
        let mut checked = false;
        let mut prerequisites = std::collections::HashSet::new();
        let (mut reservation, peer) = owned_peer(&cancel, move |request| {
            let expression = request.expression();
            let source = script_source(expression);
            if expression.contains(CHECK) {
                checked = true;
                Some(Reply::value(json!({
                    "ready": check_ready,
                    "complete": check_ready
                })))
            } else if !checked {
                // Allow ownership checks before the prompt check and on subsequent mutations.
                prerequisites.insert(source.to_owned());
                None
            } else if prerequisites.contains(source) {
                None
            } else {
                Some(Reply::Error(
                    "PUBLIC-research renderer refused activation".into(),
                ))
            }
        })
        .await;

        let prompt = prepared_prompt(json!([]));
        let error = prompt
            .activate_research(&mut reservation, Duration::from_millis(150), &cancel)
            .await
            .unwrap_err();
        let trace = stop_peer(&reservation, peer).await;
        let checks = script_args(&trace, CHECK);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0]["guardId"], prompt.cfg["guardId"]);

        let mut scripts = evaluations(&trace);
        let before_check: std::collections::HashSet<_> = scripts
            .by_ref()
            .take_while(|request| !request.expression().contains(CHECK))
            .map(|request| script_source(request.expression()).to_owned())
            .collect();
        let attempted_research =
            scripts.any(|request| !before_check.contains(script_source(request.expression())));
        assert_eq!(attempted_research, check_ready);
        if check_ready {
            assert!(
                format!("{error:#}")
                    .contains("CDP command Runtime.evaluate was refused by the endpoint."),
                "{error:#}"
            );
        } else {
            assert_eq!(
                error.to_string(),
                "Complete prompt is unavailable in its original owned editor"
            );
        }
        assert_no_input(&trace);
    }
}

#[tokio::test(start_paused = true)]
async fn an_absent_send_control_is_polled_only_for_attachments() {
    for (attachments, status, probes, expected) in [
        (
            true,
            "absent",
            2,
            "Attachments never reached the exact ready send button after 1s; tune --browser-attachment-timeout.",
        ),
        (
            false,
            "absent",
            1,
            "Exact send control is not ready; refusing dispatch",
        ),
        (
            true,
            "context-changed",
            1,
            "Exact send control is not ready; refusing dispatch",
        ),
    ] {
        let cancel = CancellationToken::new();
        let (mut reservation, peer) = owned_peer(&cancel, move |request| {
            let expression = request.expression();
            if expression.contains(CHECK) {
                Some(Reply::value(json!({"ready":true,"complete":true})))
            } else if expression.contains(SEND_FOCUS) {
                Some(Reply::value(json!({"status":status})))
            } else {
                None
            }
        })
        .await;
        let prompt = prepared_prompt(if attachments {
            json!([attachment_expectation("PUBLIC-attachment.txt")])
        } else {
            json!([])
        });
        let error = prompt
            .arm_send(&mut reservation, Duration::from_millis(50), &cancel)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert_eq!(
            reservation.state(),
            crate::browser_renderer::DispatchState::NotStarted
        );
        let trace = stop_peer(&reservation, peer).await;
        assert_eq!(script_args(&trace, SEND_FOCUS).len(), probes);
        assert!(script_args(&trace, SEND_PREPARE).is_empty());
        assert_no_input(&trace);
    }
}

#[tokio::test]
async fn data_transfer_reports_navigation_and_assignment_failures() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join(TRANSFER_NAME);
    fs::write(&source, TRANSFER_BYTES).unwrap();
    let mut file = UploadFile::open(&source).unwrap();

    for (assignment, expected) in [
        (
            json!({
                "success": false,
                "navigationBlocked": {"workSelected":true,"contextMatches":false},
                "error": "PUBLIC-less-specific transfer error"
            }),
            "ChatGPT navigated to Work or another ChatGPT context during attachment preparation; upload/send was stopped before prompt submission.",
        ),
        (
            json!({"success":false,"error":"PUBLIC-FileList assignment refused"}),
            "Failed to transfer file to browser: PUBLIC-FileList assignment refused",
        ),
        (
            json!({"success":true,"fileName":TRANSFER_NAME,"size":0}),
            "Failed to transfer file to browser: Unknown error",
        ),
    ] {
        let cancel = CancellationToken::new();
        let (mut reservation, peer) = owned_peer(&cancel, move |request| {
            let expression = request.expression();
            if expression.contains(DATA_CHUNK) {
                Some(Reply::value(json!(1)))
            } else if expression.contains(DATA_ASSIGN) {
                Some(Reply::value(assignment.clone()))
            } else {
                None
            }
        })
        .await;
        let cfg = json!({"id":"PUBLIC-transfer-receipt"});
        let error = transfer_in_page(&mut reservation, &mut file, &cfg, IO_TIMEOUT, &cancel)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected);

        let trace = stop_peer(&reservation, peer).await;
        let chunks = script_args(&trace, DATA_CHUNK);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["id"], cfg["id"]);
        assert_eq!(chunks[0]["index"], 0);
        assert_eq!(
            STANDARD
                .decode(chunks[0]["chunk"].as_str().unwrap())
                .unwrap(),
            TRANSFER_BYTES
        );
        let assigns = script_args(&trace, DATA_ASSIGN);
        assert_eq!(assigns.len(), 1);
        assert_eq!(assigns[0]["id"], cfg["id"]);
        assert_eq!(assigns[0]["chunks"], 1);
        assert_eq!(assigns[0]["name"], TRANSFER_NAME);
        assert_eq!(assigns[0]["fileBytes"], TRANSFER_BYTES.len());
        assert_eq!(assigns[0]["mimeType"], "text/plain");
    }
}

#[tokio::test]
async fn successful_upload_with_a_cleanup_error_does_not_return_a_receipt() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join(TRANSFER_NAME);
    fs::write(&source, TRANSFER_BYTES).unwrap();
    let mut file = UploadFile::open(&source).unwrap();
    file.data_transfer = true;

    let cancel = CancellationToken::new();
    let (mut reservation, peer) = owned_peer(&cancel, |request| {
        let expression = request.expression();
        if expression.contains(FILE_PREPARE) {
            Some(Reply::value(json!({"ready":true})))
        } else if expression.contains(DATA_CHUNK) {
            Some(Reply::value(json!(1)))
        } else if expression.contains(DATA_ASSIGN) {
            Some(Reply::value(json!({
                "success": true,
                "fileName": TRANSFER_NAME,
                "size": TRANSFER_BYTES.len()
            })))
        } else if expression.contains(FILE_CLEAN) {
            Some(Reply::Error("PUBLIC-file input cleanup refused".into()))
        } else {
            None
        }
    })
    .await;
    let nav = navigation(PAGE_URL).unwrap();
    let owner = json!(reservation.owner());
    let error = upload(&mut reservation, &mut file, &nav, IO_TIMEOUT, &cancel)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "Upload or exact file-input cleanup is uncertain; recovery is retained: CDP command Runtime.evaluate was refused by the endpoint."
    );

    let trace = stop_peer(&reservation, peer).await;
    let sealed = script_args(&trace, FILE_SEAL);
    let cleaned = script_args(&trace, FILE_CLEAN);
    assert_eq!(sealed.len(), 1);
    assert_eq!(cleaned.len(), 1);
    assert!(sealed[0]["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(cleaned[0]["id"], sealed[0]["id"]);
    assert_eq!(cleaned[0]["navigation"], nav);
    assert_eq!(cleaned[0]["owner"], owner);
    assert_eq!(cleaned[0]["abandon"], false);
    assert_no_input(&trace);
}

#[tokio::test(start_paused = true)]
async fn upload_reset_confirms_the_same_receipt_or_reports_the_removal_deadline() {
    for completes in [true, false] {
        let cancel = CancellationToken::new();
        let (mut reservation, peer) = owned_peer(&cancel, move |request| {
            let expression = request.expression();
            if expression.contains(NAVIGATION) {
                Some(Reply::value(json!({
                    "currentUrl": PAGE_URL,
                    "workSelected": false,
                    "modeUnverified": false
                })))
            } else if expression.contains(RESET_UPLOADS) {
                let complete = completes && request.arg()["action"] == "confirm";
                Some(Reply::value(json!({
                    "complete": complete,
                    "removed": usize::from(complete),
                    "originalInputsCleared": complete
                })))
            } else {
                None
            }
        })
        .await;
        let nav = navigation(PAGE_URL).unwrap();
        let uploads = [json!({
            "name": "PUBLIC-reset.txt",
            "rendererReceiptId": "PUBLIC-reset-receipt"
        })];
        let budget = if completes {
            IO_TIMEOUT
        } else {
            Duration::from_millis(50)
        };
        let result = reset_uploads(&mut reservation, &uploads, &nav, budget, &cancel).await;
        if completes {
            result.unwrap();
        } else {
            assert_eq!(
                result.unwrap_err().to_string(),
                "Owned attachment removal did not complete before fallback; nothing was sent"
            );
        }

        let trace = stop_peer(&reservation, peer).await;
        let resets = script_args(&trace, RESET_UPLOADS);
        assert_eq!(resets.len(), 2);
        assert_eq!(resets[0]["action"], "begin");
        assert_eq!(resets[1]["action"], "confirm");
        assert!(resets[0]["id"].as_str().is_some_and(|id| !id.is_empty()));
        assert_eq!(resets[0]["id"], resets[1]["id"]);
        for args in resets {
            assert_eq!(args["uploads"], json!(uploads));
            assert_eq!(args["navigation"], nav);
        }
        assert_eq!(
            script_args(&trace, NAVIGATION).len(),
            if completes { 2 } else { 1 }
        );
        assert_no_input(&trace);
    }
}
