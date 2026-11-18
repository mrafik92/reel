#!/usr/bin/env bash
# Unit tests for scripts/android-release.sh and clients/android-web/scripts/version-from-tag.sh (#232, #260).
# No network, no Gradle build, nothing sent: Tailscale, gh and git pushes are stubs or dry runs.
# A throwaway keystore is generated in a temp dir (needs keytool, i.e. a JDK) and deleted at the end.
#
#   scripts/tests/android-release-test.sh
# A && ok || bad is fine here: ok/bad always succeed. TAILSCALE_BIN is read by the sourced functions.
# shellcheck disable=SC2015,SC2034
set -uo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
SCRIPT="$ROOT/scripts/android-release.sh"
VERSION_SCRIPT="$ROOT/clients/android-web/scripts/version-from-tag.sh"
TMP=$(mktemp -d "${TMPDIR:-/tmp}/reel-release-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

pass=0 fail=0
ok() { pass=$((pass + 1)); echo "ok   $1"; }
bad() { fail=$((fail + 1)); echo "FAIL $1"; [[ -n "${2:-}" ]] && printf '     %s\n' "$2"; }
eq() { if [[ "$2" == "$3" ]]; then ok "$1"; else bad "$1" "expected '$3', got '$2'"; fi; }
has() { if [[ "$2" == *"$3"* ]]; then ok "$1"; else bad "$1" "missing '$3' in: $2"; fi; }
lacks() { if [[ "$2" != *"$3"* ]]; then ok "$1"; else bad "$1" "unexpected '$3' in output"; fi; }

# Load the functions without running main.
# shellcheck source=scripts/android-release.sh
source "$SCRIPT"
set +e

# --- version scheme -------------------------------------------------------------------------------------
eq "web tag → version" "$("$VERSION_SCRIPT" android-web-v1.10.3 | tr '\n' ' ')" "name=1.10.3 code=1010003 "
eq "refs/tags prefix is stripped" "$("$VERSION_SCRIPT" refs/tags/android-web-v0.1.0 | tail -1)" "code=1000"
# android-v* was the Compose app's prefix (removed in #260); it's rejected like any other foreign tag.
for badtag in android-v0.3.0 android-web-v1.0.0-rc1 android-web-v01.0.0 android-web-v0.0.0 android-tv-v1.0.0 web-v1.0.0 android-web-v1.1000.0; do
  "$VERSION_SCRIPT" "$badtag" >/dev/null 2>&1
  eq "rejects $badtag" "$?" 1
done
eq "version_code web" "$(version_code android-web-v 0.1.0)" 1000

# --- newest tag -----------------------------------------------------------------------------------------
# The repo still has the Compose app's android-v0.1.0..0.3.0 tags; they must never count as Reel Web releases.
tags=$'android-v0.3.0\nandroid-v9.0.0\nandroid-web-v0.9.0\nandroid-web-v0.10.0\nandroid-web-v1.0.0-rc1\nandroid-web-v0.2.0'
eq "newest web tag (numeric, ignores android-v* and bad tags)" "$(newest_tag android-web-v <<<"$tags")" android-web-v0.10.0
eq "no tag → empty" "$(newest_tag android-web-v <<<"android-v0.3.0")" ""

eq "next_patch none" "$(next_patch "")" 0.1.0
eq "next_patch" "$(next_patch 0.3.0)" 0.3.1
eq "next_patch keeps minor" "$(next_patch 1.10.9)" 1.10.10
next_patch 1.2.999 >/dev/null 2>&1
eq "next_patch refuses 999" "$?" 1

app_config web
eq "web package" "$APP_ID" app.reel.android.web
eq "web tag prefix" "$TAG_PREFIX" android-web-v
eq "web APK name" "$APK_BASENAME" ReelWeb
app_config compose
eq "compose app removed (#260)" "$?" 1
app_config tv
eq "unknown app rejected" "$?" 1

# --- Taildrop -------------------------------------------------------------------------------------------
apk="$TMP/ReelWeb-0.1.0.apk"
echo fake >"$apk"
stub="$TMP/tailscale"
offline_msg='rafiiiki is reportedly offline; sending anyway
Error: 502 Bad Gateway: dial tcp 100.64.0.2:43177: connect: no route to host'
offline_msg_502_only='Error: 502 Bad Gateway'
make_stub() { # make_stub EXIT OUTPUT
  printf '#!/bin/sh\necho "$@" > "%s/called"\ncat <<"OUT"\n%s\nOUT\nexit %s\n' "$TMP" "$2" "$1" >"$stub"
  chmod +x "$stub"
  rm -f "$TMP/called"
}

taildrop_offline "$offline_msg" && ok "offline output detected" || bad "offline output detected"
taildrop_offline "$offline_msg_502_only" && ok "bare 502 counts as offline" || bad "bare 502 counts as offline"
taildrop_offline "permission denied" && bad "other error isn't offline" || ok "other error isn't offline"

TAILSCALE_BIN=$stub
make_stub 0 ""
out=$(taildrop_send "$apk" rafiiiki 0 2>&1)
eq "taildrop sent → 0" "$?" 0
eq "taildrop target argument" "$(cat "$TMP/called")" "file cp $apk rafiiiki:"
has "taildrop success message" "$out" "Sent ReelWeb-0.1.0.apk to rafiiiki"

make_stub 1 "$offline_msg"
out=$(taildrop_send "$apk" rafiiiki 0 2>&1)
eq "taildrop offline → 3" "$?" 3
has "offline says so plainly" "$out" "the device 'rafiiiki' is offline, so nothing was sent"
has "offline prints the retry command" "$out" "file cp $apk rafiiiki:"

make_stub 1 "some other failure"
out=$(taildrop_send "$apk" pixel 0 2>&1)
eq "taildrop other failure → 1" "$?" 1
has "other failure shows Tailscale's output" "$out" "some other failure"

make_stub 0 ""
out=$(taildrop_send "$apk" rafiiiki 1 2>&1)
eq "taildrop dry run → 0" "$?" 0
has "dry run prints the command" "$out" "dry run: would run: $stub file cp $apk rafiiiki:"
[[ -e "$TMP/called" ]] && bad "dry run doesn't call tailscale" || ok "dry run doesn't call tailscale"

# --- GitHub release dry run -----------------------------------------------------------------------------
mkdir -p "$TMP/bin"
printf '#!/bin/sh\ntouch "%s/gh-called"\n' "$TMP" >"$TMP/bin/gh"
chmod +x "$TMP/bin/gh"
out=$(PATH="$TMP/bin:$PATH" github_release android-web-v0.1.0 "Reel Web 0.1.0" "notes" 1 "$apk" "$apk.sha256" 2>&1)
eq "release dry run → 0" "$?" 0
has "dry run shows the tag" "$out" "git tag -a android-web-v0.1.0"
has "dry run shows the push" "$out" "git push origin refs/tags/android-web-v0.1.0"
has "dry run shows gh release create" "$out" "gh release create android-web-v0.1.0 $apk $apk.sha256 -R mrafik92/reel --title Reel\\ Web\\ 0.1.0"
[[ -e "$TMP/gh-called" ]] && bad "dry run doesn't call gh" || ok "dry run doesn't call gh"
eq "no tag was created" "$(git -C "$ROOT" tag -l android-web-v0.1.0)" ""

# --- passwords ------------------------------------------------------------------------------------------
JAVA_HOME=$(find_java_home 2>/dev/null) || JAVA_HOME=""
if [[ -z "$JAVA_HOME" ]]; then
  bad "keystore tests need a JDK 17–21 (keytool)"
else
  export PATH="$JAVA_HOME/bin:$PATH"
  ks="$TMP/throwaway.jks"
  # Random, never printed, deleted with $TMP.
  TEST_PW="t$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
  KS_PW=$TEST_PW keytool -genkeypair -keystore "$ks" -storetype PKCS12 -alias reel -keyalg RSA -keysize 2048 \
    -validity 2 -storepass:env KS_PW -keypass:env KS_PW -dname "CN=Reel test, O=Reel" >/dev/null 2>&1
  [[ -s "$ks" ]] && ok "throwaway keystore created" || bad "throwaway keystore created"

  # No env, no Keychain, no terminal → a clear error, before any build.
  out=$(REEL_NO_FETCH=1 REEL_NO_KEYCHAIN=1 REEL_KEYSTORE_FILE="$ks" REEL_KEYSTORE_PASSWORD='' "$SCRIPT" web --version 0.1.0 </dev/null 2>&1)
  eq "missing password → exit 1" "$?" 1
  has "missing password message" "$out" "no keystore password, so nothing was built"
  has "missing password names the Keychain command" "$out" "security add-generic-password -s reel-android-keystore -a reel -w"
  lacks "missing password: no build started" "$out" "lint, unit tests"

  # Keychain item absent → same error (service name that can't exist).
  out=$(REEL_NO_FETCH=1 REEL_KEYCHAIN_SERVICE="reel-test-absent-$$" REEL_KEYSTORE_FILE="$ks" REEL_KEYSTORE_PASSWORD='' "$SCRIPT" web --version 0.1.0 </dev/null 2>&1)
  eq "absent Keychain item → exit 1" "$?" 1

  # Wrong password → clear error, the password isn't echoed.
  WRONG="wrong-$TEST_PW"
  out=$(REEL_NO_FETCH=1 REEL_NO_KEYCHAIN=1 REEL_KEYSTORE_FILE="$ks" REEL_KEYSTORE_PASSWORD="$WRONG" "$SCRIPT" web --version 0.1.0 </dev/null 2>&1)
  eq "wrong password → exit 1" "$?" 1
  has "wrong password message" "$out" "the keystore password is wrong"
  lacks "wrong password isn't printed" "$out" "$WRONG"
  lacks "wrong password: no build started" "$out" "lint, unit tests"

  # Wrong alias.
  out=$(REEL_NO_FETCH=1 REEL_NO_KEYCHAIN=1 REEL_KEYSTORE_FILE="$ks" REEL_KEY_ALIAS=nope REEL_KEYSTORE_PASSWORD="$TEST_PW" "$SCRIPT" web --version 0.1.0 </dev/null 2>&1)
  eq "wrong alias → exit 1" "$?" 1
  has "wrong alias message" "$out" "has no key with alias 'nope'"
  lacks "right password isn't printed" "$out" "$TEST_PW"

  # Right password: cert digest read; the passwords are unexported, so child processes don't see them.
  (
    export REEL_KEYSTORE_PASSWORD="$TEST_PW"
    REEL_NO_KEYCHAIN=1 resolve_passwords reel "$ks" || exit 1
    [[ "$KEY_PW" == "$TEST_PW" ]] || { echo "key password should default to the store password"; exit 1; }
    if env | grep -q "$TEST_PW"; then echo "password leaked into a child's environment"; exit 1; fi
    d=$(keystore_cert_sha256 "$ks" reel) || exit 1
    [[ "$d" =~ ^[0-9a-f]{64}$ ]] || { echo "bad digest '$d'"; exit 1; }
  ) >"$TMP/sub.out" 2>&1
  eq "resolve_passwords + cert digest, no env leak" "$?" 0
  [[ -s "$TMP/sub.out" ]] && cat "$TMP/sub.out"
fi

# --- release certificate --------------------------------------------------------------------------------
release=$RELEASE_CERT_SHA256 other=$(printf '0%.0s' {1..64})
check_release_cert "$release" 1 0 2>/dev/null
eq "release cert accepted for a GitHub release" "$?" 0
out=$(check_release_cert "$other" 0 0 2>&1)
eq "other key: local build continues" "$?" 0
has "other key: local build warns" "$out" "won't install over release builds"
out=$(check_release_cert "$other" 1 0 2>&1)
eq "other key: GitHub release refused" "$?" 1
has "other key: refusal says why" "$out" "--github-release refuses it"
check_release_cert "$other" 1 1 >/dev/null 2>&1
eq "other key: GitHub release dry run only warns" "$?" 0

# --- arguments ------------------------------------------------------------------------------------------
"$SCRIPT" >/dev/null 2>&1
eq "no app → usage (2)" "$?" 2
"$SCRIPT" web --bogus >/dev/null 2>&1
eq "unknown flag → usage (2)" "$?" 2
REEL_NO_FETCH=1 "$SCRIPT" web --version 1.0 >/dev/null 2>&1
eq "bad --version → usage (2)" "$?" 2
"$SCRIPT" compose >/dev/null 2>&1
eq "compose → usage (2)" "$?" 2
help=$("$SCRIPT" --help)
has "--help" "$help" "Usage: scripts/android-release.sh web [options]"
lacks "--help has no compose mode" "$help" "compose"

echo
echo "$pass passed, $fail failed"
((fail == 0))
