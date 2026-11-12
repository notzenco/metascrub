//! People and editing history recorded inside Office document content:
//! comment and tracked-change authors, comment dates, revision session IDs
//! (Word `rsid`), user IDs of signed-in authors and Excel's last-saved
//! folder path.
//!
//! Names are replaced with a neutral placeholder rather than removed, so
//! comments and tracked changes keep working; optional dates and session
//! IDs are removed. Unchanged markup is copied byte for byte.

use std::collections::BTreeSet;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::report::Category;
use crate::xmp::resolve_ref;
use quick_xml::XmlVersion;

use crate::Field;

const AUTHOR: &str = "Author";
const INITIALS: &str = "A";
const PROVIDER: &str = "None";
const EPOCH: &str = "1980-01-01T00:00:00Z";
const EPOCH_MS: &str = "1980-01-01T00:00:00.000";

#[derive(Clone, Copy)]
enum Action {
    Replace(&'static str),
    Remove,
}

struct Rule {
    /// Element local name, or `None` for any element.
    element: Option<&'static str>,
    /// Attribute local name.
    attr: &'static str,
    field: &'static str,
    category: Option<Category>,
    action: Action,
}

const fn rule(
    element: Option<&'static str>,
    attr: &'static str,
    field: &'static str,
    category: Option<Category>,
    action: Action,
) -> Rule {
    Rule {
        element,
        attr,
        field,
        category,
        action,
    }
}

use Action::{Remove, Replace};
const ID: Option<Category> = Some(Category::Identity);
const TIME: Option<Category> = Some(Category::Time);

/// The first matching rule wins, so element-specific rules come first.
const WORD: &[Rule] = &[
    rule(
        Some("comment"),
        "author",
        "CommentAuthor",
        ID,
        Replace(AUTHOR),
    ),
    rule(
        Some("comment"),
        "initials",
        "CommentInitials",
        ID,
        Replace(INITIALS),
    ),
    rule(Some("comment"), "date", "CommentDate", TIME, Remove),
    rule(Some("person"), "author", "PersonName", ID, Replace(AUTHOR)),
    rule(
        Some("presenceInfo"),
        "userId",
        "PersonUserId",
        ID,
        Replace(AUTHOR),
    ),
    rule(
        Some("presenceInfo"),
        "providerId",
        "PersonProvider",
        None,
        Replace(PROVIDER),
    ),
    rule(None, "author", "RevisionAuthor", ID, Replace(AUTHOR)),
    rule(None, "date", "RevisionDate", TIME, Remove),
    rule(None, "dateUtc", "CommentDate", TIME, Remove),
];

const EXCEL_PERSONS: &[Rule] = &[
    rule(
        Some("person"),
        "displayName",
        "PersonName",
        ID,
        Replace(AUTHOR),
    ),
    rule(
        Some("person"),
        "userId",
        "PersonUserId",
        ID,
        Replace(AUTHOR),
    ),
    rule(
        Some("person"),
        "providerId",
        "PersonProvider",
        None,
        Replace(PROVIDER),
    ),
];

const EXCEL_THREADED: &[Rule] = &[rule(None, "dT", "CommentDate", TIME, Remove)];

const EXCEL_REVISIONS: &[Rule] = &[
    rule(None, "userName", "RevisionAuthor", ID, Replace(AUTHOR)),
    rule(None, "dateTime", "RevisionDate", TIME, Replace(EPOCH)),
];

const EXCEL_WORKBOOK: &[Rule] = &[rule(Some("absPath"), "url", "SavedPath", ID, Replace(""))];

const POWERPOINT_AUTHORS: &[Rule] = &[
    rule(None, "name", "CommentAuthor", ID, Replace(AUTHOR)),
    rule(None, "initials", "CommentInitials", ID, Replace(INITIALS)),
    rule(None, "userId", "PersonUserId", ID, Replace(AUTHOR)),
    rule(
        None,
        "providerId",
        "PersonProvider",
        None,
        Replace(PROVIDER),
    ),
];

const POWERPOINT_COMMENTS: &[Rule] = &[
    rule(Some("cm"), "dt", "CommentDate", TIME, Remove),
    rule(None, "created", "CommentDate", TIME, Replace(EPOCH_MS)),
];

/// How a package part is handled.
struct PartRules {
    attrs: &'static [Rule],
    /// Word: drop `rsid*` attributes and the `w:rsids` list.
    rsids: bool,
    /// Excel legacy comments: author names are element text, and each note
    /// starts with the author's name in bold.
    excel_comments: bool,
}

fn rules_for(part: &str) -> Option<PartRules> {
    let xml = part.ends_with(".xml");
    let attrs_only = |attrs| {
        Some(PartRules {
            attrs,
            rsids: false,
            excel_comments: false,
        })
    };
    if part.starts_with("word/") && xml {
        Some(PartRules {
            attrs: WORD,
            rsids: true,
            excel_comments: false,
        })
    } else if part.starts_with("xl/comments") && xml {
        Some(PartRules {
            attrs: &[],
            rsids: false,
            excel_comments: true,
        })
    } else if part.starts_with("xl/persons/") && xml {
        attrs_only(EXCEL_PERSONS)
    } else if part.starts_with("xl/threadedComments/") && xml {
        attrs_only(EXCEL_THREADED)
    } else if part.starts_with("xl/revisions/") && xml {
        attrs_only(EXCEL_REVISIONS)
    } else if part == "xl/workbook.xml" {
        attrs_only(EXCEL_WORKBOOK)
    } else if part == "ppt/commentAuthors.xml" || part == "ppt/authors.xml" {
        attrs_only(POWERPOINT_AUTHORS)
    } else if part.starts_with("ppt/comments/") && xml {
        attrs_only(POWERPOINT_COMMENTS)
    } else {
        None
    }
}

pub fn handles(part: &str) -> bool {
    rules_for(part).is_some()
}

/// What a part reveals, and its rewritten bytes if anything had to change.
pub struct Processed {
    pub fields: Vec<Field>,
    pub cleaned: Option<Vec<u8>>,
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// Text being collected inside an element whose content may be replaced.
struct Capture {
    element: String,
    raw: String,
    value: String,
}

struct State<'a> {
    part: &'a str,
    rules: PartRules,
    fields: Vec<Field>,
    rsids: BTreeSet<String>,
    authors: Vec<String>,
}

impl State<'_> {
    fn report(&mut self, name: &str, value: &str, category: Option<Category>) {
        let field = Field::new(self.part, name, value).with_category(category);
        if !self.fields.contains(&field) {
            self.fields.push(field);
        }
    }

