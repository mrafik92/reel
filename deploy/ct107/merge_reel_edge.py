#!/usr/bin/env python3
"""Merge deploy/ct107/reel.caddy into CT 107's Caddyfile (#31, #108). Prints the result; changes nothing.

    ssh minix-local 'pct exec 107 -- cat /etc/caddy/Caddyfile' \\
      | deploy/ct107/merge_reel_edge.py [--only-h3] > /tmp/Caddyfile.new

Adds PIECE 1 (global `servers :8443 { protocols h1 h2 }`) as the first block, PIECE 2 (the public
reel.rafikii.co:8443 site) before the `:8443` catch-all, and swaps the LAN `handle @reel` for PIECE 3.
`--only-h3` leaves out PIECE 2: the #108 fix alone, with nothing public.
The runbook (backup, validate, reload, rollback) is in docs/OPERATIONS.md → "Public edge".
"""

import re
import sys
from pathlib import Path

SNIPPET = Path(__file__).with_name("reel.caddy")
CATCH_ALL = "# Deny every other Host header on the public listener."
LAN_REEL = re.compile(r"\t@reel host reel\.rafikii\.co\n\thandle @reel \{.*?\n\t\}\n", re.S)


def piece(src: str, start: str, end: str) -> str:
    i = src.index(start)
    return "\n".join(src[i : src.index(end, i)].splitlines()[1:])


def merge(live: str, src: str, only_h3: bool) -> str:
    if "reel.rafikii.co:8443" in live or 'Alt-Svc "clear"' in live:
        sys.exit("already merged (reel.rafikii.co:8443 or Alt-Svc clear present): nothing to do")
    if live.lstrip().startswith("{"):
        sys.exit("this Caddyfile already has a global options block: merge PIECE 1 into it by hand")
    p1 = piece(src, "# ─── PIECE 1:", "# ─── PIECE 2:")
    p2 = piece(src, "# ─── PIECE 2:", "# ─── PIECE 3:")
    p3 = piece(src, "# ─── PIECE 3:", "# ─── PIECE 1b")
    out = p1[p1.index("{") :].rstrip() + "\n\n" + live
    if not only_h3:
        if CATCH_ALL not in out:
            sys.exit("`:8443` catch-all comment not found: place PIECE 2 by hand")
        out = out.replace(CATCH_ALL, p2.strip() + "\n\n" + CATCH_ALL, 1)
    m = LAN_REEL.search(out)
    if not m:
        sys.exit("LAN `handle @reel { … }` not found: replace it with PIECE 3 by hand")
    p3 = p3[p3.index("\t@reel host") :].strip("\n") + "\n"
    return out[: m.start()] + p3 + out[m.end() :]


if __name__ == "__main__":
    sys.stdout.write(merge(sys.stdin.read(), SNIPPET.read_text(), "--only-h3" in sys.argv[1:]))
