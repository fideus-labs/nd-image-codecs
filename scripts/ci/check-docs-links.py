#!/usr/bin/env python3
"""External-link check for the documentation site.

Walks every `.md` file under `docs/`, collects the outbound `http(s)` URLs in
both inline (`[text](url)`) and autolink (`<url>`) form, deduplicates them, and
requests each one — `HEAD` first, falling back to `GET` when a server rejects
`HEAD`. Any non-2xx/3xx result is reported with every file and line it appears
on. Internal links between pages are *not* checked here: `myst build --strict`
(`cd docs && npm run check`) already fails on a broken internal cross-reference.

**Deliberately not part of the pull request gate.** The docs cite roughly 90
external specification and vendor URLs (ISO, ITU, LLNL, frontiersin,
kakadusoftware) that rate-limit, bot-block, and go down entirely independently
of this repository. Wiring that into CI would produce a gate that fails for
reasons no contributor can fix, and a flaky gate is worse than no gate. It runs
monthly instead, from `.github/workflows/docs-link-check.yml`, which also
carries a `workflow_dispatch` trigger; run it by hand from there or from a
terminal before a release, or when the reference list changes.

Only a definitively dead target — 404, 410, or a hostname that does not
resolve — makes this script exit non-zero. Blocked (401/403), rate-limited
(429), server-error (5xx), and timed-out URLs are reported as warnings so a
bot-hostile standards body cannot fail the run.

A 404 alone is not proof of death: some hosts (crates.io among them) answer
every non-browser request with 404 rather than 403. Before calling a URL dead,
the origin's own root is probed once — if `https://host/` is equally unhappy,
the host is blocking us and the result is downgraded to a warning.

Usage:

    python3 scripts/ci/check-docs-links.py
    python3 scripts/ci/check-docs-links.py --timeout 30 --jobs 8

Requires: python3 only (standard library, plus outbound network access).
"""

from __future__ import annotations

import argparse
import concurrent.futures
import pathlib
import re
import socket
import ssl
import sys
import urllib.error
import urllib.parse
import urllib.request
from collections import defaultdict

REPO = pathlib.Path(__file__).resolve().parents[2]
DOCS = REPO / "docs"

USER_AGENT = (
    "nd-image-codecs-docs-link-check/1.0 "
    "(+https://github.com/fideus-labs/nd-image-codecs; pre-release documentation check)"
)

DEFAULT_TIMEOUT = 20.0
DEFAULT_JOBS = 4

INLINE = re.compile(r"\[[^\]]*\]\((https?://[^()\s]+)\)")
AUTOLINK = re.compile(r"<(https?://[^<>\s]+)>")
FENCE = re.compile(r"^\s*(```|~~~)")

# Hosts whose URLs are examples of a local server, not outbound references.
LOCAL_HOSTS = ("localhost", "127.0.0.1", "0.0.0.0", "[::1]")

# A dead target, as opposed to a server that is merely unhappy with us.
DEAD_STATUS = (404, 410)

# Statuses that mean "this server dislikes HEAD", not "this URL is broken".
RETRY_WITH_GET = (400, 403, 405, 406, 501)


def is_local(url: str) -> bool:
    host = urllib.parse.urlparse(url).hostname or ""
    return host in LOCAL_HOSTS


def display(path: pathlib.Path) -> pathlib.Path:
    """Repo-relative path when possible; `--docs` may point anywhere."""
    try:
        return path.relative_to(REPO)
    except ValueError:
        return path


def collect_urls(docs: pathlib.Path) -> dict[str, list[tuple[pathlib.Path, int]]]:
    """Map each distinct external URL to every (file, line) that cites it."""
    sites: dict[str, list[tuple[pathlib.Path, int]]] = defaultdict(list)
    for md in sorted(docs.rglob("*.md")):
        if "_build" in md.parts or "node_modules" in md.parts:
            continue
        in_fence, marker = False, None
        for lineno, line in enumerate(md.read_text(encoding="utf-8").splitlines(), start=1):
            fence = FENCE.match(line)
            if fence:
                if not in_fence:
                    in_fence, marker = True, fence.group(1)
                elif fence.group(1) == marker:
                    in_fence, marker = False, None
                continue
            if in_fence:
                continue  # URLs inside code blocks are samples, not references
            for match in (*INLINE.finditer(line), *AUTOLINK.finditer(line)):
                url = match.group(1).rstrip(".,;:")
                if not is_local(url):
                    sites[url].append((display(md), lineno))
    return dict(sites)


