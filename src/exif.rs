//! EXIF (TIFF-structured) metadata, as embedded in JPEG, PNG, WebP and HEIF.

use exif::{Context, Exif, In, Reader, Tag, Value};

use crate::Field;

/// What an EXIF block contains.
pub struct Parsed {
    pub fields: Vec<Field>,
    pub notes: Vec<String>,
    /// The primary image's Orientation tag, if present and not the default.
    pub orientation: Option<u16>,
}

/// Parse a raw TIFF-structured EXIF block (starting at the `II`/`MM` header).
///
/// Damaged blocks yield whatever could be read plus a note, so inspection
/// still shows that the block exists and will be removed.
pub fn parse(tiff: &[u8]) -> Parsed {
    let mut notes = Vec::new();
    let exif = match Reader::new()
        .continue_on_error(true)
        .read_raw(tiff.to_vec())
    {
        Ok(exif) => exif,
        Err(e) => match e.distill_partial_result(|errors| {
            notes.push(format!(
                "EXIF block is partly damaged ({} error{})",
                errors.len(),
                if errors.len() == 1 { "" } else { "s" }
            ));
        }) {
            Ok(exif) => exif,
            Err(e) => {
                notes.push(format!(
                    "EXIF block ({}) could not be parsed: {e}",
                    crate::report::bytes(tiff.len())
                ));
                return Parsed {
                    fields: Vec::new(),
                    notes,
                    orientation: None,
                };
            }
        },
    };

    let mut fields = Vec::new();
    for field in exif.fields() {
        let group = if field.ifd_num == In::THUMBNAIL {
            "EXIF thumbnail"
        } else if field.tag.context() == Context::Gps {
            "GPS"
        } else {
            "EXIF"
        };
        fields.push(Field::new(
            group,
            field.tag.to_string(),
            display(&exif, field),
        ));
    }
    if let Some(position) = gps_position(&exif) {
        fields.push(Field::new("GPS", "GPSPosition", position));
    }
    if exif
        .get_field(Tag::JPEGInterchangeFormat, In::THUMBNAIL)
        .is_some()
    {
        notes.push("EXIF block embeds a thumbnail image".to_string());
    }

    let orientation = exif
        .get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .and_then(|v| u16::try_from(v).ok())
        .filter(|v| (2..=8).contains(v));

    Parsed {
        fields,
        notes,
        orientation,
    }
}

fn display(exif: &Exif, field: &exif::Field) -> String {
    match &field.value {
        Value::Ascii(parts) => parts
            .iter()
            .map(|p| {
                String::from_utf8_lossy(p)
                    .trim_end_matches('\0')
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::Undefined(bytes, _) if bytes.len() > 32 => {
            format!("({})", crate::report::bytes(bytes.len()))
        }
        _ => field.display_value().with_unit(exif).to_string(),
    }
}

/// Combine the GPS latitude and longitude tags into signed decimal degrees.
fn gps_position(exif: &Exif) -> Option<String> {
    let coordinate = |value: Tag, reference: Tag, negative: u8| -> Option<f64> {
        let Value::Rational(parts) = &exif.get_field(value, In::PRIMARY)?.value else {
            return None;
        };
        let degrees = parts
            .iter()
            .zip([1.0, 60.0, 3600.0])
            .map(|(r, div)| r.to_f64() / div)
            .sum::<f64>();
        let is_negative = match &exif.get_field(reference, In::PRIMARY)?.value {
            Value::Ascii(v) => v.first().and_then(|s| s.first()) == Some(&negative),
            _ => false,
        };
        degrees
            .is_finite()
            .then_some(if is_negative { -degrees } else { degrees })
    };
    let lat = coordinate(Tag::GPSLatitude, Tag::GPSLatitudeRef, b'S')?;
    let lon = coordinate(Tag::GPSLongitude, Tag::GPSLongitudeRef, b'W')?;
    Some(format!("{lat:.6}, {lon:.6}"))
}

/// A minimal big-endian TIFF block holding only the Orientation tag. Images
/// keep this so they still display upright after everything else is removed.
pub fn orientation_only(orientation: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(26);
    out.extend_from_slice(b"MM\0\x2a\0\0\0\x08");
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0x0112u16.to_be_bytes());
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&orientation.to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&0u32.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_only_block_round_trips() {
        let parsed = parse(&orientation_only(6));
        assert_eq!(parsed.orientation, Some(6));
        assert_eq!(parsed.fields.len(), 1);
        assert_eq!(parsed.fields[0].name, "Orientation");
        assert!(!parsed.fields[0].is_sensitive());
    }

    #[test]
    fn garbage_is_reported_not_fatal() {
        let parsed = parse(b"not a tiff");
        assert!(parsed.fields.is_empty());
        assert_eq!(parsed.notes.len(), 1);
    }
}
