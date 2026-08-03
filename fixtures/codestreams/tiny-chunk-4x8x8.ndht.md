# tiny-chunk-4x8x8.ndht

A committed `htj2k` chunk-container micro-fixture pinning the version-1
byte layout (`crates/ndic-codestream/src/container.rs`); regenerating it
must be byte-identical (`chunk_fixture_is_byte_stable` in
`crates/ndic-zarr/tests/htj2k_zarrs.rs`).

## Content

- Chunk shape `(4, 8, 8)`, dtype `uint16`, sample `i` holds `(i * 7) mod 4096`.
- Codec configuration: `xy_levels: 2`, `reversible: true`,
  `progression: "RPCL"`, `index: true` (all but `xy_levels` are defaults).
- 919 bytes total.

## Byte regions

| Range | Content |
| --- | --- |
| 0..4 | magic `"ndht"` |
| 4 | version `1` |
| 5 | flags `0x01` (coefficient-plane index present) |
| 6 | `xy_levels` = 2 |
| 7 | ndim = 3 |
| 8..20 | dims, u32 LE each: 4, 8, 8 |
| 20..116 | coefficient-plane index: 4 entries × 24 bytes |
| 116..919 | 4 independent RPCL `.j2c` codestreams |

Each index entry is `u64 offset | u32 len | 3 × u32 prefix` (little-endian);
`prefix[r]` is the byte count from the plane's first byte that decodes
resolutions `0..=r`:

| Plane | Offset | Len | Prefix (R0, R0..1, R0..2) |
| --- | --- | --- | --- |
| 0 | 116 | 200 | 161, 177, 198 |
| 1 | 316 | 200 | 161, 177, 198 |
| 2 | 516 | 201 | 162, 178, 199 |
| 3 | 717 | 202 | 163, 179, 200 |

Every plane declares its actual dynamic range in `Ssiz` (12 bits for this
data), not the storage dtype's nominal 16.
