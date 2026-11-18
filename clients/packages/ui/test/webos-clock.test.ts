import { expect, test } from "bun:test";
import { WebOsAdapter } from "../src/player";

class TestVideo extends EventTarget {
  currentTime = 0;
  duration = 90;
  removeAttribute() {}
  querySelectorAll() { return []; }
  load() {}
}

test("webOS publishes native video position without timeupdate events", async () => {
  const video = new TestVideo();
  const player = new WebOsAdapter(video as unknown as HTMLVideoElement);
  const updates: number[] = [];
  player.on("timeupdate", ({ positionSec }) => updates.push(positionSec));

  try {
    video.dispatchEvent(new Event("playing"));
    video.currentTime = 12;
    await Bun.sleep(300);
    expect(updates).toEqual([12]);

    video.dispatchEvent(new Event("pause"));
    video.currentTime = 20;
    await Bun.sleep(300);
    expect(updates).toEqual([12]);
  } finally {
    player.destroy();
  }
});
