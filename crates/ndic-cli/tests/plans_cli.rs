//! End-to-end `ndic index` / `ndic thumbnail` / `ndic expand --partial`:
//! plan against local files and a real (in-test) HTTP Range server, execute
//! with plain range fetches, and pin thumbnail-vs-full consistency.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;

fn ndic() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ndic"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ndic-plans-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn run(args: &[&str]) -> String {
    let out = ndic().args(args).output().expect("ndic runs");
    assert!(
        out.status.success(),
        "ndic {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 stdout")
}

/// A gradient `.jph` fixture (128×96, 3 levels). `tag` keeps parallel
/// tests off each other's files — the harness runs tests concurrently and
/// `std::fs::write` truncates before writing.
fn make_jph(tag: &str) -> PathBuf {
    let src = tmp(&format!("grad-{tag}.pgm"));
    let jph = tmp(&format!("grad-{tag}.jph"));
    let (w, h) = (128usize, 96usize);
    let mut pgm = format!("P5\n{w} {h}\n255\n").into_bytes();
    pgm.extend((0..w * h).map(|i| {
        let (x, y) = (i % w, i / w);
        u8::try_from((3 * x + 5 * y) % 256).unwrap()
    }));
    std::fs::write(&src, &pgm).unwrap();
    run(&[
        "compress",
        "-i",
        src.to_str().unwrap(),
        "-o",
        jph.to_str().unwrap(),
        "--levels",
        "3",
    ]);
    jph
}

/// An nd-lift-ht chunk fixture: (8, 32, 32) int32, lifted along z, then
/// `htj2k`-encoded via the same feature-free core the codec uses.
fn make_chunk() -> (PathBuf, String) {
    let shape = [8usize, 32, 32];
    let mut samples: Vec<i32> = (0..shape.iter().product::<usize>())
        .map(|i| {
            let z = i / (32 * 32);
            let xy = i % (32 * 32);
            i32::try_from(z * 100 + (xy * 7) % 640).unwrap()
        })
        .collect();
    let series = serde_json::json!([
        { "name": "nd_lift", "configuration": {
            "version": "0.1",
            "transforms": [
                { "axis": "z", "dimension": 0, "kind": "lift53", "levels": 2, "group": 0 }
            ] } },
        { "name": "htj2k", "configuration": { "xy_levels": 2 } }
    ]);
    let config: ndic_lift::NdLiftConfig =
        serde_json::from_value(series[0]["configuration"].clone()).unwrap();
    ndic_lift::forward(&mut samples, &shape, &config.transforms).unwrap();
    let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_ne_bytes()).collect();
    let chunk = ndic_zarr::htj2k::encode_chunk(
        &bytes,
        &shape,
        ndic_core::SampleType::I32,
        &serde_json::from_value(series[1]["configuration"].clone()).unwrap(),
    )
    .unwrap();
    let path = tmp("chunk.ndht");
    std::fs::write(&path, &chunk).unwrap();
    (path, series.to_string())
}

#[test]
fn index_plans_and_curl_prefix_expand_partial() {
    let jph = make_jph("curl");
    let file_len = std::fs::metadata(&jph).unwrap().len();

    let plan: serde_json::Value = serde_json::from_str(&run(&[
        "index",
        jph.to_str().unwrap(),
        "--target",
        "thumbnail",
        "--max",
        "32",
    ]))
    .expect("plan JSON");
    let ranges = plan["ranges"].as_array().unwrap();
    assert!(!ranges.is_empty() && ranges.len() <= 3, "{plan}");
    assert!(plan["total_bytes"].as_u64().unwrap() < file_len / 2);
    assert_eq!(plan["decoded_size"], serde_json::json!([24, 32]));

    // The curl workflow: fetch the planned ranges, expand --partial.
    let curl = run(&[
        "index",
        jph.to_str().unwrap(),
        "--target",
        "thumbnail",
        "--max",
        "32",
        "--format",
        "curl",
    ]);
    let data = std::fs::read(&jph).unwrap();
    let mut part = Vec::new();
    for range in curl.trim().split(',') {
        let (start, end) = range.split_once('-').unwrap();
        let (start, end): (usize, usize) = (start.parse().unwrap(), end.parse().unwrap());
        part.extend_from_slice(&data[start..=end]);
    }
    let part_path = tmp("thumb.part");
    std::fs::write(&part_path, &part).unwrap();
    let out = tmp("partial.pgm");
    run(&[
        "expand",
        "-i",
        part_path.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--partial",
    ]);
    let head = std::fs::read(&out).unwrap();
    assert!(head.starts_with(b"P5\n32 24\n"), "decoded prefix size");

    // Consistency: the partial decode equals the full decode at that
    // resolution.
    let full = tmp("full.pgm");
    run(&[
        "expand",
        "-i",
        jph.to_str().unwrap(),
        "-o",
        full.to_str().unwrap(),
        "--resolution",
        "1",
    ]);
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&full).unwrap());
}

