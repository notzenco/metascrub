use anyhow::{Result, bail};

use std::io::Cursor;

use crate::{Cleaned, Inspection, OfficeKind};

pub fn inspect(_data: &[u8]) -> Result<Inspection> {
    bail!("not implemented yet")
}

pub fn clean(_data: &[u8]) -> Result<Cleaned> {
    bail!("not implemented yet")
}

/// Identify an Office Open XML package from the parts it contains.
pub fn kind(data: &[u8]) -> Option<OfficeKind> {
    let archive = zip::ZipArchive::new(Cursor::new(data)).ok()?;
    let has = |name: &str| archive.index_for_name(name).is_some();
    if !has("[Content_Types].xml") {
        return None;
    }
    Some(if has("word/document.xml") {
        OfficeKind::Word
    } else if has("xl/workbook.xml") {
        OfficeKind::Excel
    } else if has("ppt/presentation.xml") {
        OfficeKind::PowerPoint
    } else {
        OfficeKind::Other
    })
}
