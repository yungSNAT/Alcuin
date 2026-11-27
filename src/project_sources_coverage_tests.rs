use super::*;
use crate::cdp::{
    Cdp,
    fake::{self, Reply},
};

const COMMAND: Duration = Duration::from_secs(1);
const PROJECT_PATH: &str = "/g/g-p-public/project";
const PROJECT_URL: &str = "https://chatgpt.com/g/g-p-public/project?tab=sources";
const FILE_NAME: &str = "PUBLIC-note.txt";

#[test]
fn non_object_browser_config_uses_the_trimmed_manual_profile_from_environment() {
    let home = tempfile::tempdir().unwrap();
    let profile = home.path().join("PUBLIC-profile");
    let env = Environment::from([(
        "ORACLE_BROWSER_PROFILE_DIR".into(),
        format!("  {}  ", profile.display()),
    )]);

    let config = browser_config(
        &json!("PUBLIC-not-an-object"),
        &json!({"browserKeepBrowser": true}),
        &env,
    )
    .unwrap();

    assert_eq!(
        config,
        json!({
            "manualLogin": true,
            "manualLoginProfileDir": profile,
            "cookieSync": false,
            "keepBrowser": true,
            "desiredModel": null,
            "modelStrategy": "ignore",
            "researchMode": "off"
        })
    );
}

#[test]
fn source_list_errors_report_the_browser_message_or_the_default() {
    for (value, expected) in [
        (
            json!({"ok": false, "error": "PUBLIC-sources-unavailable", "sources": []}),
            "PUBLIC-sources-unavailable",
        ),
        (
            json!({"ok": false, "error": 17, "sources": []}),
            "Unable to read ChatGPT project sources.",
        ),
        (
            json!({"sources": []}),
            "Unable to read ChatGPT project sources.",
        ),
    ] {
        assert_eq!(decode_sources(&value).unwrap_err().to_string(), expected);
    }
}

#[test]
fn an_increase_in_other_source_names_does_not_count_as_the_requested_upload() {
    assert!(!uploaded_batch(
        &rows(1),
        &rows(2),
        &["PUBLIC-missing.txt".into()]
    ));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Ready,
    Tab,
    Add,
    Mouse,
    Prepare,
    Document,
    Query,
    Resolve,
    Guard,
    Release,
    SetFiles,
    Seal,
    List,
    Confirm,
    Clear,
}

impl Step {
    fn identify(method: &str, expression: &str) -> Option<Self> {
        Some(match method {
            "Runtime.evaluate" => {
                if !expression.contains("/* alcuin-dom:project-sources */") {
                    // The reservation's own acquire, assert-active and release checks.
                    return None;
                }
                [
                    (READY, Self::Ready),
                    (TAB, Self::Tab),
                    (ADD, Self::Add),
                    (INPUT, Self::Prepare),
                    (LIST, Self::List),
                    (CONFIRM, Self::Confirm),
                    ("Array.from(upload.input.files", Self::Seal),
                    ("delete window.__oracleProjectSourcesUpload", Self::Clear),
                ]
                .into_iter()
                .find_map(|(body, step)| expression.contains(body).then_some(step))
                .expect("unexpected Project Sources expression")
            }
            "Input.dispatchMouseEvent" => Self::Mouse,
            "DOM.getDocument" => Self::Document,
            "DOM.querySelector" => Self::Query,
            "DOM.resolveNode" => Self::Resolve,
            "Runtime.callFunctionOn" => Self::Guard,
            "Runtime.releaseObject" => Self::Release,
            "DOM.setFileInputFiles" => Self::SetFiles,
            other => panic!("unexpected CDP method: {other}"),
        })
    }
}

struct ScriptedSurface {
    page: Cdp,
    reservation: RendererReservation,
    trace: tokio::task::JoinHandle<Vec<Value>>,
    required: Vec<Step>,
    repeated: Option<Step>,
}

impl ScriptedSurface {
    async fn new(
        script: Vec<(Step, Value)>,
        repeat: Option<(Step, Value)>,
        cancel: &CancellationToken,
    ) -> Self {
        let required = script.iter().map(|(step, _)| *step).collect();
        let repeated = repeat.as_ref().map(|(step, _)| *step);
        let mut script = script.into_iter();
        let (page, trace) = fake::peer(COMMAND, move |request| {
            let Some(step) = Step::identify(&request.method, request.expression()) else {
                return Reply::value(json!(true));
            };
            let (expected, response) = script
                .next()
                .or_else(|| repeat.clone())
                .unwrap_or_else(|| panic!("unexpected step after end of script: {step:?}"));
            assert_eq!(step, expected);
            if request.method == "Runtime.evaluate" {
                Reply::value(response)
            } else {
                Reply::Result(response)
            }
        })
        .await;
        let reservation =
            RendererReservation::acquire(page.clone(), "PUBLIC-claim".into(), COMMAND, cancel)
                .await
                .unwrap();
        Self {
            page,
            reservation,
            trace,
            required,
            repeated,
        }
    }

