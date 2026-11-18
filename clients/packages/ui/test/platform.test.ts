import { describe, expect, test } from "bun:test";
import {
  androidModel,
  clientNameFor,
  detectClientName,
  detectPlatform,
  deviceNameFor,
  presetFor,
  reelShell,
  shellDeviceName,
} from "../src/platform/detect";
import { isTvUserAgent } from "../src/navigation";
import { UA } from "./ua-fixtures";
import { normalizeKey, exitApp, registerTizenKeys } from "../src/platform/keys";
import { normalizeServerUrl } from "../src/platform/server";
import { parseVtt } from "../src/platform/vtt";
import { deviceProfileFromInfo, detectTvInfo } from "../src/profile";

describe("detectPlatform", () => {
  test("webOS via PalmSystem / webOS.platform / UA", () => {
    expect(detectPlatform({ PalmSystem: {} })).toBe("webos");
    expect(detectPlatform({ webOS: { platform: { tv: true } } })).toBe("webos");
    expect(detectPlatform({ navigator: { userAgent: "Mozilla/5.0 (Web0S; Linux/SmartTV)" } })).toBe("webos");
  });
  test("Tizen via global / UA", () => {
    expect(detectPlatform({ tizen: {} })).toBe("tizen");
    expect(detectPlatform({ navigator: { userAgent: "Mozilla/5.0 (SMART-TV; Linux; Tizen 6.0)" } })).toBe("tizen");
  });
  test("Android TV and desktop", () => {
    expect(detectPlatform({ navigator: { userAgent: "Mozilla/5.0 (Linux; Android 11; AFTMM) wv" } })).toBe("android-tv");
    expect(detectPlatform({ navigator: { userAgent: "Mozilla/5.0 (Linux; Android 13; Pixel 7) Mobile" } })).toBe("web");
    expect(detectPlatform({ navigator: { userAgent: "Mozilla/5.0 (Macintosh) Chrome/120" } })).toBe("web");
    expect(clientNameFor("android-tv")).toBe("web");
  });
});

describe("Reel Web Android shell (#229)", () => {
  const nav = (userAgent: string) => ({ navigator: { userAgent } });
  test("reads the user-agent token", () => {
    expect(reelShell(UA.shellPhone)).toEqual({ version: "0.1.0", tv: false });
    expect(reelShell(UA.shellEmulator)).toEqual({ version: "0.1.0-debug", tv: false });
    expect(reelShell(UA.shellTv)).toEqual({ version: "0.1.0", tv: true });
    for (const ua of [UA.chromeAndroid, UA.androidTvBrowser, UA.webos, UA.tizen, UA.desktopChrome, ""]) expect(reelShell(ua)).toBeNull();
  });
  test("phone shell: web platform, android-web client, not TV", () => {
    for (const ua of [UA.shellPhone, UA.shellEmulator, UA.shellPhoneTvModel]) {
      expect(detectPlatform(nav(ua))).toBe("web");
      expect(detectClientName(nav(ua))).toBe("android-web");
      expect(isTvUserAgent(ua)).toBe(false);
    }
  });
  test("TV shell: android-tv platform, android-web client, TV mode", () => {
    for (const ua of [UA.shellTv, UA.shellTvNoModel]) {
      expect(detectPlatform(nav(ua))).toBe("android-tv");
      expect(detectClientName(nav(ua))).toBe("android-web");
      expect(isTvUserAgent(ua)).toBe(true);
    }
  });
  test("plain Chrome on Android stays a web browser", () => {
    expect(detectPlatform(nav(UA.chromeAndroid))).toBe("web");
    expect(detectClientName(nav(UA.chromeAndroid))).toBe("web");
    expect(isTvUserAgent(UA.chromeAndroid)).toBe(false);
  });
  test("plain Android TV browsers, webOS and Tizen behave as before", () => {
    expect(detectPlatform(nav(UA.androidTvBrowser))).toBe("android-tv");
    expect(detectClientName(nav(UA.androidTvBrowser))).toBe("web");
    expect(isTvUserAgent(UA.androidTvBrowser)).toBe(false);
    expect(isTvUserAgent(UA.fireTv)).toBe(true);
    expect(detectPlatform(nav(UA.webos))).toBe("webos");
    expect(detectClientName(nav(UA.webos))).toBe("webos");
    expect(isTvUserAgent(UA.webos)).toBe(true);
    expect(detectPlatform(nav(UA.tizen))).toBe("tizen");
    expect(detectClientName(nav(UA.tizen))).toBe("tizen");
    expect(isTvUserAgent(UA.tizen)).toBe(true);
    expect(isTvUserAgent(UA.desktopChrome)).toBe(false);
  });
  test("device model and name", () => {
    expect(androidModel(UA.shellPhone)).toBe("Pixel 8");
    expect(androidModel(UA.shellTv)).toBe("BRAVIA 4K VH2");
    expect(androidModel(UA.chromeAndroid)).toBeNull();
    expect(androidModel(UA.shellTvNoModel)).toBeNull();
    expect(androidModel("Mozilla/5.0 (Linux; U; Android 4.0.3; ko-kr; LG-L160L Build/IML74K) Safari")).toBeNull();
    expect(androidModel(UA.desktopChrome)).toBeNull();
    expect(shellDeviceName(UA.shellPhone)).toBe("Reel Web on Pixel 8");
    expect(shellDeviceName(UA.shellEmulator)).toBe("Reel Web on sdk_gphone64_arm64");
    expect(shellDeviceName(UA.shellTvNoModel)).toBe("Reel Web on Android TV");
    expect(shellDeviceName("Mozilla/5.0 (Linux; Android 14; wv) Chrome/124 ReelAndroidWeb/0.1.0")).toBe("Reel Web on Android");
  });
});

