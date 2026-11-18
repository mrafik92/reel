import { describe, expect, test } from "bun:test";
import { nativeAudioChoice } from "../src/lib/nativeAudio";

describe("native direct-play audio", () => {
  const streams = [{ index: 2 }, { index: 5 }];
  const tracks = [{ id: "a:0", selected: true }, { id: "a:1", selected: false }];
  test("applies the preferred stream when the native player chose the default", () => {
    expect(nativeAudioChoice("direct_play", "copy", streams, 5, tracks)).toBe("a:1");
  });
  test("does not reselect an active track or guess with incomplete tracks", () => {
    expect(nativeAudioChoice("direct_play", "copy", streams, 2, tracks)).toBeUndefined();
    expect(nativeAudioChoice("direct_play", "copy", streams, 5, tracks.slice(0, 1))).toBeUndefined();
    expect(nativeAudioChoice("direct_play", "copy", streams, 9, tracks)).toBeUndefined();
  });
  test("does not select by position in server-generated streams", () => {
    expect(nativeAudioChoice("remux", "copy", streams, 5, tracks)).toBeUndefined();
    expect(nativeAudioChoice("direct_play", "transcode", streams, 5, tracks)).toBeUndefined();
  });
});
