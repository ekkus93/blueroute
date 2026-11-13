// Plain-language wording. Technical identifiers and capability names stay out of the
// default view; they appear only under "Technical details".
import type {
  ActionAvailability,
  ClientLinkState,
  DesktopError,
  HealthLevel,
  PlatformIssue,
  Reachability,
} from "./daemon";

export const ISSUE_TEXT: Record<PlatformIssue, string> = {
  adapter_missing_or_disabled:
    "Bluetooth is turned off or no Bluetooth adapter was found. Turn Bluetooth on, or plug in an adapter.",
  adapter_unverified: "BlueRoute could not yet confirm that Bluetooth is working on this computer.",
  pan_client_unsupported: "This computer's Bluetooth cannot join a BlueRoute network.",
  pan_host_unsupported: "This computer's Bluetooth cannot start a BlueRoute network.",
  pan_client_unverified: "BlueRoute has not yet confirmed that this computer can join a network.",
  pan_host_unverified: "BlueRoute has not yet confirmed that this computer can start a network.",
  network_backend_unsupported:
    "The network manager on this computer is not one BlueRoute can use yet.",
  network_backend_unverified:
    "BlueRoute has not yet confirmed which network manager this computer uses.",
  daemon_degraded: "The BlueRoute background service is running but reports a problem.",
  daemon_health_unknown: "The BlueRoute background service has not reported its health yet.",
  production_join_unavailable:
    "Joining a network from this app is not available yet. This is a planned feature.",
};

export const ERROR_TEXT: Record<DesktopError | "unknown", { title: string; help: string }> = {
  daemon_unavailable: {
    title: "BlueRoute's background service is not running",
    help: "Start the BlueRoute service (for example, restart your computer or ask your administrator), then this page will update by itself.",
  },
  permission_denied: {
    title: "You do not have permission to do that",
    help: "Ask your administrator to allow your account to use BlueRoute.",
  },
  unsupported: {
    title: "This computer does not support that yet",
    help: "Open the technical details below to see what is missing.",
  },
  operational: {
    title: "Something went wrong",
    help: "Try again in a moment. If it keeps happening, check the BlueRoute service logs.",
  },
  unexpected_response: {
    title: "BlueRoute's service gave an unexpected answer",
    help: "The app and the background service may be different versions. Update both to the same version.",
  },
  address_conflict: {
    title: "This network's addresses are already in use on this computer",
    help: "Another connection here, such as a VPN or a virtual machine network, uses the same private address range. Disconnect or change that connection, then try again. BlueRoute never changes connections it does not own.",
  },
  invalid_name: {
    title: "That name can't be used",
    help: "Use 1 to 64 characters, without line breaks or other control characters.",
  },
  unknown: {
    title: "Something went wrong",
    help: "Try again in a moment.",
  },
};

export const HEALTH_TEXT: Record<HealthLevel, string> = {
  healthy: "Working well",
  degraded: "Working, with problems",
  reconnecting: "Reconnecting…",
  error: "Not working",
};

/** Plain wording for a joined client's link to its host. */
export function clientLinkText(link: ClientLinkState): string {
  switch (link.type) {
    case "connecting":
      return "Connecting to the computer that runs this network…";
    case "waiting":
      return `Could not reach the computer that runs this network. Trying again in ${link.data.retry_in_seconds} seconds.`;
    case "established":
      return "Connected to the computer that runs this network.";
    case "needs_attention":
      return `Not connected: ${link.data.reason}`;
    case "gave_up":
      return "Could not reach the computer that runs this network, so BlueRoute stopped trying. You are still part of the network.";
  }
}

export const REACHABILITY_TEXT: Record<Reachability, string> = {
  local: "This computer",
  direct_link: "Connected directly",
  same_pan_segment: "Connected nearby",
  routed: "Connected through another device",
  unreachable: "Not reachable right now",
};

export const AVAILABILITY_TEXT: Record<ActionAvailability, string> = {
  prerequisites_observed: "Ready",
  blocked: "Not available on this computer",
  unknown: "Checking…",
};
