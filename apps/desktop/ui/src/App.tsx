// Desktop UI over typed daemon snapshots (P12-002/004/005/007/008/009, partial), built from the
// shared components in ./components (P12-003). The only mutations
// are creating and leaving a network and asking a joined client to reconnect to its host. Join
// stays disabled and says so. There is deliberately no Internet-sharing UI (v1 keeps it off).
import React, { useState } from "react";
import { daemon as realDaemon } from "./daemon";
import type { ClientLinkState, DesktopError, HealthLevel } from "./daemon";
import {
  AVAILABILITY_TEXT,
  clientLinkText,
  ERROR_TEXT,
  HEALTH_TEXT,
  ISSUE_TEXT,
  LIMIT_SOURCE_TEXT,
  REACHABILITY_TEXT,
} from "./text";
import { diagnose, GENERAL_TIPS } from "./diagnose";
import { Button, Card, EmptyState, Loading, Notice, StatusBadge } from "./components";
import type { Tone } from "./components";
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
      <p className="lead">Connect nearby Linux computers over Bluetooth.</p>

      {link === "disconnected" && (
        <Notice
          tone="warning"
          title="Lost contact with the BlueRoute service. Trying to reconnect…"
        >
          <p>What is shown below may be out of date until the service is back.</p>
        </Notice>
      )}

      {view.phase === "loading" && <Loading />}

      {view.phase === "error" && (
        <Notice tone="error" title={ERROR_TEXT[view.error].title}>
          <p>{ERROR_TEXT[view.error].help}</p>
          <Button variant="primary" onClick={refresh}>
            Try again
          </Button>
        </Notice>
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
  // "Your network" is only the one this computer is in; Settings lists every remembered one.
  const current = networks.filter((network) => network.id === overview.network);
  return (
    <>
      <Card id="start-heading" heading="Get started">
        <p>
          BlueRoute lets nearby Linux computers share a small private network over Bluetooth, with
          no Wi-Fi or router needed. One computer starts a network and the others join it.
        </p>
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
        <div className="actions">
          <Button disabled aria-describedby="join-note">
            Join a nearby network
          </Button>
        </div>
        <p id="join-note" className="muted">
          Joining ({AVAILABILITY_TEXT[platform.join]}) from this app is coming soon. For now, use
          the <code>blueroute</code> command.
        </p>
      </Card>

      <Card id="network-heading" heading="Your network">
        {current.length === 0 ? (
          <EmptyState>You are not part of a network yet.</EmptyState>
        ) : (
          <ul className="plain">
            {current.map((network) => (
              <li key={network.id}>
                <strong>{network.name}</strong>
                <span className="muted"> · {deviceCount(network.member_count)}</span>
              </li>
            ))}
          </ul>
        )}
        <p>
          Status:{" "}
          <StatusBadge tone={HEALTH_TONE[overview.health]}>
            {HEALTH_TEXT[overview.health]}
          </StatusBadge>
        </p>
        {clientLink !== null && <ClientLink link={clientLink} api={api} onRetried={onChanged} />}
        {current.length > 0 && <LeaveNetwork api={api} onLeft={onChanged} />}
      </Card>

      <Card id="devices-heading" heading="Devices">
        {devices.length === 0 ? (
          <EmptyState>No other devices are shown yet.</EmptyState>
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
      </Card>

      <Card id="settings-heading" heading="Settings">
        <DeviceName
          current={devices.find((device) => device.reachability === "local")?.name ?? null}
          api={api}
          onRenamed={onChanged}
        />
        <details>
          <summary>Advanced</summary>
          <dl>
            <dt>Computers that can connect at once while this computer runs a network</dt>
            <dd>
              {overview.client_limit === null
                ? "not reported"
                : `${overview.client_limit.value} (${LIMIT_SOURCE_TEXT[overview.client_limit.source]})`}
            </dd>
          </dl>
          <p className="muted">
            This is a setting that keeps networks stable, not a measured Bluetooth limit. Your
            administrator can change it.
          </p>
        </details>
        <h3>Remembered networks</h3>
        {networks.length === 0 ? (
          <EmptyState>No networks are remembered on this computer.</EmptyState>
        ) : (
          <ul className="plain" aria-label="Remembered networks">
            {networks.map((network) => (
              <li key={network.id}>{network.name}</li>
            ))}
          </ul>
        )}
      </Card>

      <details className="card">
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
    <Card id="diagnose-heading" heading="Diagnose a problem">
      {view.phase === "loading" ? (
        <Loading>Checking…</Loading>
      ) : findings.length === 0 ? (
        <EmptyState>BlueRoute has not reported any problems.</EmptyState>
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
    </Card>
  );
}

const HEALTH_TONE: Record<HealthLevel, Tone> = {
  healthy: "ok",
  degraded: "warning",
  reconnecting: "progress",
  error: "error",
};

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
      </p>
      <div className="actions">
        <input
          id="network-name"
          type="text"
          maxLength={64}
          value={name}
          disabled={!available || busy}
          onChange={(event) => setName(event.target.value)}
          aria-describedby="create-note"
        />
        <Button type="submit" variant="primary" disabled={!available || busy || name.trim() === ""}>
          {busy ? "Creating…" : "Create a network"}
        </Button>
      </div>
      <p id="create-note" className="muted">
        {available
          ? "You may be asked for your password to allow this."
          : `Starting a network: ${availability}.`}
      </p>
      {state.phase === "done" && <Notice tone="ok">The network “{state.name}” was created.</Notice>}
      {state.phase === "failed" && <Failure error={state.error} />}
    </form>
  );
}

type RenameState =
  | { phase: "idle" }
  | { phase: "busy" }
  | { phase: "done"; name: string }
  | { phase: "failed"; error: DesktopError | "unknown" };

/** Renames this computer. The name is only what people see; the identity never changes. */
function DeviceName({
  current,
  api,
  onRenamed,
}: {
  current: string | null;
  api: DaemonApi;
  onRenamed: () => void;
}) {
  const [name, setName] = useState("");
  const [state, setState] = useState<RenameState>({ phase: "idle" });
  const busy = state.phase === "busy";
  const trimmed = name.trim();

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    if (busy || trimmed === "" || trimmed === current) return;
    setState({ phase: "busy" });
    api.setDeviceName(trimmed).then(
      () => {
        setState({ phase: "done", name: trimmed });
        setName("");
        onRenamed();
      },
      (error: unknown) => setState({ phase: "failed", error: normalizeError(error) }),
    );
  };

  return (
    <form onSubmit={submit} aria-label="Rename this computer">
      <p>
        This computer is called <strong>{current ?? "(not reported yet)"}</strong>.
      </p>
      <p>
        <label htmlFor="device-name">New name</label>
      </p>
      <div className="actions">
        <input
          id="device-name"
          type="text"
          maxLength={64}
          value={name}
          disabled={busy}
          onChange={(event) => setName(event.target.value)}
          aria-describedby="rename-note"
        />
        <Button
          type="submit"
          variant="primary"
          disabled={busy || trimmed === "" || trimmed === current}
        >
          {busy ? "Renaming…" : "Rename"}
        </Button>
      </div>
      <p id="rename-note" className="muted">
        This only changes the name people see. This computer's identity stays the same, so networks
        and trusted devices are not affected. You may be asked for your password.
      </p>
      {state.phase === "done" && <Notice tone="ok">This computer is now “{state.name}”.</Notice>}
      {state.phase === "failed" && <Failure error={state.error} />}
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
      <p>
        <output>
          <StatusBadge tone={LINK_BADGE[link.type].tone}>{LINK_BADGE[link.type].label}</StatusBadge>{" "}
          <span>{clientLinkText(link)}</span>
        </output>
      </p>
      {canRetry && (
        <div className="actions">
          <Button variant="primary" onClick={retry} disabled={busy} aria-describedby="retry-note">
            {busy ? "Connecting…" : "Try connecting again"}
          </Button>
          <span id="retry-note" className="muted">
            You may be asked for your password.
          </span>
        </div>
      )}
      {state.phase === "failed" && <Failure error={state.error} />}
    </div>
  );
}

const LINK_BADGE: Record<ClientLinkState["type"], { tone: Tone; label: string }> = {
  connecting: { tone: "progress", label: "Connecting" },
  waiting: { tone: "progress", label: "Retrying" },
  established: { tone: "ok", label: "Connected" },
  needs_attention: { tone: "warning", label: "Needs attention" },
  gave_up: { tone: "error", label: "Stopped trying" },
};

/** A failed action: plain title and help, announced as an alert. */
function Failure({ error }: { error: DesktopError | "unknown" }) {
  return (
    <Notice tone="error" title={`${ERROR_TEXT[error].title}.`}>
      {ERROR_TEXT[error].help}
    </Notice>
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
        <div className="actions">
          <Button onClick={() => setState({ phase: "confirm" })}>Leave this network</Button>
        </div>
      ) : (
        <fieldset className="confirm" aria-label="Confirm leaving">
          <p>
            Leave this network? Other devices lose their connection through this computer. You may
            be asked for your password.
          </p>
          <div className="actions">
            <Button variant="danger" onClick={leave} disabled={busy}>
              {busy ? "Leaving…" : "Yes, leave"}
            </Button>
            <Button onClick={() => setState({ phase: "idle" })} disabled={busy}>
              Stay
            </Button>
          </div>
        </fieldset>
      )}
      {state.phase === "failed" && <Failure error={state.error} />}
    </div>
  );
}
