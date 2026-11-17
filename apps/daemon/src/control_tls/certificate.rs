//! Certificates and pins: pinned certificates and peers, enrollment candidates, and strict parsing of a certificate's SPKI fingerprint and node name.

use super::{
    ControlTlsError, MAX_CERTIFICATE_BYTES, MAX_ENROLLMENT_CANDIDATE_AGE, is_zero_network,
    is_zero_node,
};
use blueroute_core::{NetworkId, NodeId};
use blueroute_protocol::peer_pin::SpkiSha256;
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use sha2::{Digest, Sha256};
use std::time::Instant;
use x509_parser::prelude::{FromDer, X509Certificate};

/// A peer certificate whose NodeId and SPKI matched an expected fingerprint.
/// Durable membership and pin authorization are still rechecked before a
/// control session is activated.
#[derive(Clone, Debug)]
pub struct PinnedControlTlsCertificate {
    pub(super) network: NetworkId,
    pub(super) node: NodeId,
    pub(super) certificate: CertificateDer<'static>,
    pub(super) fingerprint: SpkiSha256,
}

/// Untrusted certificate identity received during the enrollment ceremony.
/// It must not authorize a control session unless a user independently
/// confirms the exact fingerprint and `confirm_fingerprint` succeeds.
#[derive(Clone, Debug)]
pub struct ControlTlsCertificateCandidate {
    pub(super) network: NetworkId,
    pub(super) node: NodeId,
    pub(super) certificate: CertificateDer<'static>,
    pub(super) fingerprint: SpkiSha256,
    pub(super) observed_at: Instant,
}

impl ControlTlsCertificateCandidate {
    pub const fn network(&self) -> NetworkId {
        self.network
    }

    pub const fn node(&self) -> NodeId {
        self.node
    }

    pub const fn fingerprint(&self) -> SpkiSha256 {
        self.fingerprint
    }

    /// Convert candidate material into an exact-leaf TLS trust input only
    /// when the independently confirmed fingerprint matches it.
    pub fn confirm_fingerprint(
        self,
        independently_confirmed: SpkiSha256,
    ) -> Result<PinnedControlTlsCertificate, ControlTlsError> {
        // A new confirmation deadline cannot revive a stale, untrusted
        // certificate candidate. Clones retain the original observation time.
        if !matches!(
            Instant::now().checked_duration_since(self.observed_at),
            Some(age) if age < MAX_ENROLLMENT_CANDIDATE_AGE
        ) {
            return Err(ControlTlsError::EnrollmentCandidateExpired);
        }
        if independently_confirmed != self.fingerprint {
            return Err(ControlTlsError::FingerprintMismatch);
        }
        PinnedControlTlsCertificate::new(
            self.network,
            self.node,
            self.certificate,
            independently_confirmed,
        )
    }
}

impl PinnedControlTlsCertificate {
    pub fn new(
        network: NetworkId,
        node: NodeId,
        certificate: CertificateDer<'static>,
        expected_fingerprint: SpkiSha256,
    ) -> Result<Self, ControlTlsError> {
        if is_zero_network(network) || is_zero_node(node) {
            return Err(ControlTlsError::InvalidIdentity);
        }
        if certificate.len() > MAX_CERTIFICATE_BYTES {
            return Err(ControlTlsError::InvalidCertificate);
        }
        let fingerprint = certificate_fingerprint(&certificate)?;
        if fingerprint != expected_fingerprint {
            return Err(ControlTlsError::FingerprintMismatch);
        }
        if !certificate_names_node(&certificate, node)? {
            return Err(ControlTlsError::CertificateIdentityMismatch);
        }
        let mut roots = RootCertStore::empty();
        roots
            .add(certificate.clone())
            .map_err(|_| ControlTlsError::InvalidCertificate)?;
        Ok(Self {
            network,
            node,
            certificate,
            fingerprint,
        })
    }

    pub const fn node(&self) -> NodeId {
        self.node
    }

    pub const fn fingerprint(&self) -> SpkiSha256 {
        self.fingerprint
    }

    pub const fn peer_pin(&self) -> PinnedControlTlsPeer {
        PinnedControlTlsPeer {
            network: self.network,
            node: self.node,
            fingerprint: self.fingerprint,
        }
    }
}

/// Independently approved peer identity used to validate the bounded cleartext
/// certificate preface before configuring TLS. The preface carries no trusted
/// claims: this pin must match it, then TLS must prove private-key possession.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PinnedControlTlsPeer {
    pub(super) network: NetworkId,
    pub(super) node: NodeId,
    pub(super) fingerprint: SpkiSha256,
}

