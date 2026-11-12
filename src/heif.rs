use anyhow::{Result, bail};

use crate::{Cleaned, Inspection};

pub fn inspect(_data: &[u8]) -> Result<Inspection> {
    bail!("not implemented yet")
}

pub fn clean(_data: &[u8]) -> Result<Cleaned> {
    bail!("not implemented yet")
}

/// Brands that mark an ISO-BMFF file as a HEIF still image (HEIC, AVIF).
const IMAGE_BRANDS: [&[u8; 4]; 8] = [
    b"mif1", b"mif2", b"heic", b"heix", b"heim", b"heis", b"avif", b"avci",
];

pub fn is_heif(data: &[u8]) -> bool {
    if data.get(4..8) != Some(b"ftyp") {
        return false;
    }
    let Some(size) = crate::be32(data, 0) else {
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
