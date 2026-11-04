#!/usr/bin/env python3
"""Deploy Reel to CT 113 and move the tickets that shipped to "Deployed" on the GitHub Project.

Two ways to run it:

* **From GitHub** (normal): Actions → `deploy` → *Run workflow*. Runs on the self-hosted runner (CT 116)
  with `--forced`; see `.github/workflows/deploy.yml`.
* **From the Mac** (fallback): needs `ssh minix-local`, `gh` with the `project` scope, and the checkout.

    deploy/ct113/deploy.py                     # Mac: deploy :latest, update tickets
    deploy/ct113/deploy.py --dry-run           # show what would happen, change nothing
    DEPLOY_SSH=root@10.10.10.10 deploy/ct113/deploy.py --forced
    deploy/ct113/deploy.py --since <sha>       # only (re)process tickets for <sha>..latest, no deploy
        # runner: the SSH key is pinned on minix to the forced command /usr/local/bin/reel-deploy,
        # which only understands `sha` and `deploy`

Ticket convention: reference issues in commit messages with `Refs #12` (or `#12`). Use `Fixes #12` only
when the change needs no verification — GitHub closes it on push, skipping Deployed/Verified.
Tickets referenced by commits between the previously deployed build and the new one get:
  * a comment "Deployed to CT 113 — build <sha>",
  * Status → Deployed (unless already Deployed / Verified / Done).
"""
import json
import os
import re
import subprocess
import sys
import time

REPO = "mrafik92/reel"
OWNER = "mrafik92"
PROJECT_OWNER = "@me"  # the token owner; avoids needing read:user/read:org to resolve the owner type
PROJECT = "1"
SSH = ["ssh", "minix-local"]
HEALTH = "https://reel.rafikii.co/api/v1/health"

DRY = "--dry-run" in sys.argv
SINCE = sys.argv[sys.argv.index("--since") + 1] if "--since" in sys.argv else None
FORCED = "--forced" in sys.argv
FORCED_TARGET = os.environ.get("DEPLOY_SSH", "root@10.10.10.10")
ADVANCE_FROM = {"Backlog", "To do", "Implementing", "Reviewing", None}


def run(*a, check=True):
    r = subprocess.run(a, capture_output=True, text=True)
    if check and r.returncode != 0:
        sys.exit(f"command failed: {' '.join(a[:6])}…\n{r.stderr.strip()[:400]}")
    return r.stdout.strip()


def forced(verb, check=True):
    """Forced-command mode: the remote side runs /usr/local/bin/reel-deploy with `verb`."""
    return run("ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", FORCED_TARGET, verb, check=check)


def remote(cmd, check=True):
    """Run `cmd` (argv list) inside CT 113 via the PVE host, quoting safely through ssh."""
    import shlex
    inner = " ".join(shlex.quote(c) for c in cmd)
    return run(*SSH, f"pct exec 113 -- sh -c {shlex.quote(inner)}", check=check)


def deployed_sha():
    if FORCED:
        out = forced("sha", check=False)
        return out if re.fullmatch(r"[0-9a-f]{7,40}", out or "") else None
    env = remote(["docker", "inspect", "reel", "--format", "{{range .Config.Env}}{{println .}}{{end}}"], check=False)
    m = re.search(r"^REEL_GIT_SHA=([0-9a-f]{7,40})", env, re.M)
    return m.group(1) if m else None


def do_deploy():
    if FORCED:
        print(forced("deploy"))
    else:
        remote(["sh", "-c", "cd /root/reel && docker compose pull -q && docker compose up -d && docker image prune -f"])


def latest_image_sha():
    # Newest successful image build on main.
    out = run("gh", "run", "list", "-R", REPO, "--workflow", "image", "--branch", "main", "--status", "success",
              "--limit", "1", "--json", "headSha", "--jq", ".[0].headSha")
    return out or None