    fn surface(&mut self, command: Duration) -> Surface<'_> {
        Surface {
            reservation: &mut self.reservation,
            path: PROJECT_PATH.into(),
            command,
        }
    }

    async fn finish(self) -> Vec<(Step, Value)> {
        self.page.disconnect_and_wait(COMMAND).await.unwrap();
        let trace: Vec<_> = self
            .trace
            .await
            .unwrap()
            .into_iter()
            .filter_map(|command| {
                Step::identify(
                    command["method"].as_str().unwrap(),
                    command["params"]["expression"].as_str().unwrap_or_default(),
                )
                .map(|step| (step, command))
            })
            .collect();
        let actual: Vec<_> = trace.iter().map(|(step, _)| *step).collect();
        assert!(
            actual.starts_with(&self.required),
            "expected {:?}, observed {actual:?}",
            self.required
        );
        assert!(
            actual[self.required.len()..]
                .iter()
                .all(|step| Some(*step) == self.repeated),
            "unexpected trailing steps: {actual:?}"
        );
        trace
    }
}

fn parameters(trace: &[(Step, Value)], wanted: Step) -> Vec<Value> {
    trace
        .iter()
        .filter(|(step, _)| *step == wanted)
        .map(|(_, command)| command["params"].clone())
        .collect()
}

fn arguments(trace: &[(Step, Value)], wanted: Step) -> Vec<Value> {
    parameters(trace, wanted)
        .into_iter()
        .map(|params| {
            fake::Request {
                method: "Runtime.evaluate".into(),
                params,
                session: None,
            }
            .arg()
        })
        .collect()
}

fn rows(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| json!({"name": FILE_NAME, "index": index, "status": "unknown"}))
        .collect()
}

fn list_result(count: usize) -> Value {
    json!({"ok": true, "sources": rows(count)})
}

fn upload_fixture(directory: &Path, count: usize) -> (Vec<std::path::PathBuf>, Vec<UploadFile>) {
    (0..count)
        .map(|index| {
            let folder = directory.join(format!("PUBLIC-batch-file-{index}"));
            std::fs::create_dir(&folder).unwrap();
            let path = folder.join(FILE_NAME);
            std::fs::write(&path, b"PUBLIC-upload-content").unwrap();
            let path = path.canonicalize().unwrap();
            let file = UploadFile::open(&path).unwrap();
            (path, file)
        })
        .unzip()
}

fn input_steps(already_open: bool) -> Vec<(Step, Value)> {
    let mut script = if already_open {
        vec![(Step::Add, json!({"alreadyOpen": true}))]
    } else {
        vec![
            (Step::Add, json!({"ok": true, "x": 12, "y": 34})),
            (Step::Mouse, json!({})),
            (Step::Mouse, json!({})),
            (Step::Mouse, json!({})),
        ]
    };
    script.extend([
        (Step::Prepare, json!({"ok": true})),
        (Step::Document, json!({"root": {"nodeId": 7}})),
        (Step::Query, json!({"nodeId": 11})),
        (
            Step::Resolve,
            json!({"object": {"objectId": "PUBLIC-input"}}),
        ),
        (Step::Guard, json!({"result": {"value": true}})),
        (Step::Release, json!({})),
        (Step::SetFiles, json!({})),
        (Step::Seal, json!(true)),
    ]);
    script
}

// The browser polling intervals elapse on Tokio's paused clock, not in real time.
#[tokio::test(start_paused = true)]
async fn sources_readiness_retries_a_missing_tab_and_reports_when_the_deadline_expires() {
    let cancel = CancellationToken::new();
    for becomes_ready in [true, false] {
        let script = vec![
            (Step::Ready, json!({"ready": false})),
            (Step::Tab, json!({"ok": false})),
            (Step::Ready, json!({"ready": becomes_ready})),
        ];
        let mut fixture = ScriptedSurface::new(script, None, &cancel).await;
        let command = if becomes_ready {
            COMMAND
        } else {
            Duration::from_millis(100)
        };

        let result = fixture.surface(command).ready(&cancel).await;
        if becomes_ready {
            result.unwrap();
        } else {
            assert_eq!(
                result.unwrap_err().to_string(),
                "Project Sources tab did not become ready before timeout."
            );
        }
        assert_eq!(
            arguments(&fixture.finish().await, Step::Tab)[0]["click"],
            false
        );
    }
}

