//! P9-005 software-only hub-loss/recovery integration with an observed fake PAN.
//! This does not establish physical Bluetooth, authenticated control, or routes.
use std::collections::{BTreeMap, BTreeSet};

use blueroute_core::{
    CandidateLink, CapabilitySource, CoreError, ErrorKind, NapAssignment, NapLink, NapPlanStep,
    NapSelection, NapTopologyOperator, NodeCapabilities, NodeId, Sourced, TopologyCandidate,
    TopologyPolicyInputs, execute_nap_plan, plan_hub_loss_recovery,
};

fn id(value: u8) -> NodeId {
    NodeId::from_bytes([value; 16])
}

fn link(hub: u8, client: u8) -> NapLink {
    NapLink {
        hub: id(hub),
        client: id(client),
    }
}

fn policy() -> TopologyPolicyInputs {
    let mut candidates = BTreeMap::new();
    for n in 1..=4 {
        candidates.insert(
            id(n),
            TopologyCandidate {
                id: id(n),
                capabilities: NodeCapabilities {
                    adapter_usable: Some(Sourced::new(true, CapabilitySource::Measured)),
                    nap: Some(Sourced::new(n <= 2, CapabilitySource::Measured)),
                    panu: Some(Sourced::new(n >= 3, CapabilitySource::Measured)),
                    connection_policy_ceiling: Some(Sourced::new(2, CapabilitySource::Measured)),
                    ..NodeCapabilities::default()
                },
                neighbors: BTreeMap::new(),
                currently_hosting: false,
                gateway_preferred: false,
            },
        );
    }
    for hub in [1, 2] {
        for client in [3, 4] {
            let edge = CandidateLink {
                path_cost: 1,
                quality: None,
                stable: true,
            };
            candidates
                .get_mut(&id(hub))
                .unwrap()
                .neighbors
                .insert(id(client), edge);
            candidates
                .get_mut(&id(client))
                .unwrap()
                .neighbors
                .insert(id(hub), edge);
        }
    }
    TopologyPolicyInputs { candidates }
}

#[derive(Default)]
struct FakePan {
    links: BTreeSet<NapLink>,
    route_snapshots: Vec<BTreeSet<NapLink>>,
    failed_connect: Option<NapLink>,
}

impl NapTopologyOperator for FakePan {
    fn observe_links(&mut self) -> Result<BTreeSet<NapLink>, CoreError> {
        Ok(self.links.clone())
    }

    fn connect_link(&mut self, link: NapLink) -> Result<(), CoreError> {
        if self.failed_connect == Some(link) {
            return Err(CoreError::new(
                ErrorKind::PanFailure,
                "injected link failure",
            ));
        }
        self.links.insert(link);
        Ok(())
    }

    fn disconnect_link(&mut self, link: NapLink) -> Result<(), CoreError> {
        self.links.remove(&link);
        Ok(())
    }

    fn reconcile_routes(&mut self, observed: &BTreeSet<NapLink>) -> Result<(), CoreError> {
        self.route_snapshots.push(observed.clone());
        Ok(())
    }
}

fn active_from_observation(observed: &BTreeSet<NapLink>) -> NapSelection {
    let mut stars: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
    for link in observed {
        stars.entry(link.hub).or_default().push(link.client);
    }
    NapSelection {
        stars: stars
            .into_iter()
            .map(|(hub, clients)| NapAssignment { hub, clients })
            .collect(),
        unserved: BTreeSet::new(),
    }
}

#[test]
fn confirmed_hub_loss_reforms_and_recomputes_routes_after_each_observed_link() {
    let inputs = policy();
    let mut pan = FakePan::default();
    let loss = BTreeSet::from([id(1)]);
    let plan = plan_hub_loss_recovery(&inputs, &pan.links, &loss).unwrap();
    let report = execute_nap_plan(
        &inputs,
        &active_from_observation(&pan.links),
        &plan.desired,
        &mut pan,
    )
    .unwrap();
    assert_eq!(
        report.applied,
        [
            NapPlanStep::Connect(link(2, 3)),
            NapPlanStep::Connect(link(2, 4)),
        ]
    );
    assert_eq!(report.observed, BTreeSet::from([link(2, 3), link(2, 4)]));
    assert_eq!(
        pan.route_snapshots,
        [
            BTreeSet::from([link(2, 3)]),
            BTreeSet::from([link(2, 3), link(2, 4)]),
        ]
    );
}

#[test]
fn partial_recovery_failure_replans_from_observation_without_false_route_claims() {
    let inputs = policy();
    let loss = BTreeSet::from([id(1)]);
    let mut pan = FakePan {
        failed_connect: Some(link(2, 4)),
        ..FakePan::default()
    };
    let first = plan_hub_loss_recovery(&inputs, &pan.links, &loss).unwrap();
    let error = execute_nap_plan(
        &inputs,
        &active_from_observation(&pan.links),
        &first.desired,
        &mut pan,
    )
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::PanFailure);
    assert_eq!(pan.links, BTreeSet::from([link(2, 3)]));
    assert_eq!(pan.route_snapshots.last(), Some(&pan.links));

    pan.failed_connect = None;
    let retry = plan_hub_loss_recovery(&inputs, &pan.links, &loss).unwrap();
    assert_eq!(retry.transition.steps, [NapPlanStep::Connect(link(2, 4))]);
    let report = execute_nap_plan(
        &inputs,
        &active_from_observation(&pan.links),
        &retry.desired,
        &mut pan,
    )
    .unwrap();
    assert_eq!(report.observed, BTreeSet::from([link(2, 3), link(2, 4)]));
    assert_eq!(pan.route_snapshots.last(), Some(&report.observed));
}