    /// Rewrite a start or empty tag if any attribute needs to change.
    fn rewrite_tag(&mut self, e: &BytesStart, empty: bool) -> Option<String> {
        let element = e.local_name().as_ref().to_string();
        let mut kept: Vec<(String, String)> = Vec::new();
        let mut changed = false;
        for attr in e.attributes().with_checks(false).flatten() {
            let key = attr.key.as_ref().to_string();
            let raw = attr.value.to_string();
            let value = attr
                .normalized_value(XmlVersion::Implicit1_0)
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| raw.clone());
            let name = local(&key);

            if self.rules.rsids && name.starts_with("rsid") {
                self.rsids.insert(value);
                changed = true;
                continue;
            }
            let matched = self
                .rules
                .attrs
                .iter()
                .find(|r| r.attr == name && r.element.is_none_or(|el| el == element.as_str()));
            match matched {
                Some(rule) => match rule.action {
                    Replace(placeholder) => {
                        if value != placeholder {
                            self.report(rule.field, &value, rule.category);
                            changed = true;
                        }
                        kept.push((key, placeholder.to_string()));
                    }
                    Remove => {
                        self.report(rule.field, &value, rule.category);
                        changed = true;
                    }
                },
                None => kept.push((key, raw)),
            }
        }
        if !changed {
            return None;
        }
        let mut tag = format!("<{}", e.name().as_ref());
        for (key, raw) in kept {
            let quote = if raw.contains('"') { '\'' } else { '"' };
            tag.push_str(&format!(" {key}={quote}{raw}{quote}"));
        }
        tag.push_str(if empty { "/>" } else { ">" });
        Some(tag)
    }

    /// Decide what replaces captured element text in Excel comments.
    fn replace_text(&mut self, capture: &Capture) -> Option<&'static str> {
        let value = capture.value.trim();
        if capture.element == "author" {
            if value.is_empty() || value == AUTHOR {
                return None;
            }
            self.authors.push(value.to_string());
            self.report("CommentAuthor", value, ID);
            return Some(AUTHOR);
        }
        let name = value.strip_suffix(':').unwrap_or(value);
        if self.authors.iter().any(|a| a == name) {
            return Some(if value.ends_with(':') {
                "Author:"
            } else {
                AUTHOR
            });
        }
        None
    }
}

