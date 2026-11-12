# metascrub

[![CI](https://github.com/notzenco/metascrub/actions/workflows/ci.yml/badge.svg)](https://github.com/notzenco/metascrub/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Strip identifying metadata from photos and documents before you share them.

A photo straight off a phone can carry the GPS position it was taken at, the
camera's serial number and the exact time. A Word document records who wrote
it, who last edited it and which company's copy of Office they used. A PDF
can still contain the values from before its last edit. `metascrub` shows you
what is there and writes a clean copy without it.

- **Inspect** shows every metadata field it finds and flags the privacy-sensitive
  ones: location, identity, device, time and free text.
- **Clean** writes cleaned copies by default and only touches originals with
  `--in-place`. It reports what it removed.
- **Scriptable.** `--json` prints a machine-readable report for both commands.
- **Lossless.** Image cleaning edits the container (JPEG segments, PNG
  chunks, RIFF chunks, HEIF items) and never decodes or re-encodes pixels.
- **Verified.** Every cleaned file is inspected again before it is written.
  If any sensitive field survived, metascrub refuses to write the file.
- **Formats are detected by content**, not by extension, so a JPEG named
  `scan.png` is still treated as a JPEG.

## Supported formats

| Format | What is removed | What is kept |
| --- | --- | --- |
| JPEG | EXIF (including GPS and the embedded thumbnail), XMP and extended XMP, IPTC and Photoshop resources, comments, JFIF thumbnails, MPF indexes, JUMBF/C2PA data, unknown APPn segments, data after the end-of-image marker | Compressed image data byte-for-byte, ICC colour profile, Adobe colour transform, EXIF orientation |
| PNG | `eXIf`, `tEXt`, `zTXt` and `iTXt` (including XMP), `tIME`, unknown ancillary chunks, data after `IEND` | Image data (`IDAT`) byte-for-byte, colour, gamma, transparency, physical size and APNG animation chunks, orientation |
| WebP | `EXIF` and `XMP` chunks, unknown chunks; VP8X flags and RIFF size are corrected | Bitstream, alpha, animation and ICC chunks byte-for-byte, orientation |
| HEIF (HEIC, AVIF) | EXIF and XMP items, overwritten in place with an empty EXIF block and an empty XMP packet of the same size | Every box, offset and coded image byte |
| PDF | The Info dictionary (author, creator, producer, dates, custom keys), XMP metadata streams, `/PieceInfo` private data, metadata inside embedded JPEG images, annotation authors and dates, earlier revisions left by incremental updates | Pages, text, fonts and image streams (not re-encoded), annotation text, form field names |
| DOCX, XLSX, PPTX | Core properties (author, last editor, dates, title, revision), extended properties (application, company, manager, template, editing time), custom properties, metadata in embedded images, ZIP entry timestamps, the ZIP comment. Inside the content: comment and tracked-change authors, initials and signed-in user IDs (replaced with `Author`), comment and revision dates, Word revision session IDs (`rsid`), Excel's last-saved folder path | Comments and tracked changes themselves, and every other part with its content and order unchanged |

### What metascrub does not remove

Metadata is only part of what a file can reveal. metascrub does **not**:

- change what is *in* a picture or a document: faces, street signs,
  reflections, text in screenshots, or the words of a document;
- remove names that are part of the *text*: a name typed into a document, a
  comment or a slide stays, as do PDF form field values and the contents of
  embedded OLE objects (for example a spreadsheet pasted into a document);
- clean HEIF thumbnails, depth maps or other auxiliary images, HEIF image
  sequences, PDF images other than JPEG (for example JPEG 2000), or the PDF
  document `/ID`;
- remove ICC colour profiles or the image orientation (both are needed to
  display the image correctly and are not identifying in ordinary use);
- handle GIF, TIFF and camera raw files, video, audio, OpenDocument or legacy
  Office (`.doc`, `.xls`, `.ppt`) files, or encrypted PDFs and packages. These
  are reported as unsupported rather than passed through;
- rename files. A file called `IMG_2041 Jane's house.jpg` still says so.

Cleaned files are new files. They take the original's permissions but none
of its extended attributes, so macOS details such as
`com.apple.metadata:kMDItemWhereFroms` (the download URL) and quarantine
flags are not copied. With `--in-place` the original is replaced the same way.

Treat metascrub as one step in a privacy review, not a guarantee of anonymity.

## Install

With Rust 1.89 or later:

```sh
cargo install --locked --git https://github.com/notzenco/metascrub
```

Or from a checkout:

```sh
git clone https://github.com/notzenco/metascrub
cd metascrub
cargo install --locked --path .
```

## Usage

```sh
metascrub inspect photo.jpg report.pdf    # show metadata, flag sensitive fields
metascrub inspect --sensitive ~/Desktop   # only sensitive fields, recursively

metascrub clean photo.jpg                 # writes photo.clean.jpg
metascrub clean -o shared/ trip/          # cleans a folder into shared/trip/...
metascrub clean -n memo.docx              # dry run: report only
metascrub clean -v photo.jpg              # list every removed field and value
metascrub clean --in-place photo.jpg      # replace the original
```

| Option | Meaning |
| --- | --- |
| `-o, --out-dir DIR` | Write cleaned files into `DIR`, mirroring folder arguments' layout |
| `--in-place` | Replace originals (atomically, through a temporary file) |
| `-n, --dry-run` | Report what would be removed without writing anything |
| `-f, --force` | Overwrite existing output files |
| `-v, --verbose` | List every removed field with its value |
| `-s, --sensitive` | (`inspect`) Show only privacy-sensitive fields |
| `--json` | Print a JSON report instead of text (see below) |
| `--color auto\|always\|never` | Colour output; `NO_COLOR` is respected |

Folders are searched recursively. Hidden files and symbolic links inside
folders are skipped, as are files metascrub cannot process. Without `-o`,
earlier `*.clean.*` outputs are skipped too. A file named explicitly that
cannot be processed is an error, and the exit status is 1 if any file
failed.

Metadata values and file names are printed with control characters escaped,
so a crafted file cannot inject terminal escape sequences.

### JSON output

`--json` prints one JSON array with a record per file. Per-file errors go
into the report rather than to stderr, and the exit status is the same as
for text output.

```console
$ metascrub clean --json -n photo.jpg
[
  {
    "input": "photo.jpg",
    "status": "would_clean",
    "output": "photo.clean.jpg",
    "format": "JPEG",
    "removed": ["EXIF (388 B)", "XMP (505 B)", "Photoshop/IPTC (48 B)", "comment (18 B)", "data after end of image (13 B)"],
    "kept": ["ICC colour profile", "orientation"],
    "removed_fields": [
      { "group": "EXIF", "name": "Make", "value": "MetascrubCam", "category": "device" },
      ...
    ],
    "error": null
  }
]
```

| Command | Record fields |
| --- | --- |
| `inspect` | `path`, `format`, `fields` (`group`, `name`, `value`, `category`), `notes`, `error` |
| `clean` | `input`, `status` (`cleaned`, `would_clean`, `skipped`, `error`), `output`, `format`, `removed`, `kept`, `removed_fields`, `error` |

`category` is `location`, `identity`, `device`, `time`, `text` or `null` for
fields that are not privacy-sensitive.

## Example

```console
$ metascrub inspect photo.jpg
photo.jpg (JPEG)
  EXIF
  ! Make              MetascrubCam  [device]
  ! Model             MSX-1000  [device]
    Orientation       row 0 at right and column 0 at top
  ! Software          MSFirmware 9.9  [device]
  ! DateTime          2019:07:14 10:30:00  [time]
  ! Artist            Jane Example  [identity]
  ! DateTimeOriginal  2019:07:14 10:30:00  [time]
  ! BodySerialNumber  SN-0042-SECRET  [device]
  ! LensModel         MSX-1000 Lens  [device]
  GPS
  ! GPSLatitudeRef    N  [location]
  ! GPSLatitude       48 deg 51 min 29.88 sec N  [location]
  ! GPSLongitudeRef   E  [location]
  ! GPSLongitude      2 deg 17 min 40.2 sec E  [location]
  ! GPSPosition       48.858300, 2.294500  [location]
  XMP
  ! xmp:CreatorTool   MSFirmware 9.9  [device]
  ! xmp:CreateDate    2019-07-14T10:30:00Z  [time]
  ! dc:creator        Jane Example  [identity]
  IPTC
  ! By-line           Jane Example  [identity]
  Comment
  ! Comment           Secret comment  [text]
  Other
    13 B of data after the end of the image (often a second image or vendor metadata)
  19 fields, 18 privacy-sensitive

$ metascrub clean photo.jpg memo.docx
photo.jpg → photo.clean.jpg (JPEG)
  removed EXIF (388 B), XMP (505 B), Photoshop/IPTC (48 B), comment (18 B), data after end of image (13 B)
  kept    ICC colour profile, orientation
  18 privacy-sensitive fields removed: Make, Model, Software, DateTime, Artist, DateTimeOriginal, BodySerialNumber, LensModel and 10 more
memo.docx → memo.clean.docx (DOCX)
  removed embedded image metadata (936 B), core properties (725 B), extended properties (442 B), custom properties (377 B), comment and revision authors, dates and IDs (4 parts), ZIP entry timestamps
  41 privacy-sensitive fields removed: RevisionAuthor, RevisionDate, RevisionSessionIDs, CommentAuthor, CommentDate, CommentInitials, PersonName, PersonUserId and 31 more
cleaned 2 files

$ metascrub inspect photo.clean.jpg
photo.clean.jpg (JPEG)
  EXIF
    Orientation  row 0 at right and column 0 at top
  1 field, 0 privacy-sensitive
```

The files are the test fixtures in [`tests/fixtures`](tests/fixtures); the
GPS position is the Eiffel Tower.

## How it works

Each format has a small parser for its container, written to treat input
as hostile: every offset and length is bounds-checked, decompression is
capped at 256 MiB per part, and the test suite feeds every truncation of
every fixture plus hundreds of corrupted variants through `inspect` and
`clean` to check that nothing panics.

- **JPEG** is walked marker by marker. Kept segments and the entropy-coded
  scan data are copied verbatim; an orientation-only EXIF block is written
  back when the original had a non-default orientation.
- **PNG** keeps an allowlist of rendering chunks and recomputes the CRC only
  for the orientation `eXIf` chunk it writes.
- **WebP** drops metadata chunks, clears the VP8X feature flags for what was
  removed and rewrites the RIFF size.
- **HEIF** locates EXIF and XMP items through the `iinf` and `iloc` boxes and
  overwrites their bytes in place, so no offsets change.
- **PDF** uses [`lopdf`](https://crates.io/crates/lopdf) to load the current
  revision, removes metadata keys and annotation authors, cleans embedded
  JPEGs, prunes unreachable objects and writes a single-revision file.
- **Office** packages are rewritten entry by entry with fixed timestamps.
  In comment, revision and people parts only the tags that carry a name,
  date or ID are rewritten; all other markup is copied byte for byte.
  Names become `Author` rather than disappearing, so comments and tracked
  changes still open and can still be accepted or rejected.

EXIF is decoded with [`kamadak-exif`](https://crates.io/crates/kamadak-exif)
and XML with [`quick-xml`](https://crates.io/crates/quick-xml). Files are
read into memory, so very large files need matching RAM.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The fixtures carry known metadata planted by
[`tests/fixtures/generate.py`](tests/fixtures/generate.py) (Python standard
library plus macOS `sips` and `cwebp`). The tests check that every planted
value is reported by `inspect` and absent after `clean`, that image data is
byte-identical, and that cleaning twice changes nothing. On macOS they also
decode every cleaned image with `sips`. The Office fixtures are complete
documents that render in Quick Look before and after cleaning.

See [docs/macos-integration.md](docs/macos-integration.md) for the plan to
bring metascrub to the Finder and the share sheet.

## License

[MIT](LICENSE)
