//! CR-F8 regressions (docs/CODE_REVIEW_REMEDIATION_TODO.md, OWN-1/OWN-2).
//!
//! A command must reach the same daemon owner whose compatible version was just checked. These
//! tests state the corrected contract and fail on baseline 4ca5042, where `request()` checks
//! `Version` and then sends `Request` to the well-known name again, so a replacement owner can
//! receive it. They need a private bus:
//!
//! `dbus-run-session -- cargo test -p blueroute-client --test owner_race --locked -- --ignored`
use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use blueroute_client::BlueRouteClient;
use blueroute_core::DisplayName;
use blueroute_protocol::{ApiVersion, Command, DBUS_OBJECT_PATH, DBUS_SERVICE_NAME};
use futures_lite::future;
use zbus::connection::Builder;
use zbus::{Connection, fdo, interface};

/// Compatible owner A. On the `handover_on`-th Version call it gives the well-known name to B
/// before replying, which is the race window between version check and command delivery.
struct OwnerA {
    version_calls: Arc<AtomicUsize>,
    handover_on: usize,
    own: Arc<OnceLock<Connection>>,
    successor: Arc<OnceLock<Connection>>,
}

#[interface(name = "org.blueroute.Service1")]
impl OwnerA {
    async fn version(&self) -> (u16, u16) {
        let call = self.version_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.handover_on {
            let own = self.own.get().expect("owner A connection is set");
            let successor = self.successor.get().expect("owner B connection is set");
            own.release_name(DBUS_SERVICE_NAME)
                .await
                .expect("A releases the name");
            successor
                .request_name(DBUS_SERVICE_NAME)
                .await
                .expect("B takes the name");
        }
        (ApiVersion::CURRENT.major, ApiVersion::CURRENT.minor)
    }

    fn request(&self, _payload: &str) -> fdo::Result<String> {
        Ok(r#"{"type":"ack"}"#.to_owned())
    }
}

/// Incompatible replacement B (API 99.0). It must never receive a normal command.
struct OwnerB {
    requests: Arc<AtomicUsize>,
}

#[interface(name = "org.blueroute.Service1")]
impl OwnerB {
    fn version(&self) -> (u16, u16) {
        (99, 0)
    }

    fn request(&self, _payload: &str) -> fdo::Result<String> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        Ok(r#"{"type":"ack"}"#.to_owned())
    }
}

#[test]
#[ignore = "CR-F8 regression: fails on baseline 4ca5042 until CR-F8 is fixed; needs dbus-run-session"]
fn cr_f8_incompatible_replacement_during_version_check_receives_no_command()
-> Result<(), Box<dyn Error>> {
    future::block_on(async {
        let version_calls = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(AtomicUsize::new(0));
        let own = Arc::new(OnceLock::new());
        let successor = Arc::new(OnceLock::new());

        // B serves the interface but does not own the name yet.
        let b = Builder::session()?
            .serve_at(
                DBUS_OBJECT_PATH,
                OwnerB {
                    requests: requests.clone(),
                },
            )?
            .build()
            .await?;
        let _ = successor.set(b.clone());
        // A owns the name. Call 1 is the client's connect-time check; call 2 is the check that
        // `request()` makes right before sending the command.
        let a = Builder::session()?
            .name(DBUS_SERVICE_NAME)?
            .serve_at(
                DBUS_OBJECT_PATH,
                OwnerA {
                    version_calls: version_calls.clone(),
                    handover_on: 2,
                    own: own.clone(),
                    successor: successor.clone(),
                },
            )?
            .build()
            .await?;
        let _ = own.set(a.clone());

        let client = BlueRouteClient::from_connection(Connection::session().await?).await?;
        let outcome = client
            .request(&Command::CreateNetwork {
                name: DisplayName::new("Owner race").unwrap(),
            })
            .await;

        assert_eq!(
            requests.load(Ordering::SeqCst),
            0,
            "the incompatible replacement received a normal command (outcome: {outcome:?})"
        );
        // OWN-2: A relinquished the name but is still connected, so finishing the command on A
        // (the owner that was validated) is allowed. Only migrating to B is forbidden.
        drop((a, b));
        Ok(())
    })
}
