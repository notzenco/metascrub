mod common;

use std::io::{Cursor, Read};

use common::{assert_cleans, field, fixture, markers_in};
use metascrub::{Category, Format, OfficeKind};

/// Every entry of a zip archive, decompressed, in archive order.
fn entries(data: &[u8]) -> Vec<(String, Vec<u8>, zip::DateTime)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(data)).unwrap();
    (0..archive.len())
        .map(|i| {
            let mut file = archive.by_index(i).unwrap();
            let mut content = Vec::new();
            file.read_to_end(&mut content).unwrap();
            let modified = file.last_modified().unwrap_or_default();
            (file.name().unwrap().into_owned(), content, modified)
        })
        .collect()
}

#[test]
fn inspect_reports_document_properties_and_embedded_images() {
    let (format, inspection) = metascrub::inspect(&fixture("memo.docx")).unwrap();
    assert_eq!(format, Format::Office(OfficeKind::Word));

    assert_eq!(field(&inspection, "dc:creator").value, "Jane Example");
    assert_eq!(
        field(&inspection, "dc:creator").category,
        Some(Category::Identity)
    );
    assert_eq!(
        field(&inspection, "cp:lastModifiedBy").category,
        Some(Category::Identity)
    );
    assert_eq!(
        field(&inspection, "dcterms:created").category,
        Some(Category::Time)
    );
    assert_eq!(field(&inspection, "Company").value, "ExampleCorp Pty Ltd");
    assert_eq!(
        field(&inspection, "Application").category,
        Some(Category::Device)
    );
    assert_eq!(
        field(&inspection, "ClientName").value,
        "ExampleCorp Pty Ltd"
    );
    assert!(field(&inspection, "ClientName").is_sensitive());
    assert_eq!(
        field(&inspection, "LatestEntryModified").category,
        Some(Category::Time)
    );

    let make = field(&inspection, "Make");
    assert_eq!(make.value, "MetascrubCam");
    assert!(
        make.group.starts_with("word/media/image1.jpeg"),
        "{}",
        make.group
    );
}

#[test]
fn clean_docx_strips_properties_images_and_timestamps() {
    let original = entries(&fixture("memo.docx"));
    let report = assert_cleans("memo.docx");
    let cleaned = entries(&report.data);

    let names = |e: &[(String, Vec<u8>, zip::DateTime)]| {
        e.iter().map(|(n, _, _)| n.clone()).collect::<Vec<_>>()
    };
    assert_eq!(
        names(&original),
        names(&cleaned),
        "entries and order are kept"
    );

    for (name, content, modified) in &cleaned {
        assert!(
            markers_in(content).is_empty(),
            "{name}: {:?}",
            markers_in(content)
        );
        assert_eq!(
            *modified,
            zip::DateTime::default(),
            "{name} keeps a timestamp"
        );
    }
    let document = |e: &[(String, Vec<u8>, zip::DateTime)]| {
        e.iter()
            .find(|(n, _, _)| n == "word/document.xml")
            .unwrap()
            .1
            .clone()
    };
    assert_eq!(
        document(&original),
        document(&cleaned),
        "document body is untouched"
    );

    for label in [
        "core properties",
        "extended properties",
        "custom properties",
        "image metadata",
    ] {
        assert!(
            report.removed.iter().any(|r| r.contains(label)),
            "{label} missing from {:?}",
            report.removed
        );
    }
}

#[test]
fn clean_xlsx_and_pptx() {
    for name in ["budget.xlsx", "deck.pptx"] {
        let report = assert_cleans(name);
        for (entry, content, _) in entries(&report.data) {
            assert!(markers_in(&content).is_empty(), "{name} {entry}");
        }
    }
}
