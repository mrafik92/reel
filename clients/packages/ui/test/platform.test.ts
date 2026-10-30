import { describe, expect, test } from "bun:test";
import { detectPlatform, clientNameFor } from "../src/platform/detect";
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
    exitApp({ close: () => (called = "close") }, "webos");
    expect(called).toBe("close");
  });
  test("Tizen exits via application", () => {
    let exited = false;
    exitApp({ tizen: { application: { getCurrentApplication: () => ({ exit: () => (exited = true) }) } } }, "tizen");
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
