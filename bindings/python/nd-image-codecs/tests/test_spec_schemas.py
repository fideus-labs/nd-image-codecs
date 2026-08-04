"""The staged zarr-extensions schemas must match the codecs they describe.

``spec/codecs/<name>/schema.json`` is what this project would submit to
[zarr-extensions](https://github.com/zarr-developers/zarr-extensions). A
specification that drifts from the implementation is worse than none, so:

- every codec object the committed fixture matrix produces validates against
  the schema for its name, and
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
MATRIX = REPO / "fixtures" / "codec-series" / "matrix.json"

#: The codecs this project specifies (stock Zarr codecs are out of scope).
OURS = {"nd_lift", "htj2k", "nd_zfp"}


def schema_for(name: str) -> dict:
    path = SPEC / name / "schema.json"
    assert path.is_file(), f"{name} has no staged schema at {path}"
    return json.loads(path.read_text())


def matrix_codecs() -> list[dict]:
    cases = json.loads(MATRIX.read_text())["cases"]
    return [
        codec
        for case in cases
        for codec in case.get("expected", [])
        if codec["name"] in OURS
    ]


def test_the_matrix_exercises_every_specified_codec() -> None:
    """A schema nothing validates against would pass vacuously."""
    names = {codec["name"] for codec in matrix_codecs()}
    assert names == OURS, f"the fixture matrix only produces {sorted(names)}"


def test_every_builder_emitted_codec_validates() -> None:
    schemas = {name: schema_for(name) for name in OURS}
    for codec in matrix_codecs():
        jsonschema.validate(codec, schemas[codec["name"]])


@pytest.mark.parametrize("name", sorted(OURS))
def test_schemas_are_valid_json_schema(name: str) -> None:
    schema = schema_for(name)
    jsonschema.Draft202012Validator.check_schema(schema)
    assert schema["properties"]["name"]["const"] == name


#: Configurations the codecs refuse. Each must fail its schema too — the
#: schema is a contract about what a conforming writer may emit, so it has to
#: be at least as strict as the parser.
REJECTED = [
    # Unknown members are errors, not ignored fields.
    ("htj2k", {"name": "htj2k", "configuration": {"xy_levels": 5, "bogus": 1}}),
    ("nd_zfp", {"name": "nd_zfp", "configuration": {"mode": "reversible", "rate": 8.0}}),
    ("nd_lift", {"name": "nd_lift", "configuration": {"version": "0.1", "transforms": [
        {"axis": "z", "dimension": 0, "kind": "haar", "levels": 1, "quantize": True}
    ]}}),
    # Out-of-range values.
    ("htj2k", {"name": "htj2k", "configuration": {"xy_levels": 40}}),
    ("htj2k", {"name": "htj2k", "configuration": {"progression": "SNAKE"}}),
    ("nd_zfp", {"name": "nd_zfp", "configuration": {"mode": "fixed_precision", "precision": 0}}),
    ("nd_zfp", {"name": "nd_zfp", "configuration": {"mode": "fixed_rate", "rate": 0}}),
    ("nd_zfp", {"name": "nd_zfp", "configuration": {"mode": "reversible", "dims": 5}}),
    # A mode without its parameter, and an unknown mode.
    ("nd_zfp", {"name": "nd_zfp", "configuration": {"mode": "fixed_rate"}}),
    ("nd_zfp", {"name": "nd_zfp", "configuration": {"mode": "lossless"}}),
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
