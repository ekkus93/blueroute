//! Observation-gated P9-004 topology execution. This module is a software
//! contract; it does not implement live Bluetooth PAN or authenticated routing.
use std::collections::BTreeSet;

use crate::{
    CoreError, ErrorKind, NapLink, NapPlanStep, NapSelection, TopologyPolicyInputs,
    diff_nap_topology,
};

/// Implementations must return independently observed links, never requested
/// state. Callers serialize this interface per network.
pub trait NapTopologyOperator {
    fn observe_links(&mut self) -> Result<BTreeSet<NapLink>, CoreError>;
    fn connect_link(&mut self, link: NapLink) -> Result<(), CoreError>;
    fn disconnect_link(&mut self, link: NapLink) -> Result<(), CoreError>;
    fn reconcile_routes(&mut self, observed: &BTreeSet<NapLink>) -> Result<(), CoreError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NapExecutionReport {
    pub applied: Vec<NapPlanStep>,
    pub observed: BTreeSet<NapLink>,
}

fn links(plan: &NapSelection) -> BTreeSet<NapLink> {
    plan.stars
        .iter()
        .flat_map(|star| {
            star.clients.iter().map(move |client| NapLink {
                hub: star.hub,
                client: *client,
            })
        })
        .collect()
}

/// Execute a validated plan, verifying each backend change before applying
/// routes. On any failure, stop and require a fresh observation and replan.
pub fn execute_nap_plan(
    inputs: &TopologyPolicyInputs,
    active: &NapSelection,
    desired: &NapSelection,
    operator: &mut impl NapTopologyOperator,
) -> Result<NapExecutionReport, CoreError> {
    let diff = diff_nap_topology(inputs, active, desired)?;
    let mut observed = operator.observe_links()?;
    if observed != links(active) {
        return Err(CoreError::new(
            ErrorKind::InvalidState,
            "PAN observations changed; replan before execution",
        ));
    }
    let mut applied = Vec::new();
    if diff.steps.is_empty() {
        operator.reconcile_routes(&observed)?;
    }
    for step in diff.steps {
        let mut expected = observed.clone();
        let operation = match step {
            NapPlanStep::Connect(link) => {
                if !expected.insert(link) {
                    return Err(CoreError::new(ErrorKind::InvalidState, "duplicate PAN connect"));
                }
                operator.connect_link(link)
            }
            NapPlanStep::Disconnect(link) => {
                if !expected.remove(&link) {
                    return Err(CoreError::new(ErrorKind::InvalidState, "missing PAN disconnect"));
                }
                operator.disconnect_link(link)
            }
        };
        if let Err(error) = operation {
            // An operation reporting failure might still have mutated the
            // backend; repair routes from observation, not assumptions.
            if let Ok(actual) = operator.observe_links() {
                let _ = operator.reconcile_routes(&actual);
            }
            return Err(error);
        }
        let actual = operator.observe_links()?;
        if actual != expected {
            operator.reconcile_routes(&actual)?;
            return Err(CoreError::new(
                ErrorKind::TopologyFailure,
                "PAN operation did not produce the expected observed topology",
            ));
        }
        operator.reconcile_routes(&actual)?;
        observed = actual;
        applied.push(step);
    }
    Ok(NapExecutionReport { applied, observed })
}
