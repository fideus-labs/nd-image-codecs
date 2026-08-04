#!/usr/bin/env python3
"""A static file server that honors HTTP ``Range:`` requests.

``python3 -m http.server`` ignores ``Range:`` entirely and answers 200 with
the whole file — useless for demonstrating byte-range plans. This is the
same thing with single-range support: a ``bytes=`` request is answered 206
with exactly the requested span. A multi-range request (what a coalesced
``ndic index`` plan produces) is answered as the union span of its
satisfiable members — one part whose ``Content-Range`` states exactly what
was sent, not a ``multipart/byteranges`` body — because that single
coalesced span is what ``ndic expand --partial`` consumes, and parsing MIME
parts would add machinery the examples never need. Any range-aware client
stays correct: the headers describe precisely what arrives.

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
import os


class _Span:
    """A bounded view of an open file: reads at most ``remaining`` bytes.

    ``SimpleHTTPRequestHandler.copyfile`` streams whatever ``send_head``
    returns until ``read`` comes back empty, so the cap is what turns an
    open file into exactly one range — without ever holding the span (let
    alone the file) in memory.
    """

    def __init__(self, f, remaining: int) -> None:
        self._f = f
        self._remaining = remaining

    def read(self, size: int = -1) -> bytes:
        if size < 0 or size > self._remaining:
            size = self._remaining
        data = self._f.read(size)
        self._remaining -= len(data)
        return data

    def close(self) -> None:
        self._f.close()


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
            f = open(path, "rb")  # noqa: SIM115 - handed to copyfile, closed by do_GET
        except OSError:
            self.send_error(404)
            return None
        size = os.fstat(f.fileno()).st_size
        try:
            spans = []
            for part in header.removeprefix("bytes=").split(","):
                lo, _, hi = part.partition("-")
                if lo:
                    start = int(lo)
                    end = int(hi) if hi else size - 1
                else:
                    # Suffix range: the last `hi` bytes of the file.
                    start = max(size - int(hi), 0)
                    end = size - 1
                if end < start:
                    # An inverted spec (bytes=5-2) makes the whole header
                    # syntactically invalid (RFC 9110 §14.1.1).
                    raise ValueError(part)
                spans.append((start, end))
        except ValueError:
            # Malformed Range: ignore it and serve the whole file, as real
            # static hosts do.
            f.close()
            return super().send_head()
        # Drop unsatisfiable members (start beyond EOF) so they cannot widen
        # the union below; 416 only when no member is satisfiable.
        spans = [(s, min(e, size - 1)) for s, e in spans if s < size]
        if not spans:
            f.close()
            self.send_response(416)
            self.send_header("Content-Range", f"bytes */{size}")
            self.end_headers()
            return None
        start = min(s for s, _ in spans)
        end = max(e for _, e in spans)
        f.seek(start)
        self.send_response(206)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.send_header("Content-Length", str(end - start + 1))
        self.send_header("Accept-Ranges", "bytes")
        self.end_headers()
        return _Span(f, end - start + 1)


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
