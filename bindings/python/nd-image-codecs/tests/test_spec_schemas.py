"""The codec schemas must match the codecs they describe.

Two kinds of schema are held to account here. ``spec/codecs/<name>/schema.json``
is what this project would submit to
[zarr-extensions](https://github.com/zarr-developers/zarr-extensions) for its
own codecs (``nd_lift``, ``htj2k``); ``spec/vendor/*.schema.json`` are
verbatim copies of the *registered* ``zfp`` and ``reshape`` schemas, which
the nd-zfp family adopted instead of registering an ``nd_zfp`` name. A
specification that drifts from the implementation is worse than none, so:

- every codec object the committed fixture matrix produces validates against
  the schema for its name — the vendored upstream schema for ``zfp`` and
  ``reshape``, so the builders cannot emit anything the registered codecs
  would not accept, and
- every configuration the codecs *reject* is rejected by the schema too, so
  the schema is not merely permissive.
"""

from __future__ import annotations

import json
import pathlib

import pytest

jsonschema = pytest.importorskip("jsonschema")

from conftest import REPO  # noqa: E402

SPEC = REPO / "spec" / "codecs"
VENDOR = REPO / "spec" / "vendor"
MATRIX = REPO / "fixtures" / "codec-series" / "matrix.json"

#: The codecs this project specifies (stock Zarr codecs are out of scope).
OURS = {"nd_lift", "htj2k"}
#: Registered codecs the builders emit, validated against the vendored
#: upstream schemas.
ADOPTED = {"zfp", "reshape"}


def schema_for(name: str) -> dict:
    path = (VENDOR / f"{name}.schema.json") if name in ADOPTED else (
        SPEC / name / "schema.json"
    )
    assert path.is_file(), f"{name} has no schema at {path}"
    return json.loads(path.read_text())


def matrix_codecs() -> list[dict]:
    cases = json.loads(MATRIX.read_text())["cases"]
    return [
        codec
        for case in cases
        for codec in case.get("expected", [])
        if codec["name"] in OURS | ADOPTED
    ]


def test_the_matrix_exercises_every_specified_codec() -> None:
    """A schema nothing validates against would pass vacuously."""
    names = {codec["name"] for codec in matrix_codecs()}
    assert names == OURS | ADOPTED, f"the fixture matrix only produces {sorted(names)}"


def test_every_builder_emitted_codec_validates() -> None:
    schemas = {name: schema_for(name) for name in OURS | ADOPTED}
    for codec in matrix_codecs():
        jsonschema.validate(codec, schemas[codec["name"]])


@pytest.mark.parametrize("name", sorted(OURS))
def test_schemas_are_valid_json_schema(name: str) -> None:
    schema = schema_for(name)
    jsonschema.Draft202012Validator.check_schema(schema)
    assert schema["properties"]["name"]["const"] == name


@pytest.mark.parametrize("name", sorted(ADOPTED))
def test_vendored_schemas_are_valid_json_schema(name: str) -> None:
    jsonschema.Draft202012Validator.check_schema(schema_for(name))


#: Configurations the schemas must reject. Most are things the codecs refuse
#: too; the exception is deliberate: the schemas are a contract about what a
#: conforming **writer** may emit, while the parsers are **readers** and may
#: accept more. Legacy ``dims`` is exactly that case — `NdZfpCodec.from_dict`
#: and the Rust ``NdZfpConfig`` keep accepting it so pre-adoption ``nd_zfp``
#: stores stay decodable, but the registered ``zfp`` schema (and therefore
#: every builder) must never emit it. Do not "fix" the parsers to match the
#: schema here.
REJECTED = [
    # Unknown members are errors, not ignored fields.
    ("htj2k", {"name": "htj2k", "configuration": {"xy_levels": 5, "bogus": 1}}),
    # The registered zfp schema: exactly the mode's own parameter, and no
    # legacy `dims` — a writer emitting either would break interop (readers
    # still accept `dims`; see the note above).
    ("zfp", {"name": "zfp", "configuration": {"mode": "reversible", "rate": 8.0}}),
    ("zfp", {"name": "zfp", "configuration": {"mode": "reversible", "dims": 3}}),
    ("zfp", {"name": "zfp", "configuration": {"mode": "fixed_rate"}}),
    ("zfp", {"name": "zfp", "configuration": {"mode": "lossless"}}),
    ("reshape", {"name": "reshape", "configuration": {}}),
    ("nd_lift", {"name": "nd_lift", "configuration": {"version": "0.1", "transforms": [
        {"axis": "z", "dimension": 0, "kind": "haar", "levels": 1, "quantize": True}
    ]}}),
    # Out-of-range values.
    ("htj2k", {"name": "htj2k", "configuration": {"xy_levels": 40}}),
    ("htj2k", {"name": "htj2k", "configuration": {"progression": "SNAKE"}}),
    ("zfp", {"name": "zfp", "configuration": {"mode": "fixed_precision", "precision": -1}}),
    # The nd_lift version gate, and lifting kinds without levels.
    ("nd_lift", {"name": "nd_lift", "configuration": {"version": "0.2", "transforms": []}}),
    ("nd_lift", {"name": "nd_lift", "configuration": {"transforms": []}}),
    ("nd_lift", {"name": "nd_lift", "configuration": {"version": "0.1", "transforms": [
        {"axis": "z", "dimension": 0, "kind": "lift53", "levels": 0}
    ]}}),
    ("nd_lift", {"name": "nd_lift", "configuration": {"version": "0.1", "transforms": [
        {"axis": "z", "dimension": 0, "kind": "wavelet", "levels": 1}
    ]}}),
    ("nd_lift", {"name": "nd_lift", "configuration": {"version": "0.1", "transforms": [
        {"dimension": 0, "kind": "haar", "levels": 1, "group": -1}
    ]}}),
]


@pytest.mark.parametrize(("name", "codec"), REJECTED, ids=range(len(REJECTED)))
def test_schemas_reject_what_the_codecs_reject(name: str, codec: dict) -> None:
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(codec, schema_for(name))


def test_every_staged_codec_has_a_readme() -> None:
    for name in OURS:
        readme = SPEC / name / "README.md"
        assert readme.is_file(), f"{name} has no staged README"
        text = readme.read_text()
        assert f"MUST be `{name}`" in text, "the README must pin the codec name"
