---
title: Trusted Publishing
description: The one-time setup that lets the release workflow authenticate to crates.io, PyPI, and npm with OpenID Connect instead of stored API tokens.
---

# Trusted Publishing

[Publishing](./publishing.md) runs entirely from GitHub Actions, and it holds no
registry credentials. Every upload authenticates with **OpenID Connect**: the
workflow asks GitHub for a short-lived, cryptographically signed token that
states which repository, workflow file, and environment it came from, and the
registry exchanges that for a publish token valid for minutes.

This page is the setup that has to exist before any of it works: ten publisher
registrations across three registries, done once, by hand. Nothing in this
repository can create them, and nothing here breaks if they are missing — until
the moment a release is attempted.

## Why

The alternative is an API token per registry, stored in GitHub Actions secrets.
Those tokens do not expire, they are readable by any workflow in the repository
that asks for them, and they survive the departure of whoever created them. A
supply-chain compromise anywhere in this repository's CI — a malicious
dependency in a build script, a poisoned cache entry — turns into the ability to
publish under this project's name on three registries, permanently.

With Trusted Publishing there is no such token to steal. A token exists only
inside the job that minted it, expires on its own, and is revoked when the job
ends. What replaces "who holds the secret" is "which workflow file, running in
which environment, on which repository" — a claim GitHub signs and the registry
verifies.

## Step 1: the `release` environment

**Create this first.** Every publisher below names it, and a publisher
configured for an environment rejects a token that does not carry that claim.

GitHub → **Settings** → **Environments** → **New environment** → name it
`release`.

That name is the whole security boundary. The release workflow has eleven jobs,
and only the four that upload run `environment: release`. A token minted by any
other job in the workflow — the build jobs, the changelog job, anything a
compromised dependency could reach — carries no `release` claim and is refused
by every registry. Without the environment, any job in the workflow could
publish.

Three protection rules are worth adding while you are there:

| Rule | Setting | Effect |
| --- | --- | --- |
| **Deployment branches and tags** | Selected refs → add a **tag** rule `v*` | The environment cannot be entered from a branch push or a stray tag |
| **Required reviewers** | Two or more maintainers | Every publish pauses for a human approval before any upload |
| **Prevent self-reviews** | Enabled | The person who started the release cannot be the one who approves it |

Required reviewers put a person between "a tag was pushed" and "three
registries changed forever," which is the single most effective control here.
Note what it does and does not do on its own: GitHub needs **one** listed
reviewer to approve, and by default that reviewer may be whoever triggered the
run. A maintainer releasing their own tag simply clicks approve, and the
control is a confirmation dialog rather than a second pair of eyes.

**Prevent self-reviews** is what makes it a genuine two-person action — with it
enabled, the run's initiator is excluded from approving even when they are a
listed reviewer, so a release always involves a second maintainer. That is a
real cost on a small team, and worth paying deliberately rather than by
accident: enable it and releases block until someone else is available, or
leave it off and record that the approval is a self-check.

It also makes **two** reviewers a hard requirement rather than a preference.
Only one listed reviewer has to approve, but the initiator is not eligible to
be that one — so if the single name on the list is also the one who pushed the
tag, the four publish jobs sit pending with nobody able to approve them, until
the run is cancelled. Add the second maintainer before enabling this, not after
a release wedges.

### One approval per release

The reviewer requirement applies to each job that enters the environment, and
GitHub asks once per set of jobs pending at the same moment. Four jobs enter it,
so the number of prompts is decided by the workflow's shape rather than by this
setting — and the workflow is shaped to make it exactly one.

`release.yml` has a job called `ready` that does nothing except wait for every
gate, every crate package check, the changelog, and all nine Python
distributions. The four publishing jobs declare `needs: [meta, ready]` — and
nothing else. `meta` is there for its outputs and cannot delay them, because
`ready` waits for `meta` as well and so always finishes later. `ready` is
therefore what unblocks all four, at the same instant: they appear as a single
**Review pending deployments** → `release` prompt, and one approval releases
all four.

That is a property of the `needs:` graph, and it is worth keeping. Before this,
`crates-io`, `npm`, and `npm-placeholder` started as soon as `verify` finished
while `pypi` waited another twenty minutes for the wheels, so a release stopped
twice — the second prompt arriving long after the first had already authorized
the release, which is how an approval becomes something people click through.
Giving a publishing job its own `needs:` list splits it back apart.

The **deployment tags rule** has a consequence for manual re-runs: the ref a
run was started against is what the rule tests, so a `workflow_dispatch` run
launched from `main` cannot enter the environment no matter what its `tag`
input says. Always select the tag as the ref — see
[publishing](./publishing.md), "When a release fails partway".

## Step 1b: make release tags immutable

Not part of trusted publishing, but the same one-time setup pass and it closes
a gap nothing in the workflow can close by itself.

GitHub → **Settings** → **Rules** → **Rulesets** → **New tag ruleset**:

| Field | Value |
| --- | --- |
| Target tags | Pattern `v*` |
| Enforcement | Active |
| Rules | **Restrict updates** and **Restrict deletions** |

