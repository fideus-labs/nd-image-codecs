#!/usr/bin/env python3
"""Publish the workspace to crates.io, idempotently.

`cargo publish --workspace` is the right tool: it computes the dependency order
across the seven crates, skips the `publish = false` members, and waits for the
index between uploads. What it will not do is tolerate a crate that is already
on the registry at this version — `verify_unpublished` runs over every selected
package up front and bails, so one already-published crate aborts the whole run
before a single upload.

That matters because half a release is the normal failure mode here. Nothing
about crates.io is reversible; if the run dies after four of seven uploads, the
only way forward is to publish the remaining three, and re-running the workflow
has to be the way to do that rather than a workstation with a long-lived token.

So: query the registry first, then hand cargo `--exclude` for everything already
published. Cargo still orders and paces the rest.

Usage:

    CARGO_REGISTRY_TOKEN=… python3 scripts/release/publish-crates.py 0.2.0
    python3 scripts/release/publish-crates.py 0.2.0 --dry-run

Requires: python3, cargo, and network access to crates.io.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import tomllib
import urllib.error
import urllib.request

REPO = pathlib.Path(__file__).resolve().parents[2]
API = "https://crates.io/api/v1/crates"
# crates.io answers 403 to a request without one, which is indistinguishable
# from a real authorization failure unless you know to look for it.
USER_AGENT = "nd-image-codecs release workflow (https://github.com/fideus-labs/nd-image-codecs)"


class RegistryError(Exception):
    """crates.io could not be asked whether a version exists."""


def publishable_members() -> list[tuple[str, str]]:
    """(crate name, manifest-relative path) for every member cargo would upload.

    `publish = false` members — the PyO3 shim and the two benchmark crates —
    are excluded here for the same reason `cargo publish --workspace` excludes
    them, so the registry queries below match what cargo will actually do.
    """
    manifest = tomllib.loads((REPO / "Cargo.toml").read_text(encoding="utf-8"))
    members = []
    for member in manifest["workspace"]["members"]:
        package = tomllib.loads((REPO / member / "Cargo.toml").read_text(encoding="utf-8"))["package"]
        # `publish = false` parses as False; `publish = ["some-registry"]` as a
        # list. Either way, absent or `true` means crates.io.
        if package.get("publish", True) is False:
            continue
        members.append((package["name"], member))
    return members


def declared_versions() -> dict[str, str]:
    """The version each publishable member would upload, per its own manifest.

    `version.workspace = true` — which every crate here uses — parses as
    `{"workspace": True}` and resolves against `[workspace.package]`; a literal
    `version = "…"` is taken as written, the way cargo reads it.
    """
    manifest = tomllib.loads((REPO / "Cargo.toml").read_text(encoding="utf-8"))
    inherited = manifest["workspace"]["package"]["version"]

    versions = {}
    for name, member in publishable_members():
        declared = tomllib.loads((REPO / member / "Cargo.toml").read_text(encoding="utf-8"))
        version = declared["package"]["version"]
        versions[name] = inherited if isinstance(version, dict) else version
    return versions


def published_versions(crate: str) -> set[str]:
    """Every version of `crate` on crates.io; empty if the name is unclaimed.

    A network or parse failure raises rather than returning an empty set: read
    as "not published" it would send cargo at a crate that is already there,
    which fails the release for a reason that has nothing to do with the code.
    """
    request = urllib.request.Request(f"{API}/{crate}", headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            body = json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return set()
        raise RegistryError(f"crates.io returned {error.code} for {crate}") from error
    except (urllib.error.URLError, TimeoutError) as error:
        raise RegistryError(f"could not reach crates.io for {crate}: {error}") from error
    except json.JSONDecodeError as error:
        raise RegistryError(f"crates.io returned unparseable JSON for {crate}") from error

    # A 200 without `versions` is not "no versions published" — it is a
    # response this code does not understand (an API change, a captive portal,
    # an error body served with the wrong status). Defaulting to an empty set
    # would read it as a free name and send cargo at a version that is already
    # live, which fails the release for a reason unrelated to the code.
    if "versions" not in body:
        raise RegistryError(f"crates.io returned no `versions` field for {crate}")

    return {version["num"] for version in body["versions"]}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("version", help="the release version, e.g. 0.2.0")
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="pass --dry-run to cargo: package and verify every crate, upload none",
    )
    args = parser.parse_args()

    members = publishable_members()
    print(f"{len(members)} publishable workspace members\n")

    # `args.version` decides which crates get excluded and which registry
    # queries run, but cargo uploads whatever the manifests say — the two are
    # only ever equal because "Stamp the version from the tag" ran first. If
    # that step is skipped, reordered, or half-applied, this would compute the
    # exclude list for one version and cargo would upload another, and a
    # crates.io upload cannot be replaced or withdrawn. So make the assumption
    # a check.
    mismatched = {
        name: declared for name, declared in declared_versions().items() if declared != args.version
    }
    if mismatched:
        # The member count above went to stdout, which is block-buffered when
        # it is a pipe — without this the report below lands ahead of it.
        sys.stdout.flush()
        print(
            f"publish-crates.py: asked to publish {args.version}, but the manifests read:",
            file=sys.stderr,
        )
        for name, declared in sorted(mismatched.items()):
            print(f"  {name:<18} {declared}", file=sys.stderr)
        print(
            f"\nCargo uploads the manifest version, not the argument. Run\n"
            f"  python3 scripts/release/set-version.py {args.version}\n"
            "first — in CI that is the 'Stamp the version from the tag' step.",
            file=sys.stderr,
        )
        return 1

    try:
        already = [name for name, _ in members if args.version in published_versions(name)]
    except RegistryError as error:
        print(f"publish-crates.py: {error}", file=sys.stderr)
        return 1

    for name, path in members:
        state = "already on crates.io" if name in already else "to publish"
        print(f"  {name:<18} {state:<22} ({path})")

    if len(already) == len(members) and not args.dry_run:
        print(f"\nEvery crate is already at {args.version} on crates.io — nothing to do.")
        return 0

    # --locked: the lockfile is part of the release. If cargo would have to
    # change it to build, the tag does not describe a reproducible tree and
    # that is a release-stopping fact, not something to resolve silently.
    command = ["cargo", "publish", "--workspace", "--locked"]
    if args.dry_run:
        command.append("--dry-run")
    else:
        # Excluded only for a real run: --dry-run downgrades "already exists"
        # to a warning, so the full workspace still verifies end to end.
        for name in already:
            command += ["--exclude", name]

    print("\n$", " ".join(command), flush=True)
    return subprocess.run(command, cwd=REPO, check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main())
