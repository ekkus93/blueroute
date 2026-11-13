// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { App } from "./App";
import type { DaemonSignal, SubscribeToSignals } from "./signals";
import type { DaemonApi } from "./useDaemon";

afterEach(cleanup);

function fakeApi(overrides: Partial<DaemonApi> = {}): DaemonApi {
  return {
    platformCheck: () =>
      Promise.resolve({
        summary: "This computer looks ready.",
        issues: ["production_join_unavailable"],
        create: "prerequisites_observed",
        join: "prerequisites_observed",
        join_enabled: false,
        internet_sharing_enabled: false,
      }),
    overview: () =>
      Promise.resolve({
        local_node: "node-1",
        network: "net-1",
        health: "healthy",
        adapter_available: true,
        can_host: true,
        can_join: null,
        authenticated_remote_peers: null,
        internet_sharing_available: false,
      }),
    networks: () => Promise.resolve([{ id: "net-1", name: "Lab", member_count: 1 }]),
    devices: () =>
      Promise.resolve([
        { id: "node-2", name: "Bob's laptop", reachability: "routed" as const },
        { id: "node-3", name: "Old PC", reachability: "unreachable" as const },
      ]),
    clientLink: () => Promise.resolve(null),
    createNetwork: () => Promise.resolve(),
    leaveNetwork: () => Promise.resolve(),
    retryClientReconnect: () => Promise.resolve(),
    ...overrides,
  };
}

function fakeSubscribe() {
  let emit: (signal: DaemonSignal) => void = () => {};
  const subscribe: SubscribeToSignals = (onSignal) => {
    emit = onSignal;
    return Promise.resolve(() => {});
  };
  return { subscribe, send: (signal: DaemonSignal) => emit(signal) };
}

test("shows plain-language state, never offers Internet sharing, and disables the unwired join action", async () => {
  render(<App api={fakeApi()} subscribe={fakeSubscribe().subscribe} />);
  expect(await screen.findByRole("heading", { name: "Your network" })).toBeTruthy();
  expect(screen.getByText("Lab")).toBeTruthy();
  expect(screen.getByText(/Connected through another device/)).toBeTruthy();
  expect(screen.getByText(/Not reachable right now/)).toBeTruthy();
  expect(screen.getByText("Working well")).toBeTruthy();
  expect(
    (screen.getByRole("button", { name: "Join a nearby network" }) as HTMLButtonElement).disabled,
  ).toBe(true);
  expect(screen.queryByText(/internet sharing/i)).toBeNull();
});

test("keeps technical identifiers inside the collapsed technical details", async () => {
  render(<App api={fakeApi()} subscribe={fakeSubscribe().subscribe} />);
  await screen.findByText("Technical details");
  const details = screen.getByText("Technical details").closest("details");
  expect(details?.open).toBe(false);
  expect(details?.textContent).toContain("node-2");
  expect(
    screen.getByRole("main").textContent?.replace(details?.textContent ?? "", ""),
  ).not.toContain("node-2");
});

test("explains a missing daemon with guidance and offers retry", async () => {
  const api = fakeApi({ overview: () => Promise.reject("daemon_unavailable") });
  render(<App api={api} subscribe={fakeSubscribe().subscribe} />);
  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toMatch(/background service is not running/);
  expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
});

test("refreshes on daemon signals and shows a reconnecting banner when the link drops", async () => {
  const names = ["Lab", "Lab 2"];
  let call = 0;
  const api = fakeApi({
    networks: () =>
      Promise.resolve([{ id: "net-1", name: names[Math.min(call++, 1)] ?? "?", member_count: 2 }]),
  });
  const bus = fakeSubscribe();
  render(<App api={api} subscribe={bus.subscribe} />);
  await screen.findByText("Lab");
  act(() => bus.send("changed"));
  await waitFor(() => expect(screen.getByText("Lab 2")).toBeTruthy());
  act(() => bus.send("disconnected"));
  expect(screen.getByText(/Trying to reconnect/)).toBeTruthy();
  expect(screen.getByText("Lab 2")).toBeTruthy();
});

test("a failing signal subscription does not break the page", async () => {
  const subscribe = vi.fn<SubscribeToSignals>(() => Promise.reject(new Error("no events")));
  render(<App api={fakeApi()} subscribe={subscribe} />);
  expect(await screen.findByRole("heading", { name: "Your network" })).toBeTruthy();
});

function typeName(value: string) {
  fireEvent.change(screen.getByLabelText("Network name"), { target: { value } });
}