/// Inspect a package part, and rewrite it without names, dates and IDs.
pub fn process(part: &str, xml: &[u8]) -> Processed {
    let unchanged = |fields| Processed {
        fields,
        cleaned: None,
    };
    let Some(rules) = rules_for(part) else {
        return unchanged(Vec::new());
    };
    // Parts are UTF-8 in practice; anything else is left for the
    // post-clean verification to catch.
    let Ok(text) = std::str::from_utf8(xml) else {
        return unchanged(Vec::new());
    };
    let mut state = State {
        part,
        rules,
        fields: Vec::new(),
        rsids: BTreeSet::new(),
        authors: Vec::new(),
    };
    let mut reader = Reader::from_str(text);
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut last = 0usize;
    // Depth inside a dropped `w:rsids` element.
    let mut skipping = 0usize;
    let mut capture: Option<Capture> = None;

    loop {
        let event = match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(event) => event,
            Err(_) => return unchanged(state.fields),
        };
        let pos = reader.buffer_position() as usize;
        let span = &text[last..pos];
        last = pos;

        if skipping > 0 {
            match &event {
                Event::Start(e) | Event::Empty(e) => {
                    for attr in e.attributes().flatten() {
                        if local(attr.key.as_ref()) == "val" {
                            state.rsids.insert(attr.value.to_string());
                        }
                    }
                    if matches!(event, Event::Start(_)) {
                        skipping += 1;
                    }
                }
                Event::End(_) => skipping -= 1,
                _ => {}
            }
            continue;
        }

        if let Some(c) = capture.as_mut() {
            match &event {
                Event::Text(t) => {
                    c.raw.push_str(span);
                    c.value.push_str(&t.xml10_content());
                    continue;
                }
                Event::GeneralRef(r) => {
                    c.raw.push_str(span);
                    c.value.push_str(&resolve_ref(r));
                    continue;
                }
                Event::CData(t) => {
                    c.raw.push_str(span);
                    c.value.push_str(t);
                    continue;
                }
                Event::End(_) => {
                    let c = capture.take().expect("capture is active");
                    match state.replace_text(&c) {
                        Some(replacement) => {
                            out.push_str(replacement);
                            changed = true;
                        }
                        None => out.push_str(&c.raw),
                    }
                    out.push_str(span);
                    continue;
                }
                // Nested markup: give up on replacing this element's text.
                _ => {
                    let c = capture.take().expect("capture is active");
                    out.push_str(&c.raw);
                }
            }
        }

        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let is_empty = matches!(event, Event::Empty(_));
                let element = e.local_name().as_ref().to_string();
                if state.rules.rsids && element == "rsids" {
                    changed = true;
                    if !is_empty {
                        skipping = 1;
                    }
                    continue;
                }
                match state.rewrite_tag(e, is_empty) {
                    Some(tag) => {
                        out.push_str(&tag);
                        changed = true;
                    }
                    None => out.push_str(span),
                }
                if state.rules.excel_comments
                    && !is_empty
                    && (element == "author" || element == "t")
                {
                    capture = Some(Capture {
                        element,
                        raw: String::new(),
                        value: String::new(),
                    });
                }
            }
            _ => out.push_str(span),
        }
    }
    out.push_str(&text[last..]);

    if !state.rsids.is_empty() {
        let n = state.rsids.len();
        let value = format!("{n} ID{}", if n == 1 { "" } else { "s" });
        state.report("RevisionSessionIDs", &value, Some(Category::Device));
    }
    Processed {
        fields: state.fields,
        cleaned: changed.then(|| out.into_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untouched_parts_are_not_rewritten() {
        let xml = br#"<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>Hi &amp; bye</w:t></w:r></w:p></w:body></w:document>"#;
        let processed = process("word/document.xml", xml);
        assert!(processed.fields.is_empty());
        assert!(processed.cleaned.is_none());
    }

    #[test]
    fn keeps_escaping_and_other_attributes() {
        let xml = br#"<w:ins w:id="3" w:author="A &amp; B" w:date="2020-01-01T00:00:00Z" w:x='say "hi"'/>"#;
        let processed = process("word/document.xml", xml);
        let cleaned = String::from_utf8(processed.cleaned.unwrap()).unwrap();
        assert_eq!(
            cleaned,
            r#"<w:ins w:id="3" w:author="Author" w:x='say "hi"'/>"#
        );
        assert_eq!(processed.fields[0].value, "A & B");
        let again = process("word/document.xml", cleaned.as_bytes());
        assert!(again.fields.is_empty() && again.cleaned.is_none());
    }
}
