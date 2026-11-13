// @vitest-environment jsdom
import React, { StrictMode } from "react";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { useDaemon } from "./useDaemon";
import type { DaemonApi } from "./useDaemon";
import type { DaemonSignal, SubscribeToSignals } from "./signals";

// CR-F6 (docs/CODE_REVIEW_REMEDIATION_TODO.md, UI-1..UI-3). On baseline 4ca5042 StrictMode replay
// leaked the first subscription ([0, 1] stops). Each effect run now owns its own resources.

afterEach(cleanup);

/** One subscribe() call: its signal callback, how to settle it, and how often it was stopped. */
interface Setup {
  onSignal: (signal: DaemonSignal) => void;
  resolve: () => void;
  reject: () => void;
  stops: number;
}

/** A subscribe whose setups settle only when the test says so. */
function deferredSubscribe() {
  const setups: Setup[] = [];
  const subscribe: SubscribeToSignals = (onSignal) =>
    new Promise<() => void>((resolve, reject) => {
      const setup: Setup = {
        onSignal,
        resolve: () =>
          resolve(() => {
            setup.stops += 1;
          }),
        reject: () => reject(new Error("no events")),
        stops: 0,
      };
      setups.push(setup);
    });
  /** Setups that resolved and have not been stopped: live listeners. */
  const live = () => setups.filter((setup) => setup.stops === 0);
  return { subscribe, setups, live };
}

/** An API whose reads are counted and settle when the test says so, naming its network. */
function deferredApi(label: string) {
  const waiting: Array<() => void> = [];
  let reads = 0;
  const later = <T,>(value: T) =>
    new Promise<T>((resolve) => {
      waiting.push(() => resolve(value));
    });
  const api: DaemonApi = {
    platformCheck: () => {
      reads += 1;
      // The probe shows only the network list; the other snapshots are opaque here.
      return later({} as never);
    },
    overview: () => later({} as never),
    networks: () => later([{ id: "n", name: label, member_count: 1 }]),
    devices: () => later([]),
    clientLink: () => later(null),
    createNetwork: () => Promise.resolve(),
    leaveNetwork: () => Promise.resolve(),
    retryClientReconnect: () => Promise.resolve(),
    setDeviceName: () => Promise.resolve(),
  };
  return {
    api,
    reads: () => reads,
    /** Settle every read started so far. */
    settle: async () => {
      await act(async () => {
        for (const settle of waiting.splice(0)) settle();
      });
    },
  };
}

/** Shows the hook's current state, so tests check visible data and not only counts. */
function Probe({ api, subscribe }: { api: DaemonApi; subscribe: SubscribeToSignals }) {
  const { view, link } = useDaemon(api, subscribe);
  const network = view.phase === "ready" ? (view.snapshot.networks[0]?.name ?? "") : "";
  return (
    <output>
      {view.phase}:{network}:{link}
    </output>
  );
}

const shown = () => screen.getByRole("status").textContent;

function strict(api: DaemonApi, subscribe: SubscribeToSignals) {
  return render(
    <StrictMode>
      <Probe api={api} subscribe={subscribe} />
    </StrictMode>,
  );
}

for (const order of ["in order", "in reverse"] as const) {
  test(`StrictMode replay leaves one listener and releases both on unmount (${order})`, async () => {
    const { subscribe, setups, live } = deferredSubscribe();
    const { api } = deferredApi("Lab");
    const view = strict(api, subscribe);
    // StrictMode ran setup, cleanup, setup: two subscriptions are pending.
    expect(setups.length).toBe(2);
    const settling = order === "in order" ? [0, 1] : [1, 0];
    await act(async () => {
      for (const index of settling) setups[index]?.resolve();
    });
    // The replayed (first) setup released its late result at once; the current one is live.
    expect(setups.map((setup) => setup.stops)).toEqual([1, 0]);
    expect(live()).toHaveLength(1);
    view.unmount();
    expect(setups.map((setup) => setup.stops)).toEqual([1, 1]);
  });
}

