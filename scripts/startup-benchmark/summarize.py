#!/usr/bin/env python3
"""Summarize sanitized ReelStartup lines from one client and scenario."""
import json
import math
import re
import statistics
import sys

rows = []
for line in sys.stdin:
    if "[ReelStartup] " in line:
        rows.append(json.loads(line.split("[ReelStartup] ", 1)[1]))
    elif "ReelStartup: " in line:
        fields = dict(re.findall(r"(\w+)=(\w+)", line.split("ReelStartup: ", 1)[1]))
        rows.append(fields)

frames = sorted(int(row.get("frame", row.get("frame_ms", 0))) for row in rows if row.get("outcome") == "rendered")
print(f"attempts={len(rows)} rendered={len(frames)} failures={sum(row.get('outcome') == 'failed' for row in rows)} canceled={sum(row.get('outcome') in ('canceled', 'superseded') for row in rows)}")
if frames:
    print(f"frame_ms p50={statistics.median(frames):g} p95={frames[math.ceil(len(frames) * .95) - 1]}")
