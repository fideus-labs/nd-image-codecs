#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)] // test I/O math

//! Conformance decode against the `OpenJPH` test corpus
//! (`aous72/jp2k_test_codestreams`): every reversible-5/3 file within the
//! Phase-3 reader's scope must decode **bit-exactly** to its reference.
//!
//! Streams outside the scope (tiled, subsampled YUV, 9/7) are reported as
//! skipped. Fetch the corpus with `scripts/fetch-conformance.sh`.

use std::path::{Path, PathBuf};

use ndic_codestream::{Codestream, jph};

fn corpus_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.join("target/tools/jp2k_test_codestreams/openjph")
}

/// Reads a binary PGM (P5) or PPM (P6) with 8- or 16-bit samples into
/// per-component planes.
fn read_pnm(path: &Path) -> (usize, usize, Vec<Vec<i32>>) {
    let data = std::fs::read(path).unwrap();
    let mut fields = Vec::new();
    let mut pos = 0usize;
    while fields.len() < 4 && pos < data.len() {
        // Skip whitespace and `#` comments.
        while pos < data.len() {
            if data[pos].is_ascii_whitespace() {
                pos += 1;
            } else if data[pos] == b'#' {
                while pos < data.len() && data[pos] != b'\n' {
                    pos += 1;
                }
            } else {
                break;
            }
        }
        let start = pos;
        while pos < data.len() && !data[pos].is_ascii_whitespace() {
            pos += 1;
        }
        fields.push(String::from_utf8_lossy(&data[start..pos]).to_string());
    }
    pos += 1;
    let ncomp = match fields[0].as_str() {
        "P5" => 1usize,
        "P6" => 3,
        other => panic!("unsupported PNM magic {other}"),
    };
    let w: usize = fields[1].parse().unwrap();
    let h: usize = fields[2].parse().unwrap();
    let maxval: u32 = fields[3].parse().unwrap();
    let body = &data[pos..];
    let mut comps = vec![vec![0i32; w * h]; ncomp];
    if maxval > 255 {
        for (i, ch) in body.chunks_exact(2).take(w * h * ncomp).enumerate() {
            comps[i % ncomp][i / ncomp] = i32::from(u16::from_be_bytes([ch[0], ch[1]]));
        }
    } else {
        for (i, &b) in body.iter().take(w * h * ncomp).enumerate() {
            comps[i % ncomp][i / ncomp] = i32::from(b);
        }
    }
    (w, h, comps)
}

#[test]
fn decodes_rev53_corpus_bit_exactly() {
    let dir = corpus_dir();
    let mse_pae = dir.join("mse_pae.txt");
    if !mse_pae.exists() {
        eprintln!(
            "skipping: corpus not found at {} — run scripts/fetch-conformance.sh",
            dir.display()
        );
        return;
    }
    let listing = std::fs::read_to_string(&mse_pae).unwrap();
    let mut passed = Vec::new();
    let mut skipped = Vec::new();

    for line in listing.lines() {
        let mut parts = line.split_whitespace();
        let (Some(file), Some(reference)) = (parts.next(), parts.next()) else {
            continue;
        };
        if !file.starts_with("simple_dec_rev53")
            || !std::path::Path::new(file)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("jph"))
        {
            continue;
        }
        if std::path::Path::new(reference)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("yuv"))
        {
            skipped.push((file.to_string(), "YUV 4:2:0 subsampling".to_string()));
            continue;
        }

        let bytes = std::fs::read(dir.join(file)).unwrap();
        let cs_bytes = jph::unwrap_codestream(&bytes).expect("jph unwrap");
        let cs = match Codestream::parse(cs_bytes) {
            Ok(cs) => cs,
            Err(e) => {
                skipped.push((file.to_string(), format!("parse: {e}")));
                continue;
            }
        };
        let dec = match cs.decode() {
            Ok(d) => d,
            Err(e) => {
                skipped.push((file.to_string(), format!("{e}")));
                continue;
            }
        };

        let (rw, rh, refs) = read_pnm(&dir.join(reference));
        assert_eq!((dec.width, dec.height), (rw, rh), "{file}: dimensions");
        assert_eq!(dec.comps.len(), refs.len(), "{file}: component count");
        for (c, want) in refs.iter().enumerate() {
            assert_eq!(&dec.comps[c], want, "{file}: component {c} differs");
        }
        passed.push(file.to_string());
    }

    eprintln!("bit-exact: {passed:?}");
    eprintln!("skipped: {skipped:?}");
    assert!(
        passed.len() >= 6,
        "expected at least the non-tiled rev53 set to pass: {passed:?}"
    );
    // Every skip must be an out-of-scope stream, not a failure.
    for (file, why) in &skipped {
        assert!(
            why.contains("subsampling") || why.contains("unsupported") || why.contains("tile"),
            "{file} skipped for an unexpected reason: {why}"
        );
    }
}
