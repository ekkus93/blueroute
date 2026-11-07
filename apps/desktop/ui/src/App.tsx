// Read-only desktop UI over typed daemon snapshots (P12-002/004/007/008/010, partial).
// Creating, joining and leaving networks are not wired yet: those buttons stay disabled and
// say so. There is deliberately no Internet-sharing UI (v1 keeps it off).
import "./App.css";
import { AVAILABILITY_TEXT, ERROR_TEXT, HEALTH_TEXT, ISSUE_TEXT, REACHABILITY_TEXT } from "./text";
import { useDaemon } from "./useDaemon";
import type { DaemonApi, Snapshot } from "./useDaemon";
import type { SubscribeToSignals } from "./signals";

export interface AppProps {
  api?: DaemonApi;
  subscribe?: SubscribeToSignals;
}

export function App({ api, subscribe }: AppProps) {
  const { view, link, refresh } = useDaemon(api, subscribe);

  return (
    <main>
      <h1>BlueRoute</h1>
      <p>Connect nearby Linux computers over Bluetooth.</p>

      {link === "disconnected" && (
        <p role="status" className="banner">
          Lost contact with the BlueRoute service. Trying to reconnect…
        </p>
      )}

      {view.phase === "loading" && <p role="status">Loading…</p>}

      {view.phase === "error" && (
        <section aria-labelledby="problem-heading" role="alert">
          <h2 id="problem-heading">{ERROR_TEXT[view.error].title}</h2>
          <p>{ERROR_TEXT[view.error].help}</p>
          <button type="button" onClick={refresh}>
            Try again
          </button>
        </section>
      )}

      {view.phase === "ready" && <Ready snapshot={view.snapshot} />}
    </main>
  );
}

function Ready({ snapshot }: { snapshot: Snapshot }) {
  const { platform, overview, networks, devices } = snapshot;
  return (
    <>
      <section aria-labelledby="start-heading">
        <h2 id="start-heading">Get started</h2>
        <p>{platform.summary}</p>
        {platform.issues.length > 0 && (
          <ul className="plain" aria-label="Things to fix">
            {platform.issues.map((issue) => (
              <li key={issue}>{ISSUE_TEXT[issue]}</li>
            ))}
          </ul>
        )}
        <p>
          <button type="button" disabled aria-describedby="start-note">
            Create a network
          </button>{" "}
          <button type="button" disabled aria-describedby="start-note">
            Join a nearby network
          </button>
        </p>
        <p id="start-note" className="muted">
          Starting ({AVAILABILITY_TEXT[platform.create]}) and joining (
          {AVAILABILITY_TEXT[platform.join]}) from this app are coming soon. For now, use the{" "}
          <code>blueroute</code> command.
        </p>
      </section>

      <section aria-labelledby="network-heading">
        <h2 id="network-heading">Your network</h2>
        {networks.length === 0 ? (
          <p>You are not part of a network yet.</p>
        ) : (
          <ul className="plain">
            {networks.map((network) => (
              <li key={network.id}>
                <strong>{network.name}</strong>
                <span className="muted"> · {deviceCount(network.member_count)}</span>
              </li>
            ))}
          </ul>
        )}
        <p>
          Status: <strong>{HEALTH_TEXT[overview.health]}</strong>
        </p>
      </section>

      <section aria-labelledby="devices-heading">
        <h2 id="devices-heading">Devices</h2>
        {devices.length === 0 ? (
          <p>No other devices are shown yet.</p>
        ) : (
          <ul className="plain">
            {devices.map((device) => (
              <li key={device.id}>
                <strong>{device.name}</strong>
                <span className="muted"> · {REACHABILITY_TEXT[device.reachability]}</span>
              </li>
            ))}
          </ul>
        )}
      </section>

      <details>
        <summary>Technical details</summary>
        <dl>
          <dt>This computer's ID</dt>
          <dd>{overview.local_node ?? "not assigned"}</dd>
          <dt>Current network ID</dt>
          <dd>{overview.network ?? "none"}</dd>
          <dt>Bluetooth adapter</dt>
          <dd>{tri(overview.adapter_available)}</dd>
          <dt>Can start a network</dt>
          <dd>{tri(overview.can_host)}</dd>
          <dt>Can join a network</dt>
          <dd>{tri(overview.can_join)}</dd>
        </dl>
        {devices.length > 0 && (
          <ul className="plain" aria-label="Device IDs">
            {devices.map((device) => (
              <li key={device.id}>
                {device.name}: <code>{device.id}</code>
              </li>
            ))}
          </ul>
        )}
      </details>
    </>
  );
}

function deviceCount(count: number): string {
  return count === 1 ? "1 device" : `${count} devices`;
}

function tri(value: boolean | null): string {
  if (value === null) return "not confirmed yet";
  return value ? "yes" : "no";
}
