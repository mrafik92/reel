/** Minimal ambient types for Samsung Tizen TV globals (`tizen`, `webapis`). All access must be guarded. */
type AVPlayState = "NONE" | "IDLE" | "READY" | "PLAYING" | "PAUSED";
interface AVPlayTrackInfo {
  index: number;
  type: "VIDEO" | "AUDIO" | "TEXT" | string;
  /** JSON string, e.g. {"language":"eng","channels":"6","fourCC":"EAC3"} */
  extra_info: string;
}
interface AVPlayListener {
  onbufferingstart?: () => void;
  onbufferingprogress?: (percent: number) => void;
  onbufferingcomplete?: () => void;
  oncurrentplaytime?: (ms: number) => void;
  onevent?: (type: string, data: unknown) => void;
  onstreamcompleted?: () => void;
  onerror?: (type: string) => void;
  onsubtitlechange?: (durationMs: string | number, text: string, type?: unknown, attrs?: unknown) => void;
  ondrmevent?: (type: string, data: unknown) => void;
}
interface AVPlay {
  open(url: string): void;
  close(): void;
  prepare(): void;
  prepareAsync(ok?: () => void, err?: (e: unknown) => void): void;
  play(): void;
  pause(): void;
  stop(): void;
  seekTo(ms: number, ok?: () => void, err?: (e: unknown) => void): void;
  jumpForward(ms: number, ok?: () => void, err?: (e: unknown) => void): void;
  jumpBackward(ms: number, ok?: () => void, err?: (e: unknown) => void): void;
  getDuration(): number;
  getCurrentTime(): number;
  getState(): AVPlayState;
  setDisplayRect(x: number, y: number, w: number, h: number): void;
  setDisplayMethod(m: string): void;
  setListener(l: AVPlayListener): void;
  getTotalTrackInfo(): AVPlayTrackInfo[];
  getCurrentStreamInfo(): AVPlayTrackInfo[];
  setSelectTrack(type: "AUDIO" | "TEXT" | "VIDEO", index: number): void;
  setSilentSubtitle(silent: boolean): void;
  setStreamingProperty(prop: string, value: string): void;
  setTimeout(ms: number): void;
  suspend(): void;
  restore(url?: string, resumeTimeMs?: number, bufferingTimeoutMs?: number): void;
}
interface TizenWebApis {
  avplay?: AVPlay;
  productinfo?: {
    isUdPanelSupported?: () => boolean;
    is8KPanelSupported?: () => boolean;
    getModel?: () => string;
    getFirmware?: () => string;
    getRealModel?: () => string;
  };
  avinfo?: { isHdrTvSupport?: () => boolean };
  [k: string]: unknown;
}
interface TizenGlobal {
  tvinputdevice?: {
    registerKeyBatch?: (names: string[], ok?: () => void, err?: (e: unknown) => void) => void;
    getSupportedKeys?: () => Array<{ name: string; code: number }>;
  };
  application?: { getCurrentApplication?: () => { exit: () => void; hide?: () => void } };
  [k: string]: unknown;
}
declare var tizen: TizenGlobal | undefined;
declare var webapis: TizenWebApis | undefined;
interface Window {
  tizen?: TizenGlobal;
  webapis?: TizenWebApis;
}
