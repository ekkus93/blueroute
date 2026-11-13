// Desktop UI over typed daemon snapshots (P12-002/004/005/007/008, partial). The only mutations
// are creating and leaving a network and asking a joined client to reconnect to its host. Join
// stays disabled and says so. There is deliberately no Internet-sharing UI (v1 keeps it off).
import { useState } from "react";
import "./App.css";
import { daemon as realDaemon } from "./daemon";
import type { ClientLinkState, DesktopError } from "./daemon";
import {
  AVAILABILITY_TEXT,
  clientLinkText,
  ERROR_TEXT,
  HEALTH_TEXT,
  ISSUE_TEXT,
  REACHABILITY_TEXT,
} from "./text";
import { diagnose, GENERAL_TIPS } from "./diagnose";
import { normalizeError, useDaemon } from "./useDaemon";
import type { DaemonApi, LinkState, Snapshot, View } from "./useDaemon";
import type { SubscribeToSignals } from "./signals";

export interface AppProps {
  api?: DaemonApi;
  subscribe?: SubscribeToSignals;
}

export function App({ api, subscribe }: AppProps) {
  const effectiveApi = api ?? realDaemon;
  const { view, link, refresh } = useDaemon(effectiveApi, subscribe);

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

      {view.phase === "ready" && (
        <Ready snapshot={view.snapshot} api={effectiveApi} onChanged={refresh} />
      )}

      <Diagnose view={view} link={link} />
    </main>
  );
}

function Ready({
  snapshot,
  api,
  onChanged,
}: {
  snapshot: Snapshot;
  api: DaemonApi;
  onChanged: () => void;
}) {
  const { platform, overview, networks, devices, clientLink } = snapshot;
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
        <CreateNetwork
          available={platform.create === "prerequisites_observed"}
          availability={AVAILABILITY_TEXT[platform.create]}
          api={api}
          onCreated={onChanged}
        />
        <p>
          <button type="button" disabled aria-describedby="join-note">
            Join a nearby network
          </button>
        </p>
        <p id="join-note" className="muted">
          Joining ({AVAILABILITY_TEXT[platform.join]}) from this app is coming soon. For now, use
          the <code>blueroute</code> command.
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
        {clientLink !== null && <ClientLink link={clientLink} api={api} onRetried={onChanged} />}
        {networks.length > 0 && <LeaveNetwork api={api} onLeft={onChanged} />}
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

function Diagnose({ view, link }: { view: View; link: LinkState }) {
  const findings = diagnose(view, link);
  return (
    <section aria-labelledby="diagnose-heading">
      <h2 id="diagnose-heading">Diagnose a problem</h2>
      {view.phase === "loading" ? (
        <p>Checking…</p>
      ) : findings.length === 0 ? (
        <p>BlueRoute has not reported any problems.</p>
      ) : (
        <ul className="plain" aria-label="Problems found">
          {findings.map((finding) => (
            <li key={finding.id}>
              <strong>{finding.title}.</strong> {finding.help}
            </li>
          ))}
        </ul>
      )}
      <details>
        <summary>Other things to try</summary>
        <ul className="plain">
          {GENERAL_TIPS.map((tip) => (
            <li key={tip.id}>
              <strong>{tip.title}.</strong> {tip.help}
            </li>
          ))}
        </ul>
      </details>
    </section>
  );
}

function deviceCount(count: number): string {
  return count === 1 ? "1 device" : `${count} devices`;
}

function tri(value: boolean | null): string {
  if (value === null) return "not confirmed yet";
  return value ? "yes" : "no";
}

type CreateState =
  | { phase: "idle" }
  | { phase: "busy" }
  | { phase: "done"; name: string }
  | { phase: "failed"; error: DesktopError | "unknown" };

function CreateNetwork({
  available,
  availability,
  api,
  onCreated,
}: {
  available: boolean;
  availability: string;
  api: DaemonApi;
  onCreated: () => void;
}) {
  const [name, setName] = useState("");
  const [state, setState] = useState<CreateState>({ phase: "idle" });
  const busy = state.phase === "busy";

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    const trimmed = name.trim();
    if (!available || busy || trimmed === "") return;
    setState({ phase: "busy" });
    api.createNetwork(trimmed).then(
      () => {
        setState({ phase: "done", name: trimmed });
        setName("");
        onCreated();
      },
      (error: unknown) => setState({ phase: "failed", error: normalizeError(error) }),
    );
  };

  return (
    <form onSubmit={submit} aria-label="Create a network">
      <p>
        <label htmlFor="network-name">Network name</label>
        <br />
        <input
          id="network-name"
          type="text"
          maxLength={64}
          value={name}
          disabled={!available || busy}
          onChange={(event) => setName(event.target.value)}
          aria-describedby="create-note"
        />{" "}
        <button type="submit" disabled={!available || busy || name.trim() === ""}>
          {busy ? "Creating…" : "Create a network"}
        </button>
      </p>
      <p id="create-note" className="muted">
        {available
          ? "You may be asked for your password to allow this."
          : `Starting a network: ${availability}.`}
      </p>
      {state.phase === "done" && <p role="status">The network “{state.name}” was created.</p>}
      {state.phase === "failed" && (
        <p role="alert">
          <strong>{ERROR_TEXT[state.error].title}.</strong> {ERROR_TEXT[state.error].help}
        </p>
      )}
    </form>
  );
}

