//! Office Open XML (docx, xlsx, pptx): a ZIP package whose document
//! properties live in docProps/core.xml (author, dates), docProps/app.xml
//! (application, company, template) and docProps/custom.xml. Embedded
//! images keep their own metadata, and every ZIP entry carries a timestamp.
//!
//! Cleaning rewrites the package entry by entry: property parts become
//! empty, embedded images are cleaned like standalone images, timestamps
//! are reset to the ZIP epoch (1980-01-01) and everything else is copied
//! with its content unchanged.

use std::io::{Cursor, Read, Write};

use anyhow::{Context, Result, bail};
use quick_xml::Reader;
use quick_xml::events::Event;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::report::{Category, Removals};
use crate::{
    Cleaned, Field, Inspection, MAX_INFLATE, OfficeKind, clean_as, detect_image, inspect_as,
};

const CORE: &str = "docProps/core.xml";
const APP: &str = "docProps/app.xml";
const CUSTOM: &str = "docProps/custom.xml";

const EMPTY_CORE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"/>"#;

const EMPTY_APP: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"/>"#;

const EMPTY_CUSTOM: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"/>"#;

/// Identify an Office Open XML package from the parts it contains.
pub fn kind(data: &[u8]) -> Option<OfficeKind> {
    let archive = ZipArchive::new(Cursor::new(data)).ok()?;
    let has = |name: &str| archive.index_for_name(name).is_some();
    if !has("[Content_Types].xml") {
        return None;
    }
    Some(if has("word/document.xml") {
        OfficeKind::Word
    } else if has("xl/workbook.xml") {
        OfficeKind::Excel
    } else if has("ppt/presentation.xml") {
        OfficeKind::PowerPoint
    } else {
        OfficeKind::Other
    })
}

struct Entry {
    name: String,
    content: Vec<u8>,
    is_dir: bool,
    stored: bool,
    modified: Option<DateTime>,
}

fn entries(data: &[u8]) -> Result<(Vec<Entry>, Vec<u8>)> {
    let mut archive = ZipArchive::new(Cursor::new(data)).context("cannot read ZIP package")?;
    let comment = archive.comment().to_vec();
    let mut out = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        let name = file.name()?.into_owned();
        if file.encrypted() {
            bail!("encrypted package entries are not supported ({name})");
        }
        let is_dir = file.is_dir();
        let stored = file.compression() == CompressionMethod::Stored;
        let modified = file.last_modified();
        let mut content = Vec::new();
        file.take(MAX_INFLATE as u64 + 1)
            .read_to_end(&mut content)
            .with_context(|| format!("cannot decompress {name}"))?;
        if content.len() > MAX_INFLATE {
            bail!(
                "{name} is larger than {}",
                crate::report::bytes(MAX_INFLATE)
            );
        }
        out.push(Entry {
            name,
            content,
            is_dir,
            stored,
            modified,
        });
    }
    Ok((out, comment))
}

