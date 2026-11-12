mod common;

use std::fs;
use std::path::Path;

use assert_cmd::cargo::cargo_bin_cmd;
use common::{fixture, fixture_path, markers_in};

fn metascrub() -> assert_cmd::Command {
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("--color=never");
    cmd
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn copy_fixture(name: &str, dir: &Path) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::copy(fixture_path(name), &path).unwrap();
    path
}

#[test]
fn clean_writes_a_copy_and_leaves_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let original = copy_fixture("photo.jpg", dir.path());

    let output = metascrub().arg("clean").arg(&original).assert().success();
    let out = stdout(output.get_output());
    assert!(out.contains("photo.clean.jpg"), "{out}");
    assert!(out.contains("removed EXIF"), "{out}");

    assert_eq!(fs::read(&original).unwrap(), fixture("photo.jpg"));
    let cleaned = fs::read(dir.path().join("photo.clean.jpg")).unwrap();
    assert!(markers_in(&cleaned).is_empty());
}

#[test]
fn clean_refuses_to_overwrite_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let original = copy_fixture("photo.jpg", dir.path());
    let target = dir.path().join("photo.clean.jpg");
    fs::write(&target, b"keep me").unwrap();

    let output = metascrub().arg("clean").arg(&original).assert().failure();
    assert!(String::from_utf8_lossy(&output.get_output().stderr).contains("--force"));
    assert_eq!(fs::read(&target).unwrap(), b"keep me");

    metascrub()
        .args(["clean", "--force"])
        .arg(&original)
        .assert()
        .success();
    assert!(markers_in(&fs::read(&target).unwrap()).is_empty());
}

#[test]
fn clean_in_place_replaces_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let original = copy_fixture("photo.jpg", dir.path());

    metascrub()
        .args(["clean", "--in-place"])
        .arg(&original)
        .assert()
        .success();
    assert!(markers_in(&fs::read(&original).unwrap()).is_empty());
    let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(
        entries.len(),
        1,
        "no temporary or extra files are left behind"
    );
}

#[test]
fn dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let original = copy_fixture("photo.jpg", dir.path());

    let output = metascrub()
        .args(["clean", "-n"])
        .arg(&original)
        .assert()
        .success();
    assert!(stdout(output.get_output()).contains("would write"));
    assert!(!dir.path().join("photo.clean.jpg").exists());
}

#[test]
fn clean_directory_into_out_dir_mirrors_layout_and_skips_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("trip");
    fs::create_dir_all(input.join("day1")).unwrap();
    copy_fixture("photo.jpg", &input.join("day1"));
    fs::write(input.join("notes.txt"), "not an image").unwrap();
    let out_dir = dir.path().join("out");

    let output = metascrub()
        .args(["clean", "-o"])
        .arg(&out_dir)
        .arg(&input)
        .assert()
        .success();
    let out = stdout(output.get_output());
    assert!(
        out.contains("cleaned 1 file, skipped 1 unsupported"),
        "{out}"
    );
    let cleaned = fs::read(out_dir.join("trip/day1/photo.jpg")).unwrap();
    assert!(markers_in(&cleaned).is_empty());
}

#[test]
fn clean_directory_skips_previous_outputs() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture("photo.jpg", dir.path());
    metascrub().arg("clean").arg(dir.path()).assert().success();
    metascrub()
        .args(["clean", "--force"])
        .arg(dir.path())
        .assert()
        .success();
    assert!(!dir.path().join("photo.clean.clean.jpg").exists());
}

#[test]
fn format_comes_from_content_not_extension() {
    let dir = tempfile::tempdir().unwrap();
    let disguised = dir.path().join("photo.png");
    fs::copy(fixture_path("photo.jpg"), &disguised).unwrap();

    let output = metascrub()
        .arg("inspect")
        .arg(&disguised)
        .assert()
        .success();
    assert!(stdout(output.get_output()).contains("(JPEG)"));
}

#[test]
fn inspect_highlights_sensitive_fields() {
    let output = metascrub()
        .arg("inspect")
        .arg(fixture_path("photo.jpg"))
        .assert()
        .success();
    let out = stdout(output.get_output());
    assert!(out.contains("! GPSPosition"), "{out}");
    assert!(out.contains("[location]"), "{out}");
    assert!(out.contains("Orientation"), "{out}");

    let output = metascrub()
        .args(["inspect", "--sensitive"])
        .arg(fixture_path("photo.jpg"))
        .assert()
        .success();
    assert!(!stdout(output.get_output()).contains("Orientation"));
}

#[test]
fn unsupported_files_fail_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let gif = dir.path().join("anim.gif");
    fs::write(&gif, b"GIF89a....").unwrap();

    let output = metascrub().arg("clean").arg(&gif).assert().failure();
    let stderr = String::from_utf8_lossy(&output.get_output().stderr).into_owned();
    assert!(stderr.contains("unsupported format: GIF image"), "{stderr}");
}

fn json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", stdout(output)))
}

#[test]
fn inspect_json_lists_fields_with_categories() {
    let output = metascrub()
        .args(["inspect", "--json"])
        .arg(fixture_path("photo.jpg"))
        .assert()
        .success();
    let report = json(output.get_output());
    let file = &report[0];
    assert_eq!(file["format"], "JPEG");
    assert!(file["error"].is_null());
    let fields = file["fields"].as_array().unwrap();
    let gps = fields.iter().find(|f| f["name"] == "GPSPosition").unwrap();
    assert_eq!(gps["group"], "GPS");
    assert_eq!(gps["value"], "48.858300, 2.294500");
    assert_eq!(gps["category"], "location");
    let orientation = fields.iter().find(|f| f["name"] == "Orientation").unwrap();
    assert!(orientation["category"].is_null());
    assert!(file["notes"].as_array().unwrap().len() == 1);
}

#[test]
fn clean_json_reports_each_file_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let photo = copy_fixture("photo.jpg", dir.path());
    let text = dir.path().join("notes.txt");
    fs::write(&text, "plain text").unwrap();

    let output = metascrub()
        .args(["clean", "--json"])
        .arg(&photo)
        .arg(&text)
        .assert()
        .failure();
    let out = output.get_output();
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = json(out);
    let files = report.as_array().unwrap();
    assert_eq!(files.len(), 2);

    assert_eq!(files[0]["status"], "cleaned");
    assert_eq!(files[0]["format"], "JPEG");
    let written = files[0]["output"].as_str().unwrap();
    assert!(written.ends_with("photo.clean.jpg"), "{written}");
    assert!(markers_in(&fs::read(written).unwrap()).is_empty());
    assert!(
        files[0]["removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap().starts_with("EXIF"))
    );
    assert!(
        files[0]["removed_fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "GPSPosition" && f["category"] == "location")
    );
    assert_eq!(files[0]["kept"][0], "ICC colour profile");

    assert_eq!(files[1]["status"], "error");
    assert!(files[1]["output"].is_null());
    assert!(
        files[1]["error"]
            .as_str()
            .unwrap()
            .contains("unsupported format")
    );
}

#[test]
fn clean_json_dry_run_and_skipped_files() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture("photo.jpg", dir.path());
    fs::write(dir.path().join("notes.txt"), "plain text").unwrap();

    let output = metascrub()
        .args(["clean", "--json", "-n"])
        .arg(dir.path())
        .assert()
        .success();
    let report = json(output.get_output());
    let statuses: Vec<&str> = report
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["skipped", "would_clean"]);
    assert!(!dir.path().join("photo.clean.jpg").exists());
}
