#!/usr/bin/env python3
"""Generate crates/ndic-htj2k/src/block/tables_data.rs from OpenJPH sources.

Parses the CxtVLC source tables (`table0.h` / `table1.h`) from an OpenJPH
checkout and emits the raw entries plus every derived lookup table the HT
block coder needs, as Rust statics:

- decoder CxtVLC tables (1024 entries each, initial / non-initial quad rows),
  ported from ``ojph_block_common.cpp::vlc_init_tables``;
- decoder UVLC tables + bias, ported from
  ``ojph_block_common.cpp::uvlc_init_tables``;
- encoder CxtVLC tables (2048 entries each), ported from
  ``ojph_block_encoder.cpp::vlc_init_tables``.

The tables originate from ITU-T T.814 via OpenJPH (BSD-2-Clause,
Copyright (c) 2019-2026 Aous Naman, Kakadu Software Pty Ltd, UNSW Australia).

Usage: scripts/gen-ht-tables.py [path-to-openjph] [output.rs]
"""

from __future__ import annotations

import re
import sys
from pathlib import Path


def parse_src_table(path: Path) -> list[tuple[int, ...]]:
    """Parse `{c_q, rho, u_off, e_k, e_1, cwd, cwd_len},` rows."""
    rows = []
    pat = re.compile(r"\{([^}]*)\}")
    for m in pat.finditer(path.read_text()):
        parts = [p.strip() for p in m.group(1).split(",")]
        rows.append(tuple(int(p, 0) for p in parts))
    assert all(len(r) == 7 for r in rows)
    return rows


def build_dec_vlc(src: list[tuple[int, ...]]) -> list[int]:
    """Port of ojph_block_common.cpp::vlc_init_tables (one table)."""
    tbl = [0] * 1024
    for i in range(1024):
        cwd = i & 0x7F
        c_q = i >> 7
        for (s_cq, rho, u_off, e_k, e_1, s_cwd, cwd_len) in src:
            if s_cq == c_q and s_cwd == (cwd & ((1 << cwd_len) - 1)):
                tbl[i] = (rho << 4) | (u_off << 3) | (e_k << 12) | (e_1 << 8) | cwd_len
    return tbl


def build_enc_vlc(src: list[tuple[int, ...]]) -> list[int]:
    """Port of ojph_block_encoder.cpp::vlc_init_tables (one table)."""
    tbl = [0] * 2048
    for i in range(2048):
        c_q = i >> 8
        rho = (i >> 4) & 0xF
        emb = i & 0xF
        if (emb & rho) != emb or (rho == 0 and c_q == 0):
            tbl[i] = 0
            continue
        best = None
        if emb:
            best_e_k = -1
            for row in src:
                if row[0] == c_q and row[1] == rho and row[2] == 1:
                    if (emb & row[3]) == row[4]:
                        ones = bin(row[3]).count("1")
                        if ones >= best_e_k:
                            best = row
                            best_e_k = ones
        else:
            for row in src:
                if row[0] == c_q and row[1] == rho and row[2] == 0:
                    best = row
                    break
        assert best is not None, (c_q, rho, emb)
        tbl[i] = (best[5] << 8) | (best[6] << 4) | best[3]
    return tbl


# Prefix decode helper table, indexed by the 3 LSBs of the VLC stream:
# value = prefix_len | (suffix_len << 2) | (u_pfx << 5).
DEC = [
    3 | (5 << 2) | (5 << 5),  # 000
    1 | (0 << 2) | (1 << 5),  # xx1
    2 | (0 << 2) | (2 << 5),  # x10
    1 | (0 << 2) | (1 << 5),  # xx1
    3 | (1 << 2) | (3 << 5),  # 100
    1 | (0 << 2) | (1 << 5),  # xx1
    2 | (0 << 2) | (2 << 5),  # x10
    1 | (0 << 2) | (1 << 5),  # xx1
]


