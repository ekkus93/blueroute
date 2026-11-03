//! P13 daemon-owned live capability health observation.
//!
//! Refreshing status is deliberately read-only: this does not repair NAP/PANU
//! links, infer authenticated peer state, or replace desired-state reconciliation.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use blueroute_core::HealthLevel;
use blueroute_linux::{SystemCapabilityProbe, SystemSupportLevel};
use futures_lite::future;

use crate::DaemonService;

const HEALTH_RECHECK_INTERVAL: Duration = Duration::from_secs(15);

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
                let health =
                    health_for_support(reported.ok().map(|r| r.support), startup_reconciled);
                match service.update_runtime_health(health) {
                    Ok(true) => eprintln!(
                        "{}",
                        serde_json::json!({
                            "component": "daemon_runtime",
                            "event": "capability_health_transition",
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
    use blueroute_core::{NodeCapabilities, NodeId};

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
