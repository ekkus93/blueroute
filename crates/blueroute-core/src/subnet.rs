//! Distinct IPv4 subnet allocation per PAN segment (P8-002).
//!
//! One allocator instance per network, run by the network authority and distributed to
//! members (P8-005); independent allocators on different nodes may legitimately disagree, so
//! members must apply the authority's assignments rather than recompute them. Selection is
//! deterministic: start at a slot derived from the `SegmentId` and probe forward, skipping
//! slots that overlap another segment or a prefix already active on a node's host.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr};

use crate::addressing::{ipv4_prefixes_overlap, normalized_ipv4_prefix};
use crate::{CoreError, ErrorKind, IpPrefix, Ipv4AddressPool, SegmentId};

#[derive(Clone, Debug)]
pub struct SegmentSubnets {
    pool: Ipv4AddressPool,
    assigned: BTreeMap<SegmentId, IpPrefix>,
}

impl SegmentSubnets {
    pub fn new(pool: Ipv4AddressPool) -> Result<Self, CoreError> {
        pool.validate()?;
        Ok(Self {
            pool,
            assigned: BTreeMap::new(),
        })
    }

    pub fn get(&self, segment: &SegmentId) -> Option<IpPrefix> {
        self.assigned.get(segment).copied()
    }

    pub fn assignments(&self) -> impl Iterator<Item = (&SegmentId, &IpPrefix)> {
        self.assigned.iter()
    }

    fn slot_count(&self) -> u32 {
        1_u32 << u32::from(self.pool.segment_prefix_len - self.pool.pool_prefix_len)
    }

    fn slot_prefix(&self, slot: u32) -> Result<IpPrefix, CoreError> {
        let host_bits = 32_u32 - u32::from(self.pool.segment_prefix_len);
        let base = u32::from(self.pool.network)
            .checked_add(slot << host_bits)
            .ok_or_else(|| CoreError::new(ErrorKind::InvalidInput, "IPv4 pool overflow"))?;
        IpPrefix::new(
            IpAddr::V4(Ipv4Addr::from(base)),
            self.pool.segment_prefix_len,
        )
    }

    /// Return the segment's prefix, allocating one if needed. An existing assignment is
    /// stable: it is returned unchanged even if a foreign prefix later overlaps it (use
    /// [`Self::conflicts`] to detect that and decide explicitly).
    ///
    /// `foreign` is every prefix already in use on any participating host that BlueRoute
    /// does not own (host routes, other networks, VPNs).
    pub fn allocate(
        &mut self,
        segment: SegmentId,
        foreign: &[IpPrefix],
    ) -> Result<IpPrefix, CoreError> {
        if let Some(existing) = self.assigned.get(&segment) {
            return Ok(*existing);
        }
        let foreign: Vec<IpPrefix> = foreign
            .iter()
            .filter(|p| p.address.is_ipv4() && p.prefix_len != 0)
            .map(|p| normalized_ipv4_prefix(*p))
            .collect::<Result<_, _>>()?;
        let bytes = segment.as_bytes();
        let start =
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % self.slot_count();
        for step in 0..self.slot_count() {
            let candidate = self.slot_prefix((start + step) % self.slot_count())?;
            let clashes_assigned = self
                .assigned
                .values()
                .any(|p| ipv4_prefixes_overlap(candidate, *p));
            let clashes_foreign = foreign.iter().any(|p| ipv4_prefixes_overlap(candidate, *p));
            if !clashes_assigned && !clashes_foreign {
                self.assigned.insert(segment, candidate);
                return Ok(candidate);
            }
        }
        Err(CoreError::new(
            ErrorKind::AddressConflict,
            "no conflict-free IPv4 prefix is left in the BlueRoute address pool",
        ))
    }

    /// Segments whose assigned prefix now overlaps a foreign prefix (host-route conflicts).
    pub fn conflicts(&self, foreign: &[IpPrefix]) -> BTreeSet<SegmentId> {
        self.assigned
            .iter()
            .filter(|(_, assigned)| {
                foreign.iter().any(|p| {
                    p.address.is_ipv4()
                        && p.prefix_len != 0
                        && normalized_ipv4_prefix(*p)
                            .is_ok_and(|p| ipv4_prefixes_overlap(**assigned, p))
                })
            })
            .map(|(segment, _)| *segment)
            .collect()
    }

