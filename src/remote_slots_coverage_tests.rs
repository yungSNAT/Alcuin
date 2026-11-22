use super::*;

#[tokio::test]
async fn release_skips_cancelled_and_closed_waiters_before_granting_the_next_slot() {
    let slots = RunSlots::new(1, 3).unwrap();
    let live = CancellationToken::new();
    let cancelled_token = CancellationToken::new();

    let active = slots.reserve(&live).unwrap().wait().await.unwrap();
    let cancelled = slots.reserve(&cancelled_token).unwrap();
    let mut closed = slots.reserve(&live).unwrap();
    let next = slots.reserve(&live).unwrap();

    cancelled_token.cancel();
    closed.receiver.as_mut().unwrap().close();

    // Neither stale waiter has been pruned by reserve or snapshot.
    assert!(cancelled.waiting());
    assert!(closed.waiting());
    assert!(next.waiting());

    active.release();

    assert!(!cancelled.waiting());
    assert!(!closed.waiting());
    assert!(!next.waiting());

    let Err(error) = cancelled.wait().await else {
        panic!("a cancelled waiter received a slot");
    };
    assert_eq!(error.to_string(), "cancelled while waiting for a slot");

    let Err(error) = closed.wait().await else {
        panic!("a closed receiver received a slot");
    };
    assert_eq!(
        error.to_string(),
        "Run admission owner ended before a slot was available"
    );

    let next = next.wait().await.unwrap();
    let snapshot = slots.snapshot();
    assert_eq!((snapshot.active, snapshot.queued), (1, 0));
    next.release();
    assert_eq!(slots.snapshot().active, 0);
}
