mod common;

use common::{find, fixture, markers_in};
use metascrub::{Category, Format};

fn field<'a>(inspection: &'a metascrub::Inspection, name: &str) -> &'a metascrub::Field {
    inspection
        .fields
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name} in {:#?}", inspection.fields))
}

#[test]
fn inspect_reports_planted_metadata() {
    let (format, inspection) = metascrub::inspect(&fixture("photo.jpg")).unwrap();
    assert_eq!(format, Format::Jpeg);

    assert_eq!(field(&inspection, "Make").value, "MetascrubCam");
    assert_eq!(field(&inspection, "Make").category, Some(Category::Device));
    assert_eq!(
        field(&inspection, "BodySerialNumber").value,
        "SN-0042-SECRET"
    );
    assert_eq!(
        field(&inspection, "Artist").category,
        Some(Category::Identity)
    );
    assert_eq!(
        field(&inspection, "DateTimeOriginal").category,
        Some(Category::Time)
    );
    assert_eq!(
        field(&inspection, "GPSPosition").value,
        "48.858300, 2.294500"
    );
    assert_eq!(
        field(&inspection, "GPSLatitude").category,
        Some(Category::Location)
    );
    assert_eq!(field(&inspection, "dc:creator").value, "Jane Example");
    assert_eq!(
        field(&inspection, "xmp:CreatorTool").value,
        "MSFirmware 9.9"
    );
    assert_eq!(field(&inspection, "By-line").value, "Jane Example");
    assert_eq!(field(&inspection, "Comment").value, "Secret comment");
    assert!(field(&inspection, "Orientation").category.is_none());
    assert!(
        inspection
            .notes
            .iter()
            .any(|n| n.contains("after the end of the image")),
        "{:?}",
        inspection.notes
    );
}

#[test]
fn clean_removes_all_planted_metadata() {
    let original = fixture("photo.jpg");
    let report = metascrub::clean(&original).unwrap();

    assert_eq!(markers_in(&report.data), Vec::<&str>::new());
    let (_, after) = metascrub::inspect(&report.data).unwrap();
    assert!(
        after.fields.iter().all(|f| !f.is_sensitive()),
        "{:#?}",
        after.fields
    );
    assert!(after.notes.is_empty(), "{:?}", after.notes);

    let removed: Vec<&str> = report
        .removed_fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    for name in ["Make", "GPSPosition", "dc:creator", "By-line", "Comment"] {
        assert!(
            removed.contains(&name),
            "{name} not reported in {removed:?}"
        );
    }
    assert!(report.removed.iter().any(|r| r.contains("EXIF")));
    assert!(report.removed.iter().any(|r| r.contains("XMP")));
}

#[test]
fn clean_keeps_orientation_and_colour_profile() {
    let report = metascrub::clean(&fixture("photo.jpg")).unwrap();
    let (_, after) = metascrub::inspect(&report.data).unwrap();
    let names: Vec<&str> = after.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Orientation"]);
    assert!(
        find(
            &report.data,
            b"ICC_PROFILE\0\x01\x01metascrub-test-icc-profile"
        )
        .is_some()
    );
}

#[test]
fn clean_does_not_touch_compressed_image_data() {
    let original = fixture("photo.jpg");
    let report = metascrub::clean(&original).unwrap();

    // Everything from the first quantisation table to the end-of-image
    // marker (tables, frame header, scan data) must be byte-identical.
    let start = |d: &[u8]| find(d, &[0xFF, 0xDB]).unwrap();
    let end = |d: &[u8]| d.windows(2).rposition(|w| w == [0xFF, 0xD9]).unwrap() + 2;
    let cleaned = &report.data;
    assert_eq!(
        &original[start(&original)..end(&original)],
        &cleaned[start(cleaned)..end(cleaned)]
    );
    assert_eq!(
        end(cleaned),
        cleaned.len(),
        "data after EOI must be dropped"
    );
}

#[test]
fn clean_is_idempotent() {
    let once = metascrub::clean(&fixture("photo.jpg")).unwrap();
    let twice = metascrub::clean(&once.data).unwrap();
    assert_eq!(once.data, twice.data);
    assert!(twice.removed.is_empty(), "{:?}", twice.removed);
}
