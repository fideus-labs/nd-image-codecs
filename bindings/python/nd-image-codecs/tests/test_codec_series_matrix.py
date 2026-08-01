"""Shared fixture-matrix test for the pure-Python `codec_series` builder.

Runs every case in `fixtures/codec-series/matrix.json` — the cross-language
fixture matrix shared with the Rust and TypeScript builders — and asserts the
produced pipeline JSON matches the committed expected output (or that the case
raises when marked `error`).
"""

from __future__ import annotations

import json

import pytest
from conftest import REPO
from nd_image_codecs import codec_series

MATRIX = json.loads((REPO / "fixtures" / "codec-series" / "matrix.json").read_text())
CASES = MATRIX["cases"]


def test_matrix_is_not_truncated() -> None:
    assert len(CASES) > 100


@pytest.mark.parametrize("case", CASES, ids=[c["name"] for c in CASES])
def test_matrix_case(case: dict) -> None:
    kwargs = case.get("options", {})
    if case.get("error"):
        with pytest.raises(ValueError):
            codec_series(case["axes"], case["chunk_shape"], case["dtype"], case["family"], **kwargs)
        return
    got = codec_series(case["axes"], case["chunk_shape"], case["dtype"], case["family"], **kwargs)
    assert got == case["expected"], f"{case['name']}: pipeline diverged from the committed fixture"
