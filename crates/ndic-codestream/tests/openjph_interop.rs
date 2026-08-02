#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)] // test math
#![allow(clippy::cast_sign_loss, clippy::doc_markdown)]

//! End-to-end interoperability with the `OpenJPH` reference tools:
//!
//! - streams we encode must decode bit-exactly under `ojph_expand`;
//! - streams `ojph_compress` encodes must decode bit-exactly here.
//!
//! Skips (with a note) when the tools are absent; `scripts/ht-differential.sh`
//! documents how the local OpenJPH build is produced.

use std::path::{Path, PathBuf};
use std::process::Command;

use ndic_codestream::{Codestream, encode_image};
use ndic_core::{CoeffPlane, EncodeParams, SampleType};

fn tools_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.join("target/tools/openjph/build/src/apps")
}

fn tmp_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
}

/// Writes a binary PGM (8-bit, or 16-bit big-endian when `maxval > 255`).
fn write_pgm(path: &Path, w: usize, h: usize, maxval: u32, samples: &[i32]) {
    let mut out = format!("P5\n{w} {h}\n{maxval}\n").into_bytes();
    for &s in samples {
        if maxval > 255 {
            out.extend_from_slice(&u16::try_from(s).unwrap().to_be_bytes());
        } else {
            out.push(u8::try_from(s).unwrap());
        }
    }
    std::fs::write(path, out).unwrap();
}

/// Reads a binary PGM into i32 samples.
fn read_pgm(path: &Path) -> (usize, usize, Vec<i32>) {
    let data = std::fs::read(path).unwrap();
    let text = String::from_utf8_lossy(&data[..data.len().min(64)]).to_string();
    let mut fields = Vec::new();
    let mut pos = 0usize;
    let bytes = data.as_slice();
    // Parse "P5 w h maxval" allowing arbitrary whitespace.
    while fields.len() < 4 && pos < bytes.len() {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let start = pos;
        while pos < bytes.len() && !bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        fields.push(String::from_utf8_lossy(&bytes[start..pos]).to_string());
    }
    pos += 1; // single whitespace after maxval
    assert_eq!(fields[0], "P5", "not a raw PGM: {text}");
    let w: usize = fields[1].parse().unwrap();
    let h: usize = fields[2].parse().unwrap();
    let maxval: u32 = fields[3].parse().unwrap();
    let body = &bytes[pos..];
    let samples = if maxval > 255 {
        body.chunks_exact(2)
            .map(|c| i32::from(u16::from_be_bytes([c[0], c[1]])))
            .collect()
    } else {
        body.iter().map(|&b| i32::from(b)).collect()
    };
    (w, h, samples)
}

fn synthetic(w: usize, h: usize, seed: u64, maxval: i64) -> Vec<i32> {
    let mut state = seed | 1;
    (0..w * h)
        .map(|i| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let x = (i % w) as i64;
            let y = (i / w) as i64;
            let base = (x * x / 7 + y * 5 + (x * y) / 11) % (maxval + 1);
            let noise = (state >> 40) as i64 % 64;
            ((base + noise).clamp(0, maxval)) as i32
        })
        .collect()
}

#[test]
fn ojph_expand_decodes_our_streams() {
    let expand = tools_dir().join("ojph_expand/ojph_expand");
    if !expand.exists() {
        eprintln!("skipping: {} not built", expand.display());
        return;
    }
    let tmp = tmp_dir();
    std::fs::create_dir_all(&tmp).unwrap();

    for (w, h, levels, dtype) in [
        (64usize, 64usize, 2u8, SampleType::U8),
        (65, 33, 3, SampleType::U8),
        (256, 100, 5, SampleType::U16),
        (7, 5, 1, SampleType::U8),
        (128, 128, 5, SampleType::U16),
        (1024, 4, 5, SampleType::U8),
    ] {
        let maxval = (1i64 << dtype.bit_depth()) - 1;
        let samples = synthetic(w, h, 42 + w as u64, maxval);
        let plane = CoeffPlane::new(&samples, w, h).unwrap();
        let params = EncodeParams {
            xy_levels: levels,
            ..EncodeParams::default()
        };
        let stream = encode_image(&[plane], dtype, &params).expect("encode");

        let j2c = tmp.join(format!("ours_{w}x{h}_{levels}.j2c"));
        let pgm = tmp.join(format!("ours_{w}x{h}_{levels}.pgm"));
        std::fs::write(&j2c, &stream).unwrap();
        let out = Command::new(&expand)
            .args(["-i", j2c.to_str().unwrap(), "-o", pgm.to_str().unwrap()])
            .output()
            .expect("run ojph_expand");
        assert!(
            out.status.success(),
            "ojph_expand failed for {w}x{h} L{levels}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let (dw, dh, decoded) = read_pgm(&pgm);
        assert_eq!((dw, dh), (w, h));
        assert_eq!(decoded, samples, "{w}x{h} L{levels} {dtype:?}");
    }
    eprintln!("ojph_expand round-trips verified");
}

#[test]
fn we_decode_ojph_compress_streams() {
    let compress = tools_dir().join("ojph_compress/ojph_compress");
    if !compress.exists() {
        eprintln!("skipping: {} not built", compress.display());
        return;
    }
    let tmp = tmp_dir();
    std::fs::create_dir_all(&tmp).unwrap();

    for (w, h, levels, bits) in [
        (64usize, 64usize, 2u8, 8u32),
        (65, 33, 3, 8),
        (256, 100, 5, 16),
        (7, 5, 1, 8),
        (100, 90, 5, 16),
        (4, 1024, 5, 8),
    ] {
        let maxval = (1i64 << bits) - 1;
        // Keep 16-bit values above 0 at 0 decomps to sidestep ojph's own
        // full-range edge; harmless for these level counts.
        let samples = synthetic(w, h, 977 + h as u64, maxval);
        let pgm = tmp.join(format!("theirs_{w}x{h}_{levels}.pgm"));
        let j2c = tmp.join(format!("theirs_{w}x{h}_{levels}.j2c"));
        write_pgm(&pgm, w, h, maxval as u32, &samples);
        let out = Command::new(&compress)
            .args([
                "-i",
                pgm.to_str().unwrap(),
                "-o",
                j2c.to_str().unwrap(),
                "-reversible",
                "true",
                "-num_decomps",
                &levels.to_string(),
            ])
            .output()
            .expect("run ojph_compress");
        assert!(
            out.status.success(),
            "ojph_compress failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let stream = std::fs::read(&j2c).unwrap();
        let cs = Codestream::parse(&stream).expect("parse ojph stream");
        let dec = cs.decode().expect("decode ojph stream");
        assert_eq!((dec.width, dec.height), (w, h));
        assert_eq!(dec.comps[0], samples, "{w}x{h} L{levels} {bits}-bit");
    }
    eprintln!("ojph_compress streams decoded bit-exactly");
}
