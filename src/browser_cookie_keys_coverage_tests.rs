#![cfg(target_os = "linux")]

use super::linux_password;
use std::pin::pin;
use std::time::{Duration, Instant};
use tokio::task::{consume_budget, coop::has_budget_remaining};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cancellation_after_first_poll_stops_lookup_before_keyring_access() {
    let task = tokio::spawn(async {
        for _ in 0..1024 {
            if !has_budget_remaining() {
                break;
            }
            consume_budget().await;
        }

        // Verify that select! yields before polling its branches when the
        // cooperative budget is exhausted. Probe with no keyring access.
        let mut probe = pin!(async {
            tokio::select! {
                biased;
                value = std::future::ready(true) => value,
            }
        });
        assert!(
            futures_util::poll!(probe.as_mut()).is_pending(),
            "select must yield before polling the lookup branch"
        );

        let cancel = CancellationToken::new();
        let mut lookup = pin!(linux_password(
            Instant::now() + Duration::from_secs(2),
            &cancel,
        ));

        // The first poll passes the entry checks but cannot poll the work.
        assert!(futures_util::poll!(lookup.as_mut()).is_pending());

        // On the next poll, the biased select observes cancellation first.
        cancel.cancel();
        lookup.await
    });

    let error = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("cancelled lookup should finish promptly")
        .expect("lookup task should not panic")
        .expect_err("cancelled lookup must not return a password");
    assert_eq!(error.to_string(), "Chrome Safe Storage lookup cancelled");
}
