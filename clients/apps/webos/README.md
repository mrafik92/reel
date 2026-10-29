# Reel – LG webOS app

1. Build web: `cd ../.. && bun run build`
2. Add `icon.png` (80x80) and `largeIcon.png` (130x130) to this directory (placeholders referenced by `appinfo.json`).
3. `npm i -g @webos-tooling/cli`, then `./scripts/package.sh` → `out/dev.reel.app_0.0.1_all.ipk`.

## Install on a TV (developer mode)
1. Create an LG developer account; install the **Developer Mode** app from the LG Content Store on the TV, log in, enable *Dev Mode Status* and *Key Server*, reboot.
2. `ares-setup-device --add tv --info "{'host':'<TV_IP>','port':'9922','username':'prisoner'}"`
3. `ares-novacom --device tv --getkey` (enter the passphrase shown in the Developer Mode app).
4. `ares-install --device tv out/dev.reel.app_0.0.1_all.ipk`
5. `ares-launch --device tv dev.reel.app`

The app must reach the server (set `VITE_REEL_URL=http://<server>:8420` at build time). Dev mode expires after 50 hours unless extended in the Developer Mode app.
