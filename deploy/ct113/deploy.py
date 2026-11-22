#!/usr/bin/env python3
"""Deploy Reel to CT 113 and move the tickets that shipped to "Deployed" on the GitHub Project.

Run it from the owner's Mac (the only deploy path since #268; the repo is public, so there is no
self-hosted runner). Needs `ssh minix-local`, `gh` with the `project` scope, and the checkout.

    deploy/ct113/deploy.py                     # deploy :latest, update tickets
    deploy/ct113/deploy.py --dry-run           # show what would happen, change nothing
    deploy/ct113/deploy.py --since <sha>       # only (re)process tickets for <sha>..latest, no deploy

Ticket convention: reference issues in commit messages with `Refs #12` (or `#12`). Use `Fixes #12` only
when the change needs no verification — GitHub closes it on push, skipping Deployed/Verified.
Tickets referenced by commits between the previously deployed build and the new one get:
  * a comment "Deployed to CT 113 — build <sha>",
  * Status → Deployed (unless already Deployed / Verified / Done).
"""
import json
import re
import subprocess
import sys
import time

REPO = "mrafik92/reel"
OWNER = "mrafik92"
PROJECT = "1"
SSH = ["ssh", "minix-local"]
HEALTH = "https://reel.rafikii.co/api/v1/health"

DRY = "--dry-run" in sys.argv
SINCE = sys.argv[sys.argv.index("--since") + 1] if "--since" in sys.argv else None
ADVANCE_FROM = {"Backlog", "To do", "Implementing", "Reviewing", None}


def run(*a, check=True):
    r = subprocess.run(a, capture_output=True, text=True)
    if check and r.returncode != 0:
        sys.exit(f"command failed: {' '.join(a[:6])}…\n{r.stderr.strip()[:400]}")
    return r.stdout.strip()


def gql(query, **variables):
    """GitHub GraphQL via `gh api graphql` (only needs the token's scopes for the objects touched)."""
    args = ["gh", "api", "graphql", "-f", f"query={query}"]
    for k, v in variables.items():
        args += ["-F" if isinstance(v, int) else "-f", f"{k}={v}"]
    out = json.loads(run(*args))
    if out.get("errors"):
        sys.exit(f"GraphQL error: {out['errors']}")
    return out["data"]


def remote(cmd, check=True):
    """Run `cmd` (argv list) inside CT 113 via the PVE host, quoting safely through ssh."""
    import shlex
    inner = " ".join(shlex.quote(c) for c in cmd)
    return run(*SSH, f"pct exec 113 -- sh -c {shlex.quote(inner)}", check=check)


def deployed_sha():
    env = remote(["docker", "inspect", "reel", "--format", "{{range .Config.Env}}{{println .}}{{end}}"], check=False)
    m = re.search(r"^REEL_GIT_SHA=([0-9a-f]{7,40})", env, re.M)
    return m.group(1) if m else None


def do_deploy():
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

    # Project updates go through GraphQL directly: `gh project …` insists on read:org/read:discussion,
    # while these queries only need the `project` scope.
    owner, name = REPO.split("/")
    proj = gql("""query($login:String!,$n:Int!){ user(login:$login){ projectV2(number:$n){ id
                 field(name:"Status"){ ... on ProjectV2SingleSelectField { id options { id name } } } } } }""",
               login=OWNER, n=int(PROJECT))["user"]["projectV2"]
    pid, status_field = proj["id"], proj["field"]
    deployed_opt = next(o["id"] for o in status_field["options"] if o["name"] == "Deployed")

    short = new[:7]
    for num, shas in sorted(refs.items()):
        issue = gql("""query($o:String!,$r:String!,$n:Int!){ repository(owner:$o,name:$r){ issue(number:$n){ id
                     projectItems(first:20){ nodes { id project { id }
                       fieldValueByName(name:"Status"){ ... on ProjectV2ItemFieldSingleSelectValue { name } } } } } } }""",
                    o=owner, r=name, n=num)["repository"]["issue"]
        item = next((i for i in issue["projectItems"]["nodes"] if i["project"]["id"] == pid), None)
        cur = (item.get("fieldValueByName") or {}).get("name") if item else None
        note = f"🚀 Deployed to CT 113 — build `{short}` (commits {', '.join(sorted(set(shas)))}). Verify on the real setup, then move to **Verified**."
        print(f"#{num}: status {cur!r}", "→ Deployed" if cur in ADVANCE_FROM else "(unchanged)")
        if DRY:
            continue
        run("gh", "issue", "comment", str(num), "-R", REPO, "-b", note, check=False)
        if cur in ADVANCE_FROM:
            item_id = item["id"] if item else gql(
                "mutation($p:ID!,$c:ID!){ addProjectV2ItemById(input:{projectId:$p,contentId:$c}){ item { id } } }",
                p=pid, c=issue["id"])["addProjectV2ItemById"]["item"]["id"]
            gql("""mutation($p:ID!,$i:ID!,$f:ID!,$o:String!){ updateProjectV2ItemFieldValue(input:{projectId:$p,itemId:$i,
                   fieldId:$f,value:{singleSelectOptionId:$o}}){ projectV2Item { id } } }""",
                p=pid, i=item_id, f=status_field["id"], o=deployed_opt)

if __name__ == "__main__":
    main()