#[test]
fn thumbnail_decodes_locally_and_region_plans_emit() {
    let jph = make_jph("local");
    let out = tmp("thumb.png");
    let stdout = run(&[
        "thumbnail",
        jph.to_str().unwrap(),
        "--max",
        "32",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert!(stdout.contains("32x24"), "{stdout}");
    assert!(std::fs::metadata(&out).unwrap().len() > 0);

    let plan: serde_json::Value = serde_json::from_str(&run(&[
        "index",
        jph.to_str().unwrap(),
        "--target",
        "region",
        "--rect",
        "0,0,64,64",
        "--level",
        "1",
    ]))
    .expect("region plan JSON");
    assert!(plan["ranges"].as_array().unwrap().len() <= 3);
}

#[test]
fn chunk_plans_select_low_pass_planes_and_decode_3d() {
    let (chunk, series) = make_chunk();

    // 8 z-planes lifted 2 levels → 2 low-pass planes.
    let plan: serde_json::Value = serde_json::from_str(&run(&[
        "index",
        chunk.to_str().unwrap(),
        "--target",
        "thumbnail-3d",
        "--max",
        "8",
        "--series",
        &series,
    ]))
    .expect("plan JSON");
    assert_eq!(plan["planes"], serde_json::json!([0, 1]));
    assert_eq!(plan["decoded_size"], serde_json::json!([2, 8, 8]));

    let out = tmp("preview.raw");
    let stdout = run(&[
        "thumbnail",
        chunk.to_str().unwrap(),
        "--max",
        "8",
        "--three-d",
        "--series",
        &series,
        "-o",
        out.to_str().unwrap(),
    ]);
    assert!(stdout.contains("2 plane(s) of 8x8"), "{stdout}");
    // The coefficient planes' actual range fits 16 declared bits, so the
    // volume comes back as I16 samples (2 bytes each).
    assert!(stdout.contains("I16 LE"), "{stdout}");
    let raw = std::fs::read(&out).unwrap();
    assert_eq!(raw.len(), 2 * 8 * 8 * 2);

    // A plane plan covers the index and exactly one plane.
    let plan: serde_json::Value = serde_json::from_str(&run(&[
        "index",
        chunk.to_str().unwrap(),
        "--target",
        "plane",
        "--z",
        "3",
    ]))
    .expect("plane plan");
    assert_eq!(plan["planes"], serde_json::json!([3]));
    assert!(plan["ranges"].as_array().unwrap().len() <= 2);
}

/// A minimal single-threaded HTTP server answering `Range:` requests over
/// one fixed body — what any static file host does.
fn serve_ranges(body: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        // Serve until the test process drops the listener thread.
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 4096];
            let mut request = Vec::new();
            loop {
                match stream.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        request.extend_from_slice(&buf[..n]);
                        if request.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            let text = String::from_utf8_lossy(&request);
            if text.is_empty() {
                continue;
            }
            let range = text
                .lines()
                .find_map(|l| l.strip_prefix("Range: bytes="))
                .and_then(|r| r.trim().split_once('-'))
                .and_then(|(s, e)| Some((s.parse::<usize>().ok()?, e.parse::<usize>().ok()?)));
            let response = match range {
                Some((start, end)) if start < body.len() => {
                    let end = end.min(body.len() - 1);
                    let slice = &body[start..=end];
                    let mut r = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len(),
                        slice.len()
                    )
                    .into_bytes();
                    r.extend_from_slice(slice);
                    r
                }
                _ => {
                    let mut r = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .into_bytes();
                    r.extend_from_slice(&body);
                    r
                }
            };
            let _ = stream.write_all(&response);
        }
    });
    (format!("http://{addr}"), handle)
}

#[test]
fn thumbnail_over_http_range_requests() {
    let jph = make_jph("http");
    let body = std::fs::read(&jph).unwrap();
    let total = body.len() as u64;
    let (url, _server) = serve_ranges(body);

    let out = tmp("http-thumb.pgm");
    let stdout = run(&[
        "thumbnail",
        &format!("{url}/grad.jph"),
        "--max",
        "32",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert!(stdout.contains("32x24"), "{stdout}");

    // Must match the local decode bit-for-bit.
    let local = tmp("local-thumb.pgm");
    run(&[
        "thumbnail",
        jph.to_str().unwrap(),
        "--max",
        "32",
        "-o",
        local.to_str().unwrap(),
    ]);
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&local).unwrap());

    // And it must have fetched a strict subset of the file.
    let reported: u64 = stdout
        .split('(')
        .nth(1)
        .and_then(|s| s.split(" bytes fetched").next())
        .and_then(|s| s.rsplit(' ').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(u64::MAX);
    assert!(reported < total, "{stdout}");
}

/// Phase 6 range-access audit: a thumbnail plan must fetch a bounded number
/// of bytes per decoded pixel. The budgets are regression tripwires with
/// headroom over measured values (~1.0 B/px for the 8-bit gradient,
/// ~2.2 B/voxel for the lifted chunk preview at --max 16, where the
/// micro-fixture's fixed per-plane headers still dominate), not targets; a
/// planner change that starts over-fetching fails here before it ships.
#[test]
fn thumbnail_plans_stay_within_the_bytes_per_pixel_budget() {
    let jph = make_jph("budget");
    let plan: serde_json::Value = serde_json::from_str(&run(&[
        "index",
        jph.to_str().unwrap(),
        "--target",
        "thumbnail",
        "--max",
        "32",
    ]))
    .expect("plan JSON");
    let pixels: u64 = plan["decoded_size"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_u64().unwrap())
        .product();
    let bytes = plan["total_bytes"].as_u64().unwrap();
    let per_pixel = bytes as f64 / pixels as f64;
    assert!(
        per_pixel <= 2.0,
        "thumbnail plan fetches {per_pixel:.2} B/px ({bytes} B for {pixels} px)"
    );

    let (chunk, series) = make_chunk();
    let plan: serde_json::Value = serde_json::from_str(&run(&[
        "index",
        chunk.to_str().unwrap(),
        "--target",
        "thumbnail-3d",
        "--max",
        "16",
        "--series",
        &series,
    ]))
    .expect("plan JSON");
    let voxels: u64 = plan["decoded_size"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_u64().unwrap())
        .product();
    let bytes = plan["total_bytes"].as_u64().unwrap();
    let per_voxel = bytes as f64 / voxels as f64;
    assert!(
        per_voxel <= 4.0,
        "3D preview plan fetches {per_voxel:.2} B/voxel ({bytes} B for {voxels} voxels)"
    );
}
