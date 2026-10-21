//! Host-address binding boundary for the P7 TCP-over-PAN listener.
//!
//! This checks the deterministic NAP IPv4 endpoint against independently
//! observed NetworkManager-owned addresses. It does not authenticate a peer,
//! begin accepting sockets, activate JoinNetwork, or prove Bluetooth traffic.

use std::future::Future;
use std::io::ErrorKind as IoErrorKind;
use std::net::{IpAddr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use blueroute_core::{CoreError, DaemonConfig, ErrorKind, Ipv4StarAddressPlan, NetworkId};
use blueroute_linux::{InterfaceAddress, NetworkInterfaceHandle, NetworkMembershipStore};
use blueroute_protocol::session::ControlSessionRegistry;

/// Reserved daemon-to-daemon control port from P7-001.
pub const CONTROL_TCP_PORT: u16 = 46071;

/// Derive the only permitted local NAP control socket address.
///
/// The caller must use a fresh `IpNetworkBackend::addresses()` observation
/// from the active BlueRoute-owned host backend; neither peer advertisements,
/// claimed NodeIds, Bluetooth MACs nor source IPs are authorization evidence.
/// An empty or ambiguous observation is never a reason to bind to 0.0.0.0.
pub fn owned_nap_control_address(
    network: NetworkId,
    config: &DaemonConfig,
    observed: &[InterfaceAddress],
) -> Result<SocketAddrV4, CoreError> {
    if network == NetworkId::from_bytes([0; 16]) {
        return Err(CoreError::new(
            ErrorKind::InvalidInput,
            "invalid zero control network",
        ));
    }
    config.validate()?;
    let plan = Ipv4StarAddressPlan::for_network(network, config.ipv4_address_pool)?;
    let network_string = network.to_string();
    let interface = NetworkInterfaceHandle::new(format!("brb-{}", &network_string[..8]))?;
    let IpAddr::V4(host_ip) = plan.host.address else {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "NAP control host is not IPv4",
        ));
    };

    // Linux binds a TCP socket to an IP, not to the interface provenance of
    // its NetworkManager record. Refuse an IP also attributed to any other
    // interface/network instead of guessing which interface will receive it.
    if observed.iter().any(|address| {
        address.prefix.address == plan.host.address
            && (address.owner != network || address.interface != interface)
    }) {
        return Err(CoreError::new(
            ErrorKind::AddressConflict,
            "NAP control address has ambiguous interface ownership",
        ));
    }
    // A bridge attributed to another logical network is not exclusively
    // owned, even if the foreign address differs from our planned host IP.
    // Do not rely on Linux TCP bind to distinguish the network provenance.
    if observed
        .iter()
        .any(|address| address.interface == interface && address.owner != network)
    {
        return Err(CoreError::new(
            ErrorKind::AddressConflict,
            "NAP control bridge has foreign network ownership",
        ));
    }
    // The selected bridge must not report a conflicting BlueRoute-owned
    // address, even if the expected address is also present.
    if observed.iter().any(|address| {
        address.owner == network && address.interface == interface && address.prefix != plan.host
    }) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "NAP control bridge address differs from the deterministic plan",
        ));
    }
    let found = observed.iter().any(|address| {
        address.owner == network && address.interface == interface && address.prefix == plan.host
    });
    if !found {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "active BlueRoute-owned NAP host address is not observed",
        ));
    }
    Ok(SocketAddrV4::new(host_ip, CONTROL_TCP_PORT))
}

/// Bind only after the local host backend reports the exact owned address.
/// Callers must subsequently accept connections using the independently
/// enrolled, mutually authenticated P7 control-session entry points; this
/// function deliberately does not accept any sockets or authorize membership.
pub fn bind_owned_nap_control_listener(
    network: NetworkId,
    config: &DaemonConfig,
    observed: &[InterfaceAddress],
) -> Result<TcpListener, CoreError> {
    let address = owned_nap_control_address(network, config, observed)?;
    TcpListener::bind(address).map_err(|error| {
        CoreError::with_diagnostic(
            ErrorKind::PanFailure,
            "failed to bind BlueRoute-owned NAP control address",
            error.to_string(),
        )
    })
}

/// Own a nonblocking control listener and retire it when the independently
/// observed NAP network or endpoint changes. This only hands out *untrusted*
/// TCP sockets; callers MUST run pinned mutual TLS, Hello and durable approval
/// before accepting any peer claim or network membership.
///
/// The caller must obtain the current endpoint from a fresh guarded
/// NetworkOperations::observed_owned_control_endpoint() observation and the
/// current NetworkId from trusted local membership state before *each* poll.
/// A snapshot can change immediately after a successful poll: the caller must
/// revalidate live ownership before TLS/session activation, and on teardown
/// must close active authenticated sessions independently.
pub struct ObservedControlListener {
    network: NetworkId,
    endpoint: SocketAddrV4,
    listener: Option<TcpListener>,
}

impl ObservedControlListener {
    /// Immutable local network identity captured by the guarded NAP binder.
    /// This is local provenance, never a remote peer-authentication claim.
    pub(crate) const fn network(&self) -> NetworkId {
        self.network
    }