    /// Release prefixes of segments that no longer exist, but only when `still_in_use` (live
    /// addresses or routes still carrying that prefix on any node) no longer overlaps them.
    /// Returns what was reclaimed. A prefix still in use stays assigned and is retried later.
    pub fn reclaim(
        &mut self,
        live_segments: &BTreeSet<SegmentId>,
        still_in_use: &[IpPrefix],
    ) -> Vec<(SegmentId, IpPrefix)> {
        let doomed: Vec<(SegmentId, IpPrefix)> = self
            .assigned
            .iter()
            .filter(|(segment, prefix)| {
                !live_segments.contains(segment)
                    && !still_in_use.iter().any(|p| {
                        p.address.is_ipv4()
                            && normalized_ipv4_prefix(*p)
                                .is_ok_and(|p| ipv4_prefixes_overlap(**prefix, p))
                    })
            })
            .map(|(s, p)| (*s, *p))
            .collect();
        for (segment, _) in &doomed {
            self.assigned.remove(segment);
        }
        doomed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(b: u8) -> SegmentId {
        SegmentId::from_bytes([b; 16])
    }

    fn v4(a: [u8; 4], len: u8) -> IpPrefix {
        IpPrefix::new(IpAddr::V4(Ipv4Addr::from(a)), len).unwrap()
    }

    fn subnets() -> SegmentSubnets {
        SegmentSubnets::new(Ipv4AddressPool::default()).unwrap()
    }

    #[test]
    fn many_segments_get_distinct_non_overlapping_prefixes_inside_the_pool() {
        let mut s = subnets();
        let mut seen = BTreeSet::new();
        for i in 0..=255u8 {
            let p = s.allocate(seg(i), &[]).unwrap();
            assert!(seen.insert(p), "duplicate prefix {p:?}");
            assert_eq!(p.prefix_len, 24);
            assert!(matches!(p.address, IpAddr::V4(a) if a.octets()[..2] == [10, 201]));
        }
        assert_eq!(seen.len(), 256);
        let mut extra_bytes = [1_u8; 16];
        extra_bytes[15] = 0xEE;
        let extra = SegmentId::from_bytes(extra_bytes);
        assert!(
            s.allocate(seg(1), &[]).is_ok(),
            "existing segment keeps its prefix"
        );
        assert!(s.allocate(extra, &[]).is_err(), "pool exhausted");
    }

    #[test]
    fn allocation_is_deterministic_and_stable() {
        let (mut a, mut b) = (subnets(), subnets());
        let first = a.allocate(seg(7), &[]).unwrap();
        assert_eq!(first, b.allocate(seg(7), &[]).unwrap());
        assert_eq!(
            first,
            a.allocate(seg(7), &[v4([10, 201, 0, 0], 16)]).unwrap()
        );
    }

    #[test]
    fn foreign_host_prefixes_are_avoided() {
        let mut clean = subnets();
        let wanted = clean.allocate(seg(3), &[]).unwrap();
        let mut s = subnets();
        let got = s
            .allocate(seg(3), &[wanted, v4([192, 168, 1, 0], 24)])
            .unwrap();
        assert_ne!(got, wanted);
        assert!(!ipv4_prefixes_overlap(got, wanted));
        // A broad foreign prefix (VPN /16) overlapping part of the pool is also avoided.
        let mut t = subnets();
        let got = t.allocate(seg(3), &[v4([10, 201, 0, 0], 17)]).unwrap();
        assert!(matches!(got.address, IpAddr::V4(a) if a.octets()[2] >= 128));
        // Default routes are not conflicts.
        assert_eq!(
            subnets().allocate(seg(3), &[v4([0, 0, 0, 0], 0)]).unwrap(),
            wanted
        );
    }

    #[test]
    fn later_host_conflicts_are_detected_not_silently_moved() {
        let mut s = subnets();
        let p = s.allocate(seg(1), &[]).unwrap();
        let conflicting = v4([10, 201, 0, 0], 16);
        assert_eq!(s.conflicts(&[conflicting]), BTreeSet::from([seg(1)]));
        assert!(s.conflicts(&[v4([172, 16, 0, 0], 12)]).is_empty());
        assert_eq!(s.allocate(seg(1), &[conflicting]).unwrap(), p);
    }

    #[test]
    fn reclamation_waits_until_the_prefix_is_no_longer_in_use() {
        let mut s = subnets();
        let p1 = s.allocate(seg(1), &[]).unwrap();
        let p2 = s.allocate(seg(2), &[]).unwrap();
        let live = BTreeSet::from([seg(2)]);
        assert!(
            s.reclaim(&live, &[p1]).is_empty(),
            "still carried by a node"
        );
        assert_eq!(s.get(&seg(1)), Some(p1));
        assert_eq!(s.reclaim(&live, &[]), vec![(seg(1), p1)]);
        assert_eq!(
            s.get(&seg(2)),
            Some(p2),
            "live segments are never reclaimed"
        );
        let again = s.allocate(seg(9), &[]).unwrap();
        assert!(!ipv4_prefixes_overlap(again, p2));
    }

    #[test]
    fn invalid_pool_is_rejected() {
        let bad = Ipv4AddressPool {
            network: Ipv4Addr::new(8, 8, 0, 0),
            pool_prefix_len: 16,
            segment_prefix_len: 24,
        };
        assert!(SegmentSubnets::new(bad).is_err());
    }
}
