//! PDF: metadata lives in the trailer's Info dictionary, in XMP metadata
//! streams attached with /Metadata, in application private data
//! (/PieceInfo) and inside embedded JPEG images. Earlier revisions kept by
//! incremental updates can also still hold old values.
//!
//! Cleaning rewrites the document from its current revision only, so
//! superseded objects are dropped. Page content and image streams are
//! copied without re-encoding; embedded JPEGs are cleaned like JPEG files.

use std::collections::BTreeSet;

use anyhow::{Context, Result, anyhow, bail};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::report::{Category, Removals, bytes};
use crate::{Cleaned, Field, Inspection, MAX_INFLATE, jpeg, xmp};

/// Keys removed from every dictionary in the document.
const STRIPPED_KEYS: [&[u8]; 3] = [b"Metadata", b"PieceInfo", b"LastModified"];

fn load(data: &[u8]) -> Result<Document> {
    let doc = Document::load_mem(data).map_err(|e| anyhow!("cannot parse PDF: {e}"))?;
    if doc.is_encrypted() {
        bail!("encrypted PDFs are not supported");
    }
    Ok(doc)
}

fn info_dict(doc: &Document) -> Option<(Option<ObjectId>, &Dictionary)> {
    match doc.trailer.get(b"Info").ok()? {
        Object::Reference(id) => Some((Some(*id), doc.get_object(*id).ok()?.as_dict().ok()?)),
        Object::Dictionary(dict) => Some((None, dict)),
        _ => None,
    }
}

fn text(obj: &Object) -> String {
    match obj {
        Object::String(..) => lopdf::decode_text_string(obj)
            .unwrap_or_else(|_| String::from_utf8_lossy(obj.as_str().unwrap_or_default()).into()),
        Object::Name(name) => String::from_utf8_lossy(name).into_owned(),
        Object::Integer(i) => i.to_string(),
        Object::Real(r) => r.to_string(),
        Object::Boolean(b) => b.to_string(),
        Object::Reference((n, g)) => format!("{n} {g} R"),
        other => format!("({})", other.enum_variant()),
    }
}

/// Info keys whose generic classification would be wrong: in PDF, Creator
/// and Producer name the software that made the file, not a person.
fn info_field(key: &[u8], value: &Object) -> Field {
    let name = String::from_utf8_lossy(key).into_owned();
    let field = Field::new("PDF Info", name, text(value));
    match key {
        b"Creator" | b"Producer" => field.with_category(Some(Category::Device)),
        b"Trapped" => field.with_category(None),
        // Custom Info entries are free-form and may say anything.
        _ => {
            let category = field.category.or(Some(Category::Text));
            field.with_category(category)
        }
    }
}

/// IDs of XMP metadata streams: anything referenced by a /Metadata key, plus
/// streams that declare themselves as metadata.
fn metadata_streams(doc: &Document) -> BTreeSet<ObjectId> {
    let mut ids = BTreeSet::new();
    for (id, object) in &doc.objects {
        let dict = match object {
            Object::Dictionary(d) => d,
            Object::Stream(s) => {
                if s.dict.has_type(b"Metadata") {
                    ids.insert(*id);
                }
                &s.dict
            }
            _ => continue,
        };
        if let Ok(Object::Reference(target)) = dict.get(b"Metadata") {
            ids.insert(*target);
        }
    }
    ids
}

fn stream_bytes(stream: &Stream) -> Result<Vec<u8>> {
    if stream.dict.has(b"Filter") {
        stream
            .decompressed_content_with_limit(MAX_INFLATE)
            .map_err(|e| anyhow!("cannot decompress stream: {e}"))
    } else {
        Ok(stream.content.clone())
    }
}

fn is_jpeg(stream: &Stream) -> bool {
    match stream.dict.get(b"Filter") {
        Ok(Object::Name(name)) => name == b"DCTDecode",
        Ok(Object::Array(filters)) => {
            matches!(filters.as_slice(), [Object::Name(name)] if name == b"DCTDecode")
        }
        _ => false,
    }
}

/// Revisions before the current one, as left by incremental updates.
fn earlier_revisions(data: &[u8]) -> usize {
    let count = data.windows(9).filter(|w| w == b"startxref").count();
    // A linearized file has a second cross-reference section for its first
    // page, which is not a revision.
    let head = &data[..data.len().min(1024)];
    let linearized = head.windows(11).any(|w| w == b"/Linearized");
    count.saturating_sub(1 + usize::from(linearized))
}

