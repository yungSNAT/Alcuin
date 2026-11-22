use super::*;
use std::{
    sync::TryLockError,
    thread,
    time::{Duration, Instant},
};

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

#[tokio::test]
async fn disconnecting_during_handoff_preserves_admission_for_the_next_waiter() {
    for delay_micros in [50, 100, 200, 400] {
        let slots = RunSlots::new(1, 2).unwrap();
        let live = CancellationToken::new();
        let active = slots.reserve(&live).unwrap().wait().await.unwrap();

        // Let the queue own the only handle to this parent token. Detaching
        // its children when the waiter is popped widens the interval between
        // release checking the receiver and attempting the send.
        let disconnecting = CancellationToken::new();
        let children: Vec<_> = (0..65_536).map(|_| disconnecting.child_token()).collect();
        let (sender, receiver) = oneshot::channel();
        slots.inner.state.lock().unwrap().waiting.push_back(Waiter {
            id: Uuid::new_v4(),
            sender,
            cancel: disconnecting,
        });
        let next = slots.reserve(&live).unwrap();
        assert_eq!(slots.snapshot().queued, 2);

        thread::scope(|scope| {
            let releaser = scope.spawn(move || active.release());
            let deadline = Instant::now() + Duration::from_millis(100);
            while !releaser.is_finished() && Instant::now() < deadline {
                match slots.inner.state.try_lock() {
                    Ok(guard) => {
                        drop(guard);
                        thread::yield_now();
                    }
                    Err(TryLockError::WouldBlock) => {
                        thread::sleep(Duration::from_micros(delay_micros));
                        break;
                    }
                    Err(TryLockError::Poisoned(error)) => {
                        panic!("run admission mutex was poisoned: {error}");
                    }
                }
            }

            // If the send already succeeded, dropping the receiver returns
            // the delivered slot. If the send loses the race, release must
            // retry the handoff without returning capacity twice.
            drop(receiver);
            releaser.join().unwrap();
        });
        drop(children);

        assert!(!next.waiting(), "the next waiter was not granted admission");
        let next = next.wait().await.unwrap();
        let snapshot = slots.snapshot();
        assert_eq!((snapshot.active, snapshot.queued), (1, 0));

        next.release();
        let snapshot = slots.snapshot();
        assert_eq!((snapshot.active, snapshot.queued), (0, 0));
    }
}
