import { beforeEach, describe, expect, test } from "bun:test";
import { AvPlayAdapter } from "../src/player";

function mockAvPlay() {
  const calls: string[] = [];
  let state: AVPlayState = "NONE";
  let listener: AVPlayListener = {};
  let now = 0;
  const av: AVPlay = {
    open: (u) => { calls.push(`open:${u}`); state = "IDLE"; },
    close: () => { calls.push("close"); state = "NONE"; },
    prepare: () => {},
    prepareAsync: (ok) => { state = "READY"; calls.push("prepareAsync"); ok?.(); },
    play: () => { calls.push("play"); state = "PLAYING"; },
    pause: () => { calls.push("pause"); state = "PAUSED"; },
    stop: () => { calls.push("stop"); state = "IDLE"; },
    seekTo: (ms, ok) => { calls.push(`seekTo:${ms}`); now = ms; ok?.(); },
    jumpForward: () => {},
    jumpBackward: () => {},
    getDuration: () => 120000,
    getCurrentTime: () => now,
    getState: () => state,
    setDisplayRect: (...a) => calls.push(`rect:${a.join(",")}`),
    setDisplayMethod: () => {},
    setListener: (l) => { listener = l; },
    getTotalTrackInfo: () => [
      { index: 0, type: "VIDEO", extra_info: "{}" },
      { index: 1, type: "AUDIO", extra_info: '{"language":"eng"}' },
      { index: 2, type: "AUDIO", extra_info: '{"language":"fra"}' },
      { index: 3, type: "TEXT", extra_info: '{"track_lang":"spa"}' },
    ],
    getCurrentStreamInfo: () => [{ index: 1, type: "AUDIO", extra_info: "{}" }],
    setSelectTrack: (t, i) => calls.push(`select:${t}:${i}`),
    setSilentSubtitle: (s) => calls.push(`silent:${s}`),
    setStreamingProperty: (p, v) => calls.push(`prop:${p}=${v}`),
    setTimeout: () => {},
    suspend: () => { calls.push("suspend"); },
    restore: (u, t) => { calls.push(`restore:${u}:${t}`); },
  };
  return { av, calls, fire: () => listener, setNow: (n: number) => (now = n), state: () => state };
}

describe("AvPlayAdapter", () => {
  let m: ReturnType<typeof mockAvPlay>;
  let a: AvPlayAdapter;
  let events: string[];
  beforeEach(() => {
    m = mockAvPlay();
    a = new AvPlayAdapter(undefined, () => m.av);
    events = [];
    (["play", "pause", "ended", "buffering", "ready", "error", "timeupdate", "cue"] as const).forEach((k) =>
      a.on(k, (d: any) => events.push(`${k}:${JSON.stringify(d ?? null)}`)));
  });

  test("load → prepare → start position → autoplay", async () => {
    await a.load("http://s/x.m3u8", { startPositionSec: 30, autoplay: true });
    expect(m.calls.slice(0, 2)).toEqual(["open:http://s/x.m3u8", expect.stringContaining("prop:ADAPTIVE_INFO")]);
    expect(m.calls).toContain("rect:0,0,1920,1080");
    expect(m.calls).toContain("seekTo:30000");
    expect(m.calls[m.calls.length - 1]).toBe("play");
    expect(m.state()).toBe("PLAYING");
    expect(events).toContain('ready:{"durationSec":120}');
  });

  test("pause/seek/events from listener", async () => {
    await a.load("http://s/x.mp4");
    expect(m.calls.some((c) => c.startsWith("prop:"))).toBe(false);
    await a.play();
    a.pause();
    expect(m.state()).toBe("PAUSED");
    a.seek(10);
    m.fire().oncurrentplaytime!(11000);
    m.fire().onbufferingstart!();
    m.fire().onbufferingcomplete!();
    m.fire().onstreamcompleted!();
    m.fire().onerror!("PLAYER_ERROR_CONNECTION_FAILED");
    expect(events).toContain('timeupdate:{"positionSec":11,"durationSec":120}');
    expect(events).toContain("buffering:true");
    expect(events).toContain("ended:null");
    expect(events.some((e) => e.startsWith("error:") && e.includes("CONNECTION_FAILED"))).toBe(true);
  });

  test("track mapping and selection", async () => {
    await a.load("http://s/x.mp4");
    const t = a.getTracks();
    expect(t.audio.map((x) => [x.id, x.label, x.selected])).toEqual([["a:1", "ENG", true], ["a:2", "FRA", false]]);
    expect(t.text.map((x) => [x.id, x.language])).toEqual([["emb:3", "spa"]]);
    a.selectAudioTrack("a:2");
    expect(m.calls).toContain("select:AUDIO:2");
    expect(a.getTracks().audio[1]!.selected).toBe(true);
    a.selectTextTrack("emb:3");
    expect(m.calls).toContain("select:TEXT:3");
    expect(m.calls).toContain("silent:false");
    m.fire().onsubtitlechange!("1000", "Hola");
    expect(events).toContain('cue:{"text":"Hola"}');
    a.selectTextTrack(null);
    expect(m.calls).toContain("silent:true");
  });

  test("suspend / restore keeps position and resumes playing", async () => {
    await a.load("http://s/x.mp4", { autoplay: true });
    m.setNow(45000);
    a.setSuspended(true);
    a.setSuspended(false);
    expect(m.calls).toContain("suspend");
    expect(m.calls).toContain("restore:http://s/x.mp4:45000");
    expect(m.calls[m.calls.length - 1]).toBe("play");
  });

  test("destroy stops and closes", async () => {
    await a.load("http://s/x.mp4");
    a.destroy();
    expect(m.calls.slice(-2)).toEqual(["stop", "close"]);
  });
});
