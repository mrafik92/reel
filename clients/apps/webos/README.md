# Reel – LG webOS app

Packages `apps/web` (built with `VITE_CLIENT=webos VITE_TARGET=tv`, base `./`) as an `.ipk`. Playback uses the
native `<video>` pipeline (`WebOsAdapter`): HLS, MPEG-TS (IPTV passthrough), HEVC/HDR10/Dolby Vision direct.

## Build & package
```sh
bun install                  # once, from clients/
cd apps/webos
bun run package              # builds web → .staging/, runs ares-package if installed → out/*.ipk
```
Without the CLI the script stages everything in `.staging/` and prints the commands to run. Placeholder icons
(80×80 `icon.png`, 130×130 `largeIcon.png`, 1920×1080 `splash.png`) live in `assets/` (`bun run icons` regenerates them).
Set `VITE_REEL_URL=http://<server>:8420` at build time to bake in a default server; otherwise the app asks for the
server address on first run (stored in `localStorage`).

## CLI
`npm i -g @webos-tooling/cli` → provides `ares-*`.

## Install on a TV (Developer Mode)
1. Create an LG developer account; install the **Developer Mode** app from the LG Content Store on the TV, log in,
   enable *Dev Mode Status* and *Key Server*, reboot.
2. `ares-setup-device --add tv --info "{'host':'<TV_IP>','port':'9922','username':'prisoner'}"`
   (`ares-setup-device --list` to verify)
3. `ares-novacom --device tv --getkey` (enter the passphrase shown in the Developer Mode app).
4. `ares-install --device tv out/dev.reel.app_0.0.1_all.ipk`
5. `ares-launch --device tv dev.reel.app`

## Debugging
`ares-inspect --device tv --app dev.reel.app --open` opens Chrome DevTools against the running app.
`ares-launch --device tv dev.reel.app --inspect` launches and inspects in one go.
Dev mode expires after 50 hours; extend it in the Developer Mode app.

## Notes
- `disableBackHistoryAPI` is on: the Back key (461) reaches the app, which handles it (`onRemoteKey`) and calls
  `webOS.platformBack()` / `window.close()` on the root screen.
- The server must send CORS headers (the app origin is `file://`), including for subtitle `<track>` requests.
