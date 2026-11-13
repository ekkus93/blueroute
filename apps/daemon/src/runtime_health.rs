//! P13 daemon-owned live capability health observation.
//!
//! Refreshing status is deliberately read-only: this does not repair NAP/PANU
//! links, infer authenticated peer state, or replace desired-state reconciliation.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use blueroute_core::{HealthLevel, NodeCapabilities};
use blueroute_linux::{SystemCapabilityProbe, SystemCapabilityReport, SystemSupportLevel};
use blueroute_protocol::{DiagnosticAdapter, SystemDiagnosticMetadata};
use futures_lite::future;

use crate::DaemonService;

const HEALTH_RECHECK_INTERVAL: Duration = Duration::from_secs(15);

/// Convert only locally probed Linux metadata into the shared diagnostic
/// representation. No remote peer claim or previously cached adapter is used.
pub fn system_diagnostics_from_report(report: &SystemCapabilityReport) -> SystemDiagnosticMetadata {
    SystemDiagnosticMetadata {
        // A hardware-test build must never be mistaken for a release in diagnostics.
        daemon_version: if crate::EXPERIMENTAL_JOIN {
            concat!(env!("CARGO_PKG_VERSION"), "+experimental-join").to_owned()
        } else {
            env!("CARGO_PKG_VERSION").to_owned()
        },
        platform: "Linux".to_owned(),
        kernel_release: report.kernel_release.clone(),
        bluez_version: report.bluez_version.clone(),
        network_backend_version: report.network_backend_version.clone(),
        adapters: report
            .controllers
            .iter()
            .map(|adapter| DiagnosticAdapter {
                id: adapter.handle.clone(),
                powered: adapter.powered,
                address: adapter.address.clone(),
                driver: adapter.driver.clone(),
            })
            .collect(),
    }
}

/// Owns the background capability monitor for the lifetime of the daemon.
/// Drop requests shutdown and wakes any parked monitor thread.
pub struct DaemonHealthWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for DaemonHealthWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }
}

/// Start a read-only poll of live service and adapter availability. No
/// capabilities from remote Bluetooth advertisements are trusted.
pub fn spawn_daemon_capability_health_worker(
    service: DaemonService,
    probe: SystemCapabilityProbe,
    startup_reconciled: bool,
) -> std::io::Result<DaemonHealthWorker> {
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let thread = thread::Builder::new()
        .name("blueroute-capability-health".to_owned())
        .spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                thread::park_timeout(HEALTH_RECHECK_INTERVAL);
                if worker_stop.load(Ordering::Acquire) {
                    break;
                }
                // A service restart can change capability observations. Keep
                // this expensive D-Bus probing entirely off the API executor.
                let reported = future::block_on(probe.report());
                if worker_stop.load(Ordering::Acquire) {
                    break;
                }
                let (health, capabilities, system) = match reported {
                    Ok(report) => (
                        health_for_support(Some(report.support), startup_reconciled),
                        report.runtime.node.clone(),
                        Some(system_diagnostics_from_report(&report)),
                    ),
                    Err(_) => (HealthLevel::Degraded, NodeCapabilities::default(), None),
                };
                match service.update_runtime_observation(health, capabilities, system) {
                    Ok(true) => eprintln!(
                        "{}",
                        serde_json::json!({
                            "component": "daemon_runtime",
                            "event": "runtime_observation_updated",
                            "health": format!("{health:?}"),
                        })
                    ),
                    Ok(false) => {}
                    Err(_) => {
                        eprintln!("BlueRoute capability monitor could not update daemon health");
                        break;
                    }
                }
            }
        })?;
    Ok(DaemonHealthWorker {
        stop,
        thread: Some(thread),
    })
}