    /// Takes ownership only of an already bound, non-wildcard IPv4 listener
    /// at the exact endpoint supplied by a fresh independent NAP observation.
    /// For production, obtain the socket via the guarded owned-NAP binder.
    pub fn new(
        network: NetworkId,
        observed_endpoint: SocketAddrV4,
        listener: TcpListener,
    ) -> Result<Self, CoreError> {
        if network == NetworkId::from_bytes([0; 16]) {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "control listener requires a nonzero local network",
            ));
        }
        let local = listener.local_addr().map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PanFailure,
                "cannot inspect bound NAP control listener",
                error.to_string(),
            )
        })?;
        let SocketAddr::V4(endpoint) = local else {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener must bind an IPv4 address",
            ));
        };
        if endpoint.ip().is_unspecified() {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener may not bind a wildcard address",
            ));
        }
        if endpoint != observed_endpoint {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener address differs from observed owned endpoint",
            ));
        }
        listener.set_nonblocking(true).map_err(|error| {
            CoreError::with_diagnostic(
                ErrorKind::PanFailure,
                "cannot make NAP control listener nonblocking",
                error.to_string(),
            )
        })?;
        Ok(Self {
            network,
            endpoint,
            listener: Some(listener),
        })
    }

    /// Poll without blocking the daemon. No matching fresh ownership evidence
    /// means the socket is dropped before any queued connection is accepted.
    /// A returned stream is unauthenticated and must never imply membership.
    pub fn accept_if_current(
        &mut self,
        observed_network: Option<NetworkId>,
        observed_endpoint: Option<SocketAddrV4>,
    ) -> Result<Option<TcpStream>, CoreError> {
        if self.listener.is_none() {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener has already been retired",
            ));
        }
        if observed_network != Some(self.network) || observed_endpoint != Some(self.endpoint) {
            self.listener.take();
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener lost its observed network ownership",
            ));
        }

        let listener = self.listener.as_ref().expect("checked live listener");
        match listener.accept() {
            Ok((socket, _source)) => {
                // Never use a peer's source IP as an identity or authorization.
                socket.set_nonblocking(false).map_err(|error| {
                    CoreError::with_diagnostic(
                        ErrorKind::PanFailure,
                        "cannot prepare untrusted control TCP socket",
                        error.to_string(),
                    )
                })?;
                Ok(Some(socket))
            }
            Err(error) if error.kind() == IoErrorKind::WouldBlock => Ok(None),
            Err(error) => {
                self.listener.take();
                Err(CoreError::with_diagnostic(
                    ErrorKind::PanFailure,
                    "NAP control listener accept failed",
                    error.to_string(),
                ))
            }
        }
    }

    /// Poll using independently fresh observations on both sides of TCP
    /// acceptance. An observation error retires the listener even if there
    /// was no pending connection. A successful idle poll observes once; an
    /// accepted candidate observes twice before leaving this owner.
    ///
    /// The callback must read trusted local membership and the guarded
    /// network backend each time. Never use peer-supplied identity or reuse
    /// a cached snapshot. Returned TCP sockets remain unauthenticated.
    pub fn poll_with_fresh_observations<F>(
        &mut self,
        mut observe: F,
    ) -> Result<Option<TcpStream>, CoreError>
    where
        F: FnMut() -> Result<(Option<NetworkId>, Option<SocketAddrV4>), CoreError>,
    {
        // A retired listener must never invoke a potentially expensive or
        // side-effectful backend observation, even if the caller retries.
        if self.listener.is_none() {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener has already been retired",
            ));
        }
        let (network, endpoint) = match observe() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.listener.take();
                return Err(error);
            }
        };
        self.accept_with_revalidation(network, endpoint, observe)
    }

    /// Async equivalent of fresh observation polling for NetworkManager and
    /// durable-membership providers that must be awaited. No queued TCP
    /// candidate is released without two distinct completed observations.
    /// The first observation is also required on an idle poll; a failed
    /// observation retires the listener. An accepted candidate is dropped
    /// and the listener retired if the second observation fails or changes.
    ///
    /// This remains a snapshot gate for *untrusted* TCP. The caller must
    /// recheck ownership and enrolled pins after mutual TLS/Hello and while
    /// the authenticated session remains active. Dropping this future during
    /// the second observation drops the accepted candidate, not a session.
    pub async fn poll_with_async_observations<F, Fut>(
        &mut self,
        mut observe: F,
    ) -> Result<Option<TcpStream>, CoreError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<(Option<NetworkId>, Option<SocketAddrV4>), CoreError>>,
    {
        if self.listener.is_none() {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener has already been retired",
            ));
        }
        let (network, endpoint) = match observe().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.listener.take();
                return Err(error);
            }
        };
        let Some(socket) = self.accept_if_current(network, endpoint)? else {
            return Ok(None);
        };
        self.finish_accepted_observation(socket, observe().await)
    }

    /// Prefer this entry point when polling against the running daemon's
    /// durable membership store and network backend. Each asynchronous
    /// observation reloads membership before AND after the independently
    /// guarded backend endpoint read. An absent, replaced, corrupt or
    /// concurrently changing local network permanently retires this listener.
    ///
    /// The two observations around TCP accept remain distinct. A returned
    /// socket is still untrusted and must pass enrolled mutual TLS/Hello,
    /// registry admission, and post-handshake ownership revalidation.
    /// This does not run a daemon accept loop or keep active sessions alive.
    pub async fn poll_from_durable_network_operations(
        &mut self,
        membership: &NetworkMembershipStore,
        operations: &dyn crate::NetworkOperations,
    ) -> Result<Option<TcpStream>, CoreError> {
        let expected_network = self.network;
        self.poll_with_async_observations(|| {
            observe_durable_owned_endpoint(expected_network, membership, operations)
        })
        .await
    }

    /// Revalidate the durable local network and backend-owned NAP endpoint
    /// AFTER the real pinned TLS handshake, authenticated Hello/HelloAck and
    /// exact-generation session registry admission. This is the production-
    /// oriented composition of the post-TLS release gate with the same fresh
    /// durable observation used for the pre-accept gate.
    ///
    /// On failure the registered TLS owner is dropped (closing the socket
    /// and retiring only its generation). Ownership drift also permanently
    /// retires the listener. This is not a daemon runtime supervisor: callers
    /// must continue monitoring ownership, revocation and PAN loss.
    pub async fn release_registered_from_durable_network_operations<'a>(
        &mut self,
        session: crate::RegisteredControlSession<'a>,
        membership: &NetworkMembershipStore,
        operations: &dyn crate::NetworkOperations,
    ) -> Result<crate::RegisteredControlSession<'a>, CoreError> {
        let expected_network = self.network;
        self.release_registered_if_current_async(session, || {
            observe_durable_owned_endpoint(expected_network, membership, operations)
        })
        .await
    }

    /// Drive one guarded inbound handshake against the running daemon's
    /// durable NAP membership, enrolled pins, mutual TLS 1.3, Hello/HelloAck,
    /// and exact-generation authenticated session registry.
    ///
    /// Fresh membership and guarded endpoint observations run both before
    /// and after accepting an *untrusted* TCP candidate and once more after
    /// full TLS registration. A failed handshake never grants membership.
    /// A normal invalid peer closes only its socket; lost local ownership
    /// retires the NAP listener. No caller-provided remote identity is trusted.
    ///
    /// This is a bounded synchronous TLS handshake inside an async method:
    /// the future must be driven by a dedicated daemon worker, not the D-Bus
    /// event loop. The returned session requires ongoing ownership, PAN-loss,
    /// idle-deadline and revocation supervision. This method is not a running
    /// daemon accept loop and does not enable JoinNetwork.
    pub async fn poll_registered_from_durable_network_operations<'a>(
        &mut self,
        membership: &NetworkMembershipStore,
        operations: &dyn crate::NetworkOperations,
        local: &crate::LocalControlTlsIdentity,
        trust: &'a crate::DurablePeerTrustOperations,
        setup: crate::ApprovedControlSetup,
        registry: Arc<Mutex<ControlSessionRegistry>>,
    ) -> Result<Option<crate::RegisteredControlSession<'a>>, CoreError> {
        // Refuse programming/configuration mismatches before an unauthenticated
        // peer can be accepted or a TLS record can be exchanged.
        if setup.network != self.network {
            self.listener.take();
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "authenticated control setup targets a different NAP network",
            ));
        }
        let Some(socket) = self
            .poll_from_durable_network_operations(membership, operations)
            .await?
        else {
            return Ok(None);
        };
        let registered =
            crate::accept_registered_from_durable_approval(socket, local, trust, setup, registry)
                .map_err(|error| {
                CoreError::with_diagnostic(
                    ErrorKind::AuthenticationFailed,
                    "inbound control TLS or session admission failed",
                    error.to_string(),
                )
            })?;
        let checked = self
            .release_registered_from_durable_network_operations(registered, membership, operations)
            .await?;
        Ok(Some(checked))
    }

    /// One fail-closed supervision tick for already authenticated, registered
    /// sessions on this owned NAP. Every tick reloads persisted membership,
    /// obtains the independently guarded NAP endpoint and reloads membership
    /// again; then it rechecks each live peer's current durable pin/approval.
    ///
    /// The active session vector is moved into this future BEFORE the awaited
    /// backend observation. If the future is cancelled, its owned sessions
    /// drop immediately, closing TLS sockets and retiring exact registry
    /// generations. Revocation removes only that peer, while NAP ownership
    /// loss or backend failure retires the listener and closes every session.
    ///
    /// This is a single bounded observation, not a timer, spawned task, live
    /// daemon loop, reconnect owner or proof of Bluetooth/PAN operation.
    /// Callers must run it regularly and supervise failure/rebind policy.
    pub async fn supervise_registered_from_durable_network_operations<'a>(
        &mut self,
        sessions: &mut Vec<crate::RegisteredControlSession<'a>>,
        membership: &NetworkMembershipStore,
        operations: &dyn crate::NetworkOperations,
    ) -> Result<usize, CoreError> {
        // A pending future owns all session guards so cancellation cannot
        // leave authenticated sockets live after supervision stops.
        let mut owned = std::mem::take(sessions);
        if self.listener.is_none() {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener has already been retired",
            ));
        }
        match observe_durable_owned_endpoint(self.network, membership, operations).await {
            Ok((Some(network), Some(endpoint)))
                if network == self.network && endpoint == self.endpoint =>
            {
                // One revoked peer must not close other still-approved peers.
                // A failed recheck retires its exact generation via Drop.
                owned.retain_mut(|session| {
                    session.network() == self.network && session.revalidate_approval().is_ok()
                });
                let active = owned.len();
                *sessions = owned;
                Ok(active)
            }
            Ok(_) => {
                self.listener.take();
                Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "active NAP listener lost durable network ownership",
                ))
            }
            Err(error) => {
                self.listener.take();
                Err(error)
            }
        }
    }

    /// Accept a candidate only if ownership survives a second independent
    /// observation after TCP accept. The recheck must query fresh local
    /// membership and the guarded network backend, not reuse the first
    /// snapshot. On loss or observation failure both sockets are retired.
    ///
    /// This is only a TOCTOU defense for an untrusted TCP candidate; a caller
    /// must still revalidate ownership, pins and mutual TLS before admitting
    /// an authenticated control session or changing membership.
    pub fn accept_with_revalidation<F>(
        &mut self,
        observed_network: Option<NetworkId>,
        observed_endpoint: Option<SocketAddrV4>,
        reobserve: F,
    ) -> Result<Option<TcpStream>, CoreError>
    where
        F: FnOnce() -> Result<(Option<NetworkId>, Option<SocketAddrV4>), CoreError>,
    {
        let Some(socket) = self.accept_if_current(observed_network, observed_endpoint)? else {
            return Ok(None);
        };
        self.finish_accepted_observation(socket, reobserve())
    }

    /// Common terminal gate for synchronous and asynchronous post-accept
    /// observations. Both sockets are closed on ownership loss or error.
    fn finish_accepted_observation(
        &mut self,
        socket: TcpStream,
        observation: Result<(Option<NetworkId>, Option<SocketAddrV4>), CoreError>,
    ) -> Result<Option<TcpStream>, CoreError> {
        match observation {
            Ok((Some(network), Some(endpoint)))
                if network == self.network && endpoint == self.endpoint =>
            {
                Ok(Some(socket))
            }
            Ok(_) => {
                self.listener.take();
                let _ = socket.shutdown(std::net::Shutdown::Both);
                Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "NAP control listener lost ownership after TCP accept",
                ))
            }
            Err(error) => {
                self.listener.take();
                let _ = socket.shutdown(std::net::Shutdown::Both);
                Err(error)
            }
        }
    }

    /// Release a registered, mutually authenticated session to the daemon
    /// only if its logical network and the freshly observed NAP endpoint still
    /// match after TLS/Hello and registry admission. A changed host can never
    /// inherit an authenticated session from an earlier listener generation.
    ///
    /// Failure drops the owned TLS session, closing its socket and retiring
    /// its exact registry generation; ownership loss also retires this listener.
    /// Release also rechecks durable peer approval and exact registry ownership;
    /// a revoked session cannot escape this post-handshake boundary. Success
    /// is not a lease: the daemon must keep observing PAN ownership and
    /// cancel active sessions when the host is torn down.
    pub fn release_registered_if_current<'a, F>(
        &mut self,
        mut session: crate::RegisteredControlSession<'a>,
        observe: F,
    ) -> Result<crate::RegisteredControlSession<'a>, CoreError>
    where
        F: FnOnce() -> Result<(Option<NetworkId>, Option<SocketAddrV4>), CoreError>,
    {
        if self.listener.is_none() {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener has already been retired",
            ));
        }
        if session.network() != self.network {
            self.listener.take();
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "authenticated session network differs from NAP listener",
            ));
        }
        match observe() {
            Ok((Some(network), Some(endpoint)))
                if network == self.network && endpoint == self.endpoint =>
            {
                // Trust or registry ownership can change after TLS/Hello.
                // Drop only this revoked session; unrelated peers must still
                // be able to use the correctly owned NAP listener.
                session.revalidate_approval().map_err(|_| {
                    CoreError::new(
                        ErrorKind::AuthenticationFailed,
                        "control session lost approval before listener release",
                    )
                })?;
                Ok(session)
            }
            Ok(_) => {
                self.listener.take();
                Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "NAP ownership changed during authenticated control setup",
                ))
            }
            Err(error) => {
                self.listener.take();
                Err(error)
            }
        }
    }

    /// Await an independent NAP/membership observation after pinned mutual
    /// TLS, Hello, and registry admission, before handing an authenticated
    /// session to the daemon. This is the asynchronous counterpart of
    /// `release_registered_if_current` for real backend observers.
    ///
    /// Cancelling this future while the observation is pending drops the
    /// owned authenticated session (closing TLS and retiring its exact
    /// registry generation); it does not grant a session lease or retire
    /// unrelated listener ownership. The next poll still needs fresh proof.
    pub async fn release_registered_if_current_async<'a, F, Fut>(
        &mut self,
        session: crate::RegisteredControlSession<'a>,
        observe: F,
    ) -> Result<crate::RegisteredControlSession<'a>, CoreError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(Option<NetworkId>, Option<SocketAddrV4>), CoreError>>,
    {
        // Reject retired/wrong-network ownership before starting an expensive
        // backend observation. The session is dropped on every early return.
        if self.listener.is_none() {
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "NAP control listener has already been retired",
            ));
        }
        if session.network() != self.network {
            self.listener.take();
            return Err(CoreError::new(
                ErrorKind::InvalidState,
                "authenticated session network differs from NAP listener",
            ));
        }
        let observation = observe().await;
        self.release_registered_if_current(session, || observation)
    }
}