impl PinnedControlTlsPeer {
    pub fn new(
        network: NetworkId,
        node: NodeId,
        fingerprint: SpkiSha256,
    ) -> Result<Self, ControlTlsError> {
        if is_zero_network(network) || is_zero_node(node) {
            return Err(ControlTlsError::InvalidIdentity);
        }
        Ok(Self {
            network,
            node,
            fingerprint,
        })
    }

    pub const fn network(&self) -> NetworkId {
        self.network
    }

    pub const fn node(&self) -> NodeId {
        self.node
    }

    pub const fn fingerprint(&self) -> SpkiSha256 {
        self.fingerprint
    }
}

pub(super) fn certificate_fingerprint(
    certificate: &CertificateDer<'_>,
) -> Result<SpkiSha256, ControlTlsError> {
    let digest = Sha256::digest(certificate_subject_public_key_info(certificate)?);
    let digest: [u8; 32] = digest.into();
    SpkiSha256::from_verified_certificate_digest(digest).map_err(|_| ControlTlsError::InvalidKey)
}

fn certificate_subject_public_key_info(
    certificate: &CertificateDer<'_>,
) -> Result<Vec<u8>, ControlTlsError> {
    let (remaining, parsed) = X509Certificate::from_der(certificate.as_ref())
        .map_err(|_| ControlTlsError::InvalidCertificate)?;
    // A valid prefix followed by arbitrary bytes is not a valid complete
    // certificate. Reject it consistently for enrollment and TLS pin checks.
    if !remaining.is_empty() {
        return Err(ControlTlsError::InvalidCertificate);
    }
    Ok(parsed.public_key().raw.to_vec())
}

pub(super) fn certificate_names_node(
    certificate: &CertificateDer<'_>,
    node: NodeId,
) -> Result<bool, ControlTlsError> {
    Ok(certificate_node_id(certificate)? == node)
}

pub(super) fn certificate_node_id(
    certificate: &CertificateDer<'_>,
) -> Result<NodeId, ControlTlsError> {
    let (remaining, parsed) = X509Certificate::from_der(certificate.as_ref())
        .map_err(|_| ControlTlsError::InvalidCertificate)?;
    // A valid prefix followed by arbitrary bytes is not a valid complete
    // certificate. Reject it consistently for enrollment and TLS pin checks.
    if !remaining.is_empty() {
        return Err(ControlTlsError::InvalidCertificate);
    }
    let Some(names) = parsed
        .subject_alternative_name()
        .map_err(|_| ControlTlsError::InvalidCertificate)?
    else {
        return Err(ControlTlsError::CertificateIdentityMismatch);
    };
    if names.value.general_names.len() != 1 {
        return Err(ControlTlsError::CertificateIdentityMismatch);
    }
    let x509_parser::extensions::GeneralName::DNSName(name) = &names.value.general_names[0] else {
        return Err(ControlTlsError::CertificateIdentityMismatch);
    };
    let value = name
        .strip_prefix("node-")
        .ok_or(ControlTlsError::CertificateIdentityMismatch)?;
    let node: NodeId = value
        .parse()
        .map_err(|_| ControlTlsError::CertificateIdentityMismatch)?;
    // NodeId CLI input permits uppercase hex, but a signed certificate
    // identity must have one unambiguous canonical DNS representation.
    if *name != node_dns_name(node) {
        return Err(ControlTlsError::CertificateIdentityMismatch);
    }
    Ok(node)
}

pub(super) fn node_dns_name(node: NodeId) -> String {
    format!("node-{node}")
}

#[cfg(test)]
mod tests {

    use super::super::ControlTlsError;
    use super::super::test_support::{network, node};
    use super::*;
    use crate::control_tls::identity::LocalControlTlsIdentity;

    #[test]
    fn certificate_fingerprint_is_der_spki_and_node_scoped() {
        let identity = LocalControlTlsIdentity::generate(node(7)).unwrap();
        let pinned = PinnedControlTlsCertificate::new(
            network(9),
            node(7),
            identity.certificate().clone(),
            identity.fingerprint(),
        )
        .unwrap();
        assert_eq!(pinned.fingerprint(), identity.fingerprint());
        assert!(matches!(
            PinnedControlTlsCertificate::new(
                network(9),
                node(8),
                identity.certificate().clone(),
                identity.fingerprint()
            ),
            Err(ControlTlsError::CertificateIdentityMismatch)
        ));
    }
}