A git tag is a mutable pointer. The release workflow resolves it once, gates
that commit, and hands the resolved SHA to every downstream job — so within a
run the tag cannot move under it. Across runs it can: re-running a release is
the supported way to finish one that died partway, and if the tag has been
moved in between, the packages that already published came from one commit and
the rest come from another.

`meta` detects that case by comparing against what earlier tag-push runs
recorded (`scripts/release/check-tag-sha.py`) and refuses. This ruleset means
it can never happen in the first place.

## Step 2: crates.io — seven times

Trusted publishing on crates.io is configured **per crate**, so this is repeated
for each of the seven published crates. A crate must already exist on crates.io;
all seven do, from 0.1.0.

For each of `ndic-core`, `ndic-htj2k`, `ndic-codestream`, `ndic-lift`,
`ndic-zfp`, `ndic-zarr`, `ndic-cli`:

crates.io → the crate → **Settings** → **Trusted Publishing** → **Add** →
**GitHub**, then:

| Field | Value |
| --- | --- |
| Repository owner | `fideus-labs` |
| Repository name | `nd-image-codecs` |
| Workflow filename | `release.yml` |
| Environment | `release` |

The workflow filename is the bare name, not a path. Details and the current
field list are in the
[crates.io Trusted Publishing documentation](https://crates.io/docs/trusted-publishing).

> Renaming `.github/workflows/release.yml` invalidates all seven of these at
> once, and you find out at the next release. The same is true of the npm
> configurations below.

## Step 3: PyPI

PyPI → **Your projects** → `nd-image-codecs` → **Manage** → **Publishing** →
**Add a new publisher** → **GitHub**:

| Field | Value |
| --- | --- |
| Owner | `fideus-labs` |
| Repository name | `nd-image-codecs` |
| Workflow name | `release.yml` |
| Environment name | `release` |

PyPI also signs [PEP 740](https://peps.python.org/pep-0740/) attestations for
everything uploaded this way, which is why the release workflow does not sign
the wheels itself. The attestations appear on the release's PyPI page and are
verifiable against this repository's identity.

Rehearsing on [TestPyPI](https://test.pypi.org) needs its own publisher
configured the same way; the release workflow does not upload there, so this is
only worth doing if you are debugging the upload step itself.

## Step 4: npm — twice

npm → the package → **Settings** → **Trusted Publisher** → **GitHub Actions**,
for both `@fideus-labs/nd-image-codecs` and `nd-image-codecs`:

| Field | Value |
| --- | --- |
| Organization or user | `fideus-labs` |
| Repository | `nd-image-codecs` |
| Workflow filename | `release.yml` |
| Environment name | `release` |
| Allowed actions | `npm publish` |

Two constraints worth knowing before you debug a failure:

- **npm 11.5.1 or later is required**, and the failure below that version is an
  authentication error rather than a version complaint. The release workflow
  pins its own npm rather than trusting whatever Node ships with.
- **GitHub-hosted runners only.** Self-hosted runners cannot use npm trusted
  publishing.

Provenance attestations are generated automatically for anything published this
way, so the packages carry a verifiable link back to the commit and workflow run
that built them. Nothing in `package.json` needs to opt in.

## Verifying it works

There is no dry run for authentication — the OIDC exchange happens only during a
real publish. What you can do is make the first release after this setup a
patch release, and watch the four publishing jobs. Each fails fast and loudly on
a missing or mismatched publisher, before uploading anything.

The tell for a misconfiguration is an authorization failure in a job that got as
far as building successfully:

| Symptom | Cause |
| --- | --- |
| crates.io: `authentication failed` on one crate, others fine | That crate's trusted publisher is missing — it is per crate |
| Any registry: rejected despite a correct-looking publisher | The `environment` field disagrees with `environment: release` in the workflow, or the environment does not exist |
| npm: `unable to authenticate` | npm older than 11.5.1, or `Allowed actions` does not include `npm publish` |
| All four fail together | `release.yml` was renamed, or the repository was renamed or transferred |

## Retiring the old tokens

Both mechanisms work at once, which makes the migration safe: configure trusted
publishing, run one release through it, then remove the API tokens.

- crates.io → **Account Settings** → **API Tokens**: revoke the publish token.
- PyPI → **Account settings** → **API tokens**: revoke the project token.
- npm → **Access Tokens**: revoke the automation token.
- GitHub → **Settings** → **Secrets and variables** → **Actions**: delete
  `CARGO_REGISTRY_TOKEN`, `PYPI_API_TOKEN`, `NPM_TOKEN`, or whatever they were
  named.

A token left behind is a token that can still publish. The point of this page is
that no such token exists.

## Further reading

- [crates.io Trusted Publishing](https://crates.io/docs/trusted-publishing)
- [PyPI Trusted Publishers](https://docs.pypi.org/trusted-publishers/)
- [npm Trusted Publishers](https://docs.npmjs.com/trusted-publishers)
- [GitHub: security hardening with OpenID Connect](https://docs.github.com/en/actions/deployment/security-hardening-your-deployments/about-security-hardening-with-openid-connect)
- [GitHub: using environments for deployment](https://docs.github.com/en/actions/deployment/targeting-different-environments/using-environments-for-deployment)