def build_uvlc_tbl0() -> tuple[list[int], list[int]]:
    """Port of ojph_block_common.cpp::uvlc_init_tables, initial rows."""
    tbl = [0] * (256 + 64)
    bias = [0] * (256 + 64)
    for i in range(256 + 64):
        mode = i >> 6
        vlc = i & 0x3F
        if mode == 0:
            continue
        if mode <= 2:
            d = DEC[vlc & 0x7]
            total_prefix = d & 0x3
            total_suffix = (d >> 2) & 0x7
            u0_suffix_len = total_suffix if mode == 1 else 0
            u0 = (d >> 5) if mode == 1 else 0
            u1 = 0 if mode == 1 else (d >> 5)
        elif mode == 3:
            d0 = DEC[vlc & 0x7]
            vlc >>= d0 & 0x3
            d1 = DEC[vlc & 0x7]
            if (d0 & 0x3) == 3:
                total_prefix = (d0 & 0x3) + 1
                u0_suffix_len = (d0 >> 2) & 0x7
                total_suffix = u0_suffix_len
                u0 = d0 >> 5
                u1 = (vlc & 1) + 1
                bias[i] = 4
            else:
                total_prefix = (d0 & 0x3) + (d1 & 0x3)
                u0_suffix_len = (d0 >> 2) & 0x7
                total_suffix = u0_suffix_len + ((d1 >> 2) & 0x7)
                u0 = d0 >> 5
                u1 = d1 >> 5
        else:  # mode == 4, both u_off = 1, MEL event = 1
            d0 = DEC[vlc & 0x7]
            vlc >>= d0 & 0x3
            d1 = DEC[vlc & 0x7]
            total_prefix = (d0 & 0x3) + (d1 & 0x3)
            u0_suffix_len = (d0 >> 2) & 0x7
            total_suffix = u0_suffix_len + ((d1 >> 2) & 0x7)
            u0 = (d0 >> 5) + 2
            u1 = (d1 >> 5) + 2
            bias[i] = 10
        tbl[i] = (
            total_prefix
            | (total_suffix << 3)
            | (u0_suffix_len << 7)
            | (u0 << 10)
            | (u1 << 13)
        )
    return tbl, bias


def build_uvlc_tbl1() -> list[int]:
    """Port of ojph_block_common.cpp::uvlc_init_tables, non-initial rows."""
    tbl = [0] * 256
    for i in range(256):
        mode = i >> 6
        vlc = i & 0x3F
        if mode == 0:
            continue
        if mode <= 2:
            d = DEC[vlc & 0x7]
            total_prefix = d & 0x3
            total_suffix = (d >> 2) & 0x7
            u0_suffix_len = total_suffix if mode == 1 else 0
            u0 = (d >> 5) if mode == 1 else 0
            u1 = 0 if mode == 1 else (d >> 5)
        else:  # mode == 3
            d0 = DEC[vlc & 0x7]
            vlc >>= d0 & 0x3
            d1 = DEC[vlc & 0x7]
            total_prefix = (d0 & 0x3) + (d1 & 0x3)
            u0_suffix_len = (d0 >> 2) & 0x7
            total_suffix = u0_suffix_len + ((d1 >> 2) & 0x7)
            u0 = d0 >> 5
            u1 = d1 >> 5
        tbl[i] = (
            total_prefix
            | (total_suffix << 3)
            | (u0_suffix_len << 7)
            | (u0 << 10)
            | (u1 << 13)
        )
    return tbl


def fmt_array(name: str, ty: str, vals: list[int], doc: str, per_line: int = 12) -> str:
    lines = [f"/// {doc}", "#[rustfmt::skip]", f"pub static {name}: [{ty}; {len(vals)}] = ["]
    width = 6 if ty == "u16" else 4
    for i in range(0, len(vals), per_line):
        chunk = ", ".join(f"{v:#0{width}x}" for v in vals[i : i + per_line])
        lines.append(f"    {chunk},")
    lines.append("];")
    return "\n".join(lines)


def fmt_raw(name: str, rows: list[tuple[int, ...]], doc: str) -> str:
    lines = [
        f"/// {doc}",
        "#[cfg_attr(not(test), allow(dead_code))]",
        "#[rustfmt::skip]",
        f"pub static {name}: [VlcSrcEntry; {len(rows)}] = [",
    ]
    for i in range(0, len(rows), 3):
        chunk = " ".join(
            f"e({r[0]}, {r[1]:#x}, {r[2]}, {r[3]:#x}, {r[4]:#x}, {r[5]:#04x}, {r[6]}),"
            for r in rows[i : i + 3]
        )
        lines.append(f"    {chunk}")
    lines.append("];")
    return "\n".join(lines)