describe("device name and decide preset (#229)", () => {
  test("Reel Web shell: named after the model, Chrome preset", () => {
    expect(deviceNameFor("android-web", UA.shellPhone)).toBe("Reel Web on Pixel 8");
    expect(deviceNameFor("android-web", UA.shellTv)).toBe("Reel Web on BRAVIA 4K VH2");
    expect(deviceNameFor("android-web", UA.shellTvNoModel)).toBe("Reel Web on Android TV");
    expect(presetFor("android-web", UA.shellPhone)).toBe("web_chrome");
    expect(presetFor("android-web", UA.shellTv)).toBe("web_chrome");
  });
  test("browsers keep their names and presets", () => {
    expect(deviceNameFor("web", UA.chromeAndroid)).toBe("Chrome on Android");
    expect(presetFor("web", UA.chromeAndroid)).toBe("web_chrome");
    expect(deviceNameFor("web", UA.androidTvBrowser)).toBe("Chrome on Android");
    expect(deviceNameFor("web", UA.desktopChrome)).toBe("Chrome on macOS");
    expect(deviceNameFor("web", UA.desktopSafari)).toBe("Safari on macOS");
    expect(presetFor("web", UA.desktopSafari)).toBe("web_safari");
  });
  test("TV apps", () => {
    expect(deviceNameFor("webos", UA.webos)).toBe("LG TV");
    expect(presetFor("webos", UA.webos)).toBe("webos");
    expect(deviceNameFor("tizen", UA.tizen)).toBe("Samsung TV");
    expect(presetFor("tizen", UA.tizen)).toBe("tizen");
  });
});

describe("normalizeKey", () => {
  test("back keys", () => {
    expect(normalizeKey({ keyCode: 461 })?.key).toBe("back");
    expect(normalizeKey({ keyCode: 10009 })?.key).toBe("back");
    expect(normalizeKey({ key: "Escape", keyCode: 27 })?.key).toBe("back");
    expect(normalizeKey({ key: "Backspace", keyCode: 8 })?.key).toBe("back");
  });
  test("media + colour + channel", () => {
    expect(normalizeKey({ keyCode: 415 })?.key).toBe("play");
    expect(normalizeKey({ keyCode: 19 })?.key).toBe("pause");
    expect(normalizeKey({ keyCode: 413 })?.key).toBe("stop");
    expect(normalizeKey({ keyCode: 417 })?.key).toBe("ff");
    expect(normalizeKey({ keyCode: 412 })?.key).toBe("rw");
    expect(normalizeKey({ keyCode: 427 })?.key).toBe("channelUp");
    expect(normalizeKey({ keyCode: 428 })?.key).toBe("channelDown");
    expect(normalizeKey({ keyCode: 403 })?.key).toBe("red");
    expect(normalizeKey({ keyCode: 406 })?.key).toBe("blue");
  });
  test("Android WebView media keys (#231)", () => {
    // What Reel Web on the Android TV emulator delivered (WebView 113): Play and Pause share keyCode 179.
    expect(normalizeKey({ key: "MediaPlayPause", keyCode: 179 })?.key).toBe("playpause");
    expect(normalizeKey({ key: "MediaPlay", keyCode: 179 })?.key).toBe("play");
    expect(normalizeKey({ key: "MediaPause", keyCode: 179 })?.key).toBe("pause");
    expect(normalizeKey({ key: "MediaFastForward", keyCode: 228 })?.key).toBe("ff");
    expect(normalizeKey({ key: "MediaRewind", keyCode: 227 })?.key).toBe("rw");
    expect(normalizeKey({ key: "MediaStop", keyCode: 178 })?.key).toBe("stop");
    expect(normalizeKey({ key: "ChannelUp", keyCode: 33 })?.key).toBe("channelUp");
    expect(normalizeKey({ key: "Enter", keyCode: 13 })?.key).toBe("enter");
  });
  test("digits and unknown", () => {
    expect(normalizeKey({ keyCode: 53 })).toEqual({ key: "digit", digit: 5 });
    expect(normalizeKey({ key: "7" })).toEqual({ key: "digit", digit: 7 });
    expect(normalizeKey({ keyCode: 1234 })).toBeNull();
  });
});