/// Read a properties part: one field per child of the root element, named
/// by the child's tag or, for custom properties, its `name` attribute.
fn properties(xml: &[u8], group: &str) -> Vec<Field> {
    let text = String::from_utf8_lossy(xml);
    let mut reader = Reader::from_str(&text);
    reader.config_mut().trim_text(true);
    let mut fields = Vec::new();
    let mut depth = 0usize;
    let mut current: Option<(String, Vec<String>)> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                depth += 1;
                if depth == 2 {
                    let name = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.as_ref() == "name")
                        .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok())
                        .map(|v| v.into_owned())
                        .unwrap_or_else(|| e.name().as_ref().to_string());
                    current = Some((name, Vec::new()));
                }
            }
            Ok(Event::Text(t)) => {
                if let Some((_, parts)) = current.as_mut() {
                    parts.push(t.xml10_content().into_owned());
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if let (Some((_, parts)), Ok(Some(c))) = (current.as_mut(), r.resolve_char_ref()) {
                    parts.push(c.to_string());
                }
            }
            Ok(Event::End(_)) => {
                if depth == 2
                    && let Some((name, parts)) = current.take()
                {
                    let value = parts.join(", ");
                    if !value.trim().is_empty() {
                        fields.push(Field::new(group, name, value.trim()));
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
    fields
}

fn timestamp_key(t: &DateTime) -> (u16, u8, u8, u8, u8, u8) {
    (
        t.year(),
        t.month(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
    )
}

pub fn inspect(data: &[u8]) -> Result<Inspection> {
    let (entries, comment) = entries(data)?;
    let mut fields = Vec::new();
    let mut notes = Vec::new();

    for entry in &entries {
        match entry.name.as_str() {
            CORE => fields.extend(properties(&entry.content, "Office core")),
            APP => fields.extend(properties(&entry.content, "Office app")),
            CUSTOM => fields.extend(properties(&entry.content, "Office custom").into_iter().map(
                |f| {
                    // Custom properties are free-form and may hold anything.
                    let category = f.category.or(Some(Category::Text));
                    f.with_category(category)
                },
            )),
            name => {
                if let Some(format) = detect_image(&entry.content) {
                    match inspect_as(format, &entry.content) {
                        Ok(found) => {
                            fields.extend(found.fields.into_iter().map(|f| f.within(name)));
                            notes.extend(found.notes.into_iter().map(|n| format!("{name}: {n}")));
                        }
                        Err(e) => notes.push(format!("{name}: {e}")),
                    }
                }
            }
        }
    }

    let latest = entries
        .iter()
        .filter_map(|e| e.modified)
        .filter(|t| *t != DateTime::default())
        .max_by_key(timestamp_key);
    if let Some(latest) = latest {
        fields.push(Field::new("ZIP", "LatestEntryModified", latest.to_string()));
    }
    if !comment.is_empty() {
        fields.push(Field::new(
            "ZIP",
            "Comment",
            String::from_utf8_lossy(&comment),
        ));
    }
    Ok(Inspection { fields, notes })
}

pub fn clean(data: &[u8]) -> Result<Cleaned> {
    let (entries, comment) = entries(data)?;
    let mut removed = Removals::default();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let mut timestamps = false;

    for entry in &entries {
        timestamps |= entry.modified.is_some_and(|t| t != DateTime::default());
        let options = SimpleFileOptions::default()
            .last_modified_time(DateTime::default())
            .unix_permissions(if entry.is_dir { 0o755 } else { 0o644 })
            .compression_method(if entry.stored {
                CompressionMethod::Stored
            } else {
                CompressionMethod::Deflated
            });
        if entry.is_dir {
            writer.add_directory(entry.name.as_str(), options)?;
            continue;
        }

        let replacement = match entry.name.as_str() {
            CORE => Some((EMPTY_CORE, "core properties")),
            APP => Some((EMPTY_APP, "extended properties")),
            CUSTOM => Some((EMPTY_CUSTOM, "custom properties")),
            _ => None,
        };
        let content = match replacement {
            Some((empty, label)) => {
                if entry.content != empty.as_bytes() {
                    removed.add(label, entry.content.len());
                }
                empty.as_bytes().to_vec()
            }
            None => match detect_image(&entry.content) {
                Some(format) => {
                    let cleaned = clean_as(format, &entry.content)
                        .with_context(|| format!("cannot clean embedded image {}", entry.name))?;
                    if !cleaned.removed.is_empty() {
                        removed.add(
                            "embedded image metadata",
                            entry.content.len().saturating_sub(cleaned.data.len()),
                        );
                    }
                    cleaned.data
                }
                None => entry.content.clone(),
            },
        };
        writer.start_file(entry.name.as_str(), options)?;
        writer.write_all(&content)?;
    }
    if timestamps {
        removed.add("ZIP entry timestamps", 0);
    }
    if !comment.is_empty() {
        removed.add("ZIP comment", comment.len());
    }
    let data = writer.finish()?.into_inner();
    Ok(Cleaned {
        data,
        removed: removed.into_vec(),
        kept: Vec::new(),
    })
}
