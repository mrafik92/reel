# Reel – Samsung Tizen TV app

Packages `apps/web` (built with `VITE_CLIENT=tizen VITE_TARGET=tv`, base `./`) as a signed `.wgt`.
Playback uses Samsung **AVPlay** (`AvPlayAdapter`, `<object type="application/avplayer">`).

## Prerequisites
- Tizen Studio (or the CLI-only installer) with **TV Extensions**; add `~/tizen-studio/tools/ide/bin` to `PATH`
  (provides `tizen`) and `~/tizen-studio/tools` (provides `sdb`).
- A Samsung certificate profile: Tizen Studio → *Tools → Certificate Manager* → create a **Samsung** certificate
  (author + distributor, include your TV's DUID). Note the profile name.

## Build & package
```sh
cd apps/tizen
TIZEN_CERT_PROFILE=<profile-name> bun run package
```
This builds the web app into `.staging/`, then runs `tizen build-web` and
`tizen package -t wgt -s $TIZEN_CERT_PROFILE` → `out/Reel.wgt`. Without the CLI it only stages and prints the commands.
Placeholder `assets/icon.png` (512×512) is generated (`bun run icons`). Samsung TVs show no splash image from the
widget, so none is bundled.
Set `VITE_REEL_URL=http://<server>:8420` to bake in a default server; otherwise the app asks on first run.

## Install on a TV (Developer Mode)
1. TV: Apps panel → press `12345` on the remote → enable **Developer mode**, enter your PC's IP, reboot the TV.
2. `sdb connect <TV_IP>` then `sdb devices` (note the device name).
3. `tizen install -n Reel.wgt -t <device-name> -- out/`
4. `tizen run -p ReelDev00.Reel -t <device-name>`

## Debugging
`tizen run -p ReelDev00.Reel -t <device-name>` then Tizen Studio → Device Manager → *Web Inspector*, or
`sdb -s <device> forward tcp:9222 tcp:<debug-port>` and open `chrome://inspect`.

## Notes
- Privileges in `config.xml`: internet, avplay, productinfo, `tv.inputdevice` (needed for `registerKeyBatch`: media,
  colour and number keys). Back is key code 10009; the app exits via `tizen.application.getCurrentApplication().exit()`.
- AVPlay renders on a video plane behind the page: the adapter makes `html/body/#root` transparent while active.
- The server must send CORS headers (origin is `file://`).
