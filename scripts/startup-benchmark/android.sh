#!/bin/sh
set -eu
serial=${ANDROID_SERIAL:?Set ANDROID_SERIAL to the dedicated emulator}
adb -s "$serial" logcat -c
i=0
while [ "$i" -lt 20 ]; do
  adb -s "$serial" shell am start -n app.reel.android.debug/app.reel.android.MainActivity >/dev/null
  sleep 2
  # The fixture's Home hero is displayed on a 1080 x 2400 emulator.
  adb -s "$serial" shell input tap 365 1160
  sleep 3
  adb -s "$serial" shell input keyevent 4
  sleep 2
  i=$((i + 1))
done
adb -s "$serial" logcat -d -s ReelStartup:I '*:S'