pub fn inspect(data: &[u8]) -> Result<Inspection> {
    let doc = load(data)?;
    let mut fields = Vec::new();
    let mut notes = Vec::new();

    if let Some((_, info)) = info_dict(&doc) {
        fields.extend(info.iter().map(|(k, v)| info_field(k, v)));
    }
    for id in metadata_streams(&doc) {
        let Ok(Object::Stream(stream)) = doc.get_object(id) else {
            continue;
        };
        match stream_bytes(stream) {
            Ok(packet) => {
                let found = xmp::parse(&packet);
                if found.is_empty() {
                    notes.push(format!("XMP metadata stream ({})", bytes(packet.len())));
                }
                fields.extend(found);
            }
            Err(e) => notes.push(format!("unreadable XMP metadata stream {}: {e}", id.0)),
        }
    }

    let mut piece_info = 0;
    for (id, object) in &doc.objects {
        let (dict, stream) = match object {
            Object::Dictionary(d) => (d, None),
            Object::Stream(s) => (&s.dict, Some(s)),
            _ => continue,
        };
        if dict.has(b"PieceInfo") {
            piece_info += 1;
        }
        if let Some(stream) = stream.filter(|s| is_jpeg(s)) {
            let container = format!("image object {}", id.0);
            match jpeg::inspect(&stream.content) {
                Ok(found) => {
                    fields.extend(found.fields.into_iter().map(|f| f.within(&container)));
                    notes.extend(found.notes.into_iter().map(|n| format!("{container}: {n}")));
                }
                Err(e) => notes.push(format!("{container}: {e}")),
            }
        }
    }
    if piece_info > 0 {
        notes.push(format!(
            "application private data (/PieceInfo) on {piece_info} object(s)"
        ));
    }
    let revisions = earlier_revisions(data);
    if revisions > 0 {
        notes.push(format!(
            "{revisions} earlier revision(s) from incremental updates, which may hold old values"
        ));
    }
    Ok(Inspection { fields, notes })
}

/// Remove [`STRIPPED_KEYS`] from a dictionary and any dictionaries nested in it.
fn strip(dict: &mut Dictionary, removed: &mut Removals) {
    for key in STRIPPED_KEYS {
        if dict.remove(key).is_some() && key == b"PieceInfo" {
            removed.add("application private data", 0);
        }
    }
    for (_, value) in dict.iter_mut() {
        strip_object(value, removed);
    }
}

fn strip_object(object: &mut Object, removed: &mut Removals) {
    match object {
        Object::Dictionary(dict) => strip(dict, removed),
        Object::Stream(stream) => strip(&mut stream.dict, removed),
        Object::Array(items) => {
            for item in items {
                strip_object(item, removed);
            }
        }
        _ => {}
    }
}

pub fn clean(data: &[u8]) -> Result<Cleaned> {
    let mut doc = load(data)?;
    let mut removed = Removals::default();

    if let Some((_, info)) = info_dict(&doc) {
        let entries = info.len();
        removed.add(
            format!(
                "document info ({entries} entr{})",
                if entries == 1 { "y" } else { "ies" }
            ),
            0,
        );
    }
    doc.trailer.remove(b"Info");

    for id in metadata_streams(&doc) {
        if let Ok(Object::Stream(stream)) = doc.get_object(id) {
            removed.add("XMP", stream.content.len());
        }
    }

    for (id, object) in doc.objects.iter_mut() {
        strip_object(object, &mut removed);
        if let Object::Stream(stream) = object
            && is_jpeg(stream)
        {
            let cleaned = jpeg::clean(&stream.content)
                .with_context(|| format!("embedded image object {} could not be cleaned", id.0))?;
            if !cleaned.removed.is_empty() {
                removed.add(
                    "image metadata",
                    stream.content.len().saturating_sub(cleaned.data.len()),
                );
                stream.set_content(cleaned.data);
            }
        }
    }

    let revisions = earlier_revisions(data);
    if revisions > 0 {
        removed.add(format!("{revisions} earlier revision(s)"), 0);
    }

    // Drop the Info dictionary, metadata streams and anything else that is
    // no longer reachable from the document.
    doc.prune_objects();
    // Close the gaps pruning leaves, so cleaning a clean file is a no-op.
    doc.renumber_objects();
    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| anyhow!("cannot write PDF: {e}"))?;
    Ok(Cleaned {
        data: out,
        removed: removed.into_vec(),
        kept: Vec::new(),
    })
}
