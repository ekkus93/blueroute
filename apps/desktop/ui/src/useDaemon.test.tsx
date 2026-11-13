// @vitest-environment jsdom
import React, { StrictMode } from "react";
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { useDaemon } from "./useDaemon";
import type { DaemonApi } from "./useDaemon";
import type { DaemonSignal, SubscribeToSignals } from "./signals";

afterEach(cleanup);

const idleApi: DaemonApi = {
  platformCheck: () => new Promise(() => {}),
  overview: () => new Promise(() => {}),
  networks: () => new Promise(() => {}),
  devices: () => new Promise(() => {}),
  clientLink: () => new Promise(() => {}),
  createNetwork: () => Promise.resolve(),
  leaveNetwork: () => Promise.resolve(),
  retryClientReconnect: () => Promise.resolve(),
  setDeviceName: () => Promise.resolve(),
};

/** A subscribe whose setup resolves only when the test says so, counting stops per setup. */
function deferredSubscribe() {
  const resolvers: Array<() => void> = [];
  const stops: number[] = [];
  const subscribe: SubscribeToSignals = (_onSignal: (signal: DaemonSignal) => void) => {
    const index = stops.length;
    stops.push(0);
    return new Promise<() => void>((resolve) => {
      resolvers.push(() =>
        resolve(() => {
          stops[index] = (stops[index] ?? 0) + 1;
        }),
      );
    });
  };
  return { subscribe, resolvers, stops };
}

function Probe({ api, subscribe }: { api: DaemonApi; subscribe: SubscribeToSignals }) {
  useDaemon(api, subscribe);
  return null;
}

// CR-F6 regression (docs/CODE_REVIEW_REMEDIATION_TODO.md). It states the corrected contract and
// fails on baseline 4ca5042, where StrictMode replay leaks the first subscription ([0, 1]).
// CR-F6 changes `test.fails` to `test`.
test.fails(
  "CR-F6: every subscription is released exactly once after StrictMode replay and unmount",
  async () => {
    const { subscribe, resolvers, stops } = deferredSubscribe();
    const view = render(
      <StrictMode>
        <Probe api={idleApi} subscribe={subscribe} />
      </StrictMode>,
    );
    // StrictMode ran setup, cleanup, setup: two subscriptions are pending.
    expect(resolvers.length).toBe(2);
    // Both resolve late, after the replay.
    await act(async () => {
      for (const resolve of resolvers) resolve();
    });
    view.unmount();
    expect(stops).toEqual([1, 1]);
  },
);
