//! WebP: a RIFF container whose EXIF and XMP live in their own chunks.
//! Cleaning drops those chunks, fixes the VP8X feature flags and the RIFF
//! size, and copies the image bitstream chunks verbatim.

use anyhow::{Result, bail};

use crate::report::{Removals, bytes};
use crate::{Cleaned, Inspection, exif, le32, xmp};

const FLAG_XMP: u8 = 0x04;
const FLAG_EXIF: u8 = 0x08;

/// Chunks that make up the image: bitstreams, alpha, animation, colour.
const KEEP: [&[u8; 4]; 7] = [
    b"VP8 ", b"VP8L", b"VP8X", b"ALPH", b"ANIM", b"ANMF", b"ICCP",
];

struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
    /// Header, data and padding byte.
    raw: &'a [u8],
}

fn parse(data: &[u8]) -> Result<Vec<Chunk<'_>>> {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        bail!("not a WebP file");
    }
    // Bytes beyond the declared RIFF size are not part of the image.
    let riff_end = le32(data, 4)
        .map(|size| (size as usize).saturating_add(8))
        .unwrap_or(data.len())
        .min(data.len());
    let mut chunks = Vec::new();
    let mut at = 12;
    while at + 8 <= riff_end {
        let kind: [u8; 4] = data[at..at + 4].try_into().expect("4-byte slice");
        let len = le32(data, at + 4).expect("in bounds") as usize;
        let Some(end) = (at + 8).checked_add(len).filter(|&e| e <= riff_end) else {
            bail!("corrupt WebP: chunk at offset {at} overruns the file");
        };
        let padded = (end + (len & 1)).min(riff_end);
        chunks.push(Chunk {
            kind,
            data: &data[at + 8..end],
            raw: &data[at..padded],
        });
        at = padded;
    }
    if chunks.is_empty() {
        bail!("corrupt WebP: no chunks");
    }
    Ok(chunks)
}

/// WebP EXIF chunks should hold bare TIFF data, but some writers prefix
/// the JPEG-style `Exif\0\0` identifier.
fn tiff(data: &[u8]) -> &[u8] {
    data.strip_prefix(b"Exif\0\0").unwrap_or(data)
}

fn name(kind: &[u8; 4]) -> String {
    String::from_utf8_lossy(kind).trim_end().to_string()
}

pub fn inspect(data: &[u8]) -> Result<Inspection> {
    let mut fields = Vec::new();
    let mut notes = Vec::new();
    for chunk in parse(data)? {
        match &chunk.kind {
            b"EXIF" => {
                let exif = exif::parse(tiff(chunk.data));
                fields.extend(exif.fields);
                notes.extend(exif.notes);
            }
            b"XMP " => fields.extend(xmp::parse(chunk.data)),
            kind if KEEP.contains(&kind) => {}
            kind => notes.push(format!(
                "{} chunk ({})",
                name(kind),
                bytes(chunk.data.len())
            )),
        }
    }
    Ok(Inspection { fields, notes })
}

pub fn clean(data: &[u8]) -> Result<Cleaned> {
    let chunks = parse(data)?;
    let mut removed = Removals::default();
    let mut kept = Vec::new();
    let mut body = b"WEBP".to_vec();
    let mut orientation = None;
    let mut vp8x_flags_at = None;

    for chunk in &chunks {
        match &chunk.kind {
            b"EXIF" => {
                let exif = exif::parse(tiff(chunk.data));
                orientation = orientation.or(exif.orientation);
                let ours = exif
                    .orientation
                    .is_some_and(|o| chunk.data == exif::orientation_only(o));
                if !ours {
                    removed.add("EXIF", chunk.raw.len());
                }
            }
            b"XMP " => removed.add("XMP", chunk.raw.len()),
            kind if KEEP.contains(&kind) => {
                if kind == b"VP8X" && !chunk.data.is_empty() {
                    vp8x_flags_at = Some(body.len() + 8);
                }
                if kind == b"ICCP" {
                    kept.push("ICC colour profile".to_string());
                }
                body.extend_from_slice(chunk.raw);
            }
            kind => removed.add(format!("{} chunk", name(kind)), chunk.raw.len()),
        }
    }

    // A simple (non-VP8X) WebP cannot carry EXIF, so orientation is only
    // kept when the extended header exists.
    if let Some(flags_at) = vp8x_flags_at {
        body[flags_at] &= !(FLAG_XMP | FLAG_EXIF);
        if let Some(o) = orientation {
            let tiff = exif::orientation_only(o);
            body.extend_from_slice(b"EXIF");
            body.extend_from_slice(&(tiff.len() as u32).to_le_bytes());
            body.extend_from_slice(&tiff);
            if tiff.len() % 2 == 1 {
                body.push(0);
            }
            body[flags_at] |= FLAG_EXIF;
            kept.push("orientation".to_string());
        }
    }

    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(Cleaned {
        data: out,
        removed: removed.into_vec(),
        kept,
    })
}
