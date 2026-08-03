//! Byte-range plans against real codestreams: a fetched prefix must decode
//! to exactly what a full decode downsamples to, and standard thumbnail
//! plans must stay within the 1–3 range budget.

use ndic_codestream::range::RangeIndex;
use ndic_codestream::reader::Codestream;
use ndic_codestream::writer::{encode_image, encode_image_with_depth};
use ndic_core::{CoeffPlane, EncodeParams, SampleType};

fn gradient(width: usize, height: usize) -> Vec<i32> {
    (0..width * height)
        .map(|i| {
            let (x, y) = (i % width, i / width);
            i32::try_from((7 * x + 13 * y) % 256).expect("< 256")
        })
        .collect()
}

fn encode(width: usize, height: usize, levels: u8) -> Vec<u8> {
    let samples = gradient(width, height);
    let plane = CoeffPlane::new(&samples, width, height).unwrap();
    let params = EncodeParams {
        xy_levels: levels,
        ..EncodeParams::default()
    };
    encode_image(&[plane], SampleType::U8, &params).unwrap()
}

/// Every resolution's planned prefix decodes bit-identically to the full
/// stream decoded at that resolution — the thumbnail-vs-full consistency
/// gate.
#[test]
fn prefix_decode_equals_downsampled_full_decode() {
    let bytes = encode(190, 121, 4);
    let cs = Codestream::parse(&bytes).unwrap();
    let index = RangeIndex::from_codestream(&cs).unwrap();

    for r in 0..=4u8 {
        let max_px = 190usize >> (4 - r as usize);
        let plan = index.thumbnail(max_px.max(1)).unwrap();
        assert!(plan.max_res <= r);

        // A bare-codestream thumbnail is a single contiguous prefix.
        assert_eq!(plan.ranges.len(), 1, "plan: {plan:?}");
        assert_eq!(plan.ranges[0].start, 0);
        let prefix = &bytes[..usize::try_from(plan.total_bytes).unwrap()];

        let full = cs.decode_to_resolution(plan.max_res).unwrap();
        let partial = Codestream::parse_prefix(prefix)
            .unwrap()
            .decode_to_resolution(plan.max_res)
            .unwrap();
        assert_eq!(partial.width, full.width);
        assert_eq!(partial.height, full.height);
        assert_eq!(partial.comps, full.comps);
        assert_eq!(
            (plan.decoded_size[1], plan.decoded_size[0]),
            (full.width as u64, full.height as u64),
        );
    }
}

/// The prefix really is a prefix: fetching the planned bytes of the lowest
/// resolution is far smaller than the stream.
#[test]
fn thumbnail_prefix_is_small() {
    let bytes = encode(512, 512, 5);
    let cs = Codestream::parse(&bytes).unwrap();
    let index = RangeIndex::from_codestream(&cs).unwrap();
    let plan = index.thumbnail(16).unwrap();
    assert_eq!(plan.max_res, 0);
    assert!(
        plan.total_bytes * 4 < bytes.len() as u64,
        "R0 prefix {} of {} bytes",
        plan.total_bytes,
        bytes.len()
    );
}

/// A header-only prefix (no packet bodies at all) still yields the full
/// packet index and identical plans — the remote-bootstrap path.
#[test]
fn plans_from_header_only_prefix_match_full_stream_plans() {
    let bytes = encode(190, 121, 4);
    let cs = Codestream::parse(&bytes).unwrap();
    let full_plan = RangeIndex::from_codestream(&cs).unwrap().thumbnail(48).unwrap();

    let header_len = cs.tile_parts[0].body.start;
    let header = Codestream::parse_prefix(&bytes[..header_len]).unwrap();
    assert_eq!(header.total_len(), bytes.len());
    let prefix_plan = RangeIndex::from_codestream(&header)
        .unwrap()
        .thumbnail(48)
        .unwrap();
    assert_eq!(prefix_plan, full_plan);
}

/// Region plans with the default maximal precincts degrade to the
/// resolution prefix (whole-plane precincts always intersect), still ≤ 3
/// ranges.
#[test]
fn region_plan_with_maximal_precincts_is_the_resolution_prefix() {
    let bytes = encode(256, 256, 3);
    let cs = Codestream::parse(&bytes).unwrap();
    let plan = RangeIndex::region(&cs, (64, 64, 32, 32), 1).unwrap();
    assert!(plan.ranges.len() <= 3, "plan: {plan:?}");
    assert_eq!(plan.target, "region");
    // Everything the plan fetches decodes: it covers header + R0..R1.
    let index = RangeIndex::from_codestream(&cs).unwrap();
    let thumb = index.thumbnail(64).unwrap(); // R1 for a 256px plane, 3 levels
    assert_eq!(thumb.max_res, 1);
    assert_eq!(plan.total_bytes, thumb.total_bytes);
}

/// Depth-flexible encoding round-trips signed coefficient-plane data whose
/// dynamic range fits a narrow declaration (the post-`nd_lift` shape).
#[test]
fn narrow_declared_depth_round_trips_signed_planes() {
    let width = 96;
    let height = 64;
    let samples: Vec<i32> = (0..width * height)
        .map(|i| {
            let (x, y) = (i % width, i / width);
            let (x, y) = (
                i32::try_from(x).expect("< 96"),
                i32::try_from(y).expect("< 64"),
            );
            ((11 * x - 5 * y) % 1500) - 700
        })
        .collect();
    let plane = CoeffPlane::new(&samples, width, height).unwrap();
    // Values span [-1015, 345]: 11 signed bits.
    let bytes =
        encode_image_with_depth(&[plane], 11, true, &EncodeParams::default()).unwrap();
    let decoded = Codestream::parse(&bytes).unwrap().decode().unwrap();
    assert_eq!(decoded.comps[0], samples);

    // Out-of-range samples for the declared depth are refused.
    let plane = CoeffPlane::new(&samples, width, height).unwrap();
    assert!(encode_image_with_depth(&[plane], 10, true, &EncodeParams::default()).is_err());
}
