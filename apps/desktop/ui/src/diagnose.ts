// "Diagnose a Problem" (P12-010): turns what the app already knows into guidance. Only
// conditions the daemon actually reported produce findings; nothing is guessed. Causes the app
// cannot observe (address conflicts, driver quirks) appear as general tips, never as findings.
import type { DesktopError } from "./daemon";
import type { LinkState, View } from "./useDaemon";

export interface Finding {
  id: string;
  title: string;
  help: string;
}

export const GENERAL_TIPS: readonly Finding[] = [
  {
    id: "address_conflict",
    title: "Two devices seem to use the same address",
    help: "If devices on a network cannot reach each other although they are all shown as connected, leave and re-create the network so BlueRoute picks fresh addresses. BlueRoute never changes addresses it did not create.",
  },
  {
    id: "routed_peer",
    title: "A device reached through another device stopped responding",
    help: "The device in between may have left or moved out of range. Bring it back, or move closer.",
  },
  {
    id: "driver_quirks",
    title: "Bluetooth driver or firmware problems",
    help: "Some Bluetooth adapters behave badly with certain drivers or firmware. Update your system and firmware, try another adapter, and tell us the adapter model if the problem stays.",
  },
];

const SERVICE_ERRORS: Partial<Record<DesktopError | "unknown", Finding>> = {
  daemon_unavailable: {
    id: "service_unavailable",
    title: "BlueRoute's background service is not running",
    help: "Start the BlueRoute service (for example, restart your computer or ask your administrator). This page updates by itself once it is back.",
  },
  permission_denied: {
    id: "permission_denied",
    title: "Your account is not allowed to use BlueRoute",
    help: "Ask your administrator to allow your account, then try again.",
  },
  unexpected_response: {
    id: "version_mismatch",
    title: "The app and the background service do not match",
    help: "Update the BlueRoute app and the background service to the same version.",
  },
};

/** Findings for the current view, most fundamental first. Empty means nothing was reported. */
export function diagnose(view: View, link: LinkState): Finding[] {
  const findings: Finding[] = [];
  if (view.phase === "error") {
    const known = SERVICE_ERRORS[view.error];
    findings.push(
      known ?? {
        id: "service_error",
        title: "BlueRoute hit a problem talking to its background service",
        help: "Try again in a moment. If it keeps happening, check the BlueRoute service logs.",
      },
    );
    return findings;
  }
  if (link === "disconnected") {
    findings.push({
      id: "service_reconnecting",
      title: "Lost contact with the BlueRoute service",
      help: "The app is trying to reconnect by itself. What you see may be out of date until it does.",
    });
  }
  if (view.phase !== "ready") return findings;

  const { platform, overview, devices } = view.snapshot;
  const issues = new Set(platform.issues);
  if (issues.has("adapter_missing_or_disabled")) {
    findings.push({
      id: "bluetooth_off",
      title: "Bluetooth is off or missing",
      help: "Turn Bluetooth on in your system settings, or plug in a Bluetooth adapter.",
    });
  }
  if (issues.has("pan_host_unsupported") || issues.has("pan_client_unsupported")) {
    findings.push({
      id: "pan_unavailable",
      title: "This computer's Bluetooth lacks a needed network feature",
      help: "Starting or joining a network needs Bluetooth personal-area-network support, which this adapter or driver does not offer. Try another adapter.",
    });
  }
  if (issues.has("network_backend_unsupported")) {
    findings.push({
      id: "backend_unsupported",
      title: "The network manager is not supported",
      help: "BlueRoute currently works with NetworkManager. Install and enable it to use BlueRoute.",
    });
  }
  if (
    platform.issues.some((issue) => issue.endsWith("_unverified")) ||
    issues.has("daemon_health_unknown")
  ) {
    findings.push({
      id: "still_checking",
      title: "BlueRoute is still checking this computer",
      help: "Some abilities have not been confirmed yet. Wait a moment; this page updates by itself.",
    });
  }
  if (overview.health === "reconnecting") {
    findings.push({
      id: "stale_reconnecting",
      title: "BlueRoute is reconnecting",
      help: "Devices may look out of date for a moment while connections are re-established.",
    });
  } else if (overview.health === "degraded" || overview.health === "error") {
    findings.push({
      id: "service_degraded",
      title: "The BlueRoute service reports a problem",
      help: "Check the BlueRoute service logs. Starting or joining a network may be unavailable until it recovers.",
    });
  }
  const unreachable = devices.filter((device) => device.reachability === "unreachable");
  if (unreachable.length > 0) {
    findings.push({
      id: "peer_unreachable",
      title: `${unreachable.map((device) => device.name).join(", ")} can't be reached right now`,
      help: "Make sure the device is switched on, has Bluetooth enabled and is within range, then wait a moment.",
    });
  }
  return findings;
}