def main():
    old = deployed_sha()
    new = latest_image_sha()
    print(f"deployed: {old or '?'}  →  latest image: {new or '?'}")
    if not new:
        sys.exit("no successful image build found")
    if SINCE:
        print(f"--since {SINCE[:12]}: only updating tickets for {SINCE[:7]}..{new[:7]} (no deploy)")
        old, skip_deploy = SINCE, True
    elif old and new.startswith(old[:12]):
        print("already up to date")
        return
    else:
        skip_deploy = False
    # the deploy pulls :latest; refuse if a newer image build is still running (it would race the tag)
    running = run("gh", "run", "list", "-R", REPO, "--workflow", "image", "--branch", "main", "--status", "in_progress",
                  "--limit", "1", "--json", "headSha", "--jq", ".[0].headSha // empty", check=False)
    if running and not DRY and not skip_deploy:
        sys.exit(f"an image build for {running[:7]} is still running — wait for it, then deploy again")

    if not DRY and not skip_deploy:
        do_deploy()
        for _ in range(20):
            time.sleep(3)
            cur = deployed_sha()
            if cur and new.startswith(cur[:12]):
                break
        else:
            sys.exit(f"CT 113 still reports build {deployed_sha()!r}, expected {new[:12]} — check `docker compose ps` on CT 113")
        health = ""
        for _ in range(10):
            health = run("curl", "-s", "-m", "10", HEALTH, check=False)
            if '"ok"' in health:
                break
            time.sleep(3)
        print("health:", health)
        if '"ok"' not in health:
            sys.exit("health check failed — investigate before touching tickets")
        summary = os.environ.get("GITHUB_STEP_SUMMARY")
        if summary:
            with open(summary, "a") as f:
                f.write(f"### Deployed `{new[:7]}` to CT 113\n\nPrevious build: `{(old or '?')[:7]}` · health: `{health}`\n")

    # Commits that shipped.
    run("git", "fetch", "-q", "origin")
    rng = f"{old}..{new}" if old else f"{new}~20..{new}"
    log = run("git", "log", "--format=%h%x09%s%x09%b%x1e", rng, check=False)
    refs = {}
    for entry in log.split("\x1e"):
        entry = entry.strip()
        if not entry:
            continue
        sha = entry.split("\t", 1)[0]
        entry = re.sub(r"Merge pull request #\d+", "", entry)  # the PR number of a merge commit isn't a ticket
        for n in re.findall(r"(?<![\w/])#(\d+)\b", entry):
            refs.setdefault(int(n), []).append(sha)
    # keep issues only (a bare "#70" may be a PR)
    for n in list(refs):
        kind = run("gh", "api", f"repos/{REPO}/issues/{n}", "--jq", "if .pull_request then \"pr\" else \"issue\" end", check=False)
        if kind != "issue":
            refs.pop(n)
    print(f"commits {rng}: tickets referenced: {sorted(refs) or 'none'}")
    if not refs:
        return

    pid = run("gh", "project", "view", PROJECT, "--owner", PROJECT_OWNER, "--format", "json", "--jq", ".id")
    fields = json.loads(run("gh", "project", "field-list", PROJECT, "--owner", PROJECT_OWNER, "--format", "json"))["fields"]
    status = next(f for f in fields if f["name"] == "Status")
    deployed_opt = next(o["id"] for o in status["options"] if o["name"] == "Deployed")
    items = json.loads(run("gh", "project", "item-list", PROJECT, "--owner", PROJECT_OWNER, "--format", "json", "--limit", "500"))["items"]
    by_num = {i["content"].get("number"): i for i in items if i.get("content")}

    short = new[:7]
    for num, shas in sorted(refs.items()):
        item = by_num.get(num)
        cur = item.get("status") if item else None
        note = f"🚀 Deployed to CT 113 — build `{short}` (commits {', '.join(sorted(set(shas)))}). Verify on the real setup, then move to **Verified**."
        print(f"#{num}: status {cur!r}", "→ Deployed" if cur in ADVANCE_FROM else "(unchanged)")
        if DRY:
            continue
        run("gh", "issue", "comment", str(num), "-R", REPO, "-b", note, check=False)
        if cur in ADVANCE_FROM:
            if not item:
                item_id = json.loads(run("gh", "project", "item-add", PROJECT, "--owner", PROJECT_OWNER, "--url",
                                         f"https://github.com/{REPO}/issues/{num}", "--format", "json"))["id"]
            else:
                item_id = item["id"]
            run("gh", "project", "item-edit", "--id", item_id, "--project-id", pid, "--field-id", status["id"],
                "--single-select-option-id", deployed_opt)


if __name__ == "__main__":
    main()
