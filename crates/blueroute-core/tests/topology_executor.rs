use blueroute_core::{
    CandidateLink, CapabilitySource, CoreError, ErrorKind, NapAssignment, NapLink, NapPlanStep,
    NapSelection, NapTopologyOperator, NodeCapabilities, NodeId, Sourced, TopologyCandidate,
    TopologyPolicyInputs, execute_nap_plan,
};
use std::collections::{BTreeMap, BTreeSet};

fn id(n: u8) -> NodeId {
    NodeId::from_bytes([n; 16])
}
fn link(hub: u8, client: u8) -> NapLink {
    NapLink {
        hub: id(hub),
        client: id(client),
    }
}
fn plan(clients: &[u8]) -> NapSelection {
    let stars = if clients.is_empty() {
        vec![]
    } else {
        vec![NapAssignment {
            hub: id(1),
            clients: clients.iter().map(|n| id(*n)).collect(),
        }]
    };
    let mut unserved: BTreeSet<_> = (1..=4).map(id).collect();
    if !clients.is_empty() {
        unserved.remove(&id(1));
        for n in clients {
            unserved.remove(&id(*n));
        }
    }
    NapSelection { stars, unserved }
}
fn policy() -> TopologyPolicyInputs {
    let candidates = (1..=4)
        .map(|n| {
            let neighbors = (1..=4)
                .filter(|peer| *peer != n)
                .map(|peer| {
                    (
                        id(peer),
                        CandidateLink {
                            path_cost: 1,
                            quality: None,
                            stable: true,
                        },
                    )
                })
                .collect();
            (
                id(n),
                TopologyCandidate {
                    id: id(n),
                    capabilities: NodeCapabilities {
                        nap: Some(Sourced::new(n == 1, CapabilitySource::Measured)),
                        panu: Some(Sourced::new(true, CapabilitySource::Measured)),
                        adapter_usable: Some(Sourced::new(true, CapabilitySource::Measured)),
                        connection_policy_ceiling: Some(Sourced::new(
                            2,
                            CapabilitySource::Measured,
                        )),
                        ..NodeCapabilities::default()
                    },
                    neighbors,
                    currently_hosting: n == 1,
                    gateway_preferred: false,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    TopologyPolicyInputs { candidates }
}
#[derive(Default)]
struct Fake {
    links: BTreeSet<NapLink>,
    events: Vec<&'static str>,
    fail_connect: bool,
    fail_routes: bool,
    ignore_connect: bool,
}
impl NapTopologyOperator for Fake {
    fn observe_links(&mut self) -> Result<BTreeSet<NapLink>, CoreError> {
        self.events.push("observe");
        Ok(self.links.clone())
    }
    fn connect_link(&mut self, link: NapLink) -> Result<(), CoreError> {
        self.events.push("connect");
        if self.fail_connect {
            return Err(CoreError::new(ErrorKind::PanFailure, "injected"));
        }
        if !self.ignore_connect {
            self.links.insert(link);
        }
        Ok(())
    }
    fn disconnect_link(&mut self, link: NapLink) -> Result<(), CoreError> {
        self.events.push("disconnect");
        self.links.remove(&link);
        Ok(())
    }
    fn reconcile_routes(&mut self, _: &BTreeSet<NapLink>) -> Result<(), CoreError> {
        self.events.push("routes");
        if self.fail_routes {
            return Err(CoreError::new(ErrorKind::RouteFailure, "injected"));
        }
        Ok(())
    }
}
#[test]
fn preserves_old_link_until_new_one_is_observed_and_routed() {
    let mut backend = Fake {
        links: BTreeSet::from([link(1, 2)]),
        ..Fake::default()
    };
    let report = execute_nap_plan(&policy(), &plan(&[2]), &plan(&[3]), &mut backend).unwrap();
    assert_eq!(
        report.applied,
        [
            NapPlanStep::Connect(link(1, 3)),
            NapPlanStep::Disconnect(link(1, 2))
        ]
    );
    assert_eq!(report.observed, BTreeSet::from([link(1, 3)]));
    assert_eq!(
        backend.events,
        [
            "observe",
            "connect",
            "observe",
            "routes",
            "disconnect",
            "observe",
            "routes"
        ]
    );
}
#[test]
fn stale_observation_rejects_without_mutation() {
    let mut backend = Fake::default();
    let error = execute_nap_plan(&policy(), &plan(&[2]), &plan(&[3]), &mut backend).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidState);
    assert_eq!(backend.events, ["observe"]);
}
#[test]
fn failed_connect_preserves_working_link() {
    let mut backend = Fake {
        links: BTreeSet::from([link(1, 2)]),
        fail_connect: true,
        ..Fake::default()
    };
    let error = execute_nap_plan(&policy(), &plan(&[2]), &plan(&[3]), &mut backend).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::PanFailure);
    assert_eq!(backend.links, BTreeSet::from([link(1, 2)]));
    assert_eq!(backend.events, ["observe", "connect", "observe", "routes"]);
}

#[test]
fn no_link_changes_still_reconcile_routes() {
    let mut backend = Fake::default();
    let report = execute_nap_plan(&policy(), &plan(&[]), &plan(&[]), &mut backend).unwrap();
    assert!(report.applied.is_empty());
    assert_eq!(backend.events, ["observe", "routes"]);
}
