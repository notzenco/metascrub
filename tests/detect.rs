mod common;

use common::fixture;
use metascrub::{Format, OfficeKind, detect};

#[test]
fn detects_every_fixture_by_content() {
    let cases = [
        ("photo.jpg", Format::Jpeg),
        ("photo.png", Format::Png),
        ("photo.webp", Format::WebP),
        ("photo.heic", Format::Heif),
        ("report.pdf", Format::Pdf),
        ("memo.docx", Format::Office(OfficeKind::Word)),
        ("budget.xlsx", Format::Office(OfficeKind::Excel)),
        ("deck.pptx", Format::Office(OfficeKind::PowerPoint)),
    ];
    for (name, expected) in cases {
        assert_eq!(detect(&fixture(name)), Some(expected), "{name}");
    }
}

#[test]
fn rejects_unsupported_content() {
    assert_eq!(detect(b"GIF89a"), None);
    assert_eq!(detect(b"plain text"), None);
    assert_eq!(detect(b""), None);
    // An MP4 video has an ftyp box but no HEIF image brand.
    assert_eq!(detect(b"\0\0\0\x18ftypisom\0\0\x02\0isommp41"), None);
    assert_eq!(metascrub::describe_unsupported(b"GIF89a"), "GIF image");
}

#[test]
fn formats_have_short_display_names() {
    let names: Vec<String> = [
        Format::Jpeg,
        Format::Heif,
        Format::Office(OfficeKind::Word),
        Format::Office(OfficeKind::Excel),
        Format::Office(OfficeKind::PowerPoint),
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(names, ["JPEG", "HEIF", "DOCX", "XLSX", "PPTX"]);
}
