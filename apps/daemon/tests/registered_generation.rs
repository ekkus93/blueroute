//! Compile coverage for generation-scoped cancellation of authenticated sessions.

use blueroute_daemon::RegisteredControlSession;

fn cancel<'a>(session: &mut RegisteredControlSession<'a>, generation: [u8; 16]) -> bool {
    session.cancel_if_current(generation)
}

#[test]
fn generation_scoped_cancel_is_exposed_to_daemon_runtime() {
    let _callable = cancel;
}
