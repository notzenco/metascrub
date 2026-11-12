mod common;

use common::{assert_cleans, field, fixture};
use metascrub::{Category, Format};

fn chunks(webp: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
    assert_eq!(&webp[..4], b"RIFF");
    let riff_size = u32::from_le_bytes(webp[4..8].try_into().unwrap()) as usize;
    assert_eq!(riff_size, webp.len() - 8, "RIFF size must match the file");
    let mut out = Vec::new();
    let mut at = 12;
    while at < webp.len() {
        let kind: [u8; 4] = webp[at..at + 4].try_into().unwrap();
        let len = u32::from_le_bytes(webp[at + 4..at + 8].try_into().unwrap()) as usize;
        out.push((kind, webp[at + 8..at + 8 + len].to_vec()));
        at += 8 + len + (len & 1);
    }
    out
}

#[test]
fn inspect_reports_planted_metadata() {
    let (format, inspection) = metascrub::inspect(&fixture("photo.webp")).unwrap();
    assert_eq!(format, Format::WebP);
    assert_eq!(field(&inspection, "Model").value, "MSX-1000");
    assert_eq!(
        field(&inspection, "GPSPosition").category,
        Some(Category::Location)
    );
    assert_eq!(field(&inspection, "dc:creator").value, "Jane Example");
}

#[test]
fn clean_drops_metadata_chunks_and_updates_header() {
    let original = chunks(&fixture("photo.webp"));
    let report = assert_cleans("photo.webp");
    let cleaned = chunks(&report.data);

    let kinds: Vec<&[u8]> = cleaned.iter().map(|(k, _)| k.as_slice()).collect();
    assert_eq!(kinds, [&b"VP8X"[..], b"VP8 ", b"EXIF"]);
    assert_eq!(original[1], cleaned[1], "bitstream must be untouched");

    let flags = cleaned[0].1[0];
    assert_eq!(flags & 0x04, 0, "XMP flag must be cleared");
    assert_eq!(
        flags & 0x08,
        0x08,
        "EXIF flag stays set for the orientation block"
    );

    let (_, after) = metascrub::inspect(&report.data).unwrap();
    let names: Vec<&str> = after.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Orientation"]);
}
