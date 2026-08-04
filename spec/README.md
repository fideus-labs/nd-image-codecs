# spec/ — codec specifications staged for zarr-extensions

Specification documents for the Zarr v3 codecs this project defines, in the
layout [zarr-extensions](https://github.com/zarr-developers/zarr-extensions)
expects: one directory per codec under `codecs/`, each with a `README.md` and
a `schema.json`.

| Codec | Kind | Status |
| --- | --- | --- |
| [`nd_lift`](./codecs/nd_lift/) | array → array | Ready to submit |
| [`htj2k`](./codecs/htj2k/) | array → bytes | Ready to submit |
| [`nd_zfp`](./codecs/nd_zfp/) | array → bytes | **Do not submit as-is** — see below |

## Before opening the pull request

Two things are deliberately left for a human.

**1. `nd_zfp` overlaps a codec that is already registered.** zarr-extensions
carries a `zfp` codec whose stored bytes are identical to ours for the same
data and mode — CI asserts that byte-for-byte against `imagecodecs`. Only the
name and the way the >4-dimensional case is expressed differ. Registering a
second name for a byte-identical format fragments the ecosystem, so the
recommendation is to adopt the registered `zfp` name rather than submit
`nd_zfp`; the [`nd_zfp` README](./codecs/nd_zfp/README.md) states the exact
difference and what adopting `zfp` would cost. That is a breaking format
change, so it is a project decision, not a documentation edit.

**2. Licensing.** zarr-extensions requires that extension documents be
licensed under [CC BY 3.0 Unported](https://creativecommons.org/licenses/by/3.0/),
which differs from this repository's MIT license. Opening the pull request is
an acceptance of that term for these documents, so it needs the copyright
holder's assent rather than an automated commit.

## Keeping the specs honest

`bindings/python/nd-image-codecs/tests/test_spec_schemas.py` validates every codec object the
committed fixture matrix produces against the `schema.json` beside it, and
checks that each schema rejects the configurations the codecs reject. A
specification that drifts from the implementation fails the test suite, which
is the only way a document like this stays true.

## Naming

The names `nd_lift` and `htj2k` follow the Zarr v3 extension naming convention
for unregistered codecs pending registration. If the ZFWG assigns different
names during review, changing them is a breaking format change to stored data
and must be handled as one.