/// Read local durable NAP ownership on both sides of an independently
/// guarded backend endpoint observation. Called before raw TCP accept and
/// after authenticated TLS registration; neither snapshot is a lasting lease.
async fn observe_durable_owned_endpoint(
    expected_network: NetworkId,
    membership: &NetworkMembershipStore,
    operations: &dyn crate::NetworkOperations,
) -> Result<(Option<NetworkId>, Option<SocketAddrV4>), CoreError> {
    let before = crate::current_network(&membership.load()?)?;
    if before != Some(expected_network) {
        return Ok((before, None));
    }
    let endpoint = operations.observed_owned_control_endpoint().await?;
    let after = crate::current_network(&membership.load()?)?;
    if before != after {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "NAP ownership changed during durable backend observation",
        ));
    }
    Ok((after, endpoint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueroute_core::IpPrefix;
    use std::io::{Read, Write};

    fn network(value: u8) -> NetworkId {
        NetworkId::from_bytes([value; 16])
    }

    fn address(owner: NetworkId) -> InterfaceAddress {
        let config = DaemonConfig::default();
        let plan = Ipv4StarAddressPlan::for_network(owner, config.ipv4_address_pool).unwrap();
        InterfaceAddress {
            owner,
            interface: NetworkInterfaceHandle::new(format!("brb-{}", &owner.to_string()[..8]))
                .unwrap(),
            prefix: plan.host,
        }
    }

    #[test]
    fn selected_nap_host_address_is_exact_and_never_wildcard() {
        let owner = network(7);

        let owned = address(owner);
        let result = owned_nap_control_address(
            owner,
            &DaemonConfig::default(),
            std::slice::from_ref(&owned),
        )
        .unwrap();
        assert_eq!(result.port(), CONTROL_TCP_PORT);
        assert_eq!(IpAddr::V4(*result.ip()), owned.prefix.address);
        assert!(!result.ip().is_unspecified());
        // An identical duplicate backend observation is harmless.
        assert_eq!(
            owned_nap_control_address(owner, &DaemonConfig::default(), &[owned.clone(), owned])
                .unwrap(),
            result
        );
    }

    #[test]
    fn missing_foreign_and_wrong_interface_addresses_cannot_bind() {
        let owner = network(7);
        let config = DaemonConfig::default();
        assert_eq!(
            owned_nap_control_address(owner, &config, &[])
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
        let mut foreign = address(owner);
        foreign.owner = network(9);
        assert_eq!(
            owned_nap_control_address(owner, &config, &[foreign])
                .unwrap_err()
                .kind(),
            ErrorKind::AddressConflict
        );
        let mut wrong_iface = address(owner);
        wrong_iface.interface = NetworkInterfaceHandle::new("eth0").unwrap();
        assert_eq!(
            owned_nap_control_address(owner, &config, &[wrong_iface])
                .unwrap_err()
                .kind(),
            ErrorKind::AddressConflict
        );
        assert_eq!(
            owned_nap_control_address(network(0), &config, &[])
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );
    }

    #[test]
    fn conflicting_owned_bridge_address_fails_closed() {
        let owner = network(7);
        let good = address(owner);
        let mut conflicting = good.clone();
        conflicting.prefix =
            IpPrefix::new("10.201.22.1".parse().unwrap(), good.prefix.prefix_len).unwrap();
        assert_eq!(
            owned_nap_control_address(owner, &DaemonConfig::default(), &[good, conflicting])
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
    }

    #[test]
    fn foreign_network_on_owned_bridge_cannot_bind_even_with_distinct_ip() {
        let owner = network(7);
        let owned = address(owner);
        let mut foreign = address(network(9));
        assert_ne!(owned.prefix.address, foreign.prefix.address);
        foreign.interface = owned.interface.clone();
        assert_eq!(
            owned_nap_control_address(owner, &DaemonConfig::default(), &[owned, foreign])
                .unwrap_err()
                .kind(),
            ErrorKind::AddressConflict
        );
    }

    #[test]
    fn binding_does_not_try_wildcard_when_no_host_is_observed() {
        assert_eq!(
            bind_owned_nap_control_listener(network(7), &DaemonConfig::default(), &[])
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
    }

    #[test]
    fn observed_listener_accepts_only_after_matching_local_ownership_snapshot() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = match listener.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        let mut owner = ObservedControlListener::new(network(7), addr, listener).unwrap();
        assert!(
            owner
                .accept_if_current(Some(network(7)), Some(addr))
                .unwrap()
                .is_none()
        );
        let mut remote = TcpStream::connect(addr).unwrap();
        remote.write_all(b"x").unwrap();
        let mut inbound = owner
            .accept_if_current(Some(network(7)), Some(addr))
            .unwrap()
            .unwrap();
        let mut byte = [0; 1];
        inbound.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"x");
        // A socket being accepted is not authenticated peer evidence.
    }

    #[test]
    fn observed_listener_retires_on_missing_wrong_network_or_changed_endpoint() {
        for (network_observation, endpoint_change) in [
            (None, false),
            (Some(network(9)), false),
            (Some(network(7)), true),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = match listener.local_addr().unwrap() {
                SocketAddr::V4(addr) => addr,
                SocketAddr::V6(_) => panic!("test listener must be IPv4"),
            };
            let mut owner = ObservedControlListener::new(network(7), addr, listener).unwrap();
            let endpoint = if endpoint_change {
                Some(SocketAddrV4::new(*addr.ip(), addr.port().wrapping_add(1)))
            } else {
                Some(addr)
            };
            assert_eq!(
                owner
                    .accept_if_current(network_observation, endpoint)
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidState
            );
            assert_eq!(
                owner
                    .accept_if_current(Some(network(7)), Some(addr))
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidState
            );
            assert!(TcpStream::connect(addr).is_err());
        }
    }

    #[test]
    fn observed_listener_rejects_socket_bound_to_a_different_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let actual = match listener.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        let unexpected = SocketAddrV4::new(*actual.ip(), 46071);
        let observed = if unexpected == actual {
            SocketAddrV4::new(*actual.ip(), 46072)
        } else {
            unexpected
        };
        assert_eq!(
            ObservedControlListener::new(network(7), observed, listener)
                .err()
                .unwrap()
                .kind(),
            ErrorKind::InvalidState
        );
        assert!(TcpStream::connect(actual).is_err());
    }

    #[test]
    fn observed_listener_rejects_queued_socket_after_local_ownership_loss() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let actual = match listener.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        let mut owner = ObservedControlListener::new(network(7), actual, listener).unwrap();
        let _queued = TcpStream::connect(actual).unwrap();
        assert_eq!(
            owner
                .accept_if_current(None, Some(actual))
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
        assert_eq!(
            owner
                .accept_if_current(Some(network(7)), Some(actual))
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidState
        );
    }

    #[test]
    fn fresh_observation_failure_retires_idle_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = match listener.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        let mut owner = ObservedControlListener::new(network(7), addr, listener).unwrap();
        let mut observations = 0;
        let error = owner
            .poll_with_fresh_observations(|| {
                observations += 1;
                Err(CoreError::new(
                    ErrorKind::InvalidState,
                    "fresh NAP ownership observation failed",
                ))
            })
            .err()
            .unwrap();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        assert_eq!(observations, 1);
        assert!(TcpStream::connect(addr).is_err());
        let mut retried_observations = 0;

        assert_eq!(
            owner
                .poll_with_fresh_observations(|| {
                    retried_observations += 1;
                    Ok((Some(network(7)), Some(addr)))
                })
                .err()
                .unwrap()
                .kind(),
            ErrorKind::InvalidState
        );
        assert_eq!(retried_observations, 0);
    }

    #[test]
    fn fresh_observations_run_again_after_accept_but_not_for_idle_poll() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = match listener.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        let mut owner = ObservedControlListener::new(network(7), addr, listener).unwrap();
        let mut observations = 0;
        assert!(
            owner
                .poll_with_fresh_observations(|| {
                    observations += 1;
                    Ok((Some(network(7)), Some(addr)))
                })
                .unwrap()
                .is_none()
        );
        assert_eq!(observations, 1);
        let mut remote = TcpStream::connect(addr).unwrap();
        remote.write_all(b"x").unwrap();
        let mut accepted = owner
            .poll_with_fresh_observations(|| {
                observations += 1;
                Ok((Some(network(7)), Some(addr)))
            })
            .unwrap()
            .unwrap();
        assert_eq!(observations, 3);
        let mut byte = [0; 1];
        accepted.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"x");
        // Two fresh ownership observations do not authenticate the TCP peer.
    }

    #[test]
    fn second_fresh_observation_failure_closes_accepted_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = match listener.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        let mut owner = ObservedControlListener::new(network(7), addr, listener).unwrap();
        let mut remote = TcpStream::connect(addr).unwrap();
        remote
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let mut observations = 0;
        let error = owner
            .poll_with_fresh_observations(|| {
                observations += 1;
                if observations == 1 {
                    Ok((Some(network(7)), Some(addr)))
                } else {
                    Err(CoreError::new(
                        ErrorKind::InvalidState,
                        "second NAP ownership observation failed",
                    ))
                }
            })
            .err()
            .unwrap();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
        assert_eq!(observations, 2);
        assert_eq!(remote.read(&mut [0_u8; 1]).unwrap(), 0);
        assert!(TcpStream::connect(addr).is_err());
    }

    #[test]
    fn async_fresh_observation_polls_idle_once_and_accepted_candidate_twice() {
        futures_lite::future::block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = match listener.local_addr().unwrap() {
                SocketAddr::V4(addr) => addr,
                SocketAddr::V6(_) => panic!("test listener must be IPv4"),
            };
            let mut owner = ObservedControlListener::new(network(7), addr, listener).unwrap();
            let mut observations = 0;
            assert!(
                owner
                    .poll_with_async_observations(|| {
                        observations += 1;
                        async { Ok((Some(network(7)), Some(addr))) }
                    })
                    .await
                    .unwrap()
                    .is_none()
            );
            assert_eq!(observations, 1);

            let mut remote = TcpStream::connect(addr).unwrap();
            remote.write_all(b"x").unwrap();
            let mut accepted = owner
                .poll_with_async_observations(|| {
                    observations += 1;
                    async { Ok((Some(network(7)), Some(addr))) }
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(observations, 3);
            let mut byte = [0; 1];
            accepted.read_exact(&mut byte).unwrap();
            assert_eq!(&byte, b"x");
            // A successful observation is not TLS or membership proof.
        });
    }

    #[test]
    fn async_post_accept_drift_and_observation_failure_close_both_sockets() {
        futures_lite::future::block_on(async {
            for fail_observation in [false, true] {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let addr = match listener.local_addr().unwrap() {
                    SocketAddr::V4(addr) => addr,
                    SocketAddr::V6(_) => panic!("test listener must be IPv4"),
                };
                let mut owner = ObservedControlListener::new(network(7), addr, listener).unwrap();
                let mut remote = TcpStream::connect(addr).unwrap();
                remote
                    .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                    .unwrap();
                let mut observations = 0;
                let error = owner
                    .poll_with_async_observations(|| {
                        observations += 1;
                        let index = observations;
                        async move {
                            if index == 1 {
                                Ok((Some(network(7)), Some(addr)))
                            } else if fail_observation {
                                Err(CoreError::new(
                                    ErrorKind::InvalidState,
                                    "asynchronous ownership observation failed",
                                ))
                            } else {
                                Ok((Some(network(8)), Some(addr)))
                            }
                        }
                    })
                    .await
                    .err()
                    .unwrap();
                assert_eq!(error.kind(), ErrorKind::InvalidState);
                assert_eq!(observations, 2);
                assert_eq!(remote.read(&mut [0_u8; 1]).unwrap(), 0);
                assert!(TcpStream::connect(addr).is_err());
                assert_eq!(
                    owner
                        .poll_with_async_observations(|| async {
                            Ok((Some(network(7)), Some(addr)))
                        })
                        .await
                        .err()
                        .unwrap()
                        .kind(),
                    ErrorKind::InvalidState
                );
            }
        });
    }

    #[test]
    fn async_idle_observation_error_retires_listener_without_accepting() {
        futures_lite::future::block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = match listener.local_addr().unwrap() {
                SocketAddr::V4(addr) => addr,
                SocketAddr::V6(_) => panic!("test listener must be IPv4"),
            };
            let mut owner = ObservedControlListener::new(network(7), addr, listener).unwrap();
            let error = owner
                .poll_with_async_observations(|| async {
                    Err(CoreError::new(
                        ErrorKind::InvalidState,
                        "backend unavailable during async observation",
                    ))
                })
                .await
                .err()
                .unwrap();
            assert_eq!(error.kind(), ErrorKind::InvalidState);
            assert!(TcpStream::connect(addr).is_err());
        });
    }

    #[test]
    fn post_accept_revalidation_rejects_ownership_loss_and_closes_candidate() {
        for failure in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = match listener.local_addr().unwrap() {
                SocketAddr::V4(addr) => addr,
                SocketAddr::V6(_) => panic!("test listener must be IPv4"),
            };
            let mut owner = ObservedControlListener::new(network(7), addr, listener).unwrap();
            let mut remote = TcpStream::connect(addr).unwrap();
            remote
                .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                .unwrap();
            let error = owner
                .accept_with_revalidation(Some(network(7)), Some(addr), || {
                    if failure {
                        Err(CoreError::new(
                            ErrorKind::InvalidState,
                            "fresh ownership observation failed",
                        ))
                    } else {
                        Ok((None, Some(addr)))
                    }
                })
                .err()
                .unwrap();
            assert_eq!(error.kind(), ErrorKind::InvalidState);
            assert_eq!(remote.read(&mut [0_u8; 1]).unwrap(), 0);
            assert!(TcpStream::connect(addr).is_err());
            assert_eq!(
                owner
                    .accept_if_current(Some(network(7)), Some(addr))
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidState
            );
        }
    }

    #[test]
    fn post_accept_revalidation_allows_matching_candidate_without_authorizing_it() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = match listener.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        let mut owner = ObservedControlListener::new(network(7), addr, listener).unwrap();
        let mut remote = TcpStream::connect(addr).unwrap();
        remote.write_all(b"x").unwrap();
        let mut accepted = owner
            .accept_with_revalidation(Some(network(7)), Some(addr), || {
                Ok((Some(network(7)), Some(addr)))
            })
            .unwrap()
            .unwrap();
        let mut byte = [0; 1];
        accepted.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"x");
        // This raw TCP socket still has no authenticated peer identity.
    }

    #[test]
    fn observed_listener_rejects_zero_network_and_wildcard_binding() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = match listener.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        assert_eq!(
            ObservedControlListener::new(network(0), addr, listener)
                .err()
                .unwrap()
                .kind(),
            ErrorKind::InvalidInput
        );
        let wildcard = TcpListener::bind("0.0.0.0:0").unwrap();
        let wildcard_addr = match wildcard.local_addr().unwrap() {
            SocketAddr::V4(addr) => addr,
            SocketAddr::V6(_) => panic!("test listener must be IPv4"),
        };
        assert_eq!(
            ObservedControlListener::new(network(7), wildcard_addr, wildcard)
                .err()
                .unwrap()
                .kind(),
            ErrorKind::InvalidState
        );
    }

    #[test]
    fn registered_tls_session_requires_post_handshake_nap_ownership() {
        use std::fs;
        use std::path::Path;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        use std::thread;
        use std::time::{Duration, Instant};

        use crate::{
            ApprovedControlSetup, DurablePeerTrustOperations, LocalControlTlsIdentity,
            NetworkOperationFuture, NetworkOperations, PeerPinStore, PeerTrustOperations,
            PinnedControlTlsPeer, accept_registered_from_durable_approval,
            connect_approved_control_session,
        };
        use blueroute_core::{
            DisplayName, MembershipRegistry, MembershipState, NetworkMembership, NodeId,
        };
        use blueroute_linux::NetworkMembershipStore;
        use blueroute_protocol::NetworkSummary;
        use blueroute_protocol::control::HelloFacts;
        use blueroute_protocol::session::{
            ConnectionDeadline, ConnectionStage, ControlSessionRegistry, PeerSessionKey,
        };

        static NEXT: AtomicUsize = AtomicUsize::new(0);

        fn approved_trust(path: &Path, peer: PinnedControlTlsPeer) -> DurablePeerTrustOperations {
            fs::create_dir_all(path).unwrap();
            let membership_store = NetworkMembershipStore::new(path.join("membership"));
            let mut membership =
                NetworkMembership::new(network(7), DisplayName::new("Local test").unwrap());
            membership.state = MembershipState::Member;
            let mut memberships = MembershipRegistry::default();
            memberships.remember_network(membership);
            membership_store.save(&memberships).unwrap();
            let trust = DurablePeerTrustOperations::with_peer_pin_store(
                membership_store,
                PeerPinStore::new(path.join("pins")),
            );
            futures_lite::future::block_on(trust.trust_peer(peer.node())).unwrap();
            trust
                .enroll_verified_key(
                    network(7),
                    peer.node(),
                    peer.fingerprint(),
                    peer.fingerprint(),
                )
                .unwrap();
            trust
        }

        fn setup() -> ApprovedControlSetup {
            let mut active_deadline = ConnectionDeadline::new(0);
            active_deadline
                .advance(ConnectionStage::TlsHandshake, 1)
                .unwrap();
            active_deadline.advance(ConnectionStage::Hello, 2).unwrap();
            active_deadline.advance(ConnectionStage::Active, 3).unwrap();
            ApprovedControlSetup {
                network: network(7),
                facts: HelloFacts {
                    software_version: "listener-ownership-test".to_owned(),
                    capabilities: blueroute_core::NodeCapabilities::default(),
                    segment: None,
                    direct_neighbors: Vec::new(),
                },
                handshake_deadline: Instant::now() + Duration::from_secs(5),
                active_deadline,
            }
        }

        struct DurableObservationProbe {
            endpoint: SocketAddrV4,
            membership_path: std::path::PathBuf,
            scenario: u8,
            calls: AtomicUsize,
        }
        impl NetworkOperations for DurableObservationProbe {
            fn observed_owned_control_endpoint(
                &self,
            ) -> NetworkOperationFuture<'_, Option<SocketAddrV4>> {
                Box::pin(async move {
                    let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                    if self.scenario == 15 && call == 4 {
                        return std::future::pending::<Result<Option<SocketAddrV4>, CoreError>>().await;
                    }
                    match self.scenario {
                        14 if call == 3 => {
                            NetworkMembershipStore::new(self.membership_path.clone())
                                .save(&MembershipRegistry::default())
                                .unwrap()
                        }
                        9 => fs::write(&self.membership_path, b"BLUEROUTE_MEMBERSHIP_V999\n")
                            .unwrap(),
                        11 => {
                            return Err(CoreError::new(
                                ErrorKind::InvalidState,
                                "backend ownership observation failed after TLS",
                            ));
                        }
                        12 => NetworkMembershipStore::new(self.membership_path.clone())
                            .save(&MembershipRegistry::default())
                            .unwrap(),
                        _ => {}
                    }
                    if self.scenario == 10 {
                        return Ok(Some(SocketAddrV4::new(
                            *self.endpoint.ip(),
                            self.endpoint.port() ^ 1,
                        )));
                    }
                    Ok(Some(self.endpoint))
                })
            }
            fn create_network(&self, _: DisplayName) -> NetworkOperationFuture<'_, NetworkId> {
                Box::pin(async { unreachable!("observation-only probe") })
            }
            fn leave_network(&self) -> NetworkOperationFuture<'_, ()> {
                Box::pin(async { unreachable!("observation-only probe") })
            }
            fn list_networks(&self) -> NetworkOperationFuture<'_, Vec<NetworkSummary>> {
                Box::pin(async { unreachable!("observation-only probe") })
            }
            fn start_discovery(&self) -> NetworkOperationFuture<'_, ()> {
                Box::pin(async { unreachable!("observation-only probe") })
            }
            fn stop_discovery(&self) -> NetworkOperationFuture<'_, ()> {
                Box::pin(async { unreachable!("observation-only probe") })
            }
        }

        // Existing TLS listener checks and fresh DURABLE post-TLS ownership
        // cases: stable, revoked before observation, corrupted during query,
        // endpoint drift, backend failure and revoked during query.
        for scenario in 0_u8..16 {
            let client = LocalControlTlsIdentity::generate(NodeId::from_bytes([11; 16])).unwrap();
            let server = LocalControlTlsIdentity::generate(NodeId::from_bytes([12; 16])).unwrap();
            let client_pin =
                PinnedControlTlsPeer::new(network(7), client.node(), client.fingerprint()).unwrap();
            let server_pin =
                PinnedControlTlsPeer::new(network(7), server.node(), server.fingerprint()).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = match listener.local_addr().unwrap() {
                SocketAddr::V4(addr) => addr,
                SocketAddr::V6(_) => panic!("test listener must be IPv4"),
            };
            let listener_network = if scenario == 3 {
                network(8)
            } else {
                network(7)
            };
            let mut owner = ObservedControlListener::new(listener_network, addr, listener).unwrap();
            let client_socket = TcpStream::connect(addr).unwrap();
            let root = std::env::temp_dir().join(format!(
                "blueroute-listener-post-auth-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir_all(&root).unwrap();
            let server_dir = root.join("server");
            let client_dir = root.join("client");
            let local_node = client.node();
            let server_thread = thread::spawn(move || {
                let trust = approved_trust(&server_dir, client_pin);
                let durable_membership = NetworkMembershipStore::new(server_dir.join("membership"));
                let backend = DurableObservationProbe {
                    endpoint: addr,
                    membership_path: durable_membership.path().to_owned(),
                    scenario,
                    calls: AtomicUsize::new(0),
                };
                let registry = Arc::new(Mutex::new(ControlSessionRegistry::new()));
                let session = if scenario >= 13 {
                    futures_lite::future::block_on(
                        owner.poll_registered_from_durable_network_operations(
                            &durable_membership,
                            &backend,
                            &server,
                            &trust,
                            setup(),
                            Arc::clone(&registry),
                        ),
                    )
                } else {
                    let accepted = owner
                        .poll_with_fresh_observations(|| Ok((Some(listener_network), Some(addr))))
                        .unwrap()
                        .unwrap();
                    accept_registered_from_durable_approval(
                        accepted,
                        &server,
                        &trust,
                        setup(),
                        Arc::clone(&registry),
                    )
                    .map(Some)
                    .map_err(|error| {
                        CoreError::with_diagnostic(
                            ErrorKind::AuthenticationFailed,
                            "previous registered TLS setup failed",
                            error.to_string(),
                        )
                    })
                };
                let session = if scenario == 14 {
                    assert_eq!(session.err().unwrap().kind(), ErrorKind::InvalidState,);
                    assert!(registry.lock().unwrap().is_empty());
                    return;
                } else {
                    session.unwrap().unwrap()
                };
                let key = PeerSessionKey {
                    network: network(7),
                    peer: local_node,
                };
                assert!(registry.lock().unwrap().get(key).is_some());
                if scenario == 5 {
                    futures_lite::future::block_on(trust.forget_peer(local_node)).unwrap();
                } else if scenario == 8 {
                    durable_membership
                        .save(&MembershipRegistry::default())
                        .unwrap();
                } else if scenario == 6 {
                    // Simulate a newer independently admitted generation while
                    // the original TLS owner is waiting for listener release.
                    // The old owner must not remove the replacement's slot.
                    let old_id = *session.session_id();
                    let mut entries = registry.lock().unwrap();
                    assert!(entries.remove_if_current(key, old_id));
                    entries
                        .activate_after_authentication(key, [42; 16])
                        .unwrap();
                }
                let observe = || match scenario {
                    0 => Ok((Some(network(7)), Some(addr))),
                    1 => Ok((None, Some(addr))),
                    2 => Err(CoreError::new(
                        ErrorKind::InvalidState,
                        "fresh NAP observation failed after TLS",
                    )),
                    3 => panic!("wrong-network session must not query NAP ownership"),
                    5 | 6 => Ok((Some(network(7)), Some(addr))),
                    _ => Ok((
                        Some(network(7)),
                        Some(SocketAddrV4::new(*addr.ip(), addr.port().wrapping_add(1))),
                    )),
                };
                // Exercise both release boundaries against real localhost
                // mutual TLS: the async path must preserve all existing
                // revocation, stale-generation and NAP ownership gates.
                let result =
                    if scenario == 13 || scenario == 15 {
                        Ok(session)
                    } else if scenario >= 7 {
                        futures_lite::future::block_on(
                            owner.release_registered_from_durable_network_operations(
                                session,
                                &durable_membership,
                                &backend,
                            ),
                        )
                    } else if matches!(scenario, 0 | 2 | 5 | 6) {
                        futures_lite::future::block_on(owner.release_registered_if_current_async(
                            session,
                            || async move { observe() },
                        ))
                    } else {
                        owner.release_registered_if_current(session, observe)
                    };
                if scenario >= 7 {
                    assert_eq!(
                        backend.calls.load(Ordering::SeqCst),
                        if scenario == 13 || scenario == 15 {
                            3
                        } else {
                            usize::from(scenario != 8)
                        },
                        "revoked membership must not query the backend",
                    );
                }
                if scenario == 13 {
                    let session = result.unwrap();
                    let mut active = vec![session];
                    assert!(registry.lock().unwrap().get(key).is_some());
                    assert_eq!(
                        futures_lite::future::block_on(
                            owner.supervise_registered_from_durable_network_operations(
                                &mut active,
                                &durable_membership,
                                &backend,
                            )
                        )
                        .unwrap(),
                        1,
                    );
                    assert_eq!(backend.calls.load(Ordering::SeqCst), 4);
                    durable_membership
                        .save(&MembershipRegistry::default())
                        .unwrap();
                    assert_eq!(
                        futures_lite::future::block_on(
                            owner.supervise_registered_from_durable_network_operations(
                                &mut active,
                                &durable_membership,
                                &backend,
                            )
                        )
                        .unwrap_err()
                        .kind(),
                        ErrorKind::InvalidState,
                    );
                    assert!(active.is_empty());
                    assert_eq!(backend.calls.load(Ordering::SeqCst), 4);
                    assert!(TcpStream::connect(addr).is_err());
                } else if scenario == 15 {
                    let mut active = vec![result.unwrap()];
                    let mut future = Box::pin(
                        owner.supervise_registered_from_durable_network_operations(
                            &mut active,
                            &durable_membership,
                            &backend,
                        ),
                    );
                    let mut context =
                        std::task::Context::from_waker(std::task::Waker::noop());
                    assert!(matches!(
                        std::future::Future::poll(future.as_mut(), &mut context),
                        std::task::Poll::Pending,
                    ));
                    // Cancelling the future must close the owned TLS socket and
                    // release its exact registry slot rather than grant a
                    // permanent authorization lease.
                    drop(future);
                    assert!(active.is_empty());
                    assert!(registry.lock().unwrap().get(key).is_none());
                    assert_eq!(backend.calls.load(Ordering::SeqCst), 4);
                } else if scenario == 0 || scenario == 7 {
                    let mut session = result.unwrap();
                    assert!(registry.lock().unwrap().get(key).is_some());
                    session.cancel();
                } else if scenario == 9 {
                    assert_eq!(result.err().unwrap().kind(), ErrorKind::PersistenceError);
                    assert_eq!(
                        fs::read(durable_membership.path()).unwrap(),
                        b"BLUEROUTE_MEMBERSHIP_V999\n",
                    );
                    assert!(TcpStream::connect(addr).is_err());
                } else if scenario == 5 || scenario == 6 {
                    assert_eq!(
                        result.err().unwrap().kind(),
                        ErrorKind::AuthenticationFailed
                    );
                    // Revocation or stale registry ownership must not retire
                    // an otherwise correctly owned listener for other peers.
                    assert!(
                        owner
                            .accept_if_current(Some(network(7)), Some(addr))
                            .unwrap()
                            .is_none()
                    );
                } else {
                    assert_eq!(result.err().unwrap().kind(), ErrorKind::InvalidState);
                    assert_eq!(
                        owner
                            .accept_if_current(Some(listener_network), Some(addr))
                            .err()
                            .unwrap()
                            .kind(),
                        ErrorKind::InvalidState,
                    );
                }
                if scenario == 6 {
                    assert_eq!(
                        registry.lock().unwrap().get(key).unwrap().session_id,
                        [42; 16],
                        "stale TLS owner must not retire its replacement"
                    );
                } else {
                    assert!(registry.lock().unwrap().get(key).is_none());
                }
            });
            let client_trust = approved_trust(&client_dir, server_pin);
            let session = connect_approved_control_session(
                client_socket,
                &client,
                server_pin,
                &client_trust,
                setup(),
            )
            .unwrap();
            drop(session);
            server_thread.join().unwrap();
            fs::remove_dir_all(root).unwrap();
        }
    }
}