test("a disposed generation's late signals and reads change nothing and start no reads", async () => {
  const { subscribe, setups } = deferredSubscribe();
  const lab = deferredApi("Lab");
  const view = strict(lab.api, subscribe);
  await act(async () => {
    for (const setup of setups) setup.resolve();
  });
  await lab.settle();
  expect(shown()).toBe("ready:Lab:unknown");
  const readsBefore = lab.reads();

  // The replayed generation's callback still exists; it must be inert.
  act(() => setups[0]?.onSignal("disconnected"));
  expect(shown()).toBe("ready:Lab:unknown");
  expect(lab.reads()).toBe(readsBefore);

  // The live generation's callback works.
  act(() => setups[1]?.onSignal("changed"));
  expect(shown()).toBe("ready:Lab:connected");
  expect(lab.reads()).toBe(readsBefore + 1);
  await lab.settle();

  // After unmount, late signals and completions do nothing.
  view.unmount();
  act(() => setups[1]?.onSignal("changed"));
  expect(lab.reads()).toBe(readsBefore + 1);
});

test("reads pending at unmount complete without effect", async () => {
  const { subscribe, setups } = deferredSubscribe();
  const lab = deferredApi("Lab");
  const view = render(<Probe api={lab.api} subscribe={subscribe} />);
  await act(async () => setups[0]?.resolve());
  view.unmount();
  expect(setups[0]?.stops).toBe(1);
  const reads = lab.reads();
  await lab.settle();
  expect(lab.reads()).toBe(reads);
});

test("rapid remounts keep exactly one listener while mounted and none after", async () => {
  const { subscribe, setups, live } = deferredSubscribe();
  const { api } = deferredApi("Lab");
  for (let round = 0; round < 3; round += 1) {
    render(<Probe api={api} subscribe={subscribe} />).unmount();
  }
  const view = render(<Probe api={api} subscribe={subscribe} />);
  await act(async () => {
    for (const setup of setups) setup.resolve();
  });
  expect(live()).toHaveLength(1);
  expect(setups.slice(0, 3).map((setup) => setup.stops)).toEqual([1, 1, 1]);
  view.unmount();
  expect(live()).toHaveLength(0);
});

test("changing api and subscribe moves to the new generation; the old one cannot interfere", async () => {
  const first = deferredSubscribe();
  const second = deferredSubscribe();
  const lab = deferredApi("Lab");
  const office = deferredApi("Office");
  const view = render(<Probe api={lab.api} subscribe={first.subscribe} />);
  await act(async () => first.setups[0]?.resolve());
  // Lab's first read is still pending when the dependencies change.
  view.rerender(<Probe api={office.api} subscribe={second.subscribe} />);
  expect(first.setups[0]?.stops).toBe(1);
  await act(async () => second.setups[0]?.resolve());
  await office.settle();
  expect(shown()).toBe("ready:Office:unknown");

  // The obsolete read finishing late does not overwrite the current view …
  const labReads = lab.reads();
  await lab.settle();
  expect(shown()).toBe("ready:Office:unknown");
  // … and the old subscription's callback neither changes the link nor reads anything.
  act(() => first.setups[0]?.onSignal("disconnected"));
  expect(shown()).toBe("ready:Office:unknown");
  expect(lab.reads()).toBe(labReads);

  // The obsolete pending load did not block the new generation's coalescing.
  const officeReads = office.reads();
  act(() => second.setups[0]?.onSignal("changed"));
  expect(office.reads()).toBe(officeReads + 1);
  view.unmount();
  expect(second.setups[0]?.stops).toBe(1);
});

test("a rejected subscription leaves the page working and nothing to release", async () => {
  const { subscribe, setups } = deferredSubscribe();
  const lab = deferredApi("Lab");
  const view = render(<Probe api={lab.api} subscribe={subscribe} />);
  await act(async () => setups[0]?.reject());
  await lab.settle();
  expect(shown()).toBe("ready:Lab:unknown");
  view.unmount();
  expect(setups[0]?.stops).toBe(0);
});

test("a burst of signals during a load costs at most one more read", async () => {
  const { subscribe, setups } = deferredSubscribe();
  const lab = deferredApi("Lab");
  render(<Probe api={lab.api} subscribe={subscribe} />);
  await act(async () => setups[0]?.resolve());
  // The first load is in flight.
  expect(lab.reads()).toBe(1);
  act(() => {
    for (let index = 0; index < 20; index += 1) setups[0]?.onSignal("changed");
  });
  expect(lab.reads()).toBe(1);
  await lab.settle(); // first load done; one coalesced follow-up starts
  expect(lab.reads()).toBe(2);
  await lab.settle();
  expect(lab.reads()).toBe(2);
  expect(shown()).toBe("ready:Lab:connected");
});
