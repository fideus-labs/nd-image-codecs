# tiny-chunk-2x16x16-mixed.ndht

A committed `htj2k` chunk-container micro-fixture pinning the version-2
byte layout (`crates/ndic-codestream/src/container.rs`), one coded plane and
one stored plane. Regenerating it must be byte-identical
(`chunk_fixture_is_byte_stable` in `crates/ndic-zarr/tests/htj2k_zarrs.rs`,
plus the Python and TypeScript binding tests).

## Content

- Chunk shape `(2, 16, 16)`, dtype `uint16`.
- Plane 0, sample `i` holds `(i * 7) mod 4096`. HT coding shrinks it, so it
  is a codestream.
- Plane 1, sample `i` holds the low 16 bits of a hash of `i + 1`. HT coding
  would take 765 bytes against 516 stored, so it is stored.
- Codec configuration: `xy_levels: 2`, `reversible: true`,
  `progression: "RPCL"`, `index: true` (all but `xy_levels` are defaults).
- 824 bytes total.

The hash is `h = (i + 1) * 0x9E3779B1`, `h ^= h >> 16`,
`h *= 0x85EBCA6B`, `h ^= h >> 13`, all in wrapping `u32` arithmetic.

## Byte regions

| Range | Content |
| --- | --- |
| 0..4 | magic `"ndht"` |
| 4 | version `2` (a plane is stored) |
| 5 | flags `0x01` (coefficient-plane index present) |
| 6 | `xy_levels` = 2 |
| 7 | ndim = 3 |
| 8..20 | dims, u32 LE each: 2, 16, 16 |
| 20..68 | coefficient-plane index: 2 entries × 24 bytes |
| 68..308 | plane 0, an RPCL `.j2c` codestream |
| 308..824 | plane 1, a stored plane: `"ndr"`, kind `0x02`, 256 × u16 LE |

| Plane | Offset | Len | Prefix (R0, R0..1, R0..2) |
| --- | --- | --- | --- |
| 0 | 68 | 240 | 181, 204, 238 |
| 1 | 308 | 516 | 516, 516, 516 |

A stored plane's kind byte is the sample width in bytes, with bit 7 set for
signed samples. Every prefix of a stored plane is its whole length.
