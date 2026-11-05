//! Topology executor (P9-004).
//!
//! Applies an ordered [`PlanDiff`] through a [`TopologyBackend`] and then re-reads what is
//! really active, so the daemon always continues from true state after partial failure. It
//! never removes a link whose replacement failed to come up, then asks routing to
//! recompute. Failures are reported, not hidden; retrying is just planning again.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

use crate::{CoreError, LinkChange, PlanDiff, PlannedLink};

pub type ExecFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CoreError>> + Send + 'a>>;

/// What the executor needs from the platform: PAN link lifecycle and a routing hook.
pub trait TopologyBackend: Send + Sync {
    fn create_link(&self, link: PlannedLink) -> ExecFuture<'_, ()>;
    fn remove_link(&self, link: PlannedLink) -> ExecFuture<'_, ()>;
    /// The links that really exist now (source of truth after failures).
    fn active_links(&self) -> ExecFuture<'_, BTreeSet<PlannedLink>>;
    /// Topology changed: recompute and apply routes.
    fn topology_changed(&self, active: &BTreeSet<PlannedLink>) -> ExecFuture<'_, ()>;
}

#[derive(Debug, Default)]
pub struct ExecutionReport {
    /// Links that are active after execution (re-read from the backend when anything failed).
    pub active: BTreeSet<PlannedLink>,
    pub failures: Vec<(LinkChange, CoreError)>,
    /// Removals not attempted because their replacement did not come up.
    pub skipped: Vec<LinkChange>,
    pub routing_error: Option<CoreError>,
}

impl ExecutionReport {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty() && self.skipped.is_empty() && self.routing_error.is_none()
    }
}

