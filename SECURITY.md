# Security

Please don't open a public issue for a vulnerability. Report it privately through GitHub instead:
**[Report a vulnerability](https://github.com/mrafik92/reel/security/advisories/new)** (the *Security* tab →
*Report a vulnerability*). Include what an attacker can do, the affected version or commit, and the steps to
reproduce it.

Reel is maintained by one person, so there's no fixed response time, but reports are read and answered.

## Supported versions

Only the latest commit on `main` gets fixes. There are no release branches.

## Deployment advice

Reel is meant to run on a private network for now: your LAN, or a VPN such as Tailscale. The hardening work
that has to land before it should face the open internet is tracked in the *M2 – Security gate* milestone,
and the current security posture is summarized in [docs/STATUS.md](docs/STATUS.md) and
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#7-security-model-summary).
