//! HEIF (HEIC, AVIF): an ISO-BMFF container where EXIF and XMP are stored
//! as items located by the `iloc` box. Cleaning overwrites those items'
//! bytes in place with an empty EXIF block or an empty XMP packet, so no box
//! moves, no offset changes and the coded image is untouched.

use std::ops::Range;

use anyhow::{Context, Result, bail};

use crate::report::{Removals, bytes};
use crate::{Cleaned, Inspection, be16, be32, exif, xmp};

/// Brands that mark an ISO-BMFF file as a HEIF still image (HEIC, AVIF).
const IMAGE_BRANDS: [&[u8; 4]; 8] = [
    b"mif1", b"mif2", b"heic", b"heix", b"heim", b"heis", b"avif", b"avci",
];

pub fn is_heif(data: &[u8]) -> bool {
    if data.get(4..8) != Some(b"ftyp") {
        return false;
    }
    let Some(size) = be32(data, 0) else {
        return false;
    };
    let end = (size as usize).min(data.len());
    if end < 16 {
        return false;
    }
    let major = &data[8..12];
    let (compatible, _) = data[16..end].as_chunks::<4>();
    std::iter::once(major)
        .chain(compatible.iter().map(|b| b.as_slice()))
        .any(|brand| IMAGE_BRANDS.iter().any(|b| b.as_slice() == brand))
}

struct Bmff {
    kind: [u8; 4],
    body: Range<usize>,
}

/// List the boxes within `range`.
fn boxes(data: &[u8], range: Range<usize>) -> Result<Vec<Bmff>> {
    let mut out = Vec::new();
    let mut at = range.start;
    while at + 8 <= range.end {
        let size32 = be32(data, at).expect("in bounds");
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().expect("4-byte slice");
        let (size, header) = match size32 {
            0 => (range.end - at, 8),
            1 => {
                let size = data
                    .get(at + 8..at + 16)
                    .context("truncated HEIF: box size cut off")?;
                let size = u64::from_be_bytes(size.try_into().expect("8-byte slice"));
                (usize::try_from(size).unwrap_or(usize::MAX), 16)
            }
            n => (n as usize, 8),
        };
        let end = at
            .checked_add(size)
            .filter(|&e| e <= range.end && size >= header);
        let Some(end) = end else {
            bail!(
                "corrupt HEIF: box '{}' at offset {at} overruns its container",
                String::from_utf8_lossy(&kind)
            );
        };
        out.push(Bmff {
            kind,
            body: at + header..end,
        });
        at = end;
    }
    Ok(out)
}

fn find<'a>(list: &'a [Bmff], kind: &[u8; 4]) -> Option<&'a Bmff> {
    list.iter().find(|b| &b.kind == kind)
}

/// Read an unsigned big-endian integer of 0, 4 or 8 bytes (iloc field sizes).
fn uint(data: &[u8], at: &mut usize, size: u8, limit: usize) -> Result<u64> {
    let size = size as usize;
    let bytes = data
        .get(*at..*at + size)
        .filter(|_| *at + size <= limit)
        .context("corrupt HEIF: iloc entry cut off")?;
    *at += size;
    Ok(bytes.iter().fold(0u64, |acc, &b| (acc << 8) | b as u64))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Exif,
    Xmp,
}

struct Item {
    kind: Kind,
    /// Absolute file ranges holding the item's bytes, in order.
    extents: Vec<Range<usize>>,
}

impl Item {
    fn payload(&self, data: &[u8]) -> Vec<u8> {
        self.extents
            .iter()
            .flat_map(|r| data[r.clone()].iter().copied())
            .collect()
    }

    fn len(&self) -> usize {
        self.extents.iter().map(|r| r.len()).sum()
    }
}

