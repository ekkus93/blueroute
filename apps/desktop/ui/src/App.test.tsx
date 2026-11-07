// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
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

test("shows plain-language state, never offers Internet sharing, and disables unwired actions", async () => {
  render(<App api={fakeApi()} subscribe={fakeSubscribe().subscribe} />);
  expect(await screen.findByRole("heading", { name: "Your network" })).toBeTruthy();
  expect(screen.getByText("Lab")).toBeTruthy();
  expect(screen.getByText(/Connected through another device/)).toBeTruthy();
  expect(screen.getByText(/Not reachable right now/)).toBeTruthy();
  expect(screen.getByText("Working well")).toBeTruthy();
  for (const name of ["Create a network", "Join a nearby network"]) {
    expect((screen.getByRole("button", { name }) as HTMLButtonElement).disabled).toBe(true);
  }
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
  expect(await screen.findByRole("alert")).toBeTruthy();
  expect(screen.getByText(/background service is not running/)).toBeTruthy();
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
