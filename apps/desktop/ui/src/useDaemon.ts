import { useCallback, useEffect, useRef, useState } from "react";
import { daemon as realDaemon } from "./daemon";
import type {
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
  createNetwork: (name: string) => Promise<void>;
  leaveNetwork: () => Promise<void>;
}

export interface Snapshot {
  platform: PlatformCheck;
  overview: DesktopOverview;
  networks: NetworkSummary[];
  devices: NodeSummary[];
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
]);

export function normalizeError(error: unknown): DesktopError | "unknown" {
  return typeof error === "string" && ERRORS.has(error) ? (error as DesktopError) : "unknown";
}

/** Reads daemon snapshots and refreshes them (coalesced) on payload-free daemon signals. */
export function useDaemon(
  api: DaemonApi = realDaemon,
  subscribe: SubscribeToSignals = realSubscribe,
): { view: View; link: LinkState; refresh: () => void } {
  const [view, setView] = useState<View>({ phase: "loading" });
  const [link, setLink] = useState<LinkState>("unknown");
  const inFlight = useRef(false);
  const pending = useRef(false);
  const alive = useRef(true);

  const load = useCallback(async () => {
    if (inFlight.current) {
      pending.current = true;
      return;
    }
    inFlight.current = true;
    try {
      const [platform, overview, networks, devices] = await Promise.all([
        api.platformCheck(),
        api.overview(),
        api.networks(),
        api.devices(),
      ]);
      if (alive.current) {
        setView({ phase: "ready", snapshot: { platform, overview, networks, devices } });
      }
    } catch (error) {
      if (alive.current) {
        setView({ phase: "error", error: normalizeError(error) });
      }
    } finally {
      inFlight.current = false;
      if (pending.current && alive.current) {
        pending.current = false;
        void load();
      }
    }
  }, [api]);

  useEffect(() => {
    alive.current = true;
    void load();
    let unsubscribe: (() => void) | undefined;
    void subscribe((signal) => {
      if (signal === "disconnected") {
        setLink("disconnected");
      } else {
        setLink("connected");
      }
      void load();
    })
      .then((stop) => {
        if (alive.current) {
          unsubscribe = stop;
        } else {
          stop();
        }
      })
      .catch(() => {
        // Without live signals the page still works; the user can refresh manually.
      });
    return () => {
      alive.current = false;
      unsubscribe?.();
    };
  }, [load, subscribe]);

  return { view, link, refresh: () => void load() };
}
