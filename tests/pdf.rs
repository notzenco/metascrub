mod common;

use common::{assert_cleans, field, find, fixture};
use metascrub::{Category, Format};

#[test]
fn inspect_reports_info_xmp_and_embedded_image_metadata() {
    let (format, inspection) = metascrub::inspect(&fixture("report.pdf")).unwrap();
    assert_eq!(format, Format::Pdf);

    assert_eq!(field(&inspection, "Author").value, "Jane Example");
    assert_eq!(
        field(&inspection, "Author").category,
        Some(Category::Identity)
    );
    assert_eq!(
        field(&inspection, "Creator").category,
        Some(Category::Device)
    );
    assert_eq!(
        field(&inspection, "Producer").value,
        "MetascrubCam PDF Library"
    );
    assert_eq!(field(&inspection, "ModDate").category, Some(Category::Time));
    assert_eq!(field(&inspection, "dc:creator").value, "Jane Example");

    let image_make = field(&inspection, "Make");
    assert_eq!(image_make.value, "MetascrubCam");
    assert!(image_make.group.contains("image"), "{}", image_make.group);

    assert!(
        inspection
            .notes
            .iter()
            .any(|n| n.contains("earlier revision")),
        "{:?}",
        inspection.notes
    );
}

#[test]
fn clean_removes_metadata_and_earlier_revisions() {
    let report = assert_cleans("report.pdf");
    let cleaned = &report.data;

    assert!(find(cleaned, b"/Info").is_none());
    assert!(find(cleaned, b"/Metadata").is_none());
    // The earlier revision's Company entry must not survive either.
    assert!(find(cleaned, b"ExampleCorp").is_none());

    for label in ["document info", "XMP", "image metadata"] {
        assert!(
            report.removed.iter().any(|r| r.contains(label)),
            "{label} missing from {:?}",
            report.removed
        );
    }
}

#[test]
fn clean_keeps_page_content_and_image_data() {
    let original = fixture("report.pdf");
    let report = metascrub::clean(&original).unwrap();

    let doc = lopdf::Document::load_mem(&report.data).unwrap();
    assert_eq!(doc.get_pages().len(), 1);
    assert!(find(&report.data, b"(Quarterly report) Tj").is_some());

    // The embedded JPEG's compressed scan data is carried over unchanged.
    let sos = find(&original, &[0xFF, 0xDA]).unwrap();
    let eoi = sos + find(&original[sos..], &[0xFF, 0xD9]).unwrap();
    assert!(find(&report.data, &original[sos..eoi + 2]).is_some());
}