def request(url: str, method: str, timeout: float) -> tuple[int, str]:
    """Issue one request. Returns (status, detail); status 0 means no response."""
    req = urllib.request.Request(url, method=method, headers={
        "User-Agent": USER_AGENT,
        "Accept": "*/*",
    })
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, ""
    except urllib.error.HTTPError as exc:
        return exc.code, exc.reason or ""
    except urllib.error.URLError as exc:
        reason = exc.reason
        if isinstance(reason, socket.gaierror):
            return 0, f"DNS: {reason.strerror or reason}"
        if isinstance(reason, TimeoutError):  # socket.timeout is an alias since 3.10
            return 0, f"timeout after {timeout:g}s"
        if isinstance(reason, ssl.SSLError):
            return 0, f"TLS: {reason}"
        return 0, str(reason)
    except (TimeoutError, socket.timeout):
        return 0, f"timeout after {timeout:g}s"
    except (ConnectionError, ValueError, OSError) as exc:
        return 0, str(exc)


def check(url: str, timeout: float) -> tuple[str, int, str]:
    """Check one URL. Returns (verdict, status, detail).

    verdict is one of "ok", "warn" (reported, does not fail the run), or
    "dead" (the target is definitively gone). A "dead" verdict here is still
    provisional — `confirm_dead` gets the final say.
    """
    status, detail = request(url, "HEAD", timeout)
    if status in RETRY_WITH_GET or status == 0:
        get_status, get_detail = request(url, "GET", timeout)
        # Trust GET over HEAD: a server that refuses HEAD still answers GET.
        if get_status:
            status, detail = get_status, get_detail
        elif not status:
            detail = get_detail
    if 200 <= status < 400:
        return "ok", status, detail
    if status in DEAD_STATUS:
        return "dead", status, detail
    if status == 0 and detail.startswith("DNS:"):
        return "dead", 0, detail
    return "warn", status, detail


def host_is_hostile(url: str, timeout: float, cache: dict[str, bool]) -> bool:
    """Does this URL's origin reject even its own root?

    crates.io and friends answer non-browser clients with 404 rather than 403,
    which is indistinguishable from a genuinely missing page. If the origin's
    root is equally unreachable, the host is blocking us, not missing content.
    """
    parts = urllib.parse.urlparse(url)
    origin = f"{parts.scheme}://{parts.netloc}/"
    if origin not in cache:
        status, _ = request(origin, "GET", timeout)
        cache[origin] = not (200 <= status < 400)
    return cache[origin]


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Check the external links cited by the documentation site.",
    )
    parser.add_argument(
        "--docs", type=pathlib.Path, default=DOCS,
        help=f"documentation root to walk (default: {display(DOCS)})",
    )
    parser.add_argument(
        "--timeout", type=float, default=DEFAULT_TIMEOUT,
        help=f"per-request timeout in seconds (default: {DEFAULT_TIMEOUT:g})",
    )
    parser.add_argument(
        "--jobs", type=int, default=DEFAULT_JOBS,
        help=f"concurrent requests (default: {DEFAULT_JOBS}; keep it low to stay polite)",
    )
    args = parser.parse_args()

    if not args.docs.is_dir():
        print(f"error: {args.docs} is not a directory", file=sys.stderr)
        return 2

    sites = collect_urls(args.docs)
    if not sites:
        print(f"no external links found under {args.docs}")
        return 0

    urls = sorted(sites)
    print(f"checking {len(urls)} distinct external links "
          f"({sum(len(v) for v in sites.values())} citations) "
          f"with timeout {args.timeout:g}s\n")

    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
        results = list(pool.map(lambda u: (u, *check(u, args.timeout)), urls))

    # Confirm each provisional death against its origin root, sequentially:
    # there are few of them, and one probe per host keeps the cache trivial.
    origin_cache: dict[str, bool] = {}
    confirmed = []
    for url, verdict, status, detail in results:
        if verdict == "dead" and status and host_is_hostile(url, args.timeout, origin_cache):
            verdict = "warn"
            detail = (detail + "; " if detail else "") + "host also rejects its own root — blocking, not missing"
        confirmed.append((url, verdict, status, detail))

    dead, warned = 0, 0
    for url, verdict, status, detail in confirmed:
        if verdict == "ok":
            print(f"ok   {status} {url}")
            continue
        label = "DEAD" if verdict == "dead" else "warn"
        shown = status if status else "---"
        print(f"{label} {shown} {url}" + (f" ({detail})" if detail else ""))
        for path, lineno in sites[url]:
            print(f"  {path}:{lineno}")
        if verdict == "dead":
            dead += 1
        else:
            warned += 1

    print()
    if dead:
        print(f"{dead} dead link(s), {warned} warning(s), "
              f"{len(urls) - dead - warned}/{len(urls)} ok", file=sys.stderr)
        return 1
    if warned:
        print(f"no dead links; {warned} of {len(urls)} link(s) unverified "
              f"(blocked, rate-limited, or unreachable — see above)")
        return 0
    print(f"all {len(urls)} external links resolve")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
