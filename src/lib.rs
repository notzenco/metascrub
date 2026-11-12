//! Inspect and remove identifying metadata from images and documents.
//!
//! Formats are detected from file content, never from the file name.
//! Image cleaners work on the container structure (JPEG segments, PNG
//! chunks, RIFF chunks, ISO-BMFF boxes) and never decode or re-encode pixels.

use std::fmt;

use anyhow::{Result, bail};

mod exif;
mod heif;
mod iptc;
mod jpeg;
mod office;
mod pdf;
mod png;
pub mod report;
mod webp;
mod xmp;

pub use report::{Category, Cleaned, Field, Inspection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeKind {
    Word,
    Excel,
    PowerPoint,
    Other,
}

/// A file format metascrub can process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    WebP,
    /// HEIF still images, including HEIC and AVIF.
    Heif,
    Pdf,
    Office(OfficeKind),
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Format::Jpeg => "JPEG",
            Format::Png => "PNG",
            Format::WebP => "WebP",
            Format::Heif => "HEIF",
            Format::Pdf => "PDF",
            Format::Office(OfficeKind::Word) => "Word document (docx)",
            Format::Office(OfficeKind::Excel) => "Excel workbook (xlsx)",
            Format::Office(OfficeKind::PowerPoint) => "PowerPoint presentation (pptx)",
            Format::Office(OfficeKind::Other) => "Office Open XML package",
        })
    }
}

/// Detect a supported format from the leading bytes of a file.
pub fn detect(data: &[u8]) -> Option<Format> {
    if let Some(format) = detect_image(data) {
        return Some(format);
    }
    if data.starts_with(b"%PDF-") {
        return Some(Format::Pdf);
    }
    if data.starts_with(b"PK\x03\x04") {
        return office::kind(data).map(Format::Office);
    }
    None
}

/// Detect only the image formats, which may also appear embedded in documents.
pub(crate) fn detect_image(data: &[u8]) -> Option<Format> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if data.starts_with(png::SIGNATURE) {
        Some(Format::Png)
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some(Format::WebP)
    } else if heif::is_heif(data) {
        Some(Format::Heif)
    } else {
        None
    }
}

/// Name an unsupported format well enough for a useful error message.
pub fn describe_unsupported(data: &[u8]) -> &'static str {
    if data.starts_with(b"GIF8") {
        "GIF image"
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        "TIFF or camera raw image"
    } else if data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        "legacy Office document (doc/xls/ppt)"
    } else if data.starts_with(b"PK\x03\x04") {
        "ZIP archive that is not an Office Open XML document"
    } else if data.len() >= 8 && &data[4..8] == b"ftyp" {
        "video or non-image ISO media file"
    } else if data.is_empty() {
        "empty file"
    } else {
        "unrecognised format"
    }
}

/// List the metadata in a file.
pub fn inspect(data: &[u8]) -> Result<(Format, Inspection)> {
    let Some(format) = detect(data) else {
        bail!("unsupported format: {}", describe_unsupported(data));
    };
    Ok((format, inspect_as(format, data)?))
}

pub(crate) fn inspect_as(format: Format, data: &[u8]) -> Result<Inspection> {
    match format {
        Format::Jpeg => jpeg::inspect(data),
        Format::Png => png::inspect(data),
        Format::WebP => webp::inspect(data),
        Format::Heif => heif::inspect(data),
        Format::Pdf => pdf::inspect(data),
        Format::Office(_) => office::inspect(data),
    }
}

pub(crate) fn clean_as(format: Format, data: &[u8]) -> Result<Cleaned> {
    match format {
        Format::Jpeg => jpeg::clean(data),
        Format::Png => png::clean(data),
        Format::WebP => webp::clean(data),
        Format::Heif => heif::clean(data),
        Format::Pdf => pdf::clean(data),
        Format::Office(_) => office::clean(data),
    }
}

/// The result of cleaning one file.
#[derive(Debug, Clone)]
pub struct CleanReport {
    pub format: Format,
    pub data: Vec<u8>,
    pub removed: Vec<String>,
    pub kept: Vec<String>,
    /// Fields present before cleaning and absent afterwards.
    pub removed_fields: Vec<Field>,
}

/// Remove identifying metadata from a file held in memory.
///
/// The cleaned output is inspected again before it is returned; if any
/// privacy-sensitive field survived, cleaning fails rather than producing a
/// file that only looks clean.
pub fn clean(data: &[u8]) -> Result<CleanReport> {
    let (format, before) = inspect(data)?;
    let cleaned = clean_as(format, data)?;
    let after = inspect_as(format, &cleaned.data)
        .map_err(|e| anyhow::anyhow!("cleaned output failed to parse: {e:#}"))?;

    let leftovers: Vec<String> = after
        .fields
        .iter()
        .filter(|f| f.is_sensitive())
        .map(|f| format!("{} {}", f.group, f.name))
        .collect();
    if !leftovers.is_empty() {
        bail!(
            "refusing to write output: sensitive metadata survived cleaning ({})",
            leftovers.join(", ")
        );
    }

    let removed_fields = before
        .fields
        .into_iter()
        .filter(|f| !after.fields.contains(f))
        .collect();
    Ok(CleanReport {
        format,
        data: cleaned.data,
        removed: cleaned.removed,
        kept: cleaned.kept,
        removed_fields,
    })
}

/// Read a big-endian u32 at `at`, if in bounds.
pub(crate) fn be32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// Read a big-endian u16 at `at`, if in bounds.
pub(crate) fn be16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}
