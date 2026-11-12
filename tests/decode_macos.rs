//! Cleaned images must still decode. macOS ships an image decoder for every
//! supported image format (`sips`), so this check runs there.
#![cfg(target_os = "macos")]

mod common;

use std::process::Command;

#[test]
fn cleaned_images_decode_with_sips() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["photo.jpg", "photo.png", "photo.webp", "photo.heic"] {
        let report = metascrub::clean(&common::fixture(name)).unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, &report.data).unwrap();
        // Converting to BMP forces a full decode, not just a header read.
        let bmp = dir.path().join(format!("{name}.bmp"));
        let output = Command::new("sips")
            .args(["-s", "format", "bmp"])
            .arg(&path)
            .arg("--out")
            .arg(&bmp)
            .output()
            .expect("sips is available on macOS");
        assert!(
            output.status.success() && bmp.exists(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
