//! Coverage for allocation failure bookkeeping, late cleanup once the caller has gone away, and
//! the observation branch of `close_observed`.

use super::*;
use crate::browser_profile::ProfileRegistry;
use crate::cdp::fake::{Reply, peer};
use std::sync::atomic::{AtomicBool, Ordering};

#[tokio::test]
async fn a_refused_create_marks_allocation_unconfirmed_and_refuses_replay() {
    let home = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::open(home.path()).unwrap();
    let lease = Arc::new(
        registry
            .acquire(
                "PUBLIC-refused-create",
                Duration::from_secs(5),
                &CancellationToken::new(),
            )
            .await
            .unwrap(),
    );
    let (cdp, _peer) = peer(Duration::from_secs(2), |request| {
        match request.method.as_str() {
            "Target.createTarget" => Reply::Error("PUBLIC-refused".into()),
            _ => Reply::ok(),
        }
    })
    .await;
    let acquisition = TargetAcquisition::default();
    assert_eq!(acquisition.state().await, AllocationState::Idle);
    let cancel = CancellationToken::new();
    let error = match acquisition
        .acquire(cdp.clone(), lease.clone(), &cancel, Duration::from_secs(2))
        .await
    {
        Ok(_) => panic!("a refused allocation must not yield a target"),
        Err(error) => error,
    };
    assert!(
        format!("{error:#}").contains("Target.createTarget"),
        "{error:#}"
    );
    // The worker records that the allocation outcome is unknown, with no target identity.
    assert_eq!(
        acquisition.state().await,
        AllocationState::Unconfirmed(None)
    );
    let replay = match acquisition
        .acquire(cdp.clone(), lease.clone(), &cancel, Duration::from_secs(2))
        .await
    {
        Ok(_) => panic!("a second allocation attempt must be refused"),
        Err(error) => error,
    };
    assert!(
        replay
            .to_string()
            .contains("Target allocation was already attempted; refusing replay"),
        "{replay:#}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_allocation_after_the_caller_left_is_retired_and_marked_absent() {
    let home = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::open(home.path()).unwrap();
    let lease = Arc::new(
        registry
            .acquire(
                "PUBLIC-late-allocation",
                Duration::from_secs(5),
                &CancellationToken::new(),
            )
            .await
            .unwrap(),
    );
    let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
    let closed = Arc::new(AtomicBool::new(false));
    let seen_close = closed.clone();
    let (cdp, _peer) = peer(Duration::from_secs(3), move |request| {
        match request.method.as_str() {
            "Target.createTarget" => {
                // Hold the acknowledgement until the caller has abandoned the allocation.
                let _ =
                    tokio::task::block_in_place(|| gate_rx.recv_timeout(Duration::from_secs(3)));
                Reply::Result(json!({"targetId": "PUBLIC-late"}))
            }
            "Target.attachToTarget" => Reply::Result(json!({"sessionId": "PUBLIC-session"})),
            "Target.getTargetInfo" => Reply::Result(json!({
                "targetInfo": {"targetId": "PUBLIC-late", "type": "page", "url": "about:blank"}
            })),
            "Target.getTargets" => {
                let mut rows = vec![json!({"targetId": "PUBLIC-user", "type": "page"})];
                if !seen_close.load(Ordering::SeqCst) {
                    rows.push(json!({"targetId": "PUBLIC-late", "type": "page"}));
                }
                Reply::Result(json!({"targetInfos": rows}))
            }
            "Target.closeTarget" => {
                if request.params["targetId"] == "PUBLIC-late" {
                    seen_close.store(true, Ordering::SeqCst);
                }
                Reply::Result(json!({"success": true}))
            }
            _ => Reply::ok(),
        }
    })
    .await;
    let acquisition = TargetAcquisition::default();
    let cancel = CancellationToken::new();
    // The caller gives up (its future is dropped) without cancelling the token.
    let abandoned = tokio::time::timeout(
        Duration::from_millis(150),
        acquisition.acquire(cdp.clone(), lease.clone(), &cancel, Duration::from_secs(4)),
    )
    .await;
    assert!(abandoned.is_err(), "the allocation must still be pending");
    assert!(!cancel.is_cancelled());
    assert_eq!(acquisition.state().await, AllocationState::Pending);
    gate_tx.send(()).unwrap();
    drop(gate_tx);
    let until = std::time::Instant::now() + Duration::from_secs(4);
    loop {
        let state = acquisition.state().await;
        if state == AllocationState::Absent {
            break;
        }
        assert!(
            std::time::Instant::now() < until,
            "late target was not retired: {state:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        closed.load(Ordering::SeqCst),
        "the exact late target must have been closed"
    );
}

#[tokio::test]
async fn closing_an_observed_target_with_an_unverifiable_renderer_sends_no_close() {
    let home = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::open(home.path()).unwrap();
    let lease = Arc::new(
        registry
            .acquire(
                "PUBLIC-observed-close",
                Duration::from_secs(5),
                &CancellationToken::new(),
            )
            .await
            .unwrap(),
    );
    let url = "https://chatgpt.com/c/11111111-2222-4333-8444-555555555555";
    assert!(crate::browser::conversation_id(url).is_some());
    let refuse = Arc::new(AtomicBool::new(false));
    let refusing = refuse.clone();
    let closed = Arc::new(AtomicBool::new(false));
    let seen_close = closed.clone();
    let (cdp, _peer) = peer(Duration::from_secs(2), move |request| {
        match request.method.as_str() {
            "Target.closeTarget" => {
                seen_close.store(true, Ordering::SeqCst);
                Reply::Result(json!({"success": true}))
            }
            "Target.createTarget" => Reply::Result(json!({"targetId": "PUBLIC-observed"})),
            "Target.attachToTarget" => Reply::Result(json!({"sessionId": "PUBLIC-session"})),
            _ if refusing.load(Ordering::SeqCst) => Reply::Error("PUBLIC-refused".into()),
            _ => Reply::ok(),
        }
    })
    .await;
    let acquisition = TargetAcquisition::default();
    let mut target = acquisition
        .acquire_observation(
            cdp.clone(),
            lease.clone(),
            url,
            &CancellationToken::new(),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert_eq!(
        acquisition.state().await,
        AllocationState::Transferred("PUBLIC-observed".into())
    );
    assert_eq!(target.id(), "PUBLIC-observed");
    assert_eq!(
        target.creation().and_then(TargetCreation::observation_url),
        Some(url)
    );
    refuse.store(true, Ordering::SeqCst);
    let result = target.close_observed(Duration::from_secs(1)).await;
    assert!(result.is_err(), "{result:?}");
    assert!(!target.retired);
    let release = target.release_retired_lease().unwrap_err();
    assert!(
        release
            .to_string()
            .contains("Cannot release admission before confirmed target absence"),
        "{release:#}"
    );
    assert!(
        !closed.load(Ordering::SeqCst),
        "an unverified observation must not be closed"
    );
}
