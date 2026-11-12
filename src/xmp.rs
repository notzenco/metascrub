//! XMP packets (RDF/XML), as embedded in images and PDFs.

use quick_xml::Reader;
use quick_xml::events::{BytesRef, BytesStart, Event};

use crate::Field;

/// Flatten an XMP packet into `prefix:Property = value` fields.
///
/// Arrays (`rdf:Seq`, `rdf:Bag`, `rdf:Alt`) are joined with commas and
/// attributed to their enclosing property. Malformed XML yields whatever was
/// read before the error.
pub fn parse(packet: &[u8]) -> Vec<Field> {
    let text = String::from_utf8_lossy(packet);
    let mut reader = Reader::from_str(&text);
    reader.config_mut().trim_text(true);

    let mut fields = Vec::new();
    // One entry per open element: the property name (None for rdf/x
    // structural elements) and the text collected for it.
    let mut stack: Vec<(Option<String>, String)> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = qname(&e);
                attributes(&e, &stack, &mut fields);
                let property = (!is_structural(&name)).then_some(name);
                stack.push((property, String::new()));
            }
            Ok(Event::Empty(e)) => attributes(&e, &stack, &mut fields),
            Ok(Event::Text(t)) => append(&mut stack, &t.xml10_content()),
            Ok(Event::CData(t)) => append(&mut stack, &t),
            Ok(Event::GeneralRef(r)) => append(&mut stack, &resolve_ref(&r)),
            Ok(Event::End(e)) => {
                let Some((property, text)) = stack.pop() else {
                    break;
                };
                let ended = e.name().as_ref().to_string();
                match property {
                    Some(name) => {
                        let value = text.trim().trim_end_matches(',').trim();
                        if !value.is_empty() {
                            fields.push(Field::new("XMP", name, value));
                        }
                    }
                    // Separate array items as they close.
                    None if ended == "rdf:li" => append(&mut stack, ", "),
                    None => {}
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
    fields
}

/// Resolve an entity or character reference; unknown entities become empty.
pub(crate) fn resolve_ref(r: &BytesRef) -> String {
    match r.resolve_char_ref() {
        Ok(Some(c)) => c.to_string(),
        _ => match &**r {
            "amp" => "&".into(),
            "lt" => "<".into(),
            "gt" => ">".into(),
            "quot" => "\"".into(),
            "apos" => "'".into(),
            _ => String::new(),
        },
    }
}

fn qname(e: &BytesStart) -> String {
    e.name().as_ref().to_string()
}

fn is_structural(name: &str) -> bool {
    name.starts_with("rdf:") || name.starts_with("x:")
}

/// Text belongs to the innermost enclosing property.
fn append(stack: &mut [(Option<String>, String)], text: &str) {
    if let Some((_, buffer)) = stack.iter_mut().rev().find(|(p, _)| p.is_some()) {
        if text == ", " && (buffer.is_empty() || buffer.ends_with(", ")) {
            return;
        }
        buffer.push_str(text);
    }
}

/// Properties written in attribute form, as in
/// `<rdf:Description xmp:CreatorTool="...">`.
fn attributes(e: &BytesStart, stack: &[(Option<String>, String)], fields: &mut Vec<Field>) {
    let parent = stack.iter().rev().find_map(|(p, _)| p.as_deref());
    for attr in e.attributes().flatten() {
        let key = attr.key.as_ref().to_string();
        if key.starts_with("xmlns") || is_structural(&key) || key == "xml:lang" {
            continue;
        }
        let Ok(value) = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0) else {
            continue;
        };
        if value.trim().is_empty() {
            continue;
        }
        let name = match parent {
            Some(p) => format!("{p}/{key}"),
            None => key,
        };
        fields.push(Field::new("XMP", name, value.trim()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_attributes_and_arrays() {
        let packet = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/"
              xmp:CreatorTool="Tool &amp; Co">
              <dc:creator><rdf:Seq><rdf:li>Ann</rdf:li><rdf:li>Bob</rdf:li></rdf:Seq></dc:creator>
              <xmp:CreateDate>2019-07-14</xmp:CreateDate>
            </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let fields = parse(packet);
        let pairs: Vec<(&str, &str)> = fields
            .iter()
            .map(|f| (f.name.as_str(), f.value.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("xmp:CreatorTool", "Tool & Co"),
                ("dc:creator", "Ann, Bob"),
                ("xmp:CreateDate", "2019-07-14"),
            ]
        );
    }

    #[test]
    fn tolerates_malformed_xml() {
        let fields = parse(b"<x:xmpmeta><dc:title>half");
        assert!(fields.is_empty());
    }
}
