import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { ReelEvents } from "../src/events";

// #28: the server refuses `?token=`, and a session token in a URL ends up in proxy/edge logs.
// The client must only ever open `/events` with a ticket, and retry (not fall back) when the
// ticket request fails.

const TOKEN = "session-token-secret";
let opened: string[];
let fetchImpl: () => Promise<Response>;
const realFetch = globalThis.fetch;
const realWs = (globalThis as { WebSocket?: unknown }).WebSocket;

class FakeWs {
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((m: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(url: string) {
    opened.push(url);
  }
  close() {
    this.readyState = 3;
  }
}

const settle = (ms = 30) => new Promise((r) => setTimeout(r, ms));

beforeEach(() => {
  opened = [];
  (globalThis as { WebSocket?: unknown }).WebSocket = FakeWs;
  globalThis.fetch = (() => fetchImpl()) as unknown as typeof fetch;
});

afterEach(() => {
  globalThis.fetch = realFetch;
  (globalThis as { WebSocket?: unknown }).WebSocket = realWs;
});

const client = () => new ReelEvents({ baseUrl: "http://reel.test", token: TOKEN, minBackoffMs: 5, maxBackoffMs: 10 });

describe("ReelEvents auth", () => {
  test("opens the socket with a ticket, never the session token", async () => {
    fetchImpl = async () => new Response(JSON.stringify({ ticket: "t1", expires_in_s: 60 }), { status: 200 });
    const ev = client();
    ev.connect();
    await settle();
    ev.close();
    expect(opened).toEqual(["ws://reel.test/api/v1/events?ticket=t1"]);
  });

  test("a failed ticket request retries instead of falling back to ?token=", async () => {
    let calls = 0;
    fetchImpl = async () => {
      calls++;
      if (calls === 1) throw new TypeError("network down");
      if (calls === 2) return new Response("oops", { status: 503 });
      return new Response(JSON.stringify({ ticket: "t3", expires_in_s: 60 }), { status: 200 });
    };
    const ev = client();
    ev.connect();
    await settle(150);
    ev.close();
    expect(calls).toBeGreaterThanOrEqual(3);
    expect(opened).toEqual(["ws://reel.test/api/v1/events?ticket=t3"]);
    expect(opened.some((u) => u.includes("token") || u.includes(TOKEN))).toBe(false);
  });

  test("close() during a pending retry stops reconnecting", async () => {
    fetchImpl = async () => {
      throw new TypeError("network down");
    };
    const ev = client();
    ev.connect();
    await settle(5);
    ev.close();
    await settle(60);
    expect(opened).toEqual([]);
  });
});