def main() -> None:
    ojph = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("target/tools/openjph")
    out = (
        Path(sys.argv[2])
        if len(sys.argv) > 2
        else Path("crates/ndic-htj2k/src/block/tables_data.rs")
    )
    coding = ojph / "src" / "core" / "coding"
    tbl0 = parse_src_table(coding / "table0.h")
    tbl1 = parse_src_table(coding / "table1.h")

    uvlc0, bias0 = build_uvlc_tbl0()

    parts = [
        "//! CxtVLC / UVLC lookup tables for the HT (FBCOT) block coder.",
        "//!",
        "//! GENERATED by `scripts/gen-ht-tables.py` from the OpenJPH sources",
        "//! (`table0.h` / `table1.h`, BSD-2-Clause, Copyright (c) 2019-2026",
        "//! Aous Naman, Kakadu Software Pty Ltd, UNSW Australia); the codeword",
        "//! assignments originate in ITU-T T.814 Annex C. Do not edit by hand;",
        "//! `tables::derivations_match_static_tables` re-derives every table",
        "//! from the raw entries and asserts equality.",
        "",
        "/// One row of the T.814 CxtVLC code tables: context, significance",
        "/// pattern, u-offset flag, EMB patterns, codeword and its length.",
        "/// Consumed only by the table-derivation tests.",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
        "#[cfg_attr(not(test), allow(dead_code))]",
        "pub struct VlcSrcEntry {",
        "    /// Quad context.",
        "    pub c_q: u8,",
        "    /// Significance pattern (4 bits).",
        "    pub rho: u8,",
        "    /// 1 if a u value is communicated.",
        "    pub u_off: u8,",
        "    /// EMB `e_k` pattern.",
        "    pub e_k: u8,",
        "    /// EMB `e_1` pattern.",
        "    pub e_1: u8,",
        "    /// VLC codeword (LSB-first in the bitstream).",
        "    pub cwd: u8,",
        "    /// Codeword length in bits (<= 7).",
        "    pub cwd_len: u8,",
        "}",
        "",
        "#[cfg_attr(not(test), allow(dead_code))]",
        "const fn e(c_q: u8, rho: u8, u_off: u8, e_k: u8, e_1: u8, cwd: u8, cwd_len: u8) -> VlcSrcEntry {",
        "    VlcSrcEntry { c_q, rho, u_off, e_k, e_1, cwd, cwd_len }",
        "}",
        "",
        fmt_raw("RAW_TBL0", tbl0, "T.814 CxtVLC source rows for the initial quad row."),
        "",
        fmt_raw("RAW_TBL1", tbl1, "T.814 CxtVLC source rows for non-initial quad rows."),
        "",
        fmt_array(
            "DEC_VLC_TBL0",
            "u16",
            build_dec_vlc(tbl0),
            "Decoder CxtVLC, initial quad row. Index `(c_q << 7) | next7bits`; "
            "entry `e_k<<12 | e_1<<8 | rho<<4 | u_off<<3 | cwd_len`.",
        ),
        "",
        fmt_array(
            "DEC_VLC_TBL1",
            "u16",
            build_dec_vlc(tbl1),
            "Decoder CxtVLC, non-initial quad rows. Same layout as [`DEC_VLC_TBL0`].",
        ),
        "",
        fmt_array(
            "ENC_VLC_TBL0",
            "u16",
            build_enc_vlc(tbl0),
            "Encoder CxtVLC, initial quad row. Index `(c_q << 8) | (rho << 4) | eps`; "
            "entry `cwd<<8 | cwd_len<<4 | e_k`.",
        ),
        "",
        fmt_array(
            "ENC_VLC_TBL1",
            "u16",
            build_enc_vlc(tbl1),
            "Encoder CxtVLC, non-initial quad rows. Same layout as [`ENC_VLC_TBL0`].",
        ),
        "",
        fmt_array(
            "UVLC_TBL0",
            "u16",
            uvlc0,
            "Decoder UVLC, initial quad row. Index `mode*64 + next6bits` where "
            "`mode = u_off0 + 2*u_off1 (+4 on MEL event)`; entry `total_prefix | "
            "total_suffix<<3 | u0_suffix_len<<7 | u0_pfx<<10 | u1_pfx<<13`.",
        ),
        "",
        fmt_array(
            "UVLC_BIAS",
            "u8",
            bias0,
            "Initial-row u bias pairs (2 bits each) used by u_ext decoding.",
            per_line=16,
        ),
        "",
        fmt_array(
            "UVLC_TBL1",
            "u16",
            build_uvlc_tbl1(),
            "Decoder UVLC, non-initial quad rows. Same layout as [`UVLC_TBL0`].",
        ),
        "",
    ]
    out.write_text("\n".join(parts))
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
