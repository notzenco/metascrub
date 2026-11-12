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
    let part = |e: &[(String, Vec<u8>, zip::DateTime)], name: &str| {
        e.iter().find(|(n, _, _)| n == name).unwrap().1.clone()
    };
    assert_eq!(
        part(&original, "[Content_Types].xml"),
        part(&cleaned, "[Content_Types].xml"),
        "parts without metadata are untouched"
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

fn part(data: &[u8], name: &str) -> String {
    let entry = entries(data).into_iter().find(|(n, _, _)| n == name);
    String::from_utf8(entry.unwrap_or_else(|| panic!("no {name}")).1).unwrap()
}

fn content_fields(inspection: &metascrub::Inspection, part: &str) -> Vec<(String, String)> {
    inspection
        .fields
        .iter()
        .filter(|f| f.group == part)
        .map(|f| (f.name.clone(), f.value.clone()))
        .collect()
}

#[test]
fn inspect_reports_people_inside_document_content() {
    let (_, docx) = metascrub::inspect(&fixture("memo.docx")).unwrap();
    let comments = content_fields(&docx, "word/comments.xml");
    assert!(
        comments.contains(&("CommentAuthor".into(), "Jane Example".into())),
        "{comments:?}"
    );
    assert!(comments.contains(&("CommentDate".into(), "2019-07-14T10:30:00Z".into())));
    let body = content_fields(&docx, "word/document.xml");
    assert!(
        body.contains(&("RevisionAuthor".into(), "Jane Example".into())),
        "{body:?}"
    );
    assert!(
        body.contains(&("RevisionSessionIDs".into(), "2 IDs".into())),
        "{body:?}"
    );
    let people = content_fields(&docx, "word/people.xml");
    assert!(
        people.contains(&("PersonUserId".into(), "jane@ExampleCorp.com".into())),
        "{people:?}"
    );
    assert_eq!(
        field(&docx, "CommentAuthor").category,
        Some(Category::Identity)
    );

    let (_, xlsx) = metascrub::inspect(&fixture("budget.xlsx")).unwrap();
    assert_eq!(
        field(&xlsx, "SavedPath").value,
        r"C:\Users\Jane Example\Documents\"
    );
    assert_eq!(field(&xlsx, "SavedPath").category, Some(Category::Identity));
    assert_eq!(field(&xlsx, "CommentAuthor").value, "Jane Example");
    assert_eq!(field(&xlsx, "PersonName").value, "Jane Example");

    let (_, pptx) = metascrub::inspect(&fixture("deck.pptx")).unwrap();
    assert_eq!(field(&pptx, "CommentAuthor").value, "Jane Example");
    assert_eq!(field(&pptx, "CommentDate").category, Some(Category::Time));
}

#[test]
fn clean_anonymises_authors_but_keeps_comments_and_revisions() {
    let docx = metascrub::clean(&fixture("memo.docx")).unwrap().data;
    let body = part(&docx, "word/document.xml");
    assert!(body.contains("<w:t>Quarterly plan</w:t>"), "{body}");
    assert!(
        body.contains(r#"<w:ins w:id="1" w:author="Author">"#),
        "{body}"
    );
    assert!(!body.contains("rsid"), "{body}");
    let comments = part(&docx, "word/comments.xml");
    assert!(comments.contains(r#"w:author="Author""#), "{comments}");
    assert!(comments.contains("Check the figures"), "{comments}");
    assert!(!comments.contains("w:date"), "{comments}");
    assert!(!part(&docx, "word/settings.xml").contains("rsid"));

    let xlsx = metascrub::clean(&fixture("budget.xlsx")).unwrap().data;
    let notes = part(&xlsx, "xl/comments1.xml");
    assert!(notes.contains("<author>Author</author>"), "{notes}");
    assert!(notes.contains("<t>Author:</t>"), "{notes}");
    assert!(notes.contains("Check this total"), "{notes}");
    assert!(part(&xlsx, "xl/workbook.xml").contains(r#"url="""#));

    let pptx = metascrub::clean(&fixture("deck.pptx")).unwrap().data;
    let authors = part(&pptx, "ppt/commentAuthors.xml");
    assert!(
        authors.contains(r#"name="Author" initials="A""#),
        "{authors}"
    );
    let slide_comments = part(&pptx, "ppt/comments/comment1.xml");
    assert!(!slide_comments.contains("dt="), "{slide_comments}");
    assert!(slide_comments.contains("Tighten this title"));
}
