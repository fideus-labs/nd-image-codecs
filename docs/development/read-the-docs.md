---
title: Read the Docs Deployment
short_title: Read the Docs
description: How the documentation site is deployed to Read the Docs, and the manual setup steps a maintainer must run by hand because they cannot be expressed in the repository.
---

How the documentation site reaches <https://nd-image-codecs.readthedocs.io/>. The
build recipe is in the repository; **everything else is a manual, human-run
procedure** on readthedocs.org, because creating a project, enabling pull request
builds, and managing versions are account-level operations that no file in this
repository can perform.

The project does not exist on Read the Docs yet. Run this page top to bottom once.

## What lives where

| Concern | Where it is set | Changed by |
| --- | --- | --- |
| Build recipe — OS, Node version, the commands that produce the HTML | [`.readthedocs.yaml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/.readthedocs.yaml) at the repository root | a pull request |
| Site content, structure, theme, table of contents | [`docs/myst.yml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/docs/myst.yml) | a pull request |
| Project identity, slug, pull request previews, versions, custom domains | the Read the Docs web UI | a maintainer, by hand |

Read the Docs only reads `.readthedocs.yaml` from the **repository root** — not
from `docs/`. A copy anywhere else is ignored silently.

## How the build works

Read the Docs has no native mystmd builder. `.readthedocs.yaml` therefore uses
`build.commands`, which replaces RTD's default build entirely — no Sphinx, no
mkdocs, no automatic environment creation. Four commands run:

```yaml
commands:
  - cd docs && npm ci
  - cd docs && export BASE_URL="/${READTHEDOCS_LANGUAGE}/${READTHEDOCS_VERSION}" && npm run check
  - mkdir -p "$READTHEDOCS_OUTPUT/html"
  - cp -r docs/_build/html/. "$READTHEDOCS_OUTPUT/html/"
```

Three things about that are load-bearing:

