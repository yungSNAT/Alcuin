use super::*;
use std::cell::Cell;

/// Restores the global `HANDLING` flag even if an assertion fails, so no other
/// test in this process observes a signal "already being handled".
struct ResetHandling;

impl Drop for ResetHandling {
    fn drop(&mut self) {
        HANDLING.store(false, Ordering::SeqCst);
    }
}

#[test]
fn dropping_a_registration_releases_its_flag_and_leaves_no_hook_behind() {
    let id = NEXT.fetch_add(1, Ordering::SeqCst);
    let in_flight = Arc::new(AtomicBool::new(true));
    let registration = Registration {
        id,
        in_flight: in_flight.clone(),
    };
    assert_eq!(Arc::strong_count(&in_flight), 2);

    // complete() marks the answer as captured through the shared flag.
    registration.complete();
    assert!(!in_flight.load(Ordering::SeqCst));

    // Dropping removes any hook with this id and releases the shared flag.
    drop(registration);
    assert_eq!(Arc::strong_count(&in_flight), 1);
    let hooks = HOOKS.lock().unwrap_or_else(PoisonError::into_inner);
    assert!(
        hooks.iter().all(|(hook_id, _)| *hook_id != id),
        "a dropped registration must not leave its hook registered"
    );
}

#[test]
fn a_signal_arriving_during_cleanup_is_absorbed_without_running_hooks() {
    let _reset = ResetHandling;
    HANDLING.store(true, Ordering::SeqCst);

    let ran_before = Cell::new(false);
    for (signal, code) in [
        ("SIGINT", INTERRUPT_EXIT_CODE),
        ("SIGTERM", 1),
        ("SIGQUIT", 1),
    ] {
        // While the first signal's cleanup runs, later signals report "handled"
        // and return immediately instead of exiting or re-running hooks.
        assert!(
            run_hooks(signal, code, || ran_before.set(true)),
            "{signal} must be absorbed while another signal is handled"
        );
    }
    assert!(
        !ran_before.get(),
        "the pre-hook callback must not run for a later signal"
    );
    assert!(
        HANDLING.load(Ordering::SeqCst),
        "a later signal must not clear the in-progress handling flag"
    );
}
