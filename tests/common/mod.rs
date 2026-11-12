#![allow(dead_code)]

use std::path::PathBuf;

/// Strings planted in the fixtures by tests/fixtures/generate.py.
pub const MARKERS: &[&str] = &[
    "MetascrubCam",
    "MSX-1000",
    "SN-0042-SECRET",
    "Jane Example",
    "MSFirmware",
    "Secret comment",
    "ExampleCorp",
    "TRAILERSECRET",
];

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixture_path(name)).unwrap()
}

pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

pub fn markers_in(data: &[u8]) -> Vec<&'static str> {
    MARKERS
        .iter()
        .copied()
        .filter(|m| find(data, m.as_bytes()).is_some())
        .collect()
}

pub fn field_names(inspection: &metascrub::Inspection) -> Vec<String> {
    inspection.fields.iter().map(|f| f.name.clone()).collect()
}
