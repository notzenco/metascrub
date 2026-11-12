mod common;

use common::{assert_cleans, field, find, fixture};
use metascrub::{Category, Format};

#[test]
fn inspect_reports_planted_metadata() {
    let (format, inspection) = metascrub::inspect(&fixture("photo.heic")).unwrap();
    assert_eq!(format, Format::Heif);
    assert_eq!(field(&inspection, "Make").value, "MetascrubCam");
    assert_eq!(
        field(&inspection, "BodySerialNumber").value,
        "SN-0042-SECRET"
    );
    assert_eq!(
        field(&inspection, "GPSPosition").category,
        Some(Category::Location)
    );
    assert_eq!(field(&inspection, "dc:creator").value, "Jane Example");
}

#[test]
fn clean_blanks_metadata_items_without_moving_anything() {
    let original = fixture("photo.heic");
    let report = assert_cleans("photo.heic");

    // Metadata items are overwritten in place, so every box keeps its size
    // and offset and the header boxes are byte-identical.
    assert_eq!(report.data.len(), original.len());
    let mdat = find(&original, b"mdat").unwrap();
    assert_eq!(original[..mdat], report.data[..mdat]);

    let (_, after) = metascrub::inspect(&report.data).unwrap();
    assert!(after.fields.is_empty(), "{:#?}", after.fields);
    assert!(
        report.removed.iter().any(|r| r.contains("EXIF")),
        "{:?}",
        report.removed
    );
    assert!(
        report.removed.iter().any(|r| r.contains("XMP")),
        "{:?}",
        report.removed
    );
}
