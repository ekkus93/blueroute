//! P9-006 software-only topology anti-flap gate. Costs must come from
//! verified, locally observed topology, not unauthenticated peer claims.
use crate::{CoreError, ErrorKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TopologyStabilityPolicy {
    pub min_dwell_ms: u64,
    pub min_cost_improvement: u64,
    pub retry_base_ms: u64,
    pub retry_max_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TopologyChangeObservation {
    pub now_ms: u64,
    pub last_change_ms: u64,
    pub current_viable: bool,
    pub proposed_viable: bool,
    pub current_cost: u64,
    pub proposed_cost: u64,
    pub consecutive_failures: u32,
    pub last_failure_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TopologyChangeDecision {
    KeepCurrent,
    DeferUntil(u64),
    Apply,
    NoViableProposal,
}

/// A failed current path bypasses dwell/improvement thresholds but not
/// bounded retry backoff. Clock regression fails closed.
pub fn decide_topology_change(
    policy: TopologyStabilityPolicy,
    observation: TopologyChangeObservation,
) -> Result<TopologyChangeDecision, CoreError> {
    if policy.min_dwell_ms == 0
        || policy.min_cost_improvement == 0
        || policy.retry_base_ms == 0
        || policy.retry_max_ms < policy.retry_base_ms
        || observation.now_ms < observation.last_change_ms
        || observation
            .last_failure_ms
            .is_some_and(|time| observation.now_ms < time)
        || (observation.consecutive_failures > 0 && observation.last_failure_ms.is_none())
    {
        return Err(CoreError::new(
            ErrorKind::InvalidInput,
            "invalid topology stability policy or observation",
        ));
    }
    if !observation.proposed_viable {
        return Ok(TopologyChangeDecision::NoViableProposal);
    }
    if observation.current_viable {
        let gain = observation
            .current_cost
            .saturating_sub(observation.proposed_cost);
        if gain < policy.min_cost_improvement {
            return Ok(TopologyChangeDecision::KeepCurrent);
        }
    }
    if let Some(last_failure) = observation.last_failure_ms {
        if observation.consecutive_failures > 0 {
            let exponent = observation.consecutive_failures.saturating_sub(1).min(63);
            let factor = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
            let delay = policy
                .retry_base_ms
                .saturating_mul(factor)
                .min(policy.retry_max_ms);
            let deadline = last_failure.saturating_add(delay);
            if observation.now_ms < deadline {
                return Ok(TopologyChangeDecision::DeferUntil(deadline));
            }
        }
    }
    if observation.current_viable {
        let deadline = observation
            .last_change_ms
            .saturating_add(policy.min_dwell_ms);
        if observation.now_ms < deadline {
            return Ok(TopologyChangeDecision::DeferUntil(deadline));
        }
    }
    Ok(TopologyChangeDecision::Apply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> TopologyStabilityPolicy {
        TopologyStabilityPolicy {
            min_dwell_ms: 1_000,
            min_cost_improvement: 5,
            retry_base_ms: 100,
            retry_max_ms: 800,
        }
    }

    fn observation() -> TopologyChangeObservation {
        TopologyChangeObservation {
            now_ms: 2_000,
            last_change_ms: 0,
            current_viable: true,
            proposed_viable: true,
            current_cost: 100,
            proposed_cost: 96,
            consecutive_failures: 0,
            last_failure_ms: None,
        }
    }

    #[test]
    fn small_metric_changes_preserve_known_good_topology() {
        assert_eq!(
            decide_topology_change(policy(), observation()).unwrap(),
            TopologyChangeDecision::KeepCurrent
        );
        let mut state = observation();
        state.proposed_cost = 90;
        state.last_change_ms = 1_500;
        assert_eq!(
            decide_topology_change(policy(), state).unwrap(),
            TopologyChangeDecision::DeferUntil(2_500)
        );
        state.now_ms = 2_500;
        assert_eq!(
            decide_topology_change(policy(), state).unwrap(),
            TopologyChangeDecision::Apply
        );
    }

    #[test]
    fn failed_current_path_bypasses_dwell_but_respects_retry() {
        let mut state = observation();
        state.current_viable = false;
        state.last_change_ms = 1_999;
        assert_eq!(
            decide_topology_change(policy(), state).unwrap(),
            TopologyChangeDecision::Apply
        );
        state.consecutive_failures = 4;
        state.last_failure_ms = Some(1_900);
        assert_eq!(
            decide_topology_change(policy(), state).unwrap(),
            TopologyChangeDecision::DeferUntil(2_700)
        );
        state.now_ms = 2_700;
        assert_eq!(
            decide_topology_change(policy(), state).unwrap(),
            TopologyChangeDecision::Apply
        );
    }

    #[test]
    fn invalid_clocks_and_missing_failure_history_fail_closed() {
        let mut state = observation();
        state.last_change_ms = 2_001;
        assert!(decide_topology_change(policy(), state).is_err());
        state.last_change_ms = 0;
        state.consecutive_failures = 1;
        assert!(decide_topology_change(policy(), state).is_err());
    }
}