/// Find the EXIF and XMP items in a HEIF file.
fn metadata_items(data: &[u8]) -> Result<(Vec<Item>, Vec<String>)> {
    let top = boxes(data, 0..data.len())?;
    let meta = find(&top, b"meta").context("HEIF file has no 'meta' box")?;
    // meta is a FullBox: version and flags precede its children.
    let children = boxes(data, meta.body.start + 4..meta.body.end)?;
    let mut notes = Vec::new();

    let mut kinds: Vec<(u32, Kind)> = Vec::new();
    if let Some(iinf) = find(&children, b"iinf") {
        let version = data[iinf.body.start];
        let entries = iinf.body.start + 4 + if version == 0 { 2 } else { 4 };
        for infe in boxes(data, entries.min(iinf.body.end)..iinf.body.end)? {
            if &infe.kind != b"infe" {
                continue;
            }
            if let Some(entry) = parse_infe(&data[infe.body.clone()]) {
                kinds.push(entry);
            }
        }
    }
    if kinds.is_empty() {
        return Ok((Vec::new(), notes));
    }

    let iloc = find(&children, b"iloc").context("corrupt HEIF: metadata items but no 'iloc'")?;
    let idat = find(&children, b"idat").map(|b| b.body.clone());
    let mut items = Vec::new();
    for (id, extents, method) in parse_iloc(data, iloc.body.clone())? {
        let Some(&(_, kind)) = kinds.iter().find(|(item, _)| *item == id) else {
            continue;
        };
        let base = match (method, &idat) {
            (0, _) => 0,
            (1, Some(idat)) => idat.start,
            _ => {
                notes.push(format!(
                    "{kind:?} item {id} uses an unsupported storage method and was not read"
                ));
                continue;
            }
        };
        let limit = if method == 1 {
            idat.as_ref().map_or(data.len(), |r| r.end)
        } else {
            data.len()
        };
        let mut ranges = Vec::new();
        for (offset, length) in extents {
            let start = usize::try_from(offset)
                .ok()
                .and_then(|o| o.checked_add(base))
                .filter(|&s| s <= limit)
                .context("corrupt HEIF: item offset outside the file")?;
            // A zero length means "to the end of the container".
            let end = match length {
                0 => limit,
                n => usize::try_from(n)
                    .ok()
                    .and_then(|n| start.checked_add(n))
                    .filter(|&e| e <= limit)
                    .context("corrupt HEIF: item length outside the file")?,
            };
            ranges.push(start..end);
        }
        items.push(Item {
            kind,
            extents: ranges,
        });
    }
    Ok((items, notes))
}

/// Parse an `infe` body into (item id, metadata kind) for EXIF and XMP items.
fn parse_infe(body: &[u8]) -> Option<(u32, Kind)> {
    let version = *body.first()?;
    let cstr = |at: usize| -> Option<(&[u8], usize)> {
        let rest = body.get(at..)?;
        let nul = rest.iter().position(|&b| b == 0)?;
        Some((&rest[..nul], at + nul + 1))
    };
    let (id, item_type, after_type) = match version {
        2 => (be16(body, 4)? as u32, body.get(8..12)?, 12),
        3 => (be32(body, 4)?, body.get(10..14)?, 14),
        // Versions 0 and 1 have no item type; everything is MIME-typed.
        _ => {
            let (_, after_name) = cstr(8)?;
            let (content_type, _) = cstr(after_name)?;
            return is_xmp_mime(content_type).then_some((be16(body, 4)? as u32, Kind::Xmp));
        }
    };
    match item_type {
        b"Exif" => Some((id, Kind::Exif)),
        b"mime" => {
            let (_, after_name) = cstr(after_type)?;
            let (content_type, _) = cstr(after_name)?;
            is_xmp_mime(content_type).then_some((id, Kind::Xmp))
        }
        _ => None,
    }
}

fn is_xmp_mime(content_type: &[u8]) -> bool {
    let content_type = String::from_utf8_lossy(content_type).to_ascii_lowercase();
    content_type.contains("rdf+xml") || content_type.contains("xmp")
}

/// Parse `iloc` into (item id, [(offset, length)], construction method).
type Location = (u32, Vec<(u64, u64)>, u8);

