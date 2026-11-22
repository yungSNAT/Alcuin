//! Further coverage for browser_runner: profile lock waits that end by cancellation.
use super::*;

fn expect_lock_error(result: Result<DirectoryLock>, context: &str) -> anyhow::Error {
    match result {
        Ok(_) => panic!("{context}: lock was unexpectedly acquired"),
        Err(error) => error,
    }
}

#[tokio::test]
async fn profile_lock_reports_cancellation_plainly_and_timeouts_with_holder_context() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oracle-automation.lock");
    let held = DirectoryLock::acquire(&path, Duration::from_secs(1)).unwrap();

    // A cancelled wait on a held lock returns the lock's own error without the
    // timeout wording, because the wait did not run out its budget.
    let cancel = CancellationToken::new();
    cancel.cancel();
    let refused = expect_lock_error(
        tokio::time::timeout(
            Duration::from_secs(4),
            profile_lock(&path, Duration::from_secs(2), &cancel),
        )
        .await
        .expect("cancelled lock wait is bounded"),
        "cancelled wait",
    );
    assert!(
        !format!("{refused:#}").contains("Alcuin profile lock still held"),
        "{refused:#}"
    );

    // An uncancelled wait that exhausts its budget names the holder and the wait.
    let live = CancellationToken::new();
    let expired = expect_lock_error(
        tokio::time::timeout(
            Duration::from_secs(4),
            profile_lock(&path, Duration::from_millis(300), &live),
        )
        .await
        .expect("expiring lock wait is bounded"),
        "expiring wait",
    );
    assert!(
        format!("{expired:#}").contains("Alcuin profile lock still held by"),
        "{expired:#}"
    );

    // Once released, the same path is acquired without error.
    held.release().unwrap();
    let acquired = tokio::time::timeout(
        Duration::from_secs(4),
        profile_lock(&path, Duration::from_secs(2), &live),
    )
    .await
    .expect("free lock is acquired promptly")
    .unwrap();
    acquired.release().unwrap();
}
