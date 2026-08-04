# spec/vendor/ — vendored zarr-extensions schemas

Verbatim copies of schemas from
[zarr-developers/zarr-extensions](https://github.com/zarr-developers/zarr-extensions)
(`codecs/zfp/schema.json`, `codecs/reshape/schema.json`), licensed
[CC BY 3.0 Unported](https://creativecommons.org/licenses/by/3.0/) by the
Zarr development team.

They exist so the test suite can validate every `zfp` and `reshape` codec
object the codec-series builders emit against the *registered* schemas —
the nd-zfp family adopted the upstream `zfp` codec instead of registering
an `nd_zfp` name, and these files are what holds that adoption to its
word. Refresh them deliberately (they are pinned contracts, not build
inputs) and record the upstream commit when you do.
