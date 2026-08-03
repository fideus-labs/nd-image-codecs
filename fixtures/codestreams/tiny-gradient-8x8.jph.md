# tiny-gradient-8x8.jph

The boxed (`.jph`, T.814 §B) form of `tiny-gradient-8x8.j2c` — same image,
same coding parameters; see that file's sibling notes for the codestream
internals.

## Byte regions

| Bytes | Box / segment | Notes |
| --- | --- | --- |
| 0..12 | `jP  ` signature | payload `0x0D0A870A` |
| 12..32 | `ftyp` | brand `jph `, minor 0, compat `jph ` |
| 32..77 | `jp2h` | `ihdr` 8x8, 1 component, `BPC 0x07`; `colr` EnumCS 17 (grey) |
| 77..276 | `jp2c` | exact-length box holding the codestream |
| 85..87 | — `SOC` | codestream begins |
| 87..231 | — main header + tile-part | same layout as the `.j2c`, offset by 85 |
| 231..274 | — packet bodies | |
| 274..276 | — `EOC` | |