#[tokio::test(start_paused = true)]
async fn upload_batches_count_new_occurrences_and_confirm_only_when_needed() {
    let home = tempfile::tempdir().unwrap();
    let (paths, mut files) = upload_fixture(home.path(), 11);
    let cancel = CancellationToken::new();
    let mut script = input_steps(false);
    script.insert(4, (Step::Prepare, json!({"ok": false})));
    script.extend([
        (Step::List, list_result(10)),
        (Step::Confirm, json!({"ok": true})),
        (Step::List, list_result(10)),
        (Step::List, list_result(11)),
        (Step::Clear, json!(true)),
    ]);
    script.extend(input_steps(true));
    script.extend([
        (Step::List, list_result(12)),
        (Step::List, list_result(12)),
        (Step::Clear, json!(true)),
    ]);
    let mut fixture = ScriptedSurface::new(script, None, &cancel).await;
    let owner = json!(fixture.reservation.owner());

    let result = fixture
        .surface(COMMAND)
        .upload(&mut files, &rows(1), COMMAND, &cancel)
        .await
        .unwrap();
    assert_eq!(result, rows(12));
    fixture.reservation.release_clean(COMMAND).await.unwrap();
    let trace = fixture.finish().await;

    assert_eq!(
        parameters(&trace, Step::Mouse),
        [
            json!({"type": "mouseMoved", "x": 12.0, "y": 34.0}),
            json!({"type": "mousePressed", "x": 12.0, "y": 34.0, "button": "left", "clickCount": 1}),
            json!({"type": "mouseReleased", "x": 12.0, "y": 34.0, "button": "left", "clickCount": 1}),
        ]
    );
    assert_eq!(
        parameters(&trace, Step::Document),
        vec![json!({"depth": 5}); 2]
    );
    assert_eq!(
        parameters(&trace, Step::Query),
        vec![json!({"nodeId": 7, "selector": "input[data-oracle-project-sources-input=\"1\"]"}); 2]
    );
    assert_eq!(
        parameters(&trace, Step::Resolve),
        vec![json!({"nodeId": 11}); 2]
    );
    assert_eq!(
        parameters(&trace, Step::Release),
        vec![json!({"objectId": "PUBLIC-input"}); 2]
    );
    assert_eq!(
        parameters(&trace, Step::SetFiles),
        [
            json!({"nodeId": 11, "files": paths[..10]}),
            json!({"nodeId": 11, "files": paths[10..]}),
        ]
    );

    let preparations = arguments(&trace, Step::Prepare);
    let seals = arguments(&trace, Step::Seal);
    let confirmations = arguments(&trace, Step::Confirm);
    let clears = arguments(&trace, Step::Clear);
    let guards = parameters(&trace, Step::Guard);
    let first_token = seals[0]["token"].as_str().unwrap();
    let second_token = seals[1]["token"].as_str().unwrap();
    assert!(!first_token.is_empty() && !second_token.is_empty());
    assert_ne!(first_token, second_token);
    assert_eq!(preparations[0]["token"], first_token);
    assert_eq!(preparations[1]["token"], first_token);
    assert_eq!(preparations[2]["token"], second_token);
    assert_eq!(confirmations[0]["token"], first_token);
    assert_eq!(confirmations[0]["names"], json!(vec![FILE_NAME; 10]));
    assert!(confirmations[0]["click"] == true);
    for (batch, (token, size)) in [(first_token, 10), (second_token, 1)]
        .into_iter()
        .enumerate()
    {
        assert_eq!(seals[batch]["names"], json!(vec![FILE_NAME; size]));
        assert_eq!(clears[batch]["token"], token);
        assert_eq!(guards[batch]["objectId"], "PUBLIC-input");
        assert!(guards[batch]["returnByValue"] == true);
        assert_eq!(
            guards[batch]["arguments"],
            json!([{"value": {"owner": owner, "projectPath": PROJECT_PATH, "token": token}}])
        );
    }
    for step in [
        Step::Add,
        Step::Prepare,
        Step::Seal,
        Step::List,
        Step::Confirm,
        Step::Clear,
    ] {
        for arg in arguments(&trace, step) {
            assert_eq!(arg["owner"], owner);
            assert_eq!(arg["projectPath"], PROJECT_PATH);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn upload_failures_stop_at_the_failed_stage_and_explain_what_went_wrong() {
    let home = tempfile::tempdir().unwrap();
    let (_, mut files) = upload_fixture(home.path(), 1);
    let cancel = CancellationToken::new();

    for (failed, response, expected) in [
        (
            Step::Add,
            json!({"ok": true, "x": 12}),
            "Unable to open the Project Sources Add dialog.",
        ),
        (
            Step::Prepare,
            json!({"ok": false}),
            "Project Sources upload input did not appear.",
        ),
        (
            Step::Query,
            json!({"nodeId": 0}),
            "Unable to locate the Project Sources upload input.",
        ),
        (
            Step::Resolve,
            json!({"object": {}}),
            "Project Sources input identity missing",
        ),
        (
            Step::Guard,
            json!({"result": {"value": false}}),
            "Project Sources upload input ownership changed",
        ),
        (
            Step::Seal,
            json!(false),
            "Project Sources upload selection changed",
        ),
        (
            Step::List,
            list_result(1),
            "Timed out waiting for uploaded project sources: PUBLIC-note.txt",
        ),
    ] {
        let mut script = input_steps(true);
        let mut repeat = None;
        if failed == Step::List {
            script.extend([
                (Step::List, response.clone()),
                (Step::Confirm, json!({"ok": true})),
            ]);
            repeat = Some((Step::List, response));
        } else {
            let index = script.iter().position(|(step, _)| *step == failed).unwrap();
            script.truncate(index);
            if failed == Step::Prepare {
                script.push((failed, response.clone()));
            }
            script.push((failed, response));
            if failed == Step::Guard {
                // The object must be released even when the ownership check rejects it.
                script.push((Step::Release, json!({})));
            }
        }
        let mut fixture = ScriptedSurface::new(script, repeat, &cancel).await;
        let error = fixture
            .surface(Duration::from_millis(100))
            .upload(&mut files, &rows(1), COMMAND, &cancel)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected, "{failed:?}");

        let trace = fixture.finish().await;
        assert!(parameters(&trace, Step::Clear).is_empty());
        if failed == Step::List {
            assert!(parameters(&trace, Step::List).len() > 2);
        }
    }
}

#[tokio::test]
async fn successful_quarantine_uses_the_original_renderer_and_preserves_the_lease() {
    use crate::{
        browser_chrome::AttachmentEndpoint, browser_profile::ProfileRegistry,
        browser_target::OwnedTarget,
    };

    let home = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::open(home.path()).unwrap();
    let cancel = CancellationToken::new();
    let server = fake::browser(json!([]), |request| match request.method.as_str() {
        "Target.attachToTarget" => Reply::Result(json!({"sessionId": "PUBLIC-page"})),
        "Target.getTargetInfo" => {
            Reply::Result(json!({"targetInfo": {"targetId": "PUBLIC-target", "type": "page"}}))
        }
        "Runtime.evaluate" => Reply::value(json!(true)),
        other => panic!("unexpected quarantine method: {other}"),
    })
    .await;
    let endpoint = AttachmentEndpoint {
        host: "127.0.0.1".into(),
        port: server.port,
        browser_ws_endpoint: server.browser_ws(),
    };
    let browser = Cdp::connect(&server.browser_ws(), COMMAND, &cancel)
        .await
        .unwrap();
    let lease = Arc::new(
        registry
            .acquire("PUBLIC-lease", COMMAND, &cancel)
            .await
            .unwrap(),
    );
    lease
        .update(&json!({
            "chromeHost": "127.0.0.1",
            "chromePort": server.port,
            "chromeTargetId": "PUBLIC-target"
        }))
        .unwrap();
    let target = OwnedTarget::recover(
        browser.clone(),
        lease,
        "PUBLIC-target".into(),
        COMMAND,
        &cancel,
    )
    .await
    .unwrap();
    let mut reservation = RendererReservation::acquire(
        target.page().clone(),
        "PUBLIC-claim".into(),
        COMMAND,
        &cancel,
    )
    .await
    .unwrap();

    let before = registry.records().unwrap();
    let prior_commands = server.methods().len();
    quarantine_failed_upload(&mut reservation, &target, &endpoint, PROJECT_URL)
        .await
        .unwrap();

    assert_eq!(registry.records().unwrap(), before);
    let methods = server.methods();
    let quarantine_commands = &methods[prior_commands..];
    assert!(!quarantine_commands.is_empty());
    assert!(
        quarantine_commands
            .iter()
            .all(|method| method == "Runtime.evaluate"),
        "{quarantine_commands:?}"
    );
    assert!(!reservation.page().is_closed());
    browser.disconnect_and_wait(COMMAND).await.unwrap();
}
