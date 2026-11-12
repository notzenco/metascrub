//! Metadata fields and their privacy classification.

use std::fmt;

/// Why a metadata field matters for privacy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Where the file was created (GPS coordinates, city, country).
    Location,
    /// Who created or edited the file (author, owner, company).
    Identity,
    /// What created the file (camera make and model, serial numbers, software).
    Device,
    /// When the file was created, edited or printed.
    Time,
    /// Free text that may say anything (comments, descriptions, titles).
    Text,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Location => "location",
            Category::Identity => "identity",
            Category::Device => "device",
            Category::Time => "time",
            Category::Text => "text",
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// One metadata value found in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Where the value lives, for example `EXIF`, `GPS`, `XMP` or `PDF Info`.
    pub group: String,
    pub name: String,
    pub value: String,
    /// `Some` when the field is privacy-sensitive.
    pub category: Option<Category>,
}

impl Field {
    pub fn new(
        group: impl Into<String>,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        let name = name.into();
        Field {
            category: classify(&name),
            group: group.into(),
            name,
            value: value.into(),
        }
    }

    pub fn with_category(mut self, category: Option<Category>) -> Self {
        self.category = category;
        self
    }

    /// Nest the field under a container path, for example an image inside a document.
    pub fn within(mut self, container: &str) -> Self {
        self.group = format!("{container} › {}", self.group);
        self
    }

    pub fn is_sensitive(&self) -> bool {
        self.category.is_some()
    }
}

/// Classify a field by its name. Namespace prefixes (`dc:`, `xmp:`) and
/// separators are ignored, so `dc:creator` and `Creator` classify alike.
pub fn classify(name: &str) -> Option<Category> {
    let local = name.rsplit(':').next().unwrap_or(name);
    let n: String = local
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let has = |keys: &[&str]| keys.iter().any(|k| n.contains(k));

    if n.starts_with("gps")
        || has(&[
            "latitude",
            "longitude",
            "altitude",
            "location",
            "city",
            "country",
            "province",
        ])
    {
        Some(Category::Location)
    } else if has(&[
        "serial",
        "make",
        "model",
        "lens",
        "software",
        "creatortool",
        "producer",
        "application",
        "appversion",
        "hostcomputer",
        "firmware",
        "program",
        "template",
        "camera",
        "device",
    ]) {
        Some(Category::Device)
    } else if has(&[
        "artist",
        "author",
        "creator",
        "owner",
        "copyright",
        "rights",
        "lastmodifiedby",
        "company",
        "manager",
        "byline",
        "writer",
        "credit",
        "contact",
        "email",
    ]) {
        Some(Category::Identity)
    } else if has(&[
        "comment",
        "description",
        "caption",
        "keywords",
        "subject",
        "title",
        "headline",
        "instructions",
    ]) {
        Some(Category::Text)
    } else if has(&["date", "created", "modified", "printed", "subsectime"])
        || n.starts_with("offsettime")
        || n == "totaltime"
        || n == "timestamp"
    {
        Some(Category::Time)
    } else {
        None
    }
}

/// What `inspect` found in a file.
#[derive(Debug, Clone)]
pub struct Inspection {
    pub fields: Vec<Field>,
    /// Structural findings that are not key/value fields, such as embedded
    /// thumbnails, unknown segments or data appended after the image.
    pub notes: Vec<String>,
}

/// The output of a format-specific cleaner.
#[derive(Debug, Clone, Default)]
pub struct Cleaned {
    pub data: Vec<u8>,
    /// Human-readable descriptions of each removed block.
    pub removed: Vec<String>,
    /// Metadata that was deliberately kept, with the reason implied by its name.
    pub kept: Vec<String>,
}

/// Collects removed blocks, merging repeats into one entry with a count.
#[derive(Debug, Default)]
pub struct Removals(Vec<(String, usize, usize)>);

impl Removals {
    /// Record a removed block; a `size` of 0 means the size is not meaningful.
    pub fn add(&mut self, label: impl Into<String>, size: usize) {
        let label = label.into();
        match self.0.iter_mut().find(|(l, _, _)| *l == label) {
            Some((_, count, total)) => {
                *count += 1;
                *total += size;
            }
            None => self.0.push((label, 1, size)),
        }
    }

    pub fn into_vec(self) -> Vec<String> {
        self.0
            .into_iter()
            .map(|(label, count, size)| match (count, size) {
                (1, 0) => label,
                (n, 0) => format!("{label} ×{n}"),
                (1, size) => format!("{label} ({})", bytes(size)),
                (n, size) => format!("{label} ×{n} ({})", bytes(size)),
            })
            .collect()
    }
}

/// Format a byte count for humans.
pub fn bytes(n: usize) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_fields() {
        let cases = [
            ("GPSLatitude", Some(Category::Location)),
            ("photoshop:City", Some(Category::Location)),
            ("Make", Some(Category::Device)),
            ("BodySerialNumber", Some(Category::Device)),
            ("xmp:CreatorTool", Some(Category::Device)),
            ("dc:creator", Some(Category::Identity)),
            ("cp:lastModifiedBy", Some(Category::Identity)),
            ("By-line", Some(Category::Identity)),
            ("DateTimeOriginal", Some(Category::Time)),
            ("dcterms:created", Some(Category::Time)),
            ("ModDate", Some(Category::Time)),
            ("Comment", Some(Category::Text)),
            ("ExposureTime", None),
            ("Orientation", None),
            ("FNumber", None),
        ];
        for (name, expected) in cases {
            assert_eq!(classify(name), expected, "{name}");
        }
    }

    #[test]
    fn formats_byte_counts() {
        assert_eq!(bytes(13), "13 B");
        assert_eq!(bytes(2048), "2.0 KiB");
    }
}