pub async fn execute_diff(
    backend: &dyn TopologyBackend,
    start: BTreeSet<PlannedLink>,
    diff: &PlanDiff,
) -> ExecutionReport {
    let mut report = ExecutionReport {
        active: start,
        ..ExecutionReport::default()
    };
    let mut failed_adds: Vec<PlannedLink> = Vec::new();
    let mut changed = false;

    for change in &diff.changes {
        match *change {
            LinkChange::Add(link) => match backend.create_link(link).await {
                Ok(()) => {
                    report.active.insert(link);
                    changed = true;
                }
                Err(error) => {
                    failed_adds.push(link);
                    report.failures.push((*change, error));
                }
            },
            LinkChange::Remove(link) => {
                let replacement_failed = failed_adds.iter().any(|a| {
                    [a.nap, a.panu]
                        .iter()
                        .any(|n| *n == link.nap || *n == link.panu)
                });
                if replacement_failed {
                    report.skipped.push(*change);
                    continue;
                }
                match backend.remove_link(link).await {
                    Ok(()) => {
                        report.active.remove(&link);
                        changed = true;
                    }
                    Err(error) => report.failures.push((*change, error)),
                }
            }
        }
    }

    if !report.failures.is_empty() {
        // Trust the platform over our bookkeeping: a failed call may still have changed state.
        match backend.active_links().await {
            Ok(actual) => {
                changed |= actual != report.active;
                report.active = actual;
            }
            Err(error) => report.routing_error = Some(error),
        }
    }
    if changed && let Err(error) = backend.topology_changed(&report.active).await {
        report.routing_error = Some(error);
    }
    report
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures_lite::future::block_on;

    use super::*;
    use crate::{ErrorKind, NodeId, TopologyPlan, diff_topology};

    fn n(b: u8) -> NodeId {
        NodeId::from_bytes([b; 16])
    }

    fn l(nap: u8, panu: u8) -> PlannedLink {
        PlannedLink {
            nap: n(nap),
            panu: n(panu),
        }
    }

    #[derive(Default)]
    struct Fake {
        links: Mutex<BTreeSet<PlannedLink>>,
        fail_create: Mutex<BTreeSet<PlannedLink>>,
        fail_remove: Mutex<BTreeSet<PlannedLink>>,
        /// A remove that errors after the link is actually gone (ambiguous failure).
        remove_errors_but_succeeds: Mutex<BTreeSet<PlannedLink>>,
        routing_calls: Mutex<Vec<BTreeSet<PlannedLink>>>,
    }

    impl TopologyBackend for Fake {
        fn create_link(&self, link: PlannedLink) -> ExecFuture<'_, ()> {
            Box::pin(async move {
                if self.fail_create.lock().unwrap().contains(&link) {
                    return Err(CoreError::new(ErrorKind::PanFailure, "create failed"));
                }
                self.links.lock().unwrap().insert(link);
                Ok(())
            })
        }
        fn remove_link(&self, link: PlannedLink) -> ExecFuture<'_, ()> {
            Box::pin(async move {
                if self.fail_remove.lock().unwrap().contains(&link) {
                    return Err(CoreError::new(ErrorKind::PanFailure, "remove failed"));
                }
                self.links.lock().unwrap().remove(&link);
                if self
                    .remove_errors_but_succeeds
                    .lock()
                    .unwrap()
                    .contains(&link)
                {
                    return Err(CoreError::new(ErrorKind::PanFailure, "late error"));
                }
                Ok(())
            })
        }
        fn active_links(&self) -> ExecFuture<'_, BTreeSet<PlannedLink>> {
            Box::pin(async { Ok(self.links.lock().unwrap().clone()) })
        }
        fn topology_changed(&self, active: &BTreeSet<PlannedLink>) -> ExecFuture<'_, ()> {
            let active = active.clone();
            Box::pin(async move {
                self.routing_calls.lock().unwrap().push(active);
                Ok(())
            })
        }
    }

    fn plan(links: &[PlannedLink]) -> TopologyPlan {
        TopologyPlan {
            hubs: links.iter().map(|l| l.nap).collect(),
            links: links.iter().copied().collect(),
            unassigned: BTreeSet::new(),
        }
    }

    fn fake_with(links: &[PlannedLink]) -> Fake {
        let fake = Fake::default();
        *fake.links.lock().unwrap() = links.iter().copied().collect();
        fake
    }

    async fn reconcile(fake: &Fake, desired: &TopologyPlan) -> ExecutionReport {
        let active = fake.active_links().await.unwrap();
        let diff = diff_topology(&active, desired, &|_| 2);
        execute_diff(fake, active, &diff).await
    }

    #[test]
    fn successful_execution_applies_changes_and_coordinates_routing_once() {
        block_on(async {
            let fake = fake_with(&[l(1, 2)]);
            let desired = plan(&[l(1, 3)]);
            let report = reconcile(&fake, &desired).await;
            assert!(report.is_clean());
            assert_eq!(report.active, desired.links);
            assert_eq!(fake.routing_calls.lock().unwrap().len(), 1);
            // Nothing to do: no routing churn.
            assert!(reconcile(&fake, &desired).await.is_clean());
            assert_eq!(fake.routing_calls.lock().unwrap().len(), 1);
        });
    }

    #[test]
    fn failed_replacement_keeps_the_working_link_and_recovers_next_round() {
        block_on(async {
            let fake = fake_with(&[l(1, 2)]);
            fake.fail_create.lock().unwrap().insert(l(1, 3));
            let desired = plan(&[l(1, 3)]);
            let report = reconcile(&fake, &desired).await;
            assert_eq!(report.failures.len(), 1);
            assert_eq!(report.skipped, vec![LinkChange::Remove(l(1, 2))]);
            assert_eq!(
                report.active,
                BTreeSet::from([l(1, 2)]),
                "working path kept"
            );

            fake.fail_create.lock().unwrap().clear();
            let retry = reconcile(&fake, &desired).await;
            assert!(retry.is_clean());
            assert_eq!(retry.active, desired.links);
        });
    }

    #[test]
    fn failed_removal_is_reported_and_state_comes_from_the_platform() {
        block_on(async {
            let fake = fake_with(&[l(1, 2), l(1, 3)]);
            fake.fail_remove.lock().unwrap().insert(l(1, 3));
            let desired = plan(&[l(1, 2)]);
            let report = reconcile(&fake, &desired).await;
            assert_eq!(report.failures.len(), 1);
            assert_eq!(report.active, BTreeSet::from([l(1, 2), l(1, 3)]));
            fake.fail_remove.lock().unwrap().clear();
            assert!(reconcile(&fake, &desired).await.is_clean());
        });
    }

    #[test]
    fn ambiguous_failure_is_resolved_by_rereading_actual_links_and_routing_follows_reality() {
        block_on(async {
            let fake = fake_with(&[l(1, 2), l(1, 3)]);
            fake.remove_errors_but_succeeds
                .lock()
                .unwrap()
                .insert(l(1, 3));
            let report = reconcile(&fake, &plan(&[l(1, 2)])).await;
            assert_eq!(report.failures.len(), 1);
            assert_eq!(
                report.active,
                BTreeSet::from([l(1, 2)]),
                "link really is gone"
            );
            let calls = fake.routing_calls.lock().unwrap();
            assert_eq!(calls.last().unwrap(), &BTreeSet::from([l(1, 2)]));
        });
    }

    #[test]
    fn repeated_failures_never_corrupt_state_or_panic() {
        block_on(async {
            let fake = fake_with(&[l(1, 2)]);
            fake.fail_create.lock().unwrap().insert(l(1, 3));
            fake.fail_create.lock().unwrap().insert(l(4, 5));
            let desired = plan(&[l(1, 3), l(4, 5)]);
            for _ in 0..5 {
                let report = reconcile(&fake, &desired).await;
                assert!(report.active.contains(&l(1, 2)));
                assert_eq!(report.failures.len(), 2);
            }
        });
    }
}
