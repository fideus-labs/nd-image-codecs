# tiny-gradient-8x8.j2c

A minimal single-tile HTJ2K codestream used by parser unit tests
(`crates/ndic-codestream/tests/fixtures.rs`). Byte-stable: the encoder is
deterministic and the test re-encodes and compares.

- **Image**: 8x8, one unsigned 8-bit component, sample `(x, y)` =
  `(7x + 13y) mod 256`.
- **Coding**: 1 decomposition level (5/3 reversible), HT block coder,
  64x64 nominal code-blocks, RPCL, one layer, maximal precincts,
  `TLM`/`PLT` emitted.
- **Produced by**: `ndic compress -i gradient.pgm -o tiny-gradient-8x8.j2c
  --levels 1`.

## Byte regions

| Bytes | Segment | Notes |
| --- | --- | --- |
| 0..2 | `SOC` | |
| 2..45 | `SIZ` | `Rsiz 0x4000`, 8x8 canvas = tile, 1 component `Ssiz 0x07` |
| 45..59 | `COD` | RPCL, 1 layer, 1 level, `cbstyle 0x40` (HT), 5/3 |
| 59..68 | `QCD` | `Sqcd 0x20` (1 guard bit, reversible), 4 subband exponents |
| 68..78 | `CAP` | `Pcap` bit 15, `Ccap15 0x0001` |
| 78..115 | `COM` | writer name |
| 115..125 | `TLM` | `Stlm 0x40`, one 32-bit `Ptlm` = 64 |
| 125..137 | `SOT` | `Isot 0`, `Psot 64`, `TPsot 0`, `TNsot 1` |
| 137..144 | `PLT` | 2 packet lengths |
| 144..146 | `SOD` | |
| 146..189 | packet bodies | r0 (LL) then r1 (HL, LH, HH), one precinct each |
| 189..191 | `EOC` | |
