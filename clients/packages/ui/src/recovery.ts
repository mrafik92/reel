/**
 * What a player does after a fatal playback error (#91, #93). Pure logic, no hls.js or DOM, so it
 * can be tested on its own; `HlsJsAdapter` carries the actions out.
 *
 * - Network errors (timeouts, aborted or truncated responses, HTTP/3 resets, 404s of live segments
 *   that already rotated out): load again after a short backoff, at the playhead for VOD and at the
 *   live edge for live. Three in a row → rebuild the player.
 * - Media errors (MSE append or parse failures): `recoverMediaError()`, then once more with
 *   `swapAudioCodec()`, then rebuild the player.
 * - Anything else: rebuild the player at the playhead (VOD) or the live edge (live).
 * - After `maxAttempts` attempts without a healthy stretch of playback in between: give up, and the
 *   app shows its error screen.
 */

export type FailureKind = "network" | "media" | "other";

export type RecoveryAction =
  /** Restart fragment loading. `position` -1 = live edge. */
  | { kind: "startLoad"; position: number }
  | { kind: "recoverMedia"; swapAudio: boolean }
  /** Tear the player down and load the stream again. `position` -1 = live edge. */
  | { kind: "reload"; position: number }
  | { kind: "giveUp" };

export interface RecoveryStep {
  action: RecoveryAction;
  /** Wait this long before acting (backoff). */
  delayMs: number;
  /** 1-based attempt number within the current streak. */
  attempt: number;
}

export interface RecoveryOptions {
  maxAttempts?: number;
  /** Clock (ms), for tests. */
  now?: () => number;
}

/** Two media errors closer together than this escalate (swap audio codec, then rebuild). */
const MEDIA_ESCALATE_MS = 15_000;
/** Consecutive network failures before the player is rebuilt instead of just reloading. */
const NETWORK_BEFORE_RELOAD = 3;

export class RecoveryPolicy {
  private attempts = 0;
  private networkRun = 0;
  private mediaRun = 0;
  private lastMediaAt = -Infinity;
  private readonly max: number;
  private readonly now: () => number;

  constructor(o: RecoveryOptions = {}) {
    this.max = o.maxAttempts ?? 8;
    this.now = o.now ?? (() => Date.now());
  }

  /** Attempts used in the current streak. */
  get used(): number {
    return this.attempts;
  }

  /** Playback ran fine for a while: the next failure starts a fresh streak. */
  healthy(): void {
    this.attempts = 0;
    this.networkRun = 0;
    this.mediaRun = 0;
    this.lastMediaAt = -Infinity;
  }

  next(kind: FailureKind, ctx: { live: boolean; position: number }): RecoveryStep {
    if (this.attempts >= this.max) return { action: { kind: "giveUp" }, delayMs: 0, attempt: this.attempts };
    const attempt = ++this.attempts;
    const at = ctx.live ? -1 : Math.max(0, ctx.position);
    if (kind === "network") {
      this.mediaRun = 0;
      const n = ++this.networkRun;
      const delayMs = Math.min(1000 * 2 ** (n - 1), 8000);
      if (n >= NETWORK_BEFORE_RELOAD) {
        this.networkRun = 0;
        return { action: { kind: "reload", position: at }, delayMs, attempt };
      }
      return { action: { kind: "startLoad", position: at }, delayMs, attempt };
    }
    if (kind === "media") {
      this.networkRun = 0;
      const t = this.now();
      this.mediaRun = t - this.lastMediaAt < MEDIA_ESCALATE_MS ? this.mediaRun + 1 : 1;
      this.lastMediaAt = t;
      if (this.mediaRun === 1) return { action: { kind: "recoverMedia", swapAudio: false }, delayMs: 0, attempt };
      if (this.mediaRun === 2) return { action: { kind: "recoverMedia", swapAudio: true }, delayMs: 0, attempt };
      this.mediaRun = 0;
      return { action: { kind: "reload", position: at }, delayMs: 500, attempt };
    }
    this.networkRun = 0;
    this.mediaRun = 0;
    return { action: { kind: "reload", position: at }, delayMs: 1000, attempt };
  }
}

/** hls.js error type → failure kind (`networkError`, `mediaError`, everything else). */
export function failureKind(hlsType: string | undefined): FailureKind {
  if (hlsType === "networkError") return "network";
  if (hlsType === "mediaError") return "media";
  return "other";
}

/** Human line for the "Reconnecting…" overlay. */
export function recoveryReason(kind: FailureKind, live: boolean): string {
  if (kind === "network") return live ? "Catching up with the live stream" : "Reconnecting to the server";
  if (kind === "media") return "Restarting the video decoder";
  return "Reloading the stream";
}

/**
 * hls.js settings. VOD keeps ~90 s ahead so a bitrate peak (11–13 Mbit/s on a 6 Mbit/s average,
 * #91) doesn't drain the buffer on a slower link; it waits longer for slow segments. Live keeps
 * close to the edge and jumps back to it instead of falling out of the server's window (#93).
 */
export function hlsConfig(o: { live: boolean; startPosition: number }): Record<string, unknown> {
  const retry = (n: number, base = 1000, max = 8000) => ({ maxNumRetry: n, retryDelayMs: base, maxRetryDelayMs: max });
  const policy = (ttfb: number, load: number, timeouts: number, errors: number) => ({
    default: {
      maxTimeToFirstByteMs: ttfb,
      maxLoadTimeMs: load,
      timeoutRetry: retry(timeouts, 0, 0),
      errorRetry: retry(errors),
    },
  });
  const common = {
    startPosition: o.startPosition,
    // Every playlist/init/segment request: retry with backoff before anything goes fatal.
    manifestLoadPolicy: policy(10_000, 20_000, 2, 3),
    playlistLoadPolicy: policy(10_000, 20_000, 2, o.live ? 6 : 3),
    // Recover from a stall by nudging over small gaps instead of waiting forever.
    nudgeMaxRetry: 5,
  };
  if (o.live) {
    return {
      ...common,
      fragLoadPolicy: policy(10_000, 20_000, 2, 2),
      // 3 segments behind the edge; more than 10 behind → jump forward (the window is longer).
      liveSyncDurationCount: 3,
      liveMaxLatencyDurationCount: 10,
      maxBufferLength: 30,
      backBufferLength: 30,
      lowLatencyMode: false,
    };
  }
  return {
    ...common,
    // A 4 s segment at 13 Mbit/s over a 2–3 Mbit/s link takes 20–30 s: give it time.
    fragLoadPolicy: policy(15_000, 60_000, 3, 6),
    maxBufferLength: 90,
    maxMaxBufferLength: 180,
    maxBufferSize: 200 * 1000 * 1000,
    backBufferLength: 60,
  };
}
