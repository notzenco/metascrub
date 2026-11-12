//! Photoshop image resources (JPEG APP13) and the IPTC-IIM records inside them.

use crate::{Field, be16, be32};

const IPTC_RESOURCE: u16 = 0x0404;

/// Parse the body of an APP13 segment that follows `Photoshop 3.0\0`.
pub fn parse_photoshop(irb: &[u8]) -> (Vec<Field>, Vec<String>) {
    let mut fields = Vec::new();
    let mut other = 0usize;
    let mut at = 0;
    while irb.get(at..at + 4) == Some(b"8BIM") {
        let Some(id) = be16(irb, at + 4) else { break };
        // Pascal-string name padded to an even length (length byte included).
        let Some(&name_len) = irb.get(at + 6) else {
            break;
        };
        let name_size = (1 + name_len as usize).next_multiple_of(2);
        let size_at = at + 6 + name_size;
        let Some(size) = be32(irb, size_at) else {
            break;
        };
        let start = size_at + 4;
        let Some(end) = start.checked_add(size as usize).filter(|&e| e <= irb.len()) else {
            break;
        };
        if id == IPTC_RESOURCE {
            fields.extend(parse_iim(&irb[start..end]));
        } else {
            other += 1;
        }
        at = end.next_multiple_of(2);
    }
    let mut notes = Vec::new();
    if other > 0 {
        notes.push(format!("{other} other Photoshop image resource(s)"));
    }
    (fields, notes)
}

/// Parse IPTC-IIM datasets: `0x1C record dataset length(u16) value`.
fn parse_iim(data: &[u8]) -> Vec<Field> {
    let mut fields = Vec::new();
    let mut at = 0;
    while data.get(at) == Some(&0x1C) {
        let (Some(&record), Some(&dataset), Some(len)) =
            (data.get(at + 1), data.get(at + 2), be16(data, at + 3))
        else {
            break;
        };
        // Extended (>32 KiB) datasets set the high bit; they are not
        // used for the text fields shown here.
        if len & 0x8000 != 0 {
            break;
        }
        let start = at + 5;
        let Some(value) = data.get(start..start + len as usize) else {
            break;
        };
        if record == 2 && dataset != 0 {
            let name = dataset_name(dataset)
                .map(str::to_string)
                .unwrap_or_else(|| format!("2:{dataset}"));
            fields.push(Field::new(
                "IPTC",
                name,
                String::from_utf8_lossy(value).trim(),
            ));
        }
        at = start + len as usize;
    }
    fields
}

fn dataset_name(dataset: u8) -> Option<&'static str> {
    Some(match dataset {
        5 => "ObjectName",
        25 => "Keywords",
        40 => "SpecialInstructions",
        55 => "DateCreated",
        60 => "TimeCreated",
        62 => "DigitalCreationDate",
        63 => "DigitalCreationTime",
        65 => "OriginatingProgram",
        80 => "By-line",
        85 => "By-lineTitle",
        90 => "City",
        92 => "Sub-location",
        95 => "Province-State",
        100 => "Country-PrimaryLocationCode",
        101 => "Country-PrimaryLocationName",
        105 => "Headline",
        110 => "Credit",
        115 => "Source",
        116 => "CopyrightNotice",
        118 => "Contact",
        120 => "Caption-Abstract",
        122 => "Writer-Editor",
        _ => return None,
    })
}
