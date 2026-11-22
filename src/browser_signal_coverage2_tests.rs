#![cfg(unix)]

use super::*;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

/// When the Tokio signal driver no longer exists (its runtime has shut down), installing the
/// SIGINT listener fails. `ctrl_c()` must then resolve to `false` straight away. It must not
/// panic or wait forever. Callers use that `false` to mean "no listener was installed".
#[test]
fn ctrl_c_listener_reports_false_when_the_signal_driver_is_gone() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("PUBLIC-test runtime must build");
    let handle = runtime.handle().clone();
    // Dropping the runtime drops its signal driver. The cloned handle stays usable as a
    // context, but its signal driver is gone.
    drop(runtime);
    let _guard = handle.enter();

    let mut interrupt = pin!(ctrl_c());
    let mut context = Context::from_waker(Waker::noop());
    assert_eq!(
        interrupt.as_mut().poll(&mut context),
        Poll::Ready(false),
        "a listener that could not be installed must resolve to false immediately"
    );
}
