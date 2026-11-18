/** Real-shaped user agents for the platform / client detection tests (#229). */
export const UA = {
  /** Reel Web shell on a phone (Android System WebView keeps the model and build). */
  shellPhone:
    "Mozilla/5.0 (Linux; Android 14; Pixel 8 Build/UQ1A.240205.004; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/124.0.0.0 Mobile Safari/537.36 ReelAndroidWeb/0.1.0",
  /** Reel Web shell on the phone emulator. */
  shellEmulator:
    "Mozilla/5.0 (Linux; Android 14; sdk_gphone64_arm64 Build/UE1A.230829.036; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/113.0.5672.136 Mobile Safari/537.36 ReelAndroidWeb/0.1.0-debug",
  /** Reel Web shell on Android TV (UI mode television). */
  shellTv:
    "Mozilla/5.0 (Linux; Android 12; BRAVIA 4K VH2 Build/STT1.211025.001.Z4; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/124.0.0.0 Safari/537.36 ReelAndroidWeb/0.1.0 (TV)",
  /** Shell on a TV whose user agent carries no model. */
  shellTvNoModel: "Mozilla/5.0 (Linux; Android 12; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/124.0.0.0 Safari/537.36 ReelAndroidWeb/0.2.0 (TV)",
  /** Phone shell on a device whose model name says TV: the token, not the model, decides. */
  shellPhoneTvModel:
    "Mozilla/5.0 (Linux; Android 13; SHIELD Android TV Build/TQ3A; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/124.0.0.0 Mobile Safari/537.36 ReelAndroidWeb/0.1.0",
  /** Plain Chrome on an Android phone (reduced user agent: no model). */
  chromeAndroid: "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Mobile Safari/537.36",
  /** A plain browser on Android TV, and on a Fire TV. */
  androidTvBrowser:
    "Mozilla/5.0 (Linux; Android 11; SHIELD Android TV Build/RQ1A.210105.003; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/124.0.0.0 Safari/537.36",
  fireTv: "Mozilla/5.0 (Linux; Android 11; AFTMM Build/RS8101.1849N; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/124.0.0.0 Safari/537.36",
  webos: "Mozilla/5.0 (Web0S; Linux/SmartTV) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/87.0.4280.88 Safari/537.36 WebAppManager",
  tizen: "Mozilla/5.0 (SMART-TV; LINUX; Tizen 6.0) AppleWebKit/537.36 (KHTML, like Gecko) 76.0.3809.146/6.0 TV Safari/537.36",
  desktopChrome: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
  desktopSafari: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15",
};