type RetryState =
  { phase: "idle" } | { phase: "busy" } | { phase: "failed"; error: DesktopError | "unknown" };

/** Shows a joined client's link to its host, and a retry only when the daemon stopped trying. */
function ClientLink({
  link,
  api,
  onRetried,
}: {
  link: ClientLinkState;
  api: DaemonApi;
  onRetried: () => void;
}) {
  const [state, setState] = useState<RetryState>({ phase: "idle" });
  const busy = state.phase === "busy";
  const canRetry = link.type === "gave_up" || link.type === "needs_attention";

  const retry = () => {
    setState({ phase: "busy" });
    api.retryClientReconnect().then(
      () => {
        setState({ phase: "idle" });
        onRetried();
      },
      (error: unknown) => setState({ phase: "failed", error: normalizeError(error) }),
    );
  };

  return (
    <div>
      <p role="status">{clientLinkText(link)}</p>
      {canRetry && (
        <p>
          <button type="button" onClick={retry} disabled={busy} aria-describedby="retry-note">
            {busy ? "Connecting…" : "Try connecting again"}
          </button>
          <br />
          <span id="retry-note" className="muted">
            You may be asked for your password.
          </span>
        </p>
      )}
      {state.phase === "failed" && (
        <p role="alert">
          <strong>{ERROR_TEXT[state.error].title}.</strong> {ERROR_TEXT[state.error].help}
        </p>
      )}
    </div>
  );
}

type LeaveState =
  | { phase: "idle" }
  | { phase: "confirm" }
  | { phase: "busy" }
  | { phase: "failed"; error: DesktopError | "unknown" };

function LeaveNetwork({ api, onLeft }: { api: DaemonApi; onLeft: () => void }) {
  const [state, setState] = useState<LeaveState>({ phase: "idle" });
  const busy = state.phase === "busy";

  const leave = () => {
    setState({ phase: "busy" });
    api.leaveNetwork().then(
      () => {
        setState({ phase: "idle" });
        onLeft();
      },
      (error: unknown) => setState({ phase: "failed", error: normalizeError(error) }),
    );
  };

  return (
    <div>
      {state.phase === "idle" || state.phase === "failed" ? (
        <button type="button" onClick={() => setState({ phase: "confirm" })}>
          Leave this network
        </button>
      ) : (
        <div role="group" aria-label="Confirm leaving">
          <p>
            Leave this network? Other devices lose their connection through this computer. You may
            be asked for your password.
          </p>
          <button type="button" onClick={leave} disabled={busy}>
            {busy ? "Leaving…" : "Yes, leave"}
          </button>{" "}
          <button type="button" onClick={() => setState({ phase: "idle" })} disabled={busy}>
            Stay
          </button>
        </div>
      )}
      {state.phase === "failed" && (
        <p role="alert">
          <strong>{ERROR_TEXT[state.error].title}.</strong> {ERROR_TEXT[state.error].help}
        </p>
      )}
    </div>
  );
}
