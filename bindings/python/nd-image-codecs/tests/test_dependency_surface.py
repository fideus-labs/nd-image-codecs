"""The package's dependency surface: NumPy, and nothing else.

``nd-image-codecs`` deliberately declares only NumPy. The nd-delta family
*names* ``numcodecs.delta`` in its Zarr v3 metadata, but naming a codec is not
importing one — ``zarr-python`` resolves that name through numcodecs' own
entry points, and nothing in this package ever imports numcodecs itself.

That distinction is load-bearing rather than cosmetic. numcodecs publishes no
musllinux or Windows/arm64 wheels, so a direct dependency on it makes the
package uninstallable on three of the platforms this project builds wheels
for. The same goes for a module-level ``import zarr`` in ``__init__``: it
would drag numcodecs in transitively and cost the same platforms.

A hidden import is invisible in a development environment, where zarr and
numcodecs are always installed for the rest of the suite — it only surfaces as
an install failure on a user's Alpine container. So the check runs in a
subprocess with both packages blocked at the import system, which is the one
place the mistake cannot hide.
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import tomllib

import pytest

REPO = pathlib.Path(__file__).resolve().parents[4]
PYPROJECT = REPO / "bindings" / "python" / "nd-image-codecs" / "pyproject.toml"

#: Distributions that must stay out of the base install. Every one of them is
#: either numcodecs or a package that requires it (`zarr>=3.1` pins
#: `numcodecs>=0.14`), so admitting any of them to `dependencies` reimposes
#: numcodecs' platform limits on the wheels.
FORBIDDEN_BASE_DEPS = frozenset({"numcodecs", "zarr"})


def _requirement_name(requirement: str) -> str:
    """The bare distribution name from a PEP 508 requirement string."""
    name = requirement.split(";")[0].strip()
    for separator in ("[", "(", "<", ">", "=", "!", "~", " "):
        name = name.split(separator)[0]
    return name.strip().lower().replace("_", "-")


@pytest.fixture(scope="module")
def project_metadata() -> dict:
    """The `[project]` table the wheel's metadata is generated from."""
    return tomllib.loads(PYPROJECT.read_text())["project"]


@pytest.fixture(scope="module")
def base_dependency_names(project_metadata: dict) -> set[str]:
    """Distribution names in `[project] dependencies` — the base install."""
    return {_requirement_name(r) for r in project_metadata["dependencies"]}


def test_declared_base_dependencies_are_numpy_only(base_dependency_names: set[str]) -> None:
    """`[project] dependencies` names NumPy and nothing that pulls numcodecs."""
    assert base_dependency_names == {"numpy"}, (
        f"base dependencies must stay NumPy-only, got {sorted(base_dependency_names)}; "
        "anything else has to earn a platform audit first"
    )


def test_zarr_stays_an_optional_extra(project_metadata: dict) -> None:
    """The zarr integration is opt-in, so the base wheel keeps its platforms."""
    extras = project_metadata["optional-dependencies"]

    assert "zarr" in extras, "the zarr integration must remain reachable as an extra"
    assert {_requirement_name(r) for r in extras["zarr"]} == {"zarr"}


@pytest.mark.parametrize("forbidden", sorted(FORBIDDEN_BASE_DEPS))
def test_forbidden_distributions_are_absent_from_base_metadata(
    forbidden: str, base_dependency_names: set[str]
) -> None:
    """Named individually so a regression report says *which* dep crept back."""
    assert forbidden not in base_dependency_names, (
        f"{forbidden} is back in `dependencies`; it has no musllinux or "
        "Windows/arm64 wheel, so this silently drops three wheel platforms"
    )


