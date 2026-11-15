export type RequestState =
  | "requested" | "approved" | "searching" | "not_found" | "queued"
  | "downloading" | "stalled" | "importing" | "available" | "failed" | "declined";

export interface RequestProgressEvent {
  type: "request_progress";
  request_id: string;
  tmdb_id: number;
  state: RequestState;
  /** 0..1 */
  progress: number;
  eta_seconds?: number | null;
  speed_bps?: number | null;
}

export interface ScanProgressEvent {
  type: "scan_progress";
  library_id: string;
  scanned: number;
  total: number;
}

export type ReelEvent = RequestProgressEvent | ScanProgressEvent;

type Handler<T> = (e: T) => void;

export interface ReelEventsOptions {
  /** e.g. http://host:8420 or "" for same origin */
  baseUrl: string;
  token?: string;
  minBackoffMs?: number;
  maxBackoffMs?: number;
  onOpen?: () => void;
  onClose?: () => void;
}

export class ReelEvents {
  private ws?: WebSocket;
  private handlers = new Map<string, Set<Handler<any>>>();
  private anyHandlers = new Set<Handler<ReelEvent>>();
  private attempt = 0;
  private timer?: ReturnType<typeof setTimeout>;
  private stopped = true;
  /** Bumped by every open/close so a slow ticket fetch can't open a second socket. */
  private gen = 0;
  /** A ticket is being fetched for the current generation. */
  private opening = false;

  constructor(private opts: ReelEventsOptions) {}

  on<K extends ReelEvent["type"]>(type: K, h: Handler<Extract<ReelEvent, { type: K }>>): () => void {
    let set = this.handlers.get(type);
    if (!set) this.handlers.set(type, (set = new Set()));
    set.add(h);
    return () => set!.delete(h);
  }

  onAny(h: Handler<ReelEvent>): () => void {
    this.anyHandlers.add(h);
    return () => this.anyHandlers.delete(h);
  }

  connect(): void {
    if (!this.stopped) return;
    this.stopped = false;
    void this.open();
  }

  close(): void {
    this.stopped = true;
    this.gen++;
    this.opening = false;
    if (this.timer) clearTimeout(this.timer);
    this.drop();
  }

  /** The socket is open (or opening) right now. */
  get alive(): boolean {
    const ws = this.ws;
    return !!ws && (ws.readyState === 0 || ws.readyState === 1);
  }

  /**
   * Reconnect now instead of waiting for the backoff: the tab came back or the network returned.
   * `force` also replaces a socket that still looks open — after a long sleep it is often dead
   * without the browser having noticed yet.
   */
  kick(force = false): void {
    if (this.stopped) return;
    if (!force && (this.alive || this.opening)) return;
    if (this.timer) clearTimeout(this.timer);
    this.timer = undefined;
    this.attempt = 0;
    this.gen++;
    this.opening = false;
    this.drop();
    void this.open();
  }

  private drop(): void {
    const ws = this.ws;
    this.ws = undefined;
    if (!ws) return;
    ws.onopen = ws.onmessage = ws.onerror = null;
    ws.onclose = null;
    try {
      ws.close();
    } catch {
      /* ignore */
    }
    this.opts.onClose?.();
  }

  private httpBase(): string {
    return (this.opts.baseUrl || (typeof location !== "undefined" ? location.origin : "")).replace(/\/$/, "");
  }

  /** Short-lived single-use ticket, so the session token never appears in a URL (the server
   *  refuses `?token=`, #28). `undefined` when no ticket could be fetched: retry later. */
  private async url(): Promise<string | undefined> {
    const u = this.httpBase().replace(/^http/, "ws") + "/api/v1/events";
    if (!this.opts.token) return u;
    try {
      // A hung request (wedged connection, frozen tab) must not stall reconnecting forever.
      const ctl = typeof AbortController !== "undefined" ? new AbortController() : undefined;
      const t = ctl ? setTimeout(() => ctl.abort(), 10_000) : undefined;
      const r = await fetch(`${this.httpBase()}/api/v1/auth/ws-ticket`, {
        method: "POST",
        headers: { Authorization: `Bearer ${this.opts.token}` },
        signal: ctl?.signal,
      }).finally(() => t && clearTimeout(t));
      if (r.ok) {
        const { ticket } = (await r.json()) as { ticket: string };
        return `${u}?ticket=${encodeURIComponent(ticket)}`;
      }
      if (r.status === 401) return u; // not signed in (any more): the upgrade will be refused
    } catch {
      /* network hiccup: retry with backoff */
    }
    return undefined;
  }

  /** Try again after an exponential, jittered backoff. */
  private retry(): void {
    if (this.stopped) return;
    const min = this.opts.minBackoffMs ?? 500;
    const max = this.opts.maxBackoffMs ?? 15000;
    const delay = Math.min(max, min * 2 ** this.attempt++) * (0.75 + Math.random() * 0.5);
    this.timer = setTimeout(() => void this.open(), delay);
  }

  private async open(): Promise<void> {
    const gen = ++this.gen;
    this.opening = true;
    const url = await this.url();
    if (this.stopped || gen !== this.gen) return;
    this.opening = false;
    if (url === undefined) {
      this.retry();
      return;
    }
    const ws = new WebSocket(url);
    this.ws = ws;
    ws.onopen = () => {
      this.attempt = 0;
      this.opts.onOpen?.();
    };
    ws.onmessage = (m) => {
      let ev: ReelEvent;
      try {
        ev = JSON.parse(String(m.data)) as ReelEvent;
      } catch {
        return;
      }
      this.handlers.get(ev.type)?.forEach((h) => h(ev));
      this.anyHandlers.forEach((h) => h(ev));
    };
    ws.onclose = () => {
      if (ws !== this.ws) return;
      this.ws = undefined;
      this.opts.onClose?.();
      this.retry();
    };
    ws.onerror = () => ws.close();
  }
}
