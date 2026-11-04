#!/usr/bin/env python3
"""Sync reel/docs/*.md into the Obsidian vault (~/Documents/obsidian/notes-/Reel).

Regenerates: Reel Features, Reel Architecture, Reel Status, Reel Operations Runbook, Reel Decisions Log.
`Reel Home.md` is hand-written and only gets its `updated:` date bumped.

    scripts/sync_obsidian.py            # write the notes
    scripts/sync_obsidian.py --check    # exit 1 if the vault is out of date (no writes)
"""
import datetime
import os
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
SRC = REPO / "docs"
DST = pathlib.Path(os.path.expanduser("~/Documents/obsidian/notes-/Reel"))
TODAY = datetime.date.today().isoformat()

NAMES = {"FEATURES.md": "Reel Features", "ARCHITECTURE.md": "Reel Architecture",
         "STATUS.md": "Reel Status", "OPERATIONS.md": "Reel Operations Runbook"}
META = {
    "Reel Features": (["reel", "homelab", "media-server", "features"], ["Reel feature list"]),
    "Reel Architecture": (["reel", "homelab", "media-server", "architecture"], ["Reel design"]),
    "Reel Status": (["reel", "homelab", "media-server", "status"], ["Reel current state"]),
    "Reel Operations Runbook": (["reel", "homelab", "media-server", "operations", "runbook"], ["Reel Ops", "Reel Runbook"]),
}


def linkify(text):
    for f, n in NAMES.items():
        text = re.sub(r"\[([^\]]+)\]\(" + re.escape(f) + r"(#[^)]*)?\)",
                      lambda m, n=n: f"[[{n}{m.group(2) or ''}|{m.group(1)}]]", text)
    text = text.replace("](../SPEC.md)", f"](file://{REPO}/SPEC.md)")
    text = text.replace("](../deploy/README.md)", f"](file://{REPO}/deploy/README.md)")
    return text


def frontmatter(title, tags, aliases, source=None):
    fm = ["---", f"title: {title}", "tags:"] + [f"  - {t}" for t in tags] + ["aliases:"] + [f"  - {a}" for a in aliases]
    fm.append(f"updated: {TODAY}")
    if source:
        fm.append(f"source: reel/docs/{source}")
    return "\n".join(fm + ["---", "", ""])


def render():
    notes = {}
    for f, n in NAMES.items():
        lines = (SRC / f).read_text().split("\n")
        if lines and lines[0].startswith("# "):
            lines[0] = f"# {n}"
        body = linkify("\n".join(lines))
        body = body.replace("> **No secrets in docs, shells, or chat.**",
                            "> [!danger] No secrets in notes\n> **No secrets in docs, shells, or chat.**", 1)
        head, rest = body.split("\n", 1)
        body = f"{head}\n\nBack to [[Reel Home]] · Repo: `reel/docs/{f}`\n\n{rest.lstrip()}"
        tags, aliases = META[n]
        notes[f"{n}.md"] = frontmatter(n, tags, aliases, f) + body
    # Decisions log from ARCHITECTURE §5
    arch = (SRC / "ARCHITECTURE.md").read_text()
    sec = arch.split("## 5. Architecture choices (and why)", 1)[1].split("\n## 6.", 1)[0]
    out = [frontmatter("Reel Decisions Log", ["reel", "architecture", "decisions"], ["Reel ADRs"]).rstrip("\n"), "",
           "# Reel Decisions Log", "",
           "Back to [[Reel Home]] · Context: [[Reel Architecture#5. Architecture choices (and why)|Architecture choices]]", ""]
    for r in (r for r in sec.strip().split("\n") if r.startswith("| A")):
        c = [x.strip() for x in r.strip("|").split("|")]
        out += [f"## {c[0]}. {c[1].strip('*')}", "", "> [!info] Why", f"> {c[2]}", "",
                "> [!warning] Trade-off accepted", f"> {c[3]}", ""]
    notes["Reel Decisions Log.md"] = "\n".join(out)
    return notes


def strip_date(s):
    return re.sub(r"^updated: .*$", "updated: X", s, flags=re.M)


def main():
    notes = render()
    check = "--check" in sys.argv
    stale = [name for name, text in notes.items()
             if not (DST / name).exists() or strip_date((DST / name).read_text()) != strip_date(text)]
    if check:
        print("Obsidian notes out of date: " + ", ".join(stale) if stale else "Obsidian notes up to date")
        sys.exit(1 if stale else 0)
    if not DST.exists():
        sys.exit(f"vault folder not found: {DST}")
    for name in stale:
        (DST / name).write_text(notes[name])
    home = DST / "Reel Home.md"
    if stale and home.exists():
        home.write_text(re.sub(r"^updated: .*$", f"updated: {TODAY}", home.read_text(), count=1, flags=re.M))
    print(f"synced {len(stale)} note(s): {', '.join(stale) or '—'}")


if __name__ == "__main__":
    main()
