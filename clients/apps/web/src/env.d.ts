interface ImportMetaEnv {
  readonly VITE_REEL_URL?: string;
  /** "1" to run against built-in fixture data. */
  readonly VITE_MOCK?: string;
  /** "tv" for webOS/Tizen builds (forces 10-foot mode). */
  readonly VITE_TARGET?: string;
  readonly DEV: boolean;
}
interface ImportMeta {
  readonly env: ImportMetaEnv;
}