describe("exit / key registration", () => {
  test("webOS prefers platformBack", () => {
    let called = "";
    exitApp({ webOS: { platformBack: () => (called = "webOS") } }, "webos");
    expect(called).toBe("webOS");
    exitApp({ PalmSystem: { platformBack: () => (called = "palm") } }, "webos");
    expect(called).toBe("palm");
    expect(exitApp({ close: () => (called = "close") }, "webos")).toBe(true);
    expect(called).toBe("close");
  });
  test("reports when the page can't exit by itself", () => {
    // Back on the root screen stays unhandled there, so the Reel Web Android shell can leave (#231).
    expect(exitApp({ close: () => undefined }, "android-tv")).toBe(false);
    expect(exitApp({ close: () => undefined }, "web")).toBe(false);
    expect(exitApp({}, "tizen")).toBe(false);
    expect(exitApp({}, "webos")).toBe(false);
  });
  test("Tizen exits via application", () => {
    let exited = false;
    expect(exitApp({ tizen: { application: { getCurrentApplication: () => ({ exit: () => (exited = true) }) } } }, "tizen")).toBe(true);
    expect(exited).toBe(true);
  });
  test("registerKeyBatch gets supported keys only", () => {
    let names: string[] = [];
    registerTizenKeys({
      tizen: { tvinputdevice: { getSupportedKeys: () => [{ name: "MediaPlay" }, { name: "0" }], registerKeyBatch: (n: string[]) => (names = n) } },
    });
    expect(names).toEqual(["MediaPlay", "0"]);
  });
});

describe("server url", () => {
  test("normalize", () => {
    expect(normalizeServerUrl("192.168.1.5:8420")).toBe("http://192.168.1.5:8420");
    expect(normalizeServerUrl("https://reel.example.com/")).toBe("https://reel.example.com");
    expect(normalizeServerUrl("")).toBe("");
    expect(normalizeServerUrl("ftp://x")).toBeNull();
  });
});

describe("vtt", () => {
  test("parses cues", () => {
    const c = parseVtt("WEBVTT\n\n1\n00:00:01.000 --> 00:00:02.500\n<i>Hi</i>\n\n00:01:00.000 --> 00:01:01.000\nBye");
    expect(c).toEqual([{ start: 1, end: 2.5, text: "Hi" }, { start: 60, end: 61, text: "Bye" }]);
  });
});

describe("device profile", () => {
  test("presets by default", () => {
    expect(deviceProfileFromInfo(null, "webos")).toEqual({ preset: "webos" });
    expect(deviceProfileFromInfo({ platform: "tizen", uhd: true, hdr: true }, "tizen")).toEqual({ preset: "tizen" });
  });
  test("HD non-HDR panel gets custom profile", () => {
    const r: any = deviceProfileFromInfo({ platform: "tizen", uhd: false, hdr: false }, "tizen");
    expect(r.profile.max_height).toBe(1080);
    expect(r.profile.video.every((v: any) => v.hdr.length === 0)).toBe(true);
  });
  test("detectTvInfo reads Tizen productinfo", () => {
    const info = detectTvInfo(
      { webapis: { productinfo: { isUdPanelSupported: () => true, is8KPanelSupported: () => false, getModel: () => "QN90" }, avinfo: { isHdrTvSupport: () => true } } },
      "tizen",
    );
    expect(info).toMatchObject({ platform: "tizen", uhd: true, uhd8k: false, hdr: true, model: "QN90" });
  });
});
