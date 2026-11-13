// Typed, read-only access to daemon snapshots. These are the only Tauri commands the
// frontend may call; they mirror `apps/desktop/src-tauri/build.rs` and the capability file.
import { invoke } from "@tauri-apps/api/core";

export type HealthLevel = "healthy" | "degraded" | "reconnecting" | "error";
export type ActionAvailability = "prerequisites_observed" | "blocked" | "unknown";
export type PlatformIssue =
  | "adapter_missing_or_disabled"
  | "adapter_unverified"
  | "pan_client_unsupported"
  | "pan_host_unsupported"
  | "pan_client_unverified"
  | "pan_host_unverified"
  | "network_backend_unsupported"
  | "network_backend_unverified"
  | "daemon_degraded"
  | "daemon_health_unknown"
  | "production_join_unavailable";
export type Reachability = "local" | "direct_link" | "same_pan_segment" | "routed" | "unreachable";
export type DesktopError =
  | "daemon_unavailable"
  | "permission_denied"
  | "unsupported"
  | "operational"
  | "unexpected_response"
  | "invalid_name";

export interface PlatformCheck {
  summary: string;
  issues: PlatformIssue[];
  create: ActionAvailability;
  join: ActionAvailability;
  join_enabled: boolean;
  internet_sharing_enabled: boolean;
}

export interface DesktopOverview {
  local_node: string | null;
  network: string | null;
  health: HealthLevel;
  adapter_available: boolean | null;
  can_host: boolean | null;
  can_join: boolean | null;
  authenticated_remote_peers: number | null;
  internet_sharing_available: boolean;
}

export interface NetworkSummary {
  id: string;
  name: string;
  member_count: number;
}

/** A joined client's link to its host; `null` when this computer is not a joined client. */
export type ClientLinkState =
  | { type: "connecting"; data: { attempt: number } }
  | { type: "waiting"; data: { attempt: number; retry_in_seconds: number } }
  | { type: "established" }
  | { type: "needs_attention"; data: { reason: string } }
  | { type: "gave_up"; data: { attempts: number } };

export interface NodeSummary {
  id: string;
  name: string;
  reachability: Reachability;
}

export const daemon = {
  platformCheck: () => invoke<PlatformCheck>("platform_check"),
  overview: () => invoke<DesktopOverview>("overview"),
  networks: () => invoke<NetworkSummary[]>("networks"),
  devices: () => invoke<NodeSummary[]>("devices"),
  clientLink: () => invoke<ClientLinkState | null>("client_link"),
  /** Mutation. The daemon authorizes it through PolicyKit. */
  createNetwork: (name: string) => invoke<void>("create_network", { name }),
  /** Mutation with no arguments. The daemon authorizes it through PolicyKit. */
  leaveNetwork: () => invoke<void>("leave_network"),
  /** Mutation with no arguments. The daemon authorizes it through PolicyKit. */
  retryClientReconnect: () => invoke<void>("retry_client_reconnect"),
  /** Mutation. Renames this computer (a label only). The daemon authorizes it through PolicyKit. */
  setDeviceName: (name: string) => invoke<void>("set_device_name", { name }),
} as const;
