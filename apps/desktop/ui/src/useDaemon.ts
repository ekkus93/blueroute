import { useEffect, useRef, useState } from "react";
import { daemon as realDaemon } from "./daemon";
import type {
  ClientLinkState,
  DesktopError,
  DesktopOverview,
  NetworkSummary,
  NodeSummary,
  PlatformCheck,
} from "./daemon";
import { subscribeToSignals as realSubscribe } from "./signals";
import type { SubscribeToSignals } from "./signals";

export interface DaemonApi {
  platformCheck: () => Promise<PlatformCheck>;
  overview: () => Promise<DesktopOverview>;
  networks: () => Promise<NetworkSummary[]>;
  devices: () => Promise<NodeSummary[]>;
  clientLink: () => Promise<ClientLinkState | null>;
  createNetwork: (name: string) => Promise<void>;
  leaveNetwork: () => Promise<void>;
  retryClientReconnect: () => Promise<void>;
  setDeviceName: (name: string) => Promise<void>;
}

export interface Snapshot {
  platform: PlatformCheck;
  overview: DesktopOverview;
  networks: NetworkSummary[];
  devices: NodeSummary[];
  clientLink: ClientLinkState | null;
}

export type LinkState = "unknown" | "connected" | "disconnected";
export type View =
  | { phase: "loading" }
  | { phase: "ready"; snapshot: Snapshot }
  | { phase: "error"; error: DesktopError | "unknown" };

const ERRORS: ReadonlySet<string> = new Set([
  "daemon_unavailable",
  "permission_denied",
  "unsupported",
  "operational",
  "unexpected_response",
  "invalid_name",
  "address_conflict",
]);

export function normalizeError(error: unknown): DesktopError | "unknown" {
  return typeof error === "string" && ERRORS.has(error) ? (error as DesktopError) : "unknown";
}

/**
 * Reads daemon snapshots and refreshes them (coalesced) on payload-free daemon signals.
 *
 * Every effect run is one generation that owns its subscription, callbacks, loads and coalescing
 * state (CR-F6). Nothing it owns is shared with another generation, so after StrictMode's
 * setup-cleanup-setup replay or an `api`/`subscribe` change, a late result of a disposed
 * generation can only release itself: its subscription is stopped exactly once, and its signals
 * and reads change nothing.
 */
export function useDaemon(
  api: DaemonApi = realDaemon,
  subscribe: SubscribeToSignals = realSubscribe,
): { view: View; link: LinkState; refresh: () => void } {
  const [view, setView] = useState<View>({ phase: "loading" });
  const [link, setLink] = useState<LinkState>("unknown");

  // The current generation's loader, for the Refresh button; a no-op while none is mounted.
  const reload = useRef<() => void>(() => {});

  useEffect(() => {
    let disposed = false;
    let inFlight = false;
    let pending = false;
    let unsubscribe: (() => void) | undefined;
    // Read the API's functions here, synchronously, so the effect visibly depends on `api`.
    // Every DaemonApi is a plain object of functions (no `this`), so detached calls are safe.
    const { platformCheck, overview, networks, devices, clientLink } = api;
    // One read of every snapshot. Never throws: a failure becomes the error view.
    const loadOnce = async () => {
      try {
        const results = await Promise.all([
          platformCheck(),
          overview(),
          networks(),
          devices(),
          clientLink(),
        ]);
        if (!disposed) {
          const [platform, overviewNow, networksNow, devicesNow, clientLinkNow] = results;
          setView({
            phase: "ready",
            snapshot: {
              platform,
              overview: overviewNow,
              networks: networksNow,
              devices: devicesNow,
              clientLink: clientLinkNow,
            },
          });
        }
      } catch (error) {
        if (!disposed) {
          setView({ phase: "error", error: normalizeError(error) });
        }
      }
    };
    const load = async () => {
      // Coalesce within this generation: while a load runs, further requests only mark one
      // more pass as needed. A disposed generation starts nothing.
      if (disposed) {
        return;
      }
      if (inFlight) {
        pending = true;
        return;
      }
      inFlight = true;
      let more = true;
      while (more) {
        pending = false;
        await loadOnce();
        more = pending && !disposed;
      }
      // Reached on every path, because loadOnce never throws.
      inFlight = false;
    };
    const mine = () => void load();
    reload.current = mine;
    void load();
    void subscribe((signal) => {
      if (disposed) {
        return;
      }
      setLink(signal === "disconnected" ? "disconnected" : "connected");
      void load();
    })
      .then((stop) => {
        if (disposed) {
          // This generation is gone: release what arrived late, once, and keep nothing.
          stop();
        } else {
          unsubscribe = stop;
        }
      })
      .catch(() => {
        // Without live signals the page still works; the user can refresh manually.
      });
    return () => {
      disposed = true;
      const stop = unsubscribe;
      unsubscribe = undefined;
      stop?.();
      // Only clear the refresh handler if a newer generation has not already replaced it.
      if (reload.current === mine) {
        reload.current = () => {};
      }
    };
  }, [api, subscribe]);

  return { view, link, refresh: () => reload.current() };
}