# The program below runs in a fresh interpreter: `sys.modules` there is clean,
# which is what makes "was numcodecs imported?" a question worth asking. Run
# in-process it would only observe the rest of this suite's imports.
#
# It runs twice, and the two runs catch different mistakes:
#
# `block` — the extras cannot import at all, which is a user's Alpine
#   container. A module-level `import numcodecs` fails the probe outright.
# `observe` — the extras are installed and importable, and the probe reports
#   whether merely importing the package reached for them. This is the run
#   that catches an import hidden behind `try: ... except ImportError:`,
#   which the blocking run would swallow and pass.
_IMPORT_PROBE = '''
import json, sys


class _Blocked:
    """A meta-path finder that refuses the named distributions.

    Raising from `find_spec` (rather than returning None) makes the failure
    the same ImportError a user without the package installed would see.
    """

    def __init__(self, names):
        self.names = frozenset(names)

    def find_spec(self, fullname, path=None, target=None):
        if fullname.split(".")[0] in self.names:
            raise ImportError(f"{fullname} is blocked: the base install must not need it")
        return None


MODE, WATCHED = sys.argv[1], json.loads(sys.argv[2])
for name in list(sys.modules):
    if name.split(".")[0] in WATCHED:
        del sys.modules[name]
if MODE == "block":
    sys.meta_path.insert(0, _Blocked(WATCHED))

import numpy as np

from nd_image_codecs import NdLift, codec_series

# Captured immediately: this is exactly what a user's `import nd_image_codecs`
# pulled in, before anything below touches the import system again.
result = {
    "mode": MODE,
    "reached": sorted(n for n in sys.modules if n.split(".")[0] in WATCHED),
    "series": {},
    "native": None,
}

for family in ("nd-delta", "nd-lift-ht", "nd-zfp"):
    pipeline = codec_series(list("tczyx"), [2, 1, 8, 32, 32], "uint16", family=family)
    result["series"][family] = [codec["name"] for codec in pipeline]

# The pure-Python lifting transform, which is what `nd_lift` runs.
chunk = np.arange(4 * 8 * 8, dtype=np.int32).reshape(4, 8, 8) % 977
lift = NdLift(transforms=[{"axis": "z", "dimension": 0, "kind": "lift53", "levels": 2, "group": 0}])
result["lift_roundtrip"] = bool(np.array_equal(chunk, lift.decode(lift.encode(chunk), np.int32)))

# The native codecs, when a wheel (rather than the bare source tree) is under
# test. They are the whole reason the base install has to work on musllinux.
try:
    from nd_image_codecs import _nd_image_codecs as native
except ImportError:
    pass
else:
    samples = (np.arange(2 * 16 * 16, dtype=np.uint16) % 4096).reshape(2, 16, 16)
    shape = list(samples.shape)
    htj2k = native.htj2k_decode(native.htj2k_encode(samples.tobytes(), shape, "uint16"), shape, "uint16")
    zfp = native.nd_zfp_decode(native.nd_zfp_encode(samples.tobytes(), shape, "uint16"), shape, "uint16")
    result["native"] = {
        "htj2k_roundtrip": bool(np.array_equal(samples, np.frombuffer(htj2k, dtype=np.uint16).reshape(samples.shape))),
        "zfp_roundtrip": bool(np.array_equal(samples, np.frombuffer(zfp, dtype=np.uint16).reshape(samples.shape))),
    }

# Last, because it imports them: which watched packages this environment
# actually has. Without it an `observe` run cannot tell "did not reach for
# numcodecs" apart from "numcodecs was not installed anyway".
available = []
for name in WATCHED:
    try:
        __import__(name)
    except ImportError:
        continue
    available.append(name)
result["available"] = sorted(available)

print(json.dumps(result))
'''


def _run_probe(mode: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-c", _IMPORT_PROBE, mode, json.dumps(sorted(FORBIDDEN_BASE_DEPS))],
        capture_output=True,
        text=True,
        # conftest puts the bare source tree on sys.path when no wheel is
        # installed; the child needs the same view to import the package.
        env={**os.environ, "PYTHONPATH": os.pathsep.join(p for p in sys.path if p)},
        cwd=REPO,
        check=False,
    )


@pytest.fixture(scope="module")
def blocked_probe() -> dict:
    """Exercise the base API in an interpreter where the extras cannot import."""
    completed = _run_probe("block")
    if completed.returncode != 0:
        pytest.fail(
            "the base API could not run without numcodecs/zarr — a hidden "
            f"import has crept in:\n{completed.stderr}"
        )
    return json.loads(completed.stdout.strip().splitlines()[-1])


@pytest.fixture(scope="module")
def observed_probe() -> dict:
    """Same API, but with the extras installed and importable.

    The blocking run cannot see an import written as ``try: import numcodecs
    / except ImportError: pass`` — the block raises, the handler swallows it,
    and the probe passes while the package still reaches for numcodecs
    whenever it is present. This run is what catches that.
    """
    completed = _run_probe("observe")
    if completed.returncode != 0:
        pytest.fail(f"the base API failed in a normal environment:\n{completed.stderr}")
    return json.loads(completed.stdout.strip().splitlines()[-1])


def test_base_api_runs_without_numcodecs_or_zarr(blocked_probe: dict) -> None:
    """Neither package is importable, and the builder still produces all three families."""
    assert set(blocked_probe["series"]) == {"nd-delta", "nd-lift-ht", "nd-zfp"}
    assert blocked_probe["series"]["nd-lift-ht"] == ["transpose", "nd_lift", "htj2k"]
    assert blocked_probe["series"]["nd-zfp"] == ["transpose", "reshape", "zfp"]
    assert blocked_probe["lift_roundtrip"], "the NumPy lifting transform must round-trip"


def test_nd_delta_names_numcodecs_without_importing_it(blocked_probe: dict) -> None:
    """Naming a codec is not depending on one — the point of the whole file."""
    assert "numcodecs.delta" in blocked_probe["series"]["nd-delta"]


def test_importing_the_package_does_not_reach_for_the_extras(observed_probe: dict) -> None:
    """With numcodecs and zarr installed, importing the package still ignores them."""
    if not observed_probe["available"]:
        pytest.skip("neither numcodecs nor zarr is installed; nothing to observe")

    assert observed_probe["reached"] == [], (
        f"`import nd_image_codecs` pulled in {observed_probe['reached']}; the base "
        "install must not depend on them, even behind a try/except ImportError"
    )


def test_native_codecs_run_without_numcodecs_or_zarr(blocked_probe: dict) -> None:
    """htj2k and zfp are the payload the NumPy-only install exists to deliver."""
    native = blocked_probe["native"]
    if native is None:
        pytest.skip("native extension not installed (bare source tree)")
    assert native["htj2k_roundtrip"], "htj2k must round-trip with only NumPy present"
    assert native["zfp_roundtrip"], "zfp must round-trip with only NumPy present"
