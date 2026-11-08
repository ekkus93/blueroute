//! Untrusted peer locator for P6-004 join (design D2).
//!
//! A BlueRoute advertisement names a network, not a trustworthy host: anyone can broadcast it.
//! The locator therefore only answers "which one BlueZ device should we try to connect to?" and
//! the connection is authenticated afterwards (pinned mutual TLS against the operator-confirmed
//! host). Zero or several candidates fail closed; the locator never picks one silently.

use blueroute_core::{CoreError, ErrorKind, NetworkId};
use blueroute_linux::{BluetoothBackend, BluezBackend, PeerHandle};

use crate::JoinNetworkFuture;

pub trait PeerLocator: Send + Sync {
    /// The single BlueZ device currently advertising `network`.
    fn locate(&self, network: NetworkId) -> JoinNetworkFuture<'_, PeerHandle>;
}

/// Require exactly one distinct candidate. Duplicates (the same device seen twice) collapse.
pub fn select_single_peer(mut peers: Vec<PeerHandle>) -> Result<PeerHandle, CoreError> {
    peers.sort();
    peers.dedup();
    match peers.len() {
        0 => Err(CoreError::new(
            ErrorKind::PanFailure,
            "no nearby device is advertising that BlueRoute network",
        )),
        1 => Ok(peers.remove(0)),
        _ => Err(CoreError::new(
            ErrorKind::InvalidState,
            "several nearby devices advertise that BlueRoute network; refusing to guess which is the host",
        )),
    }
}

/// Looks at BlueZ's current view across powered adapters. Discovery must already be running (or
/// the device cached) for a peer to be visible; this never starts scanning itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct BluezPeerLocator;

impl PeerLocator for BluezPeerLocator {
    fn locate(&self, network: NetworkId) -> JoinNetworkFuture<'_, PeerHandle> {
        Box::pin(async move {
            let bluez = BluezBackend::connect_system().await?;
            let mut candidates = Vec::new();
            let mut any_powered = false;
            for adapter in bluez.adapters().await? {
                if !adapter.powered {
                    continue;
                }
                any_powered = true;
                candidates.extend(
                    bluez
                        .discovered_network_peers(adapter.handle, network)
                        .await?,
                );
            }
            if !any_powered {
                return Err(CoreError::new(
                    ErrorKind::AdapterDisabled,
                    "no powered Bluetooth adapter is available to find the network host",
                ));
            }
            select_single_peer(candidates)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(path: &str) -> PeerHandle {
        PeerHandle::new(path).unwrap()
    }

    #[test]
    fn exactly_one_distinct_candidate_is_selected() {
        let only = peer("/org/bluez/hci0/dev_AA_00_00_00_00_01");
        assert_eq!(select_single_peer(vec![only.clone()]).unwrap(), only);
        // The same device reported twice is still one device.
        assert_eq!(
            select_single_peer(vec![only.clone(), only.clone()]).unwrap(),
            only
        );
    }

    #[test]
    fn none_is_retryable_and_several_are_refused() {
        let none = select_single_peer(Vec::new()).unwrap_err();
        assert_eq!(none.kind(), ErrorKind::PanFailure);
        assert!(none.kind().is_retryable_transport_failure());
        let several = select_single_peer(vec![
            peer("/org/bluez/hci0/dev_AA_00_00_00_00_01"),
            peer("/org/bluez/hci0/dev_AA_00_00_00_00_02"),
        ])
        .unwrap_err();
        assert_eq!(several.kind(), ErrorKind::InvalidState);
        assert!(!several.kind().is_retryable_transport_failure());
    }
}
