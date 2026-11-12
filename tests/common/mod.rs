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

pub fn field<'a>(inspection: &'a metascrub::Inspection, name: &str) -> &'a metascrub::Field {
    inspection
        .fields
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name} in {:#?}", inspection.fields))
}

/// Clean a fixture and check the properties every format must have: no
/// planted marker survives, no sensitive field survives, and cleaning the
/// output again changes nothing.
pub fn assert_cleans(name: &str) -> metascrub::CleanReport {
    let report = metascrub::clean(&fixture(name)).unwrap();
    assert_eq!(markers_in(&report.data), Vec::<&str>::new(), "{name}");
    let (_, after) = metascrub::inspect(&report.data).unwrap();
    let sensitive: Vec<_> = after.fields.iter().filter(|f| f.is_sensitive()).collect();
    assert!(sensitive.is_empty(), "{name}: {sensitive:#?}");
    assert!(after.notes.is_empty(), "{name}: {:?}", after.notes);

    let again = metascrub::clean(&report.data).unwrap();
    assert!(
        again.data == report.data,
        "{name}: second clean changed the output"
    );
    assert!(again.removed.is_empty(), "{name}: {:?}", again.removed);
    report
}
