use anyhow::{Result, bail};

use crate::{Cleaned, Inspection};

pub fn inspect(_data: &[u8]) -> Result<Inspection> {
    bail!("not implemented yet")
}

pub fn clean(_data: &[u8]) -> Result<Cleaned> {
    bail!("not implemented yet")
}
pub const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
