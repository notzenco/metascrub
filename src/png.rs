//! PNG: metadata lives in ancillary chunks. Cleaning keeps an allowlist of
//! chunks needed to render the image and copies them verbatim, so IDAT
//! (the compressed pixels) is never decoded.

use anyhow::{Result, bail};

use crate::report::{Category, Removals, bytes};
use crate::{Cleaned, Field, Inspection, be16, be32, exif, inflate_zlib, xmp};

pub const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

const XMP_KEYWORD: &[u8] = b"XML:com.adobe.xmp";

/// Ancillary chunks that affect how the image looks or animates.
const KEEP: [&[u8; 4]; 15] = [
    b"tRNS", b"gAMA", b"cHRM", b"sRGB", b"iCCP", b"sBIT", b"bKGD", b"hIST", b"pHYs", b"sPLT",
    b"cICP", b"mDCV", b"cLLI", b"acTL", b"fcTL",
];

struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
    /// The whole chunk: length, type, data and CRC.
    raw: &'a [u8],
}

impl Chunk<'_> {
    fn is(&self, kind: &[u8; 4]) -> bool {
        &self.kind == kind
    }

    fn is_critical(&self) -> bool {
        self.kind[0].is_ascii_uppercase()
    }

    fn name(&self) -> String {
        String::from_utf8_lossy(&self.kind).into_owned()
    }
}

struct Parsed<'a> {
    chunks: Vec<Chunk<'a>>,
    trailing: &'a [u8],
}

fn parse(data: &[u8]) -> Result<Parsed<'_>> {
    if !data.starts_with(SIGNATURE) {
        bail!("not a PNG file");
    }
    let mut chunks = Vec::new();
    let mut at = SIGNATURE.len();
    loop {
        let Some(len) = be32(data, at) else {
            bail!("truncated PNG: missing IEND chunk");
        };
        let end = (at + 12)
            .checked_add(len as usize)
            .filter(|&e| e <= data.len());
        let Some(end) = end else {
            bail!("truncated PNG: chunk at offset {at} overruns the file");
        };
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().expect("4-byte slice");
        if !kind.iter().all(u8::is_ascii_alphabetic) {
            bail!("corrupt PNG: invalid chunk type at offset {at}");
        }
        let chunk = Chunk {
            kind,
            data: &data[at + 8..end - 4],
            raw: &data[at..end],
        };
        let is_end = chunk.is(b"IEND");
        chunks.push(chunk);
        at = end;
        if is_end {
            return Ok(Parsed {
                chunks,
                trailing: &data[at..],
            });
        }
    }
}

/// Decode tEXt, zTXt and iTXt into (keyword, text).
fn text_chunk(chunk: &Chunk) -> Option<(String, Vec<u8>)> {
    let nul = chunk.data.iter().position(|&b| b == 0)?;
    let keyword = String::from_utf8_lossy(&chunk.data[..nul]).into_owned();
    let rest = &chunk.data[nul + 1..];
    let text = match &chunk.kind {
        b"tEXt" => rest.to_vec(),
        // Compression method byte, then zlib data.
        b"zTXt" => inflate_zlib(rest.get(1..)?).ok()?,
        b"iTXt" => {
            let (&compressed, rest) = rest.split_first()?;
            let rest = rest.get(1..)?;
            // Language tag and translated keyword, each NUL-terminated.
            let lang_end = rest.iter().position(|&b| b == 0)?;
            let rest = &rest[lang_end + 1..];
            let translated_end = rest.iter().position(|&b| b == 0)?;
            let rest = &rest[translated_end + 1..];
            if compressed == 1 {
                inflate_zlib(rest).ok()?
            } else {
                rest.to_vec()
            }
        }
        _ => return None,
    };
    Some((keyword, text))
}

fn timestamp(data: &[u8]) -> Option<String> {
    let year = be16(data, 0)?;
    let [month, day, hour, minute, second] = data.get(2..7)?.try_into().ok()?;
    Some(format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
    ))
}

pub fn inspect(data: &[u8]) -> Result<Inspection> {
    let parsed = parse(data)?;
    let mut fields = Vec::new();
    let mut notes = Vec::new();

    for chunk in &parsed.chunks {
        match &chunk.kind {
            b"eXIf" => {
                let exif = exif::parse(chunk.data);
                fields.extend(exif.fields);
                notes.extend(exif.notes);
            }
            b"tEXt" | b"zTXt" | b"iTXt" => match text_chunk(chunk) {
                Some((keyword, text)) if keyword.as_bytes() == XMP_KEYWORD => {
                    fields.extend(xmp::parse(&text));
                }
                Some((keyword, text)) => {
                    let field = Field::new("PNG text", keyword, String::from_utf8_lossy(&text));
                    // Text chunks hold arbitrary prose, so anything that is
                    // not otherwise classified is still free text.
                    let category = field.category.or(Some(Category::Text));
                    fields.push(field.with_category(category));
                }
                None => notes.push(format!("unreadable {} chunk", chunk.name())),
            },
            b"tIME" => {
                if let Some(time) = timestamp(chunk.data) {
                    fields.push(Field::new("PNG", "LastModified", time));
                }
            }
            _ if is_kept(chunk) => {}
            _ => notes.push(format!(
                "{} chunk ({})",
                chunk.name(),
                bytes(chunk.data.len())
            )),
        }
    }
    if !parsed.trailing.is_empty() {
        notes.push(format!(
            "{} of data after the end of the image",
            bytes(parsed.trailing.len())
        ));
    }
    Ok(Inspection { fields, notes })
}

/// Chunks that are part of the image itself. Unknown critical chunks are
/// kept because a decoder cannot render the image without them.
fn is_kept(chunk: &Chunk) -> bool {
    chunk.is_critical() || KEEP.contains(&&chunk.kind) || chunk.is(b"fdAT")
}

pub fn clean(data: &[u8]) -> Result<Cleaned> {
    let parsed = parse(data)?;
    let mut out = SIGNATURE.to_vec();
    let mut removed = Removals::default();
    let mut kept = Vec::new();
    let mut orientation = None;
    let mut wrote_orientation = false;

    for chunk in &parsed.chunks {
        if chunk.is(b"eXIf") {
            let exif = exif::parse(chunk.data);
            orientation = orientation.or(exif.orientation);
            let ours = exif
                .orientation
                .is_some_and(|o| chunk.data == exif::orientation_only(o));
            if !ours {
                removed.add("EXIF", chunk.raw.len());
            }
            continue;
        }
        if chunk.is(b"IDAT") && !wrote_orientation {
            // eXIf must precede the image data.
            if let Some(o) = orientation {
                write_chunk(&mut out, b"eXIf", &exif::orientation_only(o));
                kept.push("orientation".to_string());
            }
            wrote_orientation = true;
        }
        if is_kept(chunk) {
            if chunk.is(b"iCCP") {
                kept.push("ICC colour profile".to_string());
            }
            out.extend_from_slice(chunk.raw);
            continue;
        }
        let label = match &chunk.kind {
            b"tEXt" | b"zTXt" | b"iTXt" => match text_chunk(chunk) {
                Some((keyword, _)) if keyword.as_bytes() == XMP_KEYWORD => "XMP".to_string(),
                _ => "text".to_string(),
            },
            b"tIME" => "timestamp".to_string(),
            _ => format!("{} chunk", chunk.name()),
        };
        removed.add(label, chunk.raw.len());
    }
    if !parsed.trailing.is_empty() {
        removed.add("data after end of image", parsed.trailing.len());
    }
    Ok(Cleaned {
        data: out,
        removed: removed.into_vec(),
        kept,
    })
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
}
