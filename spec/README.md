# spec/ — codec specifications for zarr-extensions

Specification material for the Zarr v3 codecs this project defines or
adopts, in the layout
[zarr-extensions](https://github.com/zarr-developers/zarr-extensions)
expects: one directory per codec under `codecs/`, each with a `README.md`
and a `schema.json`.

| Codec | Kind | Status |
| --- | --- | --- |
| [`nd_lift`](./codecs/nd_lift/) | array → array | Ready to submit |
| [`htj2k`](./codecs/htj2k/) | array → bytes | Ready to submit |
| `zfp` + `reshape` | array → bytes / array → array | **Adopted** from zarr-extensions — nothing to submit |

## The zfp adoption

zarr-extensions already registers a
[`zfp`](https://github.com/zarr-developers/zarr-extensions/tree/main/codecs/zfp)
codec whose stored bytes are identical to what this project produced under
its provisional `nd_zfp` name, so the project adopted the registered name
(and the registered
[`reshape`](https://github.com/zarr-developers/zarr-extensions/tree/main/codecs/reshape)
codec for collapsing singleton chunk dimensions) instead of submitting a
second name for a byte-identical format. `nd_zfp` remains a read alias for
stores written before the adoption; its legacy `dims` member selects the
old in-codec chunk mapping.

[`vendor/`](./vendor/) holds verbatim copies of the registered `zfp` and
`reshape` schemas; the test suite validates every configuration the
codec-series builders emit against them, so the adoption cannot drift.

## Before opening the pull request

For the two remaining submissions (`nd_lift`, `htj2k`):

- zarr-extensions requires extension documents be licensed under
  [CC BY 3.0 Unported](https://creativecommons.org/licenses/by/3.0/), which
  differs from this repository's MIT license. Opening the pull request is
  an acceptance of that term for these documents, so it needs the copyright
  holder's assent rather than an automated commit.
- If the ZFWG assigns different names during review, changing them is a
  breaking format change to stored data and must be handled as one.

## Keeping the specs honest

`bindings/python/nd-image-codecs/tests/test_spec_schemas.py` validates every
codec object the committed fixture matrix produces against the schema for
its name — the staged schema for `nd_lift`/`htj2k`, the vendored upstream
schema for `zfp`/`reshape` — and checks that each schema rejects the
configurations the codecs reject. A specification that drifts from the
implementation fails the test suite, which is the only way a document like
this stays true.
