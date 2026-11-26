#![cfg(target_os = "linux")]

use super::*;

#[tokio::test]
async fn delivery_rejects_nul_bytes_in_notification_arguments() {
    for (title, body, sound) in [
        ("PUBLIC-title\0", "PUBLIC-body", false),
        ("PUBLIC-title", "PUBLIC-body\0", true),
    ] {
        // Rust rejects arguments containing NUL before it can launch notify-send.
        let error = deliver(title, body, sound)
            .await
            .expect_err("a NUL byte makes the notifier arguments invalid");
        let io_error = error
            .downcast_ref::<std::io::Error>()
            .expect("delivery preserves the argument validation error");

        assert_eq!(io_error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            io_error.to_string().contains("nul byte"),
            "unexpected delivery error for {title:?}/{body:?}: {error:#}"
        );
    }
}

#[tokio::test]
async fn completion_logs_delivery_failure_when_no_test_guard_is_set() {
    const CHILD_ARG: &str = "--skip=PUBLIC-notify-child-probe";

    if std::env::args_os().any(|arg| arg == CHILD_ARG) {
        assert!(!disabled_here(), "an empty environment must allow delivery");

        // Both the session name and the preview contain NUL, so the notifier
        // command fails argument validation without launching notify-send.
        let metadata = json!({
            "id": "PUBLIC-session\0",
            "notifications": {"enabled": true, "sound": false}
        });
        completed(&metadata, "PUBLIC-answer\0").await;
        return;
    }

    // Cargo sets the notification guard in this process. Re-run only this
    // test in a child with no inherited environment; leave this process alone.
    let directory = tempfile::tempdir().expect("create isolated working directory");
    let (_, module) = module_path!()
        .split_once("::")
        .expect("the test module is below the crate root");
    let test_name = format!("{module}::completion_logs_delivery_failure_when_no_test_guard_is_set");
    let mut child =
        tokio::process::Command::new(std::env::current_exe().expect("locate the test executable"));
    child.env_clear().current_dir(directory.path()).args([
        "--exact",
        test_name.as_str(),
        "--nocapture",
        CHILD_ARG,
    ]);

    let output = tokio::time::timeout(Duration::from_secs(3), child.output())
        .await
        .expect("the isolated test finishes within three seconds")
        .expect("run the isolated test");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "isolated test failed: {}\n{stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        stderr.contains("(notify skipped: nul byte found in provided data)"),
        "completion did not report the argument validation failure: {stderr}"
    );
}