test("creates a network with the trimmed name, confirms it and refreshes", async () => {
  const createNetwork = vi.fn(() => Promise.resolve());
  const networks = vi.fn(() => Promise.resolve([{ id: "net-1", name: "Lab", member_count: 1 }]));
  render(<App api={fakeApi({ createNetwork, networks })} subscribe={fakeSubscribe().subscribe} />);
  await screen.findByRole("heading", { name: "Your network" });
  const button = screen.getByRole("button", { name: "Create a network" }) as HTMLButtonElement;
  expect(button.disabled).toBe(true);
  typeName("  Workshop ");
  expect(button.disabled).toBe(false);
  fireEvent.click(button);
  expect(await screen.findByText(/“Workshop” was created/)).toBeTruthy();
  expect(createNetwork).toHaveBeenCalledExactlyOnceWith("Workshop");
  await waitFor(() => expect(networks.mock.calls.length).toBeGreaterThan(1));
});

test("shows plain guidance when creating fails and lets the user retry", async () => {
  const createNetwork = vi
    .fn<DaemonApi["createNetwork"]>()
    .mockImplementationOnce(() => Promise.reject("permission_denied"))
    .mockImplementationOnce(() => Promise.reject("invalid_name"));
  render(<App api={fakeApi({ createNetwork })} subscribe={fakeSubscribe().subscribe} />);
  await screen.findByRole("heading", { name: "Your network" });
  typeName("Workshop");
  fireEvent.click(screen.getByRole("button", { name: "Create a network" }));
  expect((await screen.findByRole("alert")).textContent).toContain("do not have permission");
  fireEvent.click(screen.getByRole("button", { name: "Create a network" }));
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toContain("That name can't be used"),
  );
});

test("does not offer creation when the computer cannot start a network", async () => {
  const platformCheck = () =>
    Promise.resolve({
      summary: "Bluetooth is off.",
      issues: ["adapter_missing_or_disabled" as const],
      create: "blocked" as const,
      join: "blocked" as const,
      join_enabled: false,
      internet_sharing_enabled: false,
    });
  render(<App api={fakeApi({ platformCheck })} subscribe={fakeSubscribe().subscribe} />);
  await screen.findByRole("heading", { name: "Your network" });
  expect((screen.getByLabelText("Network name") as HTMLInputElement).disabled).toBe(true);
  expect(
    (screen.getByRole("button", { name: "Create a network" }) as HTMLButtonElement).disabled,
  ).toBe(true);
  expect(screen.getByText(/Starting a network: Not available on this computer/)).toBeTruthy();
});

test("Diagnose a problem reports only what the daemon reported, with guidance", async () => {
  render(<App api={fakeApi()} subscribe={fakeSubscribe().subscribe} />);
  const list = await screen.findByRole("list", { name: "Problems found" });
  expect(list.textContent).toContain("Old PC can't be reached right now");
  expect(list.textContent).not.toMatch(/Bluetooth is off/);
  expect(screen.getByText("Other things to try").closest("details")?.open).toBe(false);
});

test("Diagnose a problem explains an unavailable service and a missing adapter", async () => {
  const down = fakeApi({ overview: () => Promise.reject("daemon_unavailable") });
  render(<App api={down} subscribe={fakeSubscribe().subscribe} />);
  const list = await screen.findByRole("list", { name: "Problems found" });
  expect(list.textContent).toContain("background service is not running");
  cleanup();

  const noBluetooth = fakeApi({
    platformCheck: () =>
      Promise.resolve({
        summary: "Check Bluetooth.",
        issues: ["adapter_missing_or_disabled", "pan_host_unsupported"],
        create: "blocked",
        join: "blocked",
        join_enabled: false,
        internet_sharing_enabled: false,
      }),
  });
  render(<App api={noBluetooth} subscribe={fakeSubscribe().subscribe} />);
  const found = await screen.findByRole("list", { name: "Problems found" });
  expect(found.textContent).toContain("Bluetooth is off or missing");
  expect(found.textContent).toContain("lacks a needed network feature");
});

test("the create workflow is a labelled form that submits from the keyboard", async () => {
  const createNetwork = vi.fn(() => Promise.resolve());
  render(<App api={fakeApi({ createNetwork })} subscribe={fakeSubscribe().subscribe} />);
  const input = (await screen.findByLabelText("Network name")) as HTMLInputElement;
  fireEvent.change(input, { target: { value: "Lab 2" } });
  fireEvent.submit(screen.getByRole("form", { name: "Create a network" }));
  await waitFor(() => expect(createNetwork).toHaveBeenCalledWith("Lab 2"));
});