fn parse_iloc(data: &[u8], body: Range<usize>) -> Result<Vec<Location>> {
    let limit = body.end;
    let mut at = body.start;
    let version = *data.get(at).context("corrupt HEIF: empty iloc")?;
    at += 4;
    let sizes = uint(data, &mut at, 2, limit)? as u16;
    let offset_size = (sizes >> 12) as u8;
    let length_size = ((sizes >> 8) & 0xF) as u8;
    let base_offset_size = ((sizes >> 4) & 0xF) as u8;
    let index_size = if version >= 1 { (sizes & 0xF) as u8 } else { 0 };
    for size in [offset_size, length_size, base_offset_size, index_size] {
        if ![0, 4, 8].contains(&size) {
            bail!("corrupt HEIF: invalid iloc field size {size}");
        }
    }
    let count = uint(data, &mut at, if version < 2 { 2 } else { 4 }, limit)?;

    let mut out = Vec::new();
    for _ in 0..count {
        let id = uint(data, &mut at, if version < 2 { 2 } else { 4 }, limit)? as u32;
        let method = if version >= 1 {
            (uint(data, &mut at, 2, limit)? & 0xF) as u8
        } else {
            0
        };
        let _data_reference_index = uint(data, &mut at, 2, limit)?;
        let base = uint(data, &mut at, base_offset_size, limit)?;
        let extent_count = uint(data, &mut at, 2, limit)?;
        let mut extents = Vec::new();
        for _ in 0..extent_count {
            let _index = uint(data, &mut at, index_size, limit)?;
            let offset = uint(data, &mut at, offset_size, limit)?;
            let length = uint(data, &mut at, length_size, limit)?;
            extents.push((base.saturating_add(offset), length));
        }
        out.push((id, extents, method));
    }
    Ok(out)
}

/// The TIFF block inside a HEIF EXIF item, which starts with a 4-byte
/// offset to the TIFF header (usually skipping an `Exif\0\0` prefix).
fn exif_tiff(payload: &[u8]) -> Option<&[u8]> {
    let skip = be32(payload, 0)? as usize;
    payload.get(4usize.checked_add(skip)?..)
}

pub fn inspect(data: &[u8]) -> Result<Inspection> {
    let (items, mut notes) = metadata_items(data)?;
    let mut fields = Vec::new();
    for item in &items {
        let payload = item.payload(data);
        match item.kind {
            Kind::Exif => match exif_tiff(&payload) {
                Some(tiff) => {
                    let exif = exif::parse(tiff);
                    fields.extend(exif.fields);
                    notes.extend(exif.notes);
                }
                None => notes.push(format!("unreadable EXIF item ({})", bytes(payload.len()))),
            },
            Kind::Xmp => fields.extend(xmp::parse(&payload)),
        }
    }
    Ok(Inspection { fields, notes })
}

/// Same-length replacement content for a metadata item.
fn blank(kind: Kind, len: usize) -> Vec<u8> {
    let mut out = match kind {
        // Offset 0, then a big-endian TIFF header with an empty IFD.
        Kind::Exif => b"\0\0\0\0MM\0\x2a\0\0\0\x08\0\0\0\0\0\0".to_vec(),
        Kind::Xmp => {
            let head = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
                        <x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>";
            let tail = "<?xpacket end=\"w\"?>";
            if head.len() + tail.len() <= len {
                let mut packet = head.as_bytes().to_vec();
                packet.resize(len - tail.len(), b' ');
                packet.extend_from_slice(tail.as_bytes());
                packet
            } else {
                vec![b' '; len]
            }
        }
    };
    out.resize(len, 0);
    out
}

pub fn clean(data: &[u8]) -> Result<Cleaned> {
    let (items, notes) = metadata_items(data)?;
    if let Some(note) = notes.first() {
        bail!("cannot clean this HEIF file safely: {note}");
    }
    let mut out = data.to_vec();
    let mut removed = Removals::default();
    for item in &items {
        let replacement = blank(item.kind, item.len());
        if item.payload(data) == replacement {
            continue;
        }
        let mut source = replacement.as_slice();
        for range in &item.extents {
            let (head, rest) = source.split_at(range.len());
            out[range.clone()].copy_from_slice(head);
            source = rest;
        }
        let label = match item.kind {
            Kind::Exif => "EXIF item",
            Kind::Xmp => "XMP item",
        };
        removed.add(label, item.len());
    }
    Ok(Cleaned {
        data: out,
        removed: removed.into_vec(),
        kept: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_exif_parses_to_nothing() {
        let blank = blank(Kind::Exif, 64);
        let tiff = exif_tiff(&blank).unwrap();
        let parsed = exif::parse(tiff);
        assert!(parsed.fields.is_empty() && parsed.notes.is_empty());
    }

    #[test]
    fn blank_xmp_is_a_valid_empty_packet() {
        let blank = blank(Kind::Xmp, 200);
        assert_eq!(blank.len(), 200);
        assert!(xmp::parse(&blank).is_empty());
        assert!(blank.ends_with(b"<?xpacket end=\"w\"?>"));
    }
}
