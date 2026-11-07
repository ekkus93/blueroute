import { beforeEach, expect, test, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const { daemon } = await import("./daemon");

beforeEach(() => {
  invoke.mockReset();
});

test("each snapshot maps to exactly one read-only Tauri command", async () => {
  invoke.mockResolvedValue([]);
  await daemon.platformCheck();
  await daemon.overview();
  await daemon.networks();
  await daemon.devices();
  expect(invoke.mock.calls).toEqual([["platform_check"], ["overview"], ["networks"], ["devices"]]);
});

test("typed daemon errors reach callers unchanged", async () => {
  invoke.mockImplementation(() => Promise.reject("daemon_unavailable"));
  const outcome = await daemon.overview().then(
    () => "resolved",
    (error: unknown) => error,
  );
  expect(outcome).toBe("daemon_unavailable");
});
