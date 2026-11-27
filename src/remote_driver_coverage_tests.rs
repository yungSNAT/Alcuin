use super::*;
use crate::sessions::SessionStore;

const RUN_ID: &str = "23456789-1234-1234-1234-123456789abc";
const PROMPT: &str = "PUBLIC-original prompt for remote preparation";

fn fixture(payload: Value) -> (tempfile::TempDir, BrowserDriver, RunContext) {
    let home = tempfile::tempdir().unwrap();
    let driver = BrowserDriver {
        environment: Environment::new(),
        cwd: home.path().to_path_buf(),
        browser_config: json!({}),
    };
    let context = RunContext::fixture(
        RUN_ID,
        payload,
        SessionStore::new(home.path().join(".oracle")),
    );
    (home, driver, context)
}

#[tokio::test]
async fn driver_reports_missing_prompt_and_preparation_cancellation() {
    let (_home, driver, context) = fixture(json!({}));
    let run = driver.driver();

    for (cancelled, expected) in [
        (false, "Missing remote prompt"),
        (true, "Browser run cancelled before preparation"),
    ] {
        if cancelled {
            context.cancel.cancel();
        }

        let Err(error) = run(context.clone()).await else {
            panic!("driver unexpectedly accepted the request (cancelled: {cancelled})");
        };
        assert_eq!(error.to_string(), expected);
        assert!(!context.store.root().exists());
    }
}

#[test]
fn prepare_propagates_invalid_follow_up_error_before_creating_session() {
    let follow_ups = json!([false]);
    let Err(expected) = crate::browser_turns::prompts(&json!({"browserFollowUps": follow_ups}))
    else {
        panic!("the follow-up parser accepted a boolean prompt");
    };
    let (_home, driver, context) = fixture(json!({
        "prompt": PROMPT,
        "options": {"followUpPrompts": follow_ups},
    }));

    let error = driver.prepare(&context).unwrap_err();
    assert_eq!(error.to_string(), expected.to_string());
    assert!(!context.store.root().exists());
}

#[test]
fn malformed_primary_or_fallback_attachment_prevents_request_commit() {
    let attachments = [json!({
        "fileName": "PUBLIC-source.txt",
        "displayPath": "PUBLIC-source.txt",
        "contentBase64": false,
    })];
    let (_reference_home, reference_driver, reference_context) = fixture(json!({"prompt": PROMPT}));
    let id = reference_driver.prepare(&reference_context).unwrap();
    let Err(expected) = StagedUploads::capture_remote(&reference_context.store, &id, &attachments)
    else {
        panic!("attachment staging accepted a non-string contentBase64 value");
    };

    for (location, payload) in [
        (
            "primary",
            json!({"prompt": PROMPT, "attachments": attachments}),
        ),
        (
            "fallback",
            json!({
                "prompt": PROMPT,
                "fallbackSubmission": {
                    "prompt": "PUBLIC-fallback composer prompt",
                    "attachments": attachments,
                },
            }),
        ),
    ] {
        let (_home, driver, context) = fixture(payload);
        let error = driver.prepare(&context).unwrap_err();
        assert_eq!(error.to_string(), expected.to_string(), "{location}");

        let metadata = context.store.require(&id).unwrap();
        assert_eq!(metadata["status"], "pending", "{location}");
        assert!(metadata["browser"]["remoteDelivery"]["runId"].is_null());
        let directory = context.store.directory(&id).unwrap();
        assert!(!directory.join("browser-request.json").exists());
    }
}

#[test]
fn prepare_commits_browser_request_and_marks_delivery_as_pending() {
    let (_home, driver, context) = fixture(json!({
        "prompt": PROMPT,
        "options": {"followUpPrompts": ["PUBLIC-follow-up asking for more detail"]},
    }));

    let id = driver.prepare(&context).unwrap();
    assert_eq!(id, remote_result::session_id(RUN_ID).unwrap());

    let path = context
        .store
        .directory(&id)
        .unwrap()
        .join("browser-request.json");
    let request: BrowserRequest = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(request.version, 1);
    assert_eq!(request.session_id, id);
    assert_eq!(request.prompt, PROMPT);
    assert_eq!(request.prompt_sha256, artifacts::sha256(PROMPT.as_bytes()));
    assert!(request.upload_paths.is_empty());
    assert!(!request.fallback_used);
    assert_eq!(request.extra.get("attachmentMode"), Some(&json!("remote")));

    let metadata = context.store.require(&id).unwrap();
    assert_eq!(metadata["status"], "pending");
    assert_eq!(
        metadata["browser"]["remoteDelivery"],
        json!({
            "runId": RUN_ID,
            "acknowledged": false,
            "producerFinished": false,
        })
    );
}
