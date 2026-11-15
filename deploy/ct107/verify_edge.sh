#!/usr/bin/env bash
# External checks for Reel's public edge (#31, #32). Run from OUTSIDE the home network and with
# Tailscale off (phone hotspot, a VPS), after the Caddy/OPNsense/Cloudflare changes are applied.
# Read-only apart from one failed login with a throwaway username (step 6). Needs curl; nc is
# optional. Prints PASS/FAIL/INFO lines and exits non-zero on any FAIL.
#
#   deploy/ct107/verify_edge.sh [host]        # default host: reel.rafikii.co
set -u
HOST=${1:-reel.rafikii.co}
U="https://$HOST"
fails=0
pass() { printf 'PASS  %s\n' "$*"; }
fail() { printf 'FAIL  %s\n' "$*"; fails=$((fails + 1)); }
info() { printf 'INFO  %s\n' "$*"; }
code() { curl -s -o /dev/null -w '%{http_code}' --max-time 15 "$@"; }

# 0. We really are outside: the name must not resolve to the LAN address.
ip4=$(dig +short "$HOST" A @1.1.1.1 | tail -1)
ip6=$(dig +short "$HOST" AAAA @1.1.1.1 | tail -1)
info "public A=${ip4:-none} AAAA=${ip6:-none}"
case "$(curl -s --max-time 10 https://api.ipify.org)" in
  85.144.240.8) fail "this machine's public IP is the home WAN: run from outside" ;;
  *) : ;;
esac

# 1. The app loads; first-run setup is closed and can't be reached.
[ "$(code "$U/")" = 200 ] && pass "web app loads" || fail "web app does not load"
body=$(curl -s --max-time 15 "$U/api/v1/setup")
[ "$body" = '{"needs_setup":false}' ] && pass "GET /api/v1/setup → needs_setup false" \
  || fail "GET /api/v1/setup answered: $body"
c=$(code -X POST -H 'Content-Type: application/json' -H 'X-Forwarded-For: 192.168.1.10' \
  -d '{"username":"x","password":"y"}' "$U/api/v1/setup")
[ "$c" = 403 ] && pass "POST /api/v1/setup (spoofed LAN XFF) → 403" || fail "POST /api/v1/setup → $c"

# 2. LAN-only API areas are refused at the edge, also in disguise.
for p in /api/v1/admin/users /api/v1/admin/iptv/hub /API/V1/ADMIN/users /api/v1/%61dmin/users \
  /api/v1//admin/users /api/v1/webhooks/arr /api/v1/cast/devices /api/v1/setup/x; do
  c=$(code --path-as-is "$U$p")
  [ "$c" = 403 ] && pass "$p → 403" || fail "$p → $c (expected 403)"
done
c=$(code -X POST "$U/api/v1/webhooks/arr?server=1&token=x")
[ "$c" = 403 ] && pass "POST webhooks → 403" || fail "POST webhooks → $c"
# normal API still answers (401 without a session)
c=$(code "$U/api/v1/auth/me")
[ "$c" = 401 ] && pass "/api/v1/auth/me → 401 (API reachable, needs login)" || fail "/api/v1/auth/me → $c"

# 3. Security headers.
hdrs=$(curl -s -D - -o /dev/null --max-time 15 "$U/")
has() { printf '%s' "$hdrs" | grep -qi "^$1:"; }
for h in Strict-Transport-Security Permissions-Policy X-Content-Type-Options X-Frame-Options Referrer-Policy; do
  has "$h" && pass "header $h" || fail "missing header $h"
done
if has Content-Security-Policy; then pass "CSP enforced"
elif has Content-Security-Policy-Report-Only; then info "CSP is report-only (switch to enforcing once clean)"
else fail "no CSP header"; fi
has Server && fail "Server header present" || pass "no Server header"

# 4. HTTP/3 off (#108).
alt=$(printf '%s' "$hdrs" | grep -i '^alt-svc:' | tr -d '\r')
case "$alt" in *h3*) fail "Alt-Svc still advertises h3: $alt" ;; *) pass "no h3 in Alt-Svc (${alt:-none})" ;; esac
if curl --version | grep -q HTTP3; then
  c=$(code --http3-only "$U/")
  [ "$c" = 000 ] && pass "HTTP/3 connection refused" || fail "HTTP/3 answered $c"
else info "curl has no HTTP/3 support; Alt-Svc check only"; fi

# 5. Only 443 is open; Reel's own port and Caddy's internal listener are not.
if command -v nc >/dev/null; then
  for target in "$ip4" "$ip6"; do
    [ -z "$target" ] && continue
    for port in 8420 8443 80 32400; do
      if nc -z -w 5 "$target" "$port" 2>/dev/null; then fail "$target:$port is open"
      else pass "$target:$port closed"; fi
    done
  done
  [ -z "$ip6" ] && info "no AAAA record: IPv6 not published (CT 113 has no global IPv6 either)"
else info "nc not installed: skipped the port checks"; fi
# The raw IP (no SNI for Reel) must not serve Reel.
[ -n "$ip4" ] && { c=$(code -k "https://$ip4/api/v1/health"); [ "$c" != 200 ] \
  && pass "https://<ip>/ does not serve Reel ($c)" || fail "Reel answers on the bare IP"; }

# 6. A spoofed X-Forwarded-For is ignored. One failed login with a throwaway name and a LAN XFF;
# Reel must record THIS machine's public address. Check it from the LAN afterwards.
probe="edgeprobe$(date +%s)"
c=$(code -X POST -H 'Content-Type: application/json' -H 'X-Forwarded-For: 192.168.1.10' \
  -H 'X-Real-IP: 10.0.0.1' -d "{\"username\":\"$probe\",\"password\":\"wrong-wrong\"}" \
  "$U/api/v1/auth/login")
info "failed login as '$probe' sent ($c) from $(curl -s --max-time 10 https://api.ipify.org)"
info "on the LAN, as admin: Settings → Audit, or GET /api/v1/admin/audit — the login_failure for"
info "'$probe' must show the address above, not 192.168.1.10 / 10.0.0.1"

echo
[ "$fails" = 0 ] && echo "All external checks passed." || echo "$fails check(s) FAILED."
exit $((fails > 0))
