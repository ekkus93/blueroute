//! Generation-scoped cancellation API regression.

use blueroute_daemon::RegisteredControlSession;

#[test]
fn cancellation_requires_an_exact_exporter_generation() {
    let _cancel: for<'a> fn(&mut RegisteredControlSession<'a>, [u8; 16]) -> bool =
        RegisteredControlSession::cancel_if_current;
}
