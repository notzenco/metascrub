//! JPEG: metadata lives in APPn and COM marker segments, and sometimes in
//! data appended after the end-of-image marker. Cleaning drops those pieces
//! and copies every other byte verbatim, so the compressed image is untouched.

use anyhow::{Result, bail};

use crate::report::{Removals, bytes};
use crate::{Cleaned, Field, Inspection, be16, exif, iptc, xmp};

const EXIF_ID: &[u8] = b"Exif\0\0";
const XMP_ID: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const XMP_EXTENSION_ID: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const ICC_ID: &[u8] = b"ICC_PROFILE\0";
const PHOTOSHOP_ID: &[u8] = b"Photoshop 3.0\0";
const MPF_ID: &[u8] = b"MPF\0";

const APP0: u8 = 0xE0;
const APP1: u8 = 0xE1;
const APP2: u8 = 0xE2;
const APP11: u8 = 0xEB;
const APP13: u8 = 0xED;
const APP14: u8 = 0xEE;
const COM: u8 = 0xFE;
const SOS: u8 = 0xDA;

enum Piece<'a> {
    /// A marker segment with a length field.
    Segment {
        marker: u8,
        bytes: &'a [u8],
        payload: &'a [u8],
    },
    /// Standalone markers and entropy-coded scan data, copied verbatim.
    Raw(&'a [u8]),
}

struct Parsed<'a> {
    pieces: Vec<Piece<'a>>,
    trailing: &'a [u8],
    missing_eoi: bool,
}

fn parse(data: &[u8]) -> Result<Parsed<'_>> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        bail!("not a JPEG file");
    }
    let mut pieces = vec![Piece::Raw(&data[..2])];
    let mut i = 2;
    loop {
        if i >= data.len() {
            // Truncated files still render in most viewers; keep what is there.
            return Ok(Parsed {
                pieces,
                trailing: &[],
                missing_eoi: true,
            });
        }
        if data[i] != 0xFF {
            bail!("corrupt JPEG: expected a marker at offset {i}");
        }
        // Any number of 0xFF fill bytes may precede a marker.
        let mut m = i + 1;
        while data.get(m) == Some(&0xFF) {
            m += 1;
        }
        let Some(&marker) = data.get(m) else {
            bail!("truncated JPEG: marker cut off at offset {i}");
        };
        match marker {
            0xD9 => {
                pieces.push(Piece::Raw(&data[i..=m]));
                return Ok(Parsed {
                    pieces,
                    trailing: &data[m + 1..],
                    missing_eoi: false,
                });
            }
            0xD0..=0xD7 | 0x01 => {
                pieces.push(Piece::Raw(&data[i..=m]));
                i = m + 1;
            }
            0x00 | 0xD8 => bail!("corrupt JPEG: unexpected marker 0x{marker:02X} at offset {m}"),
            _ => {
                let Some(len) = be16(data, m + 1) else {
                    bail!("truncated JPEG: segment length cut off at offset {m}");
                };
                let end = m + 1 + len as usize;
                if len < 2 || end > data.len() {
                    bail!("corrupt JPEG: segment 0x{marker:02X} at offset {m} overruns the file");
                }
                pieces.push(Piece::Segment {
                    marker,
                    bytes: &data[i..end],
                    payload: &data[m + 3..end],
                });
                i = end;
                if marker == SOS {
                    let scan_end = scan_end(data, i);
                    pieces.push(Piece::Raw(&data[i..scan_end]));
                    i = scan_end;
                }
            }
        }
    }
}

/// Find the end of entropy-coded data: the next 0xFF that is neither a
/// stuffed zero byte nor a restart marker.
fn scan_end(data: &[u8], mut at: usize) -> usize {
    while at < data.len() {
        if data[at] == 0xFF {
            match data.get(at + 1) {
                Some(0x00 | 0xD0..=0xD7) => at += 2,
                Some(_) => return at,
                None => return data.len(),
            }
        } else {
            at += 1;
        }
    }
    data.len()
}

/// A printable label for an unrecognised APPn segment.
fn identifier(payload: &[u8]) -> String {
    let id: String = payload
        .iter()
        .take_while(|&&b| b != 0)
        .take(24)
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '.'
            }
        })
        .collect();
    if id.is_empty() { "unnamed".into() } else { id }
}

fn jfif_has_thumbnail(payload: &[u8]) -> bool {
    payload.starts_with(b"JFIF\0") && payload.len() >= 14 && payload[12] != 0 && payload[13] != 0
}

