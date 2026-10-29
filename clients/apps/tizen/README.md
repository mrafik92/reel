# Reel – Samsung Tizen TV app

1. Build web: `cd ../.. && bun run build`
2. Add `icon.png` (117x117) here.
3. Install Tizen Studio + TV Extensions; create a Samsung certificate profile (Certificate Manager).
4. `TIZEN_CERT_PROFILE=<name> ./scripts/package.sh` → `out/Reel.wgt`.

## Install on a TV (developer mode)
1. TV: Apps panel → press `12345` on remote → enable **Developer mode**, enter your PC's IP, reboot TV.
2. `sdb connect <TV_IP>` (then `sdb devices`).
3. `tizen install -n Reel.wgt -t <device-name> -- out/` (or run from Tizen Studio Device Manager).
4. `tizen run -p ReelDev00.Reel -t <device-name>`

Playback via AVPlay (`AvPlayAdapter`) is not yet implemented. Set `VITE_REEL_URL=http://<server>:8420` at build time.
