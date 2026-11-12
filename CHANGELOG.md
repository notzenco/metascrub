# Changelog

All notable changes to this project are documented here. The format is based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `inspect` lists metadata and flags privacy-sensitive fields as location,
  identity, device, time or free text.
- `clean` writes cleaned copies (`name.clean.ext`), into a directory with
  `--out-dir`, or over the originals with `--in-place`; `--dry-run`,
  `--force` and `--verbose` are supported.
- `--json` output for both commands, with per-file errors in the report.
- Formats, detected by content: JPEG, PNG, WebP, HEIF (HEIC and AVIF), PDF,
  and Office Open XML (DOCX, XLSX, PPTX).
- Lossless image cleaning: container metadata is removed without decoding
  or re-encoding pixels; ICC profiles and orientation are kept.
- PDF cleaning of the Info dictionary, XMP streams, `/PieceInfo`, metadata in
  embedded JPEGs, annotation authors and dates, and earlier revisions.
- Office cleaning of document properties, embedded image metadata, ZIP
  timestamps, comment and tracked-change authors, revision session IDs and
  Excel's last-saved folder path.
- Every cleaned file is inspected again, and output is refused if any
  privacy-sensitive field survived.
- Terminal output escapes control characters from metadata and file names.
