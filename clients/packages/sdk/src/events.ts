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
    this.open();
  }

  close(): void {
    this.stopped = true;
    if (this.timer) clearTimeout(this.timer);
    this.ws?.close();
    this.ws = undefined;
  }

  private url(): string {
    const base = this.opts.baseUrl || (typeof location !== "undefined" ? location.origin : "");
    const u = base.replace(/^http/, "ws").replace(/\/$/, "") + "/api/v1/events";
    return this.opts.token ? `${u}?token=${encodeURIComponent(this.opts.token)}` : u;
  }

  private open(): void {
    const ws = new WebSocket(this.url());
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
      this.opts.onClose?.();
      if (this.stopped) return;
      const min = this.opts.minBackoffMs ?? 500;
      const max = this.opts.maxBackoffMs ?? 15000;
      const delay = Math.min(max, min * 2 ** this.attempt++) * (0.75 + Math.random() * 0.5);
      this.timer = setTimeout(() => this.open(), delay);
    };
    ws.onerror = () => ws.close();
  }
}
