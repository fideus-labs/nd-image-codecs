"""Make the pure-Python package importable without an installed wheel.

An *installed* package (wheel or `maturin develop`) wins over the source
tree: it carries the native extension module the htj2k codec needs, which
the bare source tree does not.
"""

from __future__ import annotations

import importlib.util
import pathlib
import sys

_PKG = pathlib.Path(__file__).resolve().parents[1] / "python"
if importlib.util.find_spec("nd_image_codecs") is None and str(_PKG) not in sys.path:
    sys.path.insert(0, str(_PKG))

REPO = pathlib.Path(__file__).resolve().parents[4]

# Shared synthetic-fixture generator (also used by the benchmark lanes).
_BENCH_PY = REPO / "bench" / "py"
if str(_BENCH_PY) not in sys.path:
    sys.path.insert(0, str(_BENCH_PY))
