interface ImportMetaEnv {
  readonly VITE_REEL_URL?: string;
  /** "1" to run against built-in fixture data. */
  readonly VITE_MOCK?: string;
  /** "tv" for webOS/Tizen builds (forces 10-foot mode). */
  readonly VITE_TARGET?: string;
  /** "web" (default), "webos" or "tizen" — reported to the server on login and decide. Unset, the
   *  runtime detection decides (`android-web` inside the Reel Web Android shell). */
  readonly VITE_CLIENT?: string;
  /** Git commit + build time of this bundle (set by the image build). */
  readonly VITE_GIT_SHA?: string;
  readonly VITE_BUILD_DATE?: string;
  readonly DEV: boolean;
}
interface ImportMeta {
  readonly env: ImportMetaEnv;
}
