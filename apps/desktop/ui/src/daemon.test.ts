import { beforeEach, expect, test, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const { daemon } = await import("./daemon");

beforeEach(() => {
  invoke.mockReset();
});

test("each call maps to exactly one fixed Tauri command", async () => {
  invoke.mockResolvedValue([]);
  await daemon.platformCheck();
  await daemon.overview();
  await daemon.networks();
  await daemon.devices();
  await daemon.clientLink();
  expect(invoke.mock.calls).toEqual([
    ["platform_check"],
    ["overview"],
    ["networks"],
    ["devices"],
    ["client_link"],
  ]);
});

test("renaming passes only the name to one fixed command", async () => {
  invoke.mockResolvedValue(undefined);
  await daemon.setDeviceName("Kitchen laptop");
  expect(invoke.mock.calls).toEqual([["set_device_name", { name: "Kitchen laptop" }]]);
});

test("retrying the client link takes no arguments and maps to one fixed command", async () => {
  invoke.mockResolvedValue(undefined);
  await daemon.retryClientReconnect();
  expect(invoke.mock.calls).toEqual([["retry_client_reconnect"]]);
});

test("leaving takes no arguments and maps to one fixed command", async () => {
  invoke.mockResolvedValue(undefined);
  await daemon.leaveNetwork();
  expect(invoke.mock.calls).toEqual([["leave_network"]]);
});

test("typed daemon errors reach callers unchanged", async () => {
  invoke.mockImplementation(() => Promise.reject("daemon_unavailable"));
  const outcome = await daemon.overview().then(
    () => "resolved",
    (error: unknown) => error,
  );
  expect(outcome).toBe("daemon_unavailable");
});