**The build is `npm run check`, not an inline `myst build`.** That script in
`docs/package.json` is the single canonical strict-build command, and the `docs`
job in [`.github/workflows/ci.yml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/.github/workflows/ci.yml)
runs the very same script on every pull request — so a green `docs` check means
this deployment will build. Change the build by editing that script; inlining a
different invocation in either place lets the two silently diverge. `BASE_URL` is
the only thing Read the Docs adds on top, because CI serves nothing and needs no
path prefix.

**Each entry runs in its own shell, starting from the checkout root.** A bare
`cd docs` on its own line does not carry over to the next entry — it changes the
directory of a shell that then exits. Every command that needs `docs/` chains its
own `cd`.

**`BASE_URL` is mandatory.** Read the Docs never serves a project at the domain
root: the default version is published under `/en/latest/` and a pull request
preview under `/en/<pr>/`. Without `BASE_URL`, mystmd emits root-absolute URLs
for the stylesheets, every JavaScript chunk, the logo, the favicon, and every
inter-page link, all of which 404 under a path prefix — the site loads as
unstyled, un-hydrated HTML. It is derived from `READTHEDOCS_LANGUAGE` and
`READTHEDOCS_VERSION` rather than hardcoded to `/en/latest` because
`READTHEDOCS_VERSION` holds the **pull request number** on preview builds;
hardcoding it would publish a working `latest` and a broken preview for every
pull request.

`--strict` fails the build on any warning. A broken cross-reference should stop
the deploy, not publish a degraded site.

## 0. Pre-flight

Rehearse the exact RTD build locally before touching the web UI. Each command
runs in its own shell from the repository root, exactly as RTD runs them:

```bash
OUT=$(mktemp -d)
rm -rf docs/_build

sh -c 'cd docs && npm ci'
sh -c "cd docs && READTHEDOCS_LANGUAGE=en READTHEDOCS_VERSION=latest \
  BASE_URL=\"/en/latest\" npm run check"
sh -c "mkdir -p '$OUT/html'"
sh -c "cp -r docs/_build/html/. '$OUT/html/'"

test -f "$OUT/html/index.html" && echo OK
```

Then serve it the way RTD does — under the version prefix, not at the root — and
confirm nothing 404s:

```bash
SERVE=$(mktemp -d); mkdir -p "$SERVE/en"
cp -r "$OUT/html" "$SERVE/en/latest"
(cd "$SERVE" && python3 -m http.server 8000)
# open http://localhost:8000/en/latest/
```

> Serving `$OUT/html` at the root will appear to work even when the deployment is
> broken. The prefix is the whole point of the test.

Clean up afterwards: `rm -rf docs/_build "$OUT" "$SERVE"`.

## 1. Sign in and connect GitHub

Sign in at <https://app.readthedocs.org/> — the **community** site, which is free
for open source and correct for an MIT-licensed project.

> readthedocs.**com** is Read the Docs for Business, a paid product for private
> documentation. It is not needed here, and signing up there instead is an easy
> mistake to make: the two have separate accounts and separate dashboards.

Connect the GitHub account under **Settings → Connected Services → Connect to
GitHub**, and grant access to the `fideus-labs` organization. Without this, RTD
cannot see `fideus-labs/nd-image-codecs`, cannot install the webhook, and cannot
report build status back onto pull requests.

## 2. Import the repository

**Dashboard → Import a Project**, pick `fideus-labs/nd-image-codecs` from the
list, and confirm the slug reads exactly:

```text
nd-image-codecs
```

It was free as of 2026-08-01, checked both ways — `https://readthedocs.org/projects/nd-image-codecs/`
and `https://nd-image-codecs.readthedocs.io/` each returned 404. If RTD offers a
different slug, someone has taken it since; fall back to `nd-image-codecs-fideus`
and update every URL in this page, the badge in `README.md`, and the link in the
project README.

> The slug is baked into the published URL, the badge URL, and every preview
> URL. Changing it later means a support request and breaking every existing
> link. Get it right at import.

Do **not** fill in anything that `.readthedocs.yaml` already covers. The importer
may offer documentation-type and requirements settings; `build.commands` overrides
all of them.

## 3. Confirm the webhook

**Admin → Integrations** should list a *GitHub incoming webhook* with a URL like
`https://app.readthedocs.org/api/v2/webhook/nd-image-codecs/<id>/`. RTD installs
it automatically when the connected account has permission.

If pushes stop triggering builds, open that integration's detail page. It logs
the HTTP exchange between GitHub and RTD:

| What you see | What it means |
| --- | --- |
| No recent deliveries | GitHub is not calling RTD — re-sync from **Admin → Integrations**, or check the webhook still exists under the repository's GitHub settings |
| Deliveries logged, no builds | RTD received the event but the version is inactive or does not match — check **Versions** |

Manually created integrations cannot report commit status back to GitHub. Prefer
the connected-account webhook.

## 4. Enable pull request builds

**Admin → Settings → Pull request builds**, tick **"Build pull requests for this
project"**, and click **Update**.

> This cannot be set from `.readthedocs.yaml`. It is a project setting, so it
> lives only in the web UI — a fresh import has it **off**, and nothing in the
> repository will turn it on.

Once enabled, every pull request gets a build and a commit status check. Previews
are published to:

```text
https://nd-image-codecs--<pr>.readthedocs.build/en/<pr>/
```

Note the separate domain (`readthedocs.build`, not `readthedocs.io`) and that the
version segment is the pull request number — which is exactly why `BASE_URL` is
derived from the environment.

Previews are not indexed by search engines, are not searchable through RTD's own
search, and are **kept for 90 days** after the pull request is closed or merged,
then deleted. Treat a preview URL as a review aid, never as a citable link.

Anyone who can open a pull request can trigger a build, so never put a secret in
a build environment variable marked public.

## 5. Versions

**Versions** in the project sidebar.

| Version | Tracks | Notes |
| --- | --- | --- |
| `latest` | the default branch, `main` | Created automatically; the default version. Always keep it active |
| `stable` | the highest semver tag, excluding pre-releases | Appears automatically once `v0.0.1` is pushed (see [publishing](./publishing.md)) |

Each version is independently **active** (built and served) or inactive, and
**hidden** (reachable by direct link but absent from the flyout menu and search)
or visible. Deactivating a version deletes its artifacts and serves a 404.

Until there is a real release, leave `latest` as the default version so
<https://nd-image-codecs.readthedocs.io/> redirects to
<https://nd-image-codecs.readthedocs.io/en/latest/>. After 0.0.1 ships, decide
deliberately whether readers should land on `stable` (the released codecs) or
`latest` (the current `main`) — while the roadmap phases are unfinished, `latest`
is the more useful landing page.

Feature branches get their own versions and default to inactive. Leave them that
way; pull request previews already cover in-flight work.

## 6. When a build fails

**Builds** in the project sidebar lists every build; open one for the full log,
which is the raw stdout of the four commands above.

The commands RTD runs are the same ones CI runs — `npm ci` and `myst build
--html --strict` — so a **red RTD build against green CI is usually an
environment difference, not a content problem**. Check in this order:

| Symptom | Likely cause |
| --- | --- |
| `npm ci` fails | `docs/package-lock.json` out of sync with `docs/package.json`; commit the regenerated lockfile |
| `myst: not found` | The command did not run inside `docs/` — an entry lost its `cd` |
| `Premature close` / template fetch error | Transient: mystmd downloads the `book-theme` template from GitHub on a cold build. Re-run the build |
| Strict-mode warnings | A real content problem. Reproduce locally with `cd docs && npm run check` — see [development commands](./commands.md) |
| Site renders unstyled, assets 404 | `BASE_URL` was not exported, or was exported in a different `build.commands` entry than the build |

## Setup checklist

- [ ] Signed in at readthedocs.org (community, **not** readthedocs.com)
- [ ] GitHub connected, with access granted to the `fideus-labs` organization
- [ ] Repository imported as slug `nd-image-codecs`
- [ ] First build green; <https://nd-image-codecs.readthedocs.io/en/latest/> renders with styles, logo, and working navigation
- [ ] **Admin → Settings → "Build pull requests for this project"** enabled
- [ ] Preview verified on a throwaway pull request at `https://nd-image-codecs--<pr>.readthedocs.build/en/<pr>/`
- [ ] Webhook present under **Admin → Integrations** and delivering
- [ ] Default version set to `latest`
- [ ] Badge in the root `README.md` resolves