test("leaving needs a confirmation, then calls the daemon once and refreshes", async () => {
  const leaveNetwork = vi.fn(() => Promise.resolve());
  render(<App api={fakeApi({ leaveNetwork })} subscribe={fakeSubscribe().subscribe} />);
  fireEvent.click(await screen.findByRole("button", { name: "Leave this network" }));
  expect(leaveNetwork).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Stay" }));
  expect(screen.queryByRole("button", { name: "Yes, leave" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Leave this network" }));
  fireEvent.click(screen.getByRole("button", { name: "Yes, leave" }));
  await waitFor(() => expect(leaveNetwork).toHaveBeenCalledTimes(1));
});

test("a refused leave shows the reason and keeps the network", async () => {
  const leaveNetwork = vi.fn(() => Promise.reject("permission_denied"));
  render(<App api={fakeApi({ leaveNetwork })} subscribe={fakeSubscribe().subscribe} />);
  fireEvent.click(await screen.findByRole("button", { name: "Leave this network" }));
  fireEvent.click(screen.getByRole("button", { name: "Yes, leave" }));
  expect(await screen.findByText(/do not have permission/)).toBeTruthy();
  expect(screen.getByText("Lab")).toBeTruthy();
});

test("there is no leave button when not in a network", async () => {
  render(
    <App
      api={fakeApi({ networks: () => Promise.resolve([]) })}
      subscribe={fakeSubscribe().subscribe}
    />,
  );
  await screen.findByRole("heading", { name: "Your network" });
  expect(screen.queryByRole("button", { name: "Leave this network" })).toBeNull();
});

test("a host or non-member shows no client link and no reconnect action", async () => {
  render(<App api={fakeApi()} subscribe={fakeSubscribe().subscribe} />);
  await screen.findByRole("heading", { name: "Your network" });
  expect(screen.queryByText(/computer that runs this network/)).toBeNull();
  expect(screen.queryByRole("button", { name: "Try connecting again" })).toBeNull();
});

test("a connecting or connected client link has no retry button", async () => {
  const clientLink = () => Promise.resolve({ type: "connecting" as const, data: { attempt: 1 } });
  render(<App api={fakeApi({ clientLink })} subscribe={fakeSubscribe().subscribe} />);
  expect(await screen.findByText(/Connecting to the computer/)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Try connecting again" })).toBeNull();
  cleanup();

  const connected = () => Promise.resolve({ type: "established" as const });
  render(<App api={fakeApi({ clientLink: connected })} subscribe={fakeSubscribe().subscribe} />);
  expect(await screen.findByText(/^Connected to the computer/)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Try connecting again" })).toBeNull();
});

test("a client that gave up offers a retry that calls the daemon once and refreshes", async () => {
  const retryClientReconnect = vi.fn(() => Promise.resolve());
  const clientLink = vi.fn(() =>
    Promise.resolve({ type: "gave_up" as const, data: { attempts: 8 } }),
  );
  render(
    <App
      api={fakeApi({ clientLink, retryClientReconnect })}
      subscribe={fakeSubscribe().subscribe}
    />,
  );
  expect(await screen.findByText(/stopped trying/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Try connecting again" }));
  await waitFor(() => expect(retryClientReconnect).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(clientLink.mock.calls.length).toBeGreaterThan(1));
});

test("a client needing attention shows the daemon's reason and a refused retry explains why", async () => {
  const clientLink = () =>
    Promise.resolve({
      type: "needs_attention" as const,
      data: { reason: "the host's key is missing" },
    });
  const retryClientReconnect = vi.fn(() => Promise.reject("permission_denied"));
  render(
    <App
      api={fakeApi({ clientLink, retryClientReconnect })}
      subscribe={fakeSubscribe().subscribe}
    />,
  );
  expect(await screen.findByText(/the host's key is missing/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Try connecting again" }));
  expect((await screen.findByRole("alert")).textContent).toContain("do not have permission");
});

test("explains BlueRoute plainly and shows health as a labelled badge", async () => {
  const degraded = fakeApi({
    overview: () =>
      Promise.resolve({
        local_node: "node-1",
        network: "net-1",
        health: "degraded" as const,
        adapter_available: true,
        can_host: true,
        can_join: null,
        authenticated_remote_peers: null,
        internet_sharing_available: false,
      }),
  });
  render(<App api={degraded} subscribe={fakeSubscribe().subscribe} />);
  expect(await screen.findByText(/share a small private network over Bluetooth/)).toBeTruthy();
  const badge = screen.getByText("Working, with problems");
  expect(badge.className).toContain("badge-warning");
});
