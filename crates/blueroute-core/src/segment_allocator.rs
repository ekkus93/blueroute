//! Software-only IPv4 prefix reservations for distinct routed PAN segments.
//! This does not install routes or authorize a Bluetooth peer. Applying and
//! removing routes remains owned by the privileged Linux backend/reconciler.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};

use crate::{
    CoreError, ErrorKind, IpPrefix, Ipv4AddressPool, Ipv4StarAddressPlan, NetworkId, SegmentId,
    ensure_ipv4_segment_available, normalized_ipv4_prefix,
};

pub const MAX_RESERVED_PAN_SEGMENTS: usize = 256;
/// Limit worst-case scanning when a user-configured RFC1918 pool is enormous.
pub const MAX_PREFIX_PROBES: u32 = 4096;

#[derive(Clone, Debug)]
pub struct SegmentIpv4Allocator {
    pool: Ipv4AddressPool,
    reservations: BTreeMap<SegmentId, IpPrefix>,
}

impl SegmentIpv4Allocator {
    pub fn new(pool: Ipv4AddressPool) -> Result<Self, CoreError> {
        pool.validate()?;
        Ok(Self {
            pool,
            reservations: BTreeMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.reservations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.reservations.is_empty()
    }

    pub fn reserved(&self) -> impl Iterator<Item = (&SegmentId, &IpPrefix)> {
        self.reservations.iter()
    }

    pub fn get(&self, segment: SegmentId) -> Option<IpPrefix> {
        self.reservations.get(&segment).copied()
    }

    /// Preserve P6-005's existing single-star subnet for the bootstrap PAN.
    /// Only the local allocator reserves this mapping; no OS routes are touched.
    pub fn reserve_bootstrap(
        &mut self,
        network: NetworkId,
        segment: SegmentId,
        foreign_host_prefixes: impl IntoIterator<Item = IpPrefix>,
    ) -> Result<IpPrefix, CoreError> {
        let plan = Ipv4StarAddressPlan::for_network(network, self.pool)?;
        self.reserve_exact(segment, plan.segment, foreign_host_prefixes)
    }

    pub fn reserve_exact(
        &mut self,
        segment: SegmentId,
        prefix: IpPrefix,
        foreign_host_prefixes: impl IntoIterator<Item = IpPrefix>,
    ) -> Result<IpPrefix, CoreError> {
        self.ensure_prefix_in_pool(prefix)?;
        if let Some(existing) = self.get(segment) {
            return if existing == prefix {
                Ok(existing)
            } else {
                Err(CoreError::new(
                    ErrorKind::AddressConflict,
                    "segment already owns a different IPv4 prefix",
                ))
            };
        }
        self.ensure_room()?;
        ensure_ipv4_segment_available(prefix, self.reservations.values().copied())?;
        ensure_ipv4_segment_available(prefix, foreign_host_prefixes)?;
        self.reservations.insert(segment, prefix);
        Ok(prefix)
    }

    /// Deterministic collision probing from a stable segment identity.
    /// Candidate prefixes must not overlap other reservations or live host
    /// routes/addresses; default routes are intentionally excluded by P6-005.
    pub fn allocate(
        &mut self,
        segment: SegmentId,
        foreign_host_prefixes: impl IntoIterator<Item = IpPrefix>,
    ) -> Result<IpPrefix, CoreError> {
        if let Some(prefix) = self.get(segment) {
            return Ok(prefix);
        }
        self.ensure_room()?;
        let foreign: Vec<_> = foreign_host_prefixes.into_iter().collect();
        let choices = self.segment_count();
        let first =
            u32::from_be_bytes(segment.as_bytes()[..4].try_into().map_err(|_| {
                CoreError::new(ErrorKind::InvalidInput, "invalid segment selector")
            })?) % choices;
        for offset in 0..choices.min(MAX_PREFIX_PROBES) {
            let slot = (first + offset) % choices;
            let candidate = self.prefix_at(slot)?;
            if ensure_ipv4_segment_available(candidate, self.reservations.values().copied())
                .is_err()
                || ensure_ipv4_segment_available(candidate, foreign.iter().copied()).is_err()
            {
                continue;
            }
            self.reservations.insert(segment, candidate);
            return Ok(candidate);
        }
        Err(CoreError::new(
            ErrorKind::AddressConflict,
            "no conflict-free PAN prefix found in bounded IPv4 pool search",
        ))
    }

    /// Releases only this allocator's reservation. The caller's network
    /// backend must separately remove only BlueRoute-owned kernel state.
    pub fn release(&mut self, segment: SegmentId) -> Option<IpPrefix> {
        self.reservations.remove(&segment)
    }

    fn ensure_room(&self) -> Result<(), CoreError> {
        if self.reservations.len() >= MAX_RESERVED_PAN_SEGMENTS {
            Err(CoreError::new(
                ErrorKind::CapabilityUnavailable,
                "PAN segment allocator reservation limit reached",
            ))
        } else {
            Ok(())
        }
    }

    fn segment_count(&self) -> u32 {
        1_u32 << u32::from(self.pool.segment_prefix_len - self.pool.pool_prefix_len)
    }

    fn prefix_at(&self, slot: u32) -> Result<IpPrefix, CoreError> {
        let host_bits = 32 - u32::from(self.pool.segment_prefix_len);
        let offset = slot.checked_shl(host_bits).ok_or_else(|| {
            CoreError::new(ErrorKind::InvalidInput, "IPv4 segment shift overflow")
        })?;
        let number = u32::from(self.pool.network)
            .checked_add(offset)
            .ok_or_else(|| CoreError::new(ErrorKind::InvalidInput, "IPv4 segment pool overflow"))?;
        IpPrefix::new(
            IpAddr::V4(Ipv4Addr::from(number)),
            self.pool.segment_prefix_len,
        )
    }

    fn ensure_prefix_in_pool(&self, prefix: IpPrefix) -> Result<(), CoreError> {
        if prefix.prefix_len != self.pool.segment_prefix_len
            || normalized_ipv4_prefix(prefix)? != prefix
        {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "not a canonical PAN segment prefix",
            ));
        }
        let IpAddr::V4(address) = prefix.address else {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "PAN segment must be IPv4",
            ));
        };
        let address = u32::from(address);
        let base = u32::from(self.pool.network);
        let size = 1_u32 << (32 - u32::from(self.pool.segment_prefix_len));
        if address < base
            || (address - base) % size != 0
            || (address - base) / size >= self.segment_count()
        {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "prefix outside BlueRoute address pool",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(value: u8) -> SegmentId {
        SegmentId::from_bytes([value; 16])
    }

    fn v4(bytes: [u8; 4], len: u8) -> IpPrefix {
        IpPrefix::new(IpAddr::V4(Ipv4Addr::from(bytes)), len).unwrap()
    }

    #[test]
    fn bootstrap_and_new_routed_segments_never_share_subnets() {
        let pool = Ipv4AddressPool::default();
        let network = NetworkId::from_bytes([1; 16]);
        let mut alloc = SegmentIpv4Allocator::new(pool).unwrap();
        let original = alloc.reserve_bootstrap(network, segment(1), []).unwrap();
        assert_eq!(
            original,
            Ipv4StarAddressPlan::for_network(network, pool)
                .unwrap()
                .segment
        );
        let second = alloc.allocate(segment(2), []).unwrap();
        let third = alloc.allocate(segment(3), []).unwrap();
        assert_ne!(original, second);
        assert_ne!(original, third);
        assert_ne!(second, third);
        assert_eq!(alloc.allocate(segment(2), []).unwrap(), second);
        assert_eq!(alloc.len(), 3);
    }

    #[test]
    fn overlapping_host_routes_are_skipped_without_modification() {
        let mut alloc = SegmentIpv4Allocator::new(Ipv4AddressPool::default()).unwrap();
        let collision = v4([10, 201, 5, 99], 24);
        let segment_id = SegmentId::from_bytes([5; 16]);
        let reserved = alloc
            .allocate(segment_id, [collision, v4([0, 0, 0, 0], 0)])
            .unwrap();
        assert_ne!(reserved, v4([10, 201, 5, 0], 24));
        assert!(ensure_ipv4_segment_available(reserved, [collision]).is_ok());
        assert_eq!(collision, v4([10, 201, 5, 99], 24));
    }

    #[test]
    fn release_is_owner_scoped_and_idempotent() {
        let mut alloc = SegmentIpv4Allocator::new(Ipv4AddressPool::default()).unwrap();
        let one = alloc.allocate(segment(1), []).unwrap();
        let two = alloc.allocate(segment(2), []).unwrap();
        assert_eq!(alloc.release(segment(1)), Some(one));
        assert_eq!(alloc.release(segment(1)), None);
        assert_eq!(alloc.get(segment(2)), Some(two));
        assert_eq!(alloc.allocate(segment(1), []).unwrap(), one);
    }

    #[test]
    fn unavailable_pool_and_noncanonical_prefixes_fail_closed() {
        let mut alloc = SegmentIpv4Allocator::new(
            Ipv4AddressPool::new(Ipv4Addr::new(10, 201, 8, 0), 30, 30).unwrap(),
        )
        .unwrap();
        let first = alloc.allocate(segment(1), []).unwrap();
        assert_eq!(first, v4([10, 201, 8, 0], 30));
        assert_eq!(
            alloc.allocate(segment(2), []).unwrap_err().kind(),
            ErrorKind::AddressConflict
        );
        assert!(
            alloc
                .reserve_exact(segment(3), v4([10, 201, 8, 1], 30), [])
                .is_err()
        );
        assert!(
            alloc
                .reserve_exact(segment(3), v4([10, 201, 9, 0], 30), [])
                .is_err()
        );
    }
}
