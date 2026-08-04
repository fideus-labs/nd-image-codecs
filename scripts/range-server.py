#!/usr/bin/env python3
"""A static file server that honors HTTP ``Range:`` requests.

``python3 -m http.server`` ignores ``Range:`` entirely and answers 200 with
the whole file — useless for demonstrating byte-range plans. This is the
same thing with single-range support: a ``bytes=`` request is answered 206
with exactly the requested span, and a multi-range request (what a coalesced
``ndic index`` plan produces) is answered as the union span, which is what
``ndic expand --partial`` accepts.

The usage documentation's examples run against this server, and the docs CI
job executes those examples verbatim — so the server the docs tell you to
start is the server CI actually tests. Any real static host that honors
``Range:`` (S3, GCS, nginx, …) behaves the same way.

Usage: range-server.py PORT [--dir DIRECTORY]
"""

from __future__ import annotations

import argparse
import functools
import http.server
import io
import pathlib


class RangeHandler(http.server.SimpleHTTPRequestHandler):
    """SimpleHTTPRequestHandler plus single-range support."""

    def log_message(self, *args) -> None:  # noqa: ANN002 - stdlib signature
        pass

    def send_head(self):  # noqa: ANN201 - stdlib signature
        header = self.headers.get("Range")
        if not header or not header.startswith("bytes="):
            return super().send_head()
        path = self.translate_path(self.path)
        try:
            data = pathlib.Path(path).read_bytes()
        except OSError:
            self.send_error(404)
            return None
        spans = []
        for part in header.removeprefix("bytes=").split(","):
            lo, _, hi = part.partition("-")
            start = int(lo) if lo else 0
            end = int(hi) if hi else len(data) - 1
            spans.append((start, min(end, len(data) - 1)))
        start = min(s for s, _ in spans)
        end = max(e for _, e in spans)
        body = data[start : end + 1]
        self.send_response(206)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Range", f"bytes {start}-{end}/{len(data)}")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Accept-Ranges", "bytes")
        self.end_headers()
        return io.BytesIO(body)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("port", type=int)
    parser.add_argument("--dir", default=".", help="directory to serve (default: cwd)")
    args = parser.parse_args()
    handler = functools.partial(RangeHandler, directory=args.dir)
    with http.server.ThreadingHTTPServer(("127.0.0.1", args.port), handler) as server:
        server.serve_forever()


if __name__ == "__main__":
    main()
