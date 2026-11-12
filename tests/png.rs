mod common;

use common::{assert_cleans, field, find, fixture};
use metascrub::{Category, Format};

/// Split a PNG into (type, data) chunks, checking every CRC.
fn chunks(png: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    let mut out = Vec::new();
    let mut at = 8;
    while at < png.len() {
        let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = png[at + 4..at + 8].try_into().unwrap();
        let data = &png[at + 8..at + 8 + len];
        let crc = u32::from_be_bytes(png[at + 8 + len..at + 12 + len].try_into().unwrap());
        let mut expected = flate2::Crc::new();
        expected.update(&png[at + 4..at + 8 + len]);
        assert_eq!(
            crc,
            expected.sum(),
            "bad CRC on {}",
            String::from_utf8_lossy(&kind)
        );
        out.push((kind, data.to_vec()));
        at += 12 + len;
    }
    out
}

#[test]
fn inspect_reports_planted_metadata() {
    let (format, inspection) = metascrub::inspect(&fixture("photo.png")).unwrap();
    assert_eq!(format, Format::Png);
    assert_eq!(field(&inspection, "Make").value, "MetascrubCam");
    assert_eq!(
        field(&inspection, "GPSPosition").category,
        Some(Category::Location)
    );
    assert_eq!(field(&inspection, "Author").value, "Jane Example");
    assert_eq!(
        field(&inspection, "Author").category,
        Some(Category::Identity)
    );
    assert_eq!(field(&inspection, "Comment").value, "Secret comment");
    assert_eq!(field(&inspection, "dc:creator").value, "Jane Example");
    assert_eq!(
        field(&inspection, "LastModified").value,
        "2019-07-14 10:30:00"
    );
    assert_eq!(
        field(&inspection, "LastModified").category,
        Some(Category::Time)
    );
}

#[test]
fn clean_removes_metadata_chunks_and_keeps_image_chunks() {
    let original = fixture("photo.png");
    let report = assert_cleans("photo.png");

    let before = chunks(&original);
    let after = chunks(&report.data);
    let kinds: Vec<&[u8]> = after.iter().map(|(k, _)| k.as_slice()).collect();
    assert_eq!(kinds, [&b"IHDR"[..], b"gAMA", b"eXIf", b"IDAT", b"IEND"]);

    let idat = |c: &[([u8; 4], Vec<u8>)]| {
        c.iter()
            .filter(|(k, _)| k == b"IDAT")
            .map(|(_, d)| d.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(idat(&before), idat(&after), "pixel data must be untouched");

    let (_, inspection) = metascrub::inspect(&report.data).unwrap();
    let names: Vec<&str> = inspection.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Orientation"]);
    for label in ["EXIF", "text", "XMP", "timestamp"] {
        assert!(
            report.removed.iter().any(|r| r.contains(label)),
            "{label} missing from {:?}",
            report.removed
        );
    }
    assert!(find(&report.data, b"tEXt").is_none());
}
