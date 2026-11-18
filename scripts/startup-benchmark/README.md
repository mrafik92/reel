# Playback startup measurements

The mock server uses generated media and accepts any test login. It binds to loopback and does not log requests. Generate the fixtures with FFmpeg, start the server, and run the development web app against it.

```sh
scripts/startup-benchmark/fixtures.sh /tmp/reel210-media
python3 scripts/startup-benchmark/mock_server.py /tmp/reel210-media 18765
# In another terminal
cd clients && bun install
cd apps/web && REEL_PROXY_TARGET=http://127.0.0.1:18765 bun run dev --host 127.0.0.1 --port 15173
# In another terminal, with Playwright and Chrome installed
node scripts/startup-benchmark/web.cjs > /tmp/reel210-web.log
python3 scripts/startup-benchmark/summarize.py < /tmp/reel210-web.log
```

The script measures the fixture's MP4 at its 45-second resume point, in Auto quality. Mock HLS uses pre-generated segments and does not measure server remux or transcoding startup. Reel Web for Android runs this same web app in a WebView, so its marks are the web ones (see [design/225 §8](../../docs/design/225-android-web.md#8-playback-in-webview-230)). The native Android runner (`android.sh`) measured the Compose app and was removed with it in #260; `summarize.py` still reads its logcat lines.

Web development builds log the attempt outcome, route and decision marks, load, `loadeddata` and the first video frame callback. A missing mark means the callback was unavailable or did not occur. The browser needs `requestVideoFrameCallback` to report a rendered frame.

Collect each scenario independently with asset, start point, quality, actual decision mode, build, device, network and cold/warm conditions recorded. Keep the raw output limited to the sanitized timing tag. Collect at least 20 attempts per cell and report p50, nearest-rank p95 and failures. Test direct MP4, MKV resume, remux, transcode and controlled IPTV VOD separately. Check the IPTV hub before using the single provider connection. The synthetic server exercises the instrumentation and does not establish real-server or provider performance.

## Recorded harness check

On 2026-11-18, commit base `8f54828` with this instrumentation used an existing generated H.264/AAC MP4 (SHA-256 `d3896d3643a79b413336a6a428f0f9d859e9619c222b9309aa9c58b098766d54`). Both clients requested Auto and received direct play at 45 seconds. Android was the Compose app (removed in #260) on an API 35 arm64 phone emulator on the development Mac and entered from Home; its times came from Media3 events, which are defined differently from the web marks. Web used headless Chrome 156.0.8078.48 on macOS 27.2 and entered from item details. Both used the same local mock server. These runs used a prior version of the fixture generator, so newly generated media may have a different hash.

| Client and scenario | Condition | Attempts | Failures | First frame p50 / p95 |
|---|---|---:|---:|---:|
| Android emulator (Compose app), MP4 resume, direct play | warm, local loopback | 20 | 0 | 543 / 598 ms |
| Desktop Chrome, MP4 resume, direct play | warm, local loopback | 20 | 0 | 131 / 148 ms |

The clients ran on different execution environments and used different launch routes. These numbers validate the measurement path and do not establish a phone-versus-web performance gap. The largest observed interval in this check was after loading began, but Media3's decoder and load callbacks overlap and web's usable-frame event is defined differently.

| Scenario | Remaining evidence |
|---|---|
| Direct MP4 from start | Cold and warm runs on matched physical clients over LAN and Tailscale |
| MKV resume | Matched physical-client runs with the same file, start point and actual delivery mode |
| HLS remux | Real server startup and matched physical-client runs |
| HLS transcode | Real server startup and matched physical-client runs |
| IPTV VOD | Controlled provider availability and matched physical-client runs |

A comparable release threshold and the leading real-world delay require the owner's phone measurements. No real provider playback was attempted.
