//! Joined-client restart reconciliation (P6-009 S4, design D3): observe, then decide.
//!
//! One pass of [`reconcile_client_once`] looks at what the system actually has and does the
//! smallest thing that brings a persisted `Client` back, never mutating blindly:
//!
//! | Observation | Action |
//! | --- | --- |
//! | PANU up, owned lease present | Adopt both, start the supervisor. |
//! | PANU up, owned lease missing | Re-apply the deterministic lease if the segment is free, else `Degraded`; then the supervisor. |
//! | PANU down | Re-establish (hint, then locator), then lease, then supervisor. |
//! | Owned profile present, PANU gone | Keep it, report `Reconnecting`; never removed here. |
//! | Anything not carrying this network's ownership | Ignored. |
//!
//! Failures are split in two. A **retryable** transport failure becomes `Reconnecting`; anything
//! that needs a person (an address conflict, an ambiguous host, a failed authentication) becomes
//! `Degraded` and is never retried blindly. Retrying with bounded backoff is the caller's job
//! (S5); this pass only reports. Nothing here removes state: removal belongs to leave.
//!
//! The persisted host comes from the member stores under the single-host rule, never from a
//! Bluetooth address, name, IP or advertisement.

use blueroute_core::{CoreError, ErrorKind, IpPrefix, NetworkId, NetworkMembership, NodeId};
use blueroute_linux::{BackendFuture, InterfaceAddress, PanAttachment, PanRole, PeerHandle};
use blueroute_protocol::peer_pin::{PeerKeyPins, SpkiSha256};

use crate::{JoinIpLease, JoinPanLink};

/// What the system currently shows for this client. Read-only.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ObservedClient {
    /// The live PANU attachment to the recorded host device, if any.
    pub panu: Option<PanAttachment>,
    /// Addresses carrying this network's ownership (unfiltered by interface).
    pub owned_addresses: Vec<InterfaceAddress>,
    /// An owned NetworkManager profile exists for this network.
    pub owned_profile: bool,
}

/// The system effects the reconciler uses. Fakeable so every row runs without hardware.
pub trait ClientRuntime: Send + Sync {
    fn observe(
        &self,
        network: NetworkId,
        hint: Option<PeerHandle>,
    ) -> BackendFuture<'_, ObservedClient>;

    /// Whether the deterministic client segment is free of foreign networks.
    fn lease_segment_free(&self, network: NetworkId) -> BackendFuture<'_, bool>;

    /// Reconnect PANU to the host: the hint first, then the untrusted locator.
    fn reestablish_panu(
        &self,
        network: NetworkId,
        hint: Option<PeerHandle>,
    ) -> BackendFuture<'_, JoinPanLink>;

    /// Apply the deterministic owned lease on the link's interface (idempotent).
    fn apply_lease(&self, network: NetworkId, link: &JoinPanLink)
    -> BackendFuture<'_, JoinIpLease>;

    /// Start the supervised control session, dialing the pinned host.
    fn start_supervisor(
        &self,
        network: NetworkId,
        host: ClientHost,
        link: JoinPanLink,
        lease: JoinIpLease,
    ) -> BackendFuture<'_, ()>;
}

/// The single approved host this client is joined to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientHost {
    pub node: NodeId,
    pub fingerprint: SpkiSha256,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ClientReconcileOutcome {
    /// The supervisor was started; health becomes healthy when it reports `established`.
    Started,
    /// A retryable failure; durable membership is untouched and a later pass may succeed.
    Reconnecting(CoreError),
    /// Needs a person: fail closed, take no further action.
    Degraded(CoreError),
}

/// Single-host rule: exactly one node that membership trusts and the pin store has a key for.
pub fn single_approved_host(
    membership: &NetworkMembership,
    pins: &PeerKeyPins,
) -> Result<ClientHost, CoreError> {
    let network = membership.network_id;
    let mut hosts = pins
        .entries()
        .filter(|(pinned_network, node, _)| {
            *pinned_network == network && membership.is_peer_trusted(node)
        })
        .map(|(_, node, fingerprint)| ClientHost { node, fingerprint });
    match (hosts.next(), hosts.next()) {
        (Some(host), None) => Ok(host),
        (None, _) => Err(CoreError::new(
            ErrorKind::AuthenticationFailed,
            "no approved host key is recorded for this joined network",
        )),
        (Some(_), Some(_)) => Err(CoreError::new(
            ErrorKind::InvalidState,
            "several approved hosts are recorded for this joined network; refusing to guess",
        )),
    }
}

