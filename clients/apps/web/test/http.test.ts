import { afterEach, describe, expect, mock, test } from "bun:test";

// The real @reel/ui barrel pulls in Solid components; http.ts only needs serverUrl.
mock.module("@reel/ui", () => ({ serverUrl: (p: string) => `http://reel.test${p}` }));
const { ApiError, classifyFailure, isAborted, request } = await import("../src/api/http");

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
});

/** Replace fetch with a script of outcomes: a Response, or an Error to throw. */
function script(...outcomes: (Response | Error | ((init: RequestInit) => Promise<Response>))[]): { calls: number } {
  const state = { calls: 0 };
  globalThis.fetch = ((_url: string, init: RequestInit) => {
    const o = outcomes[Math.min(state.calls++, outcomes.length - 1)]!;
    if (typeof o === "function") return o(init);
    return o instanceof Error ? Promise.reject(o) : Promise.resolve(o);
  }) as typeof fetch;
  return state;
}
const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status, headers: { "Content-Type": "application/json" } });

describe("classifyFailure", () => {
  const base = { callerAborted: false, unloading: false, timedOut: false, online: true };
  test("plain connection failure is `network`", () => expect(classifyFailure(base)).toBe("network"));
  test("caller abort and page unload are `aborted`, whatever else happened", () => {
    expect(classifyFailure({ ...base, callerAborted: true, timedOut: true })).toBe("aborted");
    expect(classifyFailure({ ...base, unloading: true, online: false })).toBe("aborted");
  });
  test("no network beats timeout", () => {
    expect(classifyFailure({ ...base, online: false, timedOut: true })).toBe("offline");
    expect(classifyFailure({ ...base, timedOut: true })).toBe("timeout");
  });
});

describe("request", () => {
  test("a read is retried once after a network blip", async () => {
    const s = script(new TypeError("Failed to fetch"), json({ ok: 1 }));
    expect(await request<{ ok: number }>("GET", "/x")).toEqual({ ok: 1 });
    expect(s.calls).toBe(2);
  });

  test("two network failures become a status-0 `network` error", async () => {
    const s = script(new TypeError("Failed to fetch"));
    const e = await request("GET", "/x").catch((x) => x);
    expect(e).toBeInstanceOf(ApiError);
    expect(e.status).toBe(0);
    expect(e.network).toBe("network");
    expect(e.message).toBe("Can't reach Reel");
    expect(isAborted(e)).toBe(false);
    expect(s.calls).toBe(2);
  });

  test("writes aren't retried", async () => {
    const s = script(new TypeError("Failed to fetch"), json({}));
    const e = await request("POST", "/x", { body: {} }).catch((x) => x);
    expect(e.status).toBe(0);
    expect(s.calls).toBe(1);
  });

  test("a request the caller cancels is `aborted` and isn't retried", async () => {
    const ctl = new AbortController();
    const s = script(
      (init) =>
        new Promise((_, rej) => {
          init.signal?.addEventListener("abort", () => rej(new DOMException("aborted", "AbortError")));
        }),
    );
    const p = request("GET", "/x", { signal: ctl.signal }).catch((x) => x);
    ctl.abort();
    const e = await p;
    expect(isAborted(e)).toBe(true);
    expect(s.calls).toBe(1);
  });

  test("an already-cancelled signal never fetches", async () => {
    const s = script(json({}));
    const ctl = new AbortController();
    ctl.abort();
    expect(isAborted(await request("GET", "/x", { signal: ctl.signal }).catch((x) => x))).toBe(true);
    expect(s.calls).toBe(0);
  });

  test("server errors keep their status and message, no retry", async () => {
    const s = script(json({ error: "provider down" }, 503));
    const e = await request("GET", "/x").catch((x) => x);
    expect(e.status).toBe(503);
    expect(e.message).toBe("provider down");
    expect(e.network).toBeUndefined();
    expect(s.calls).toBe(1);
  });

  test("a 5xx without a body gets a generic message", async () => {
    script(new Response("oops", { status: 500 }));
    const e = await request("GET", "/x").catch((x) => x);
    expect(e.message).toBe("Server error (500)");
  });
});