fn health_for_support(
    support: Option<SystemSupportLevel>,
    startup_reconciled: bool,
) -> HealthLevel {
    match support {
        Some(SystemSupportLevel::Unsupported) => HealthLevel::Error,
        Some(SystemSupportLevel::Degraded) | None => HealthLevel::Degraded,
        Some(SystemSupportLevel::ClientOnly | SystemSupportLevel::FullySupported) => {
            if startup_reconciled {
                HealthLevel::Healthy
            } else {
                HealthLevel::Degraded
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueroute_core::{CapabilitySource, NodeId, Sourced};
    use blueroute_protocol::DiagnosticAdapter;

    #[test]
    fn live_service_loss_and_return_update_shared_status_and_diagnostics() {
        let service = DaemonService::new(
            NodeId::from_bytes([7; 16]),
            HealthLevel::Healthy,
            NodeCapabilities::default(),
        );
        let client_handle = service.clone();
        for (support, expected) in [
            (None, HealthLevel::Degraded),
            (Some(SystemSupportLevel::Unsupported), HealthLevel::Error),
            (Some(SystemSupportLevel::Degraded), HealthLevel::Degraded),
            (
                Some(SystemSupportLevel::FullySupported),
                HealthLevel::Healthy,
            ),
            (Some(SystemSupportLevel::ClientOnly), HealthLevel::Healthy),
        ] {
            assert_eq!(health_for_support(support, true), expected);
            assert!(service.update_runtime_health(expected).is_ok());
            assert_eq!(client_handle.status_snapshot().unwrap().health, expected);
            assert_eq!(
                client_handle.diagnostics_snapshot().unwrap().health,
                expected
            );
        }
    }

    #[test]
    fn failed_startup_reconciliation_cannot_report_healthy_on_service_return() {
        for support in [
            SystemSupportLevel::FullySupported,
            SystemSupportLevel::ClientOnly,
        ] {
            assert_eq!(
                health_for_support(Some(support), false),
                HealthLevel::Degraded
            );
        }
        assert_eq!(
            health_for_support(Some(SystemSupportLevel::Unsupported), false),
            HealthLevel::Error
        );
    }

    fn observed_adapter(name: &str, powered: bool) -> SystemDiagnosticMetadata {
        SystemDiagnosticMetadata {
            daemon_version: "0.1.0".into(),
            platform: "Linux".into(),
            kernel_release: Some("test-kernel".into()),
            bluez_version: Some("test-bluez".into()),
            network_backend_version: Some("test-networkmanager".into()),
            adapters: vec![DiagnosticAdapter {
                id: name.into(),
                powered,
                address: None,
                driver: None,
            }],
        }
    }

    #[test]
    fn live_adapter_inventory_and_capabilities_replace_stale_startup_data() {
        let service = DaemonService::new(
            NodeId::from_bytes([7; 16]),
            HealthLevel::Healthy,
            NodeCapabilities::default(),
        );
        let cloned_api = service.clone();
        let initial = observed_adapter("hci0", true);
        let ready = NodeCapabilities {
            adapter_usable: Some(Sourced::new(true, CapabilitySource::Discovered)),
            ..NodeCapabilities::default()
        };
        assert!(
            service
                .update_runtime_observation(
                    HealthLevel::Healthy,
                    ready.clone(),
                    Some(initial.clone())
                )
                .unwrap()
        );
        assert!(
            !service
                .update_runtime_observation(HealthLevel::Healthy, ready, Some(initial))
                .unwrap()
        );

        // Simulate a lost BlueZ adapter and unavailable NetworkManager.
        let lost = NodeCapabilities {
            adapter_usable: Some(Sourced::new(false, CapabilitySource::Discovered)),
            ..NodeCapabilities::default()
        };
        let mut missing = observed_adapter("hci0", false);
        missing.network_backend_version = None;
        assert!(
            service
                .update_runtime_observation(
                    HealthLevel::Degraded,
                    lost.clone(),
                    Some(missing.clone())
                )
                .unwrap()
        );
        assert_eq!(cloned_api.status_snapshot().unwrap().capabilities, lost);
        let degraded = cloned_api.diagnostics_snapshot().unwrap();
        assert_eq!(degraded.health, HealthLevel::Degraded);
        assert_eq!(degraded.system, Some(missing));
        assert!(degraded.observed_peers.is_none());
        assert!(degraded.topology.is_none());

        // An entirely failed probe must not preserve a stale available adapter.
        assert!(
            service
                .update_runtime_observation(
                    HealthLevel::Degraded,
                    NodeCapabilities::default(),
                    None,
                )
                .unwrap()
        );
        let unknown = cloned_api.diagnostics_snapshot().unwrap();
        assert_eq!(unknown.capabilities, NodeCapabilities::default());
        assert!(unknown.system.is_none());

        // A fresh controller observation replaces the old metadata on return.
        let replacement = observed_adapter("hci1", true);
        let recovered = NodeCapabilities {
            adapter_usable: Some(Sourced::new(true, CapabilitySource::Discovered)),
            ..NodeCapabilities::default()
        };
        assert!(
            service
                .update_runtime_observation(
                    HealthLevel::Healthy,
                    recovered.clone(),
                    Some(replacement.clone()),
                )
                .unwrap()
        );
        let status = cloned_api.status_snapshot().unwrap();
        assert_eq!(status.health, HealthLevel::Healthy);
        assert_eq!(status.capabilities, recovered);
        assert_eq!(
            cloned_api.diagnostics_snapshot().unwrap().system,
            Some(replacement)
        );
    }

    #[test]
    fn repeated_identical_sample_does_not_emit_another_transition() {
        let service = DaemonService::new(
            NodeId::from_bytes([7; 16]),
            HealthLevel::Healthy,
            NodeCapabilities::default(),
        );
        assert!(!service.update_runtime_health(HealthLevel::Healthy).unwrap());
        assert!(
            service
                .update_runtime_health(HealthLevel::Degraded)
                .unwrap()
        );
        assert!(
            !service
                .update_runtime_health(HealthLevel::Degraded)
                .unwrap()
        );
    }
}