pub fn inspect(data: &[u8]) -> Result<Inspection> {
    let parsed = parse(data)?;
    let mut fields = Vec::new();
    let mut notes = Vec::new();

    for piece in &parsed.pieces {
        let Piece::Segment {
            marker, payload, ..
        } = piece
        else {
            continue;
        };
        let (marker, payload) = (*marker, *payload);
        match marker {
            APP1 if payload.starts_with(EXIF_ID) => {
                let exif = exif::parse(&payload[EXIF_ID.len()..]);
                fields.extend(exif.fields);
                notes.extend(exif.notes);
            }
            APP1 if payload.starts_with(XMP_ID) => {
                fields.extend(xmp::parse(&payload[XMP_ID.len()..]));
            }
            APP1 if payload.starts_with(XMP_EXTENSION_ID) => {
                notes.push(format!("extended XMP chunk ({})", bytes(payload.len())));
            }
            APP13 if payload.starts_with(PHOTOSHOP_ID) => {
                let (iptc, other) = iptc::parse_photoshop(&payload[PHOTOSHOP_ID.len()..]);
                fields.extend(iptc);
                notes.extend(other);
            }
            APP0 if jfif_has_thumbnail(payload) => notes.push("JFIF thumbnail image".into()),
            APP0 if payload.starts_with(b"JFIF\0") => {}
            APP2 if payload.starts_with(ICC_ID) => {}
            APP14 if payload.starts_with(b"Adobe") => {}
            APP2 if payload.starts_with(MPF_ID) => notes.push(
                "Multi-Picture Format index (extra images stored after the main image)".into(),
            ),
            APP11 => notes.push(format!(
                "APP11 JUMBF data, e.g. C2PA content credentials ({})",
                bytes(payload.len())
            )),
            0xE0..=0xEF => notes.push(format!(
                "APP{} segment \"{}\" ({})",
                marker - 0xE0,
                identifier(payload),
                bytes(payload.len())
            )),
            COM => fields.push(Field::new(
                "Comment",
                "Comment",
                String::from_utf8_lossy(payload).trim(),
            )),
            _ => {}
        }
    }
    if !parsed.trailing.is_empty() {
        notes.push(format!(
            "{} of data after the end of the image (often a second image or vendor metadata)",
            bytes(parsed.trailing.len())
        ));
    }
    if parsed.missing_eoi {
        notes.push("file is truncated (no end-of-image marker)".into());
    }
    Ok(Inspection { fields, notes })
}

pub fn clean(data: &[u8]) -> Result<Cleaned> {
    let parsed = parse(data)?;
    let mut removed = Removals::default();
    let mut kept = Vec::new();
    let mut out: Vec<u8> = Vec::with_capacity(data.len());
    // Where an orientation-only EXIF segment goes: after SOI and any JFIF.
    let mut insert_at = 2;
    let mut orientation = None;

    for (index, piece) in parsed.pieces.iter().enumerate() {
        let (marker, segment, payload) = match piece {
            Piece::Raw(raw) => {
                out.extend_from_slice(raw);
                continue;
            }
            Piece::Segment {
                marker,
                bytes,
                payload,
            } => (*marker, *bytes, *payload),
        };
        match marker {
            APP0 if payload.starts_with(b"JFIF\0") => {
                if jfif_has_thumbnail(payload) {
                    removed.add("JFIF thumbnail", payload.len() - 14);
                    write_segment(&mut out, APP0, &without_jfif_thumbnail(payload));
                } else {
                    out.extend_from_slice(segment);
                }
                if index == 1 {
                    insert_at = out.len();
                }
            }
            APP1 if payload.starts_with(EXIF_ID) => {
                let tiff = &payload[EXIF_ID.len()..];
                let parsed = exif::parse(tiff);
                if orientation.is_none() {
                    orientation = parsed.orientation;
                }
                // An orientation-only block written by a previous run is
                // rewritten identically, so it is not reported as removed.
                let ours = parsed
                    .orientation
                    .is_some_and(|o| tiff == exif::orientation_only(o));
                if !ours {
                    removed.add("EXIF", segment.len());
                }
            }
            APP1 if payload.starts_with(XMP_ID) => removed.add("XMP", segment.len()),
            APP1 if payload.starts_with(XMP_EXTENSION_ID) => {
                removed.add("extended XMP", segment.len())
            }
            APP2 if payload.starts_with(ICC_ID) => {
                if !kept.iter().any(|k| k == "ICC colour profile") {
                    kept.push("ICC colour profile".to_string());
                }
                out.extend_from_slice(segment);
            }
            APP14 if payload.starts_with(b"Adobe") => out.extend_from_slice(segment),
            APP2 if payload.starts_with(MPF_ID) => removed.add("MPF index", segment.len()),
            APP11 => removed.add("JUMBF/C2PA", segment.len()),
            APP13 if payload.starts_with(PHOTOSHOP_ID) => {
                removed.add("Photoshop/IPTC", segment.len())
            }
            0xE0..=0xEF => removed.add(format!("APP{}", marker - 0xE0), segment.len()),
            COM => removed.add("comment", segment.len()),
            _ => out.extend_from_slice(segment),
        }
    }
    if !parsed.trailing.is_empty() {
        removed.add("data after end of image", parsed.trailing.len());
    }

    if let Some(o) = orientation {
        let mut payload = EXIF_ID.to_vec();
        payload.extend_from_slice(&exif::orientation_only(o));
        let mut segment = Vec::new();
        write_segment(&mut segment, APP1, &payload);
        out.splice(insert_at..insert_at, segment);
        kept.push("orientation".to_string());
    }
    Ok(Cleaned {
        data: out,
        removed: removed.into_vec(),
        kept,
    })
}

/// Rewrite a JFIF APP0 payload with its optional thumbnail dropped.
fn without_jfif_thumbnail(payload: &[u8]) -> Vec<u8> {
    let mut header = payload[..14].to_vec();
    header[12] = 0;
    header[13] = 0;
    header
}

fn write_segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(payload);
}
