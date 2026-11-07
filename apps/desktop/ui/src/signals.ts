// Payload-free daemon change signals forwarded by the Tauri backend. The frontend
// re-reads typed snapshots on each signal; it never receives raw daemon events.
import { listen } from "@tauri-apps/api/event";

export type DaemonSignal = "connected" | "changed" | "disconnected";
export type Unsubscribe = () => void;
export type SubscribeToSignals = (onSignal: (signal: DaemonSignal) => void) => Promise<Unsubscribe>;

const KNOWN: ReadonlySet<string> = new Set(["connected", "changed", "disconnected"]);

export const subscribeToSignals: SubscribeToSignals = (onSignal) =>
  listen<string>("daemon-signal", (event) => {
    if (KNOWN.has(event.payload)) {
      onSignal(event.payload as DaemonSignal);
    }
  });
