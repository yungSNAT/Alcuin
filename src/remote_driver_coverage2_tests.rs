use super::*;
use crate::sessions::SessionStore;
use std::{fs, time::Duration};

#[tokio::test]
async fn driver_reports_missing_worker_home_without_capturing_a_result() {
    const RUN_ID: &str = "34567890-1234-1234-1234-123456789abc";

    let home = tempfile::tempdir().unwrap();
    let blocked_parent = home.path().join("PUBLIC-browser-path-blocker");
    fs::write(&blocked_parent, b"PUBLIC-regular-file").unwrap();

    // The empty worker environment has no user home. Keep the configured
    // executable and profile unusable as well so the fixture cannot launch Chrome.
    let driver = BrowserDriver {
        environment: Environment::new(),
        cwd: home.path().to_path_buf(),
        browser_config: json!({
            "chromePath": blocked_parent.join("PUBLIC-chrome"),
            "manualLogin": true,
            "manualLoginProfileDir": blocked_parent.join("PUBLIC-profile"),
            "attachRunning": false,
            "cookieSync": false,
        }),
    };
    let context = RunContext::fixture(
        RUN_ID,
        json!({"prompt": "PUBLIC-prompt for a browser startup failure"}),
        SessionStore::new(home.path().join(".oracle")),
    );

    let outcome = tokio::time::timeout(Duration::from_secs(3), driver.run(context.clone()))
        .await
        .expect("the worker should reject the missing home without waiting for a browser");
    let Err(error) = outcome else {
        panic!("the driver produced a result without a worker home");
    };
    let id = remote_result::session_id(RUN_ID).unwrap();
    assert_eq!(
        format!("{error:#}"),
        format!(
            "Provider attempt {id}-attempt-1 failed before sending its prompt; nothing was sent: Native browser worker has no user home"
        )
    );

    let metadata = context.store.require(&id).unwrap();
    let status = metadata["status"].as_str().expect("session status");
    assert_ne!(
        status, "pending",
        "the producer should have taken ownership"
    );
    let delivery = &metadata["browser"]["remoteDelivery"];
    assert_eq!(delivery["runId"], RUN_ID);
    assert_eq!(delivery["acknowledged"].as_bool(), Some(false));
    assert_eq!(delivery["producerFinished"].as_bool(), Some(false));
    assert!(remote_result::read(&context.store, RUN_ID).is_err());
    assert_eq!(fs::read(blocked_parent).unwrap(), b"PUBLIC-regular-file");
}
