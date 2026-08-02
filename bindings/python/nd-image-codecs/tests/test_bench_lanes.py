"""Argument validation for the shared benchmark lane runner (``bench/py/lanes.py``).

The lane runners are operator tools, so a bad flag has to fail before any
measurement starts — not partway through a run, and never as a silent
success. These cases pin the four rejections; the measurement path itself is
exercised by the benchmark lanes, not here.

``conftest`` puts ``bench/py`` on ``sys.path``.
"""

from __future__ import annotations

import numpy as np
import pytest

from lanes import run_lanes

LANES: dict[str, list[dict]] = {"lane-a": [], "lane-b": []}


def call(monkeypatch: pytest.MonkeyPatch, *argv: str) -> None:
    """Invoke the runner CLI with ``argv``, on a fixture too small to bench."""
    monkeypatch.setattr("sys.argv", ["run_probe.py", *argv])
    run_lanes(
        "probe",
        "probe",
        "slug",
        LANES,
        np.zeros((2, 2), dtype=np.uint16),
        (2, 2),
        ["y", "x"],
    )


@pytest.mark.parametrize(
    ("argv", "message"),
    [
        # `raw_ns` would be empty and `min(raw_ns)` would raise mid-run.
        (["--samples", "0"], "--samples must be at least 1"),
        (["--samples", "-1"], "--samples must be at least 1"),
        # `range(warmup + samples)` runs short while `i >= warmup` stays true,
        # so the record would claim more samples than it holds.
        (["--warmup", "-1"], "--warmup must be non-negative"),
        # Would exit 0 having written nothing, reading as a successful run.
        (["--lanes", ""], "must select at least one lane"),
        (["--lanes", ",, ,"], "must select at least one lane"),
        (["--lanes", "lane-a,nope"], "unknown lanes ['nope']"),
    ],
)
def test_cli_rejects_unusable_arguments(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    argv: list[str],
    message: str,
) -> None:
    with pytest.raises(SystemExit) as exc:
        call(monkeypatch, *argv)
    assert exc.value.code == 2
    assert message in capsys.readouterr().err


def test_lane_errors_list_the_available_lanes(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    # Both lane rejections have to be actionable, not just refusals.
    for argv in (["--lanes", ""], ["--lanes", "nope"]):
        with pytest.raises(SystemExit):
            call(monkeypatch, *argv)
        assert "['lane-a', 'lane-b']" in capsys.readouterr().err