/// Classify a failure: transport trouble is retried, everything else needs a person.
fn classify(error: CoreError) -> ClientReconcileOutcome {
    if error.kind().is_retryable_transport_failure() {
        ClientReconcileOutcome::Reconnecting(error)
    } else {
        ClientReconcileOutcome::Degraded(error)
    }
}

/// One observe-then-act pass for a persisted `Client`. `expected_lease` is the deterministic
/// prefix for this network; an owned address with any other prefix is a mismatch, not adopted.
pub async fn reconcile_client_once(
    runtime: &dyn ClientRuntime,
    network: NetworkId,
    host: ClientHost,
    hint: Option<PeerHandle>,
    expected_lease: IpPrefix,
) -> ClientReconcileOutcome {
    let observed = match runtime.observe(network, hint.clone()).await {
        Ok(observed) => observed,
        Err(error) => return classify(error),
    };

    // Only a PANU attachment in the PANU role counts as the client link.
    let live = observed
        .panu
        .filter(|attachment| attachment.role == PanRole::Panu && attachment.peer.is_some());

    let link = match live {
        Some(attachment) => {
            let peer = attachment.peer.clone().expect("filtered to a present peer");
            match JoinPanLink::new(peer, attachment) {
                Ok(link) => link,
                Err(error) => return ClientReconcileOutcome::Degraded(error),
            }
        }
        // PANU down: re-establish. An owned profile that is still present is kept and reused.
        None => match runtime.reestablish_panu(network, hint).await {
            Ok(link) => link,
            Err(error) => return classify(error),
        },
    };

    // An owned lease on this link's interface is adopted; one elsewhere is not this link's.
    let on_link: Vec<&InterfaceAddress> = observed
        .owned_addresses
        .iter()
        .filter(|address| address.owner == network && &address.interface == link.interface())
        .collect();
    let lease = match on_link.as_slice() {
        [] => {
            match runtime.lease_segment_free(network).await {
                Ok(true) => {}
                Ok(false) => {
                    return ClientReconcileOutcome::Degraded(CoreError::new(
                        ErrorKind::AddressConflict,
                        "the client address segment is in use by another network; not re-applying the lease",
                    ));
                }
                Err(error) => return classify(error),
            }
            match runtime.apply_lease(network, &link).await {
                Ok(lease) => lease,
                Err(error) => return classify(error),
            }
        }
        [address] if address.prefix == expected_lease => JoinIpLease::new((*address).clone()),
        _ => {
            return ClientReconcileOutcome::Degraded(CoreError::new(
                ErrorKind::InvalidState,
                "the owned client address does not match this network's deterministic lease",
            ));
        }
    };

    match runtime.start_supervisor(network, host, link, lease).await {
        Ok(()) => ClientReconcileOutcome::Started,
        Err(error) => classify(error),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use blueroute_core::DisplayName;

    use super::*;
    use blueroute_linux::NetworkInterfaceHandle;

    fn net() -> NetworkId {
        NetworkId::from_bytes([0x51; 16])
    }

    fn node() -> NodeId {
        NodeId::from_bytes([0x52; 16])
    }

    fn key() -> SpkiSha256 {
        SpkiSha256::from_verified_certificate_digest([0x53; 32]).unwrap()
    }

    fn host() -> ClientHost {
        ClientHost {
            node: node(),
            fingerprint: key(),
        }
    }

    fn peer() -> PeerHandle {
        PeerHandle::new("/org/bluez/hci0/dev_AA_00_00_00_00_01").unwrap()
    }

    fn iface(name: &str) -> NetworkInterfaceHandle {
        NetworkInterfaceHandle::new(name).unwrap()
    }

    fn prefix(ip: &str) -> IpPrefix {
        IpPrefix::new(ip.parse().unwrap(), 24).unwrap()
    }

    fn lease_prefix() -> IpPrefix {
        prefix("192.168.77.2")
    }

    fn address(owner: NetworkId, name: &str, ip: &str) -> InterfaceAddress {
        InterfaceAddress {
            interface: iface(name),
            prefix: prefix(ip),
            owner,
        }
    }

    fn attachment(name: &str) -> PanAttachment {
        PanAttachment {
            role: PanRole::Panu,
            interface: iface(name),
            peer: Some(peer()),
        }
    }

    #[derive(Default)]
    struct Script {
        observed: ObservedClient,
        observe_error: Option<CoreError>,
        segment_free: Option<Result<bool, CoreError>>,
        reestablish: Option<Result<String, CoreError>>,
        apply: Option<Result<(), CoreError>>,
        supervisor: Option<CoreError>,
        calls: Vec<&'static str>,
    }

    struct Fake(Mutex<Script>);

    impl Fake {
        fn new(build: impl FnOnce(&mut Script)) -> Self {
            let mut script = Script {
                segment_free: Some(Ok(true)),
                apply: Some(Ok(())),
                ..Script::default()
            };
            build(&mut script);
            Self(Mutex::new(script))
        }

        fn calls(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().calls.clone()
        }
    }

    impl ClientRuntime for Fake {
        fn observe(
            &self,
            _network: NetworkId,
            _hint: Option<PeerHandle>,
        ) -> BackendFuture<'_, ObservedClient> {
            Box::pin(async move {
                let mut script = self.0.lock().unwrap();
                script.calls.push("observe");
                match script.observe_error.clone() {
                    Some(error) => Err(error),
                    None => Ok(script.observed.clone()),
                }
            })
        }

        fn lease_segment_free(&self, _network: NetworkId) -> BackendFuture<'_, bool> {
            Box::pin(async move {
                let mut script = self.0.lock().unwrap();
                script.calls.push("segment");
                script.segment_free.clone().unwrap()
            })
        }

        fn reestablish_panu(
            &self,
            _network: NetworkId,
            _hint: Option<PeerHandle>,
        ) -> BackendFuture<'_, JoinPanLink> {
            Box::pin(async move {
                let mut script = self.0.lock().unwrap();
                script.calls.push("reestablish");
                script
                    .reestablish
                    .clone()
                    .expect("reestablish was not expected")
                    .map(|name| JoinPanLink::new(peer(), attachment(&name)).unwrap())
            })
        }

        fn apply_lease(
            &self,
            network: NetworkId,
            link: &JoinPanLink,
        ) -> BackendFuture<'_, JoinIpLease> {
            let interface = link.interface().as_str().to_owned();
            Box::pin(async move {
                let mut script = self.0.lock().unwrap();
                script.calls.push("apply");
                script
                    .apply
                    .clone()
                    .unwrap()
                    .map(|()| JoinIpLease::new(address(network, &interface, "192.168.77.2")))
            })
        }

        fn start_supervisor(
            &self,
            _network: NetworkId,
            _host: ClientHost,
            _link: JoinPanLink,
            _lease: JoinIpLease,
        ) -> BackendFuture<'_, ()> {
            Box::pin(async move {
                let mut script = self.0.lock().unwrap();
                script.calls.push("supervisor");
                match script.supervisor.clone() {
                    Some(error) => Err(error),
                    None => Ok(()),
                }
            })
        }
    }

    fn run(fake: &Fake) -> ClientReconcileOutcome {
        futures_lite::future::block_on(reconcile_client_once(
            fake,
            net(),
            host(),
            Some(peer()),
            lease_prefix(),
        ))
    }

    fn degraded_kind(outcome: ClientReconcileOutcome) -> ErrorKind {
        match outcome {
            ClientReconcileOutcome::Degraded(error) => error.kind(),
            other => panic!("expected Degraded, got {other:?}"),
        }
    }

    fn reconnecting_kind(outcome: ClientReconcileOutcome) -> ErrorKind {
        match outcome {
            ClientReconcileOutcome::Reconnecting(error) => error.kind(),
            other => panic!("expected Reconnecting, got {other:?}"),
        }
    }

    #[test]
    fn panu_up_and_lease_present_adopts_both_and_starts_the_supervisor() {
        let fake = Fake::new(|s| {
            s.observed.panu = Some(attachment("bnep0"));
            s.observed.owned_addresses = vec![address(net(), "bnep0", "192.168.77.2")];
        });
        assert_eq!(run(&fake), ClientReconcileOutcome::Started);
        assert_eq!(fake.calls(), ["observe", "supervisor"]);
    }

    #[test]
    fn panu_up_and_lease_missing_reapplies_it_when_the_segment_is_free() {
        let fake = Fake::new(|s| s.observed.panu = Some(attachment("bnep0")));
        assert_eq!(run(&fake), ClientReconcileOutcome::Started);
        assert_eq!(fake.calls(), ["observe", "segment", "apply", "supervisor"]);
    }

    #[test]
    fn a_conflicting_segment_stays_degraded_and_applies_nothing() {
        let fake = Fake::new(|s| {
            s.observed.panu = Some(attachment("bnep0"));
            s.segment_free = Some(Ok(false));
        });
        assert_eq!(degraded_kind(run(&fake)), ErrorKind::AddressConflict);
        assert_eq!(fake.calls(), ["observe", "segment"]);
    }

    #[test]
    fn panu_down_reestablishes_then_applies_the_lease_then_supervises() {
        let fake = Fake::new(|s| s.reestablish = Some(Ok("bnep1".into())));
        assert_eq!(run(&fake), ClientReconcileOutcome::Started);
        assert_eq!(
            fake.calls(),
            ["observe", "reestablish", "segment", "apply", "supervisor"]
        );
    }

    #[test]
    fn panu_down_with_an_owned_profile_that_cannot_reconnect_keeps_it_and_reports_reconnecting() {
        let fake = Fake::new(|s| {
            s.observed.owned_profile = true;
            s.observed.owned_addresses = vec![address(net(), "bnep0", "192.168.77.2")];
            s.reestablish = Some(Err(CoreError::new(
                ErrorKind::PanFailure,
                "no nearby device is advertising that BlueRoute network",
            )));
        });
        assert_eq!(reconnecting_kind(run(&fake)), ErrorKind::PanFailure);
        // Nothing was applied, removed or started; the state is kept for the next pass.
        assert_eq!(fake.calls(), ["observe", "reestablish"]);
    }

    #[test]
    fn an_ambiguous_host_is_degraded_not_retried() {
        let fake = Fake::new(|s| {
            s.reestablish = Some(Err(CoreError::new(
                ErrorKind::InvalidState,
                "several nearby devices advertise that BlueRoute network",
            )));
        });
        assert_eq!(degraded_kind(run(&fake)), ErrorKind::InvalidState);
    }

    #[test]
    fn a_lease_on_another_interface_or_owned_by_another_network_is_ignored() {
        let fake = Fake::new(|s| {
            s.observed.panu = Some(attachment("bnep0"));
            s.observed.owned_addresses = vec![
                address(net(), "bnep9", "192.168.77.2"),
                address(NetworkId::from_bytes([0x99; 16]), "bnep0", "192.168.77.2"),
            ];
        });
        assert_eq!(run(&fake), ClientReconcileOutcome::Started);
        // Neither foreign address counted as this link's lease, so a fresh one was applied.
        assert_eq!(fake.calls(), ["observe", "segment", "apply", "supervisor"]);
    }

    #[test]
    fn an_owned_address_with_the_wrong_prefix_is_a_mismatch_not_adopted() {
        let fake = Fake::new(|s| {
            s.observed.panu = Some(attachment("bnep0"));
            s.observed.owned_addresses = vec![address(net(), "bnep0", "192.168.50.2")];
        });
        assert_eq!(degraded_kind(run(&fake)), ErrorKind::InvalidState);
        assert_eq!(fake.calls(), ["observe"]);
    }

    #[test]
    fn a_non_panu_or_peerless_attachment_is_not_the_client_link() {
        for bad in [
            PanAttachment {
                role: PanRole::Nap,
                ..attachment("bnep0")
            },
            PanAttachment {
                peer: None,
                ..attachment("bnep0")
            },
        ] {
            let fake = Fake::new(|s| {
                s.observed.panu = Some(bad);
                s.reestablish = Some(Err(CoreError::new(ErrorKind::PanFailure, "down")));
            });
            assert_eq!(reconnecting_kind(run(&fake)), ErrorKind::PanFailure);
            assert_eq!(fake.calls(), ["observe", "reestablish"]);
        }
    }

    #[test]
    fn failures_are_classified_retryable_versus_needs_a_person() {
        let observe = Fake::new(|s| {
            s.observe_error = Some(CoreError::new(ErrorKind::BluezUnavailable, "bluez down"));
        });
        assert_eq!(
            reconnecting_kind(run(&observe)),
            ErrorKind::BluezUnavailable
        );

        let supervisor_auth = Fake::new(|s| {
            s.observed.panu = Some(attachment("bnep0"));
            s.observed.owned_addresses = vec![address(net(), "bnep0", "192.168.77.2")];
            s.supervisor = Some(CoreError::new(ErrorKind::AuthenticationFailed, "bad key"));
        });
        assert_eq!(
            degraded_kind(run(&supervisor_auth)),
            ErrorKind::AuthenticationFailed
        );

        let supervisor_io = Fake::new(|s| {
            s.observed.panu = Some(attachment("bnep0"));
            s.observed.owned_addresses = vec![address(net(), "bnep0", "192.168.77.2")];
            s.supervisor = Some(CoreError::new(ErrorKind::PanFailure, "link flapped"));
        });
        assert_eq!(
            reconnecting_kind(run(&supervisor_io)),
            ErrorKind::PanFailure
        );
    }

    #[test]
    fn a_failed_lease_apply_keeps_the_link_and_does_not_start_the_supervisor() {
        let fake = Fake::new(|s| {
            s.observed.panu = Some(attachment("bnep0"));
            s.apply = Some(Err(CoreError::new(
                ErrorKind::NetworkBackendUnavailable,
                "nm down",
            )));
        });
        assert_eq!(
            reconnecting_kind(run(&fake)),
            ErrorKind::NetworkBackendUnavailable
        );
        assert_eq!(fake.calls(), ["observe", "segment", "apply"]);
    }

    fn membership_trusting(nodes: &[NodeId]) -> NetworkMembership {
        let mut membership = NetworkMembership::new(net(), DisplayName::new("Joined").unwrap());
        for node in nodes {
            membership.trust_peer(*node);
        }
        membership
    }

    fn pins_for(entries: &[(NetworkId, NodeId)]) -> PeerKeyPins {
        let mut pins = PeerKeyPins::new();
        for (network, node) in entries {
            let digest = [node.as_bytes()[0]; 32];
            let key = SpkiSha256::from_verified_certificate_digest(digest).unwrap();
            pins.enroll_verified(*network, *node, key).unwrap();
        }
        pins
    }

    #[test]
    fn the_single_host_rule_needs_exactly_one_trusted_and_pinned_node() {
        let other = NodeId::from_bytes([0x54; 16]);
        let third = NodeId::from_bytes([0x55; 16]);
        // Exactly one trusted and pinned node.
        let host = single_approved_host(
            &membership_trusting(&[node()]),
            &pins_for(&[(net(), node())]),
        )
        .unwrap();
        assert_eq!(host.node, node());
        // Trusted but unpinned: no key to authenticate with.
        let error =
            single_approved_host(&membership_trusting(&[node()]), &PeerKeyPins::new()).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);
        // Pinned but not trusted by membership.
        let error = single_approved_host(&membership_trusting(&[]), &pins_for(&[(net(), node())]))
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);
        // A pin for another network does not count.
        let error = single_approved_host(
            &membership_trusting(&[node()]),
            &pins_for(&[(NetworkId::from_bytes([0x77; 16]), node())]),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AuthenticationFailed);
        // Two approved hosts: refuse to guess.
        let error = single_approved_host(
            &membership_trusting(&[node(), other, third]),
            &pins_for(&[(net(), node()), (net(), other)]),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
    }
}
