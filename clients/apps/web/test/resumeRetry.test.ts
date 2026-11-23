import { afterEach, describe, expect, mock, test } from "bun:test";
import { uiMock } from "./uiMock";

mock.module("@reel/ui", uiMock);
const { ApiError } = await import("../src/api/http");
const { isResumeNetworkFailure, waitForNetwork, withResumeRetry } = await import("../src/components/live/resumeRetry");

const netErr = () => new ApiError(0, "Can't reach Reel", undefined, undefined, "network");

/** A tune request scripted as a list of outcomes: a value resolves, an Error rejects. */
function script<T>(...outcomes: (T | Error)[]): { send: () => Promise<T>; calls: () => number } {
  let n = 0;
  return {
    send: () => {
      const o = outcomes[Math.min(n++, outcomes.length - 1)]!;
      return o instanceof Error ? Promise.reject(o) : Promise.resolve(o as T);
    },
    calls: () => n,
  };
}
const noWait = () => Promise.resolve();

describe("isResumeNetworkFailure", () => {
  test("no answer (connection failed, offline) is a network failure", () => {
    expect(isResumeNetworkFailure(netErr())).toBe(true);
    expect(isResumeNetworkFailure(new ApiError(0, "You're offline", undefined, undefined, "offline"))).toBe(true);
  });
  test("a cancelled request or our own timeout is not", () => {
    expect(isResumeNetworkFailure(new ApiError(0, "Request cancelled", undefined, undefined, "aborted"))).toBe(false);
    expect(isResumeNetworkFailure(new ApiError(0, "Reel took too long to answer", undefined, undefined, "timeout"))).toBe(false);
  });
  test("any server answer is not", () => {
    for (const s of [409, 429, 500, 502, 503]) expect(isResumeNetworkFailure(new ApiError(s, "x"))).toBe(false);
    expect(isResumeNetworkFailure(new Error("boom"))).toBe(false);
  });
});

describe("withResumeRetry", () => {
  test("network error → exactly one retry", async () => {
    const s = script(netErr(), "session");
    let waited = 0;
    const r = await withResumeRetry(s.send, { wait: async () => void waited++ });
    expect(r).toBe("session");
    expect(s.calls()).toBe(2);
    expect(waited).toBe(1);
  });

  test("network error twice → the second error shows, no third try", async () => {
    const second = netErr();
    const s = script<string>(netErr(), second, "never");
    await expect(withResumeRetry(s.send, { wait: noWait })).rejects.toBe(second);
    expect(s.calls()).toBe(2);
  });

  test("server or provider error → no retry, shown at once", async () => {
    for (const status of [429, 409, 503, 500]) {
      const err = new ApiError(status, "provider says no");
      const s = script<string>(err, "never");
      let waited = 0;
      await expect(withResumeRetry(s.send, { wait: async () => void waited++ })).rejects.toBe(err);
      expect(s.calls()).toBe(1);
      expect(waited).toBe(0);
    }
  });

  test("success → no retry", async () => {
    const s = script("session");
    expect(await withResumeRetry(s.send, { wait: noWait })).toBe("session");
    expect(s.calls()).toBe(1);
  });

  test("viewer zapped or left during the wait → no second request", async () => {
    const first = netErr();
    const s = script<string>(first, "never");
    await expect(withResumeRetry(s.send, { wait: noWait, still: () => false })).rejects.toBe(first);
    expect(s.calls()).toBe(1);
  });
});

describe("waitForNetwork", () => {
  const realNav = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  const realWin = Object.getOwnPropertyDescriptor(globalThis, "window");
  afterEach(() => {
    if (realNav) Object.defineProperty(globalThis, "navigator", realNav);
    if (realWin) Object.defineProperty(globalThis, "window", realWin);
    else delete (globalThis as { window?: unknown }).window;
  });
  const setOnline = (online: boolean) => Object.defineProperty(globalThis, "navigator", { value: { onLine: online }, configurable: true });
  const fakeWindow = () => {
    const t = new EventTarget();
    Object.defineProperty(globalThis, "window", { value: t, configurable: true, writable: true });
    return t;
  };

  test("already online → a short settle pause", async () => {
    setOnline(true);
    const t0 = Date.now();
    await waitForNetwork({ settleMs: 30, capMs: 5_000 });
    expect(Date.now() - t0).toBeGreaterThanOrEqual(25);
    expect(Date.now() - t0).toBeLessThan(1_000);
  });

  test("offline → resolves on `online`", async () => {
    setOnline(false);
    const w = fakeWindow();
    const t0 = Date.now();
    const p = waitForNetwork({ settleMs: 5_000, capMs: 5_000 });
    setTimeout(() => w.dispatchEvent(new Event("online")), 20);
    await p;
    expect(Date.now() - t0).toBeLessThan(1_000);
  });

  test("offline and the network never comes back → gives up at the cap", async () => {
    setOnline(false);
    fakeWindow();
    const t0 = Date.now();
    await waitForNetwork({ settleMs: 5_000, capMs: 40 });
    expect(Date.now() - t0).toBeGreaterThanOrEqual(35);
  });
});
