use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anstyle::{AnsiColor, Style};
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use metascrub::{Category, Field};
use serde::Serialize;

const ABOUT: &str = "Inspect and strip identifying metadata from files before sharing them";

const LONG_ABOUT: &str = "\
Inspect and strip identifying metadata from files before sharing them.

Supported formats (detected by content, not extension): JPEG, PNG, WebP,
HEIF/HEIC/AVIF, PDF, and Office Open XML (docx, xlsx, pptx).";

#[derive(Parser)]
#[command(name = "metascrub", version, about = ABOUT, long_about = LONG_ABOUT)]
struct Cli {
    /// When to use colour in output
    #[arg(long, value_enum, default_value_t = ColorWhen::Auto, global = true)]
    color: ColorWhen,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the metadata in files, highlighting privacy-sensitive fields
    Inspect {
        /// Only show privacy-sensitive fields
        #[arg(short, long)]
        sensitive: bool,

        /// Print a JSON report instead of text
        #[arg(long)]
        json: bool,

        /// Files or directories to inspect (directories are searched recursively)
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },

    /// Write copies of files with identifying metadata removed
    ///
    /// By default `photo.jpg` is cleaned into `photo.clean.jpg` next to it.
    /// Originals are only replaced with --in-place.
    Clean {
        /// Write cleaned files into DIR instead of next to the originals
        #[arg(short, long, value_name = "DIR", conflicts_with = "in_place")]
        out_dir: Option<PathBuf>,

        /// Replace the original files instead of writing copies
        #[arg(long)]
        in_place: bool,

        /// Show what would be removed without writing anything
        #[arg(short = 'n', long)]
        dry_run: bool,

        /// Overwrite output files that already exist
        #[arg(short, long)]
        force: bool,

        /// List every removed field and its value
        #[arg(short, long)]
        verbose: bool,

        /// Print a JSON report instead of text; errors are reported in it
        #[arg(long)]
        json: bool,

        /// Files or directories to clean (directories are searched recursively)
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ColorWhen {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Copy,
    OutDir,
    InPlace,
}

/// A file to process. `relative` is set for files found inside a directory
/// argument and is used to mirror the directory layout under --out-dir.
struct Input {
    path: PathBuf,
    relative: Option<PathBuf>,
    explicit: bool,
}

const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();
const GROUP: Style = AnsiColor::Cyan.on_default().bold();
const OK: Style = AnsiColor::Green.on_default().bold();
const ERR: Style = AnsiColor::Red.on_default().bold();

fn main() -> ExitCode {
    let cli = Cli::parse();
    anstream::ColorChoice::write_global(match cli.color {
        ColorWhen::Auto => anstream::ColorChoice::Auto,
        ColorWhen::Always => anstream::ColorChoice::Always,
        ColorWhen::Never => anstream::ColorChoice::Never,
    });

    let ok = match cli.command {
        Command::Inspect {
            sensitive,
            json,
            paths,
        } => run_inspect(&paths, sensitive, json),
        Command::Clean {
            out_dir,
            in_place,
            dry_run,
            force,
            verbose,
            json,
            paths,
        } => {
            let mode = match (&out_dir, in_place) {
                (Some(_), _) => Mode::OutDir,
                (None, true) => Mode::InPlace,
                (None, false) => Mode::Copy,
            };
            let options = CleanOptions {
                mode,
                out_dir,
                dry_run,
                force,
                verbose,
                json,
            };
            run_clean(&paths, &options)
        }
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn error(path: &Path, err: &anyhow::Error) {
    anstream::eprintln!(
        "{ERR}error{ERR:#}: {}: {}",
        display_path(path),
        sanitize(&format!("{err:#}"), 400)
    );
}

fn run_inspect(paths: &[PathBuf], sensitive_only: bool, json: bool) -> bool {
    let mut ok = true;
    let mut first = true;
    let mut records = Vec::new();
    for input in collect_inputs(paths, false, &mut ok) {
        let result = fs::read(&input.path)
            .context("cannot read file")
            .and_then(|data| {
                if !input.explicit && metascrub::detect(&data).is_none() {
                    return Ok(None);
                }
                metascrub::inspect(&data).map(Some)
            });
        match result {
            Ok(None) => {}
            Ok(Some((format, inspection))) if json => {
                records.push(JsonInspection::new(
                    &input.path,
                    format,
                    &inspection,
                    sensitive_only,
                ));
            }
            Ok(Some((format, inspection))) => {
                if !first {
                    anstream::println!();
                }
                first = false;
                print_inspection(&input.path, format, &inspection, sensitive_only);
            }
            Err(e) => {
                ok = false;
                if json {
                    records.push(JsonInspection::error(&input.path, &e));
                } else {
                    error(&input.path, &e);
                }
            }
        }
    }
    if json {
        print_json(&records);
    }
    ok
}

fn print_inspection(
    path: &Path,
    format: metascrub::Format,
    inspection: &metascrub::Inspection,
    sensitive_only: bool,
) {
    let mut out = anstream::stdout().lock();
    let _ = writeln!(
        out,
        "{BOLD}{}{BOLD:#} {DIM}({format}){DIM:#}",
        display_path(path)
    );
    let fields: Vec<&Field> = inspection
        .fields
        .iter()
        .filter(|f| !sensitive_only || f.is_sensitive())
        .collect();
    if fields.is_empty() && inspection.notes.is_empty() {
        let _ = writeln!(out, "  {OK}no metadata found{OK:#}");
        return;
    }

    let width = fields
        .iter()
        .map(|f| sanitize(&f.name, 40).chars().count())
        .max()
        .unwrap_or(0);
    let mut group: Option<&str> = None;
    for field in &fields {
        if group != Some(field.group.as_str()) {
            let _ = writeln!(out, "  {GROUP}{}{GROUP:#}", sanitize(&field.group, 120));
            group = Some(field.group.as_str());
        }
        let name = sanitize(&field.name, 40);
        let value = sanitize(&field.value, 80);
        match field.category {
            Some(category) => {
                let style = category_style(category);
                let _ = writeln!(
                    out,
                    "  {style}!{style:#} {style}{name:<width$}{style:#}  {value}  {style}[{category}]{style:#}"
                );
            }
            None => {
                let _ = writeln!(out, "    {name:<width$}  {DIM}{value}{DIM:#}");
            }
        }
    }
    if !inspection.notes.is_empty() {
        let _ = writeln!(out, "  {GROUP}Other{GROUP:#}");
        for note in &inspection.notes {
            let _ = writeln!(out, "    {}", sanitize(note, 200));
        }
    }
    let sensitive = inspection
        .fields
        .iter()
        .filter(|f| f.is_sensitive())
        .count();
    let _ = writeln!(
        out,
        "  {DIM}{} field{}, {sensitive} privacy-sensitive{DIM:#}",
        inspection.fields.len(),
        plural(inspection.fields.len())
    );
}

fn category_style(category: Category) -> Style {
    match category {
        Category::Location | Category::Identity => AnsiColor::Red.on_default().bold(),
        Category::Device | Category::Time | Category::Text => AnsiColor::Yellow.on_default(),
    }
}

struct CleanOptions {
    mode: Mode,
    out_dir: Option<PathBuf>,
    dry_run: bool,
    force: bool,
    verbose: bool,
    json: bool,
}

fn run_clean(paths: &[PathBuf], options: &CleanOptions) -> bool {
    let mut ok = true;
    let inputs = collect_inputs(paths, options.mode == Mode::Copy, &mut ok);
    let mut planned = HashSet::new();
    let (mut cleaned, mut failed, mut skipped) = (0usize, 0usize, 0usize);
    let mut records = Vec::new();

    for input in &inputs {
        match clean_one(input, options, &mut planned) {
            Ok(Some((target, report))) => {
                cleaned += 1;
                if options.json {
                    records.push(JsonClean::cleaned(input, &target, &report, options.dry_run));
                } else {
                    print_clean(input, &target, &report, options);
                }
            }
            Ok(None) => {
                skipped += 1;
                if options.json {
                    records.push(JsonClean::skipped(input));
                }
            }
            Err(e) => {
                failed += 1;
                if options.json {
                    records.push(JsonClean::error(input, &e));
                } else {
                    error(&input.path, &e);
                }
            }
        }
    }
    if options.json {
        print_json(&records);
    } else if inputs.len() > 1 || skipped > 0 {
        let verb = if options.dry_run {
            "would clean"
        } else {
            "cleaned"
        };
        let mut summary = format!("{verb} {cleaned} file{}", plural(cleaned));
        if failed > 0 {
            summary += &format!(", {failed} failed");
        }
        if skipped > 0 {
            summary += &format!(", skipped {skipped} unsupported");
        }
        anstream::println!("{BOLD}{summary}{BOLD:#}");
    }
    ok && failed == 0
}

/// Clean one input and return where it was (or would be) written. Returns
/// `Ok(None)` when a file found during a directory walk is skipped because
/// its format is unsupported.
fn clean_one(
    input: &Input,
    options: &CleanOptions,
    planned: &mut HashSet<PathBuf>,
) -> Result<Option<(PathBuf, metascrub::CleanReport)>> {
    let data = fs::read(&input.path).context("cannot read file")?;
    if !input.explicit && metascrub::detect(&data).is_none() {
        return Ok(None);
    }
    let report = metascrub::clean(&data)?;
    let target = output_path(input, options)?;

    if options.mode != Mode::InPlace {
        if same_file(&input.path, &target) {
            bail!("output would overwrite the original; use --in-place to do that deliberately");
        }
        if target.exists() && !options.force {
            bail!(
                "{} already exists (use --force to overwrite)",
                display_path(&target)
            );
        }
    }
    if !planned.insert(target.clone()) {
        bail!("another input is also written to {}", display_path(&target));
    }

    if !options.dry_run {
        write_atomically(&input.path, &target, &report.data)?;
    }
    Ok(Some((target, report)))
}

fn print_clean(
    input: &Input,
    target: &Path,
    report: &metascrub::CleanReport,
    options: &CleanOptions,
) {
    let mut out = anstream::stdout().lock();
    let arrow = match (options.dry_run, options.mode) {
        (true, Mode::InPlace) => "would clean in place".to_string(),
        (true, _) => format!("would write {}", display_path(target)),
        (false, Mode::InPlace) => "cleaned in place".to_string(),
        (false, _) => format!("→ {}", display_path(target)),
    };
    let _ = writeln!(
        out,
        "{BOLD}{}{BOLD:#} {arrow} {DIM}({}){DIM:#}",
        display_path(&input.path),
        report.format
    );
    if report.removed.is_empty() {
        let _ = writeln!(out, "  {OK}nothing to remove{OK:#}");
    } else {
        let _ = writeln!(
            out,
            "  {BOLD}removed{BOLD:#} {}",
            sanitize(&report.removed.join(", "), 1000)
        );
    }
    if !report.kept.is_empty() {
        let _ = writeln!(
            out,
            "  {DIM}kept    {}{DIM:#}",
            sanitize(&report.kept.join(", "), 1000)
        );
    }

    let sensitive: Vec<&Field> = report
        .removed_fields
        .iter()
        .filter(|f| f.is_sensitive())
        .collect();
    if options.verbose {
        for field in &report.removed_fields {
            let marker = match field.category {
                Some(c) => {
                    let style = category_style(c);
                    format!("{style}!{style:#}")
                }
                None => " ".to_string(),
            };
            let _ = writeln!(
                out,
                "  {marker} {DIM}{}{DIM:#} {}: {}",
                sanitize(&field.group, 120),
                sanitize(&field.name, 40),
                sanitize(&field.value, 80)
            );
        }
    } else if !sensitive.is_empty() {
        let mut names: Vec<String> = Vec::new();
        for field in &sensitive {
            let name = sanitize(&field.name, 40);
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let shown = names.len().min(8);
        let mut list = names[..shown].join(", ");
        if names.len() > shown {
            list += &format!(" and {} more", names.len() - shown);
        }
        let _ = writeln!(
            out,
            "  {} privacy-sensitive field{} removed: {list}",
            sensitive.len(),
            plural(sensitive.len())
        );
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn output_path(input: &Input, options: &CleanOptions) -> Result<PathBuf> {
    let file_name = input
        .path
        .file_name()
        .context("input path has no file name")?;
    Ok(match options.mode {
        Mode::InPlace => input.path.clone(),
        Mode::OutDir => {
            let dir = options
                .out_dir
                .as_deref()
                .expect("out_dir set in OutDir mode");
            dir.join(input.relative.as_deref().unwrap_or(Path::new(file_name)))
        }
        Mode::Copy => input.path.with_file_name(clean_name(Path::new(file_name))),
    })
}

/// `photo.jpg` → `photo.clean.jpg`; `README` → `README.clean`.
fn clean_name(name: &Path) -> PathBuf {
    let stem = name.file_stem().unwrap_or(name.as_os_str()).to_os_string();
    let mut out = stem;
    out.push(".clean");
    if let Some(ext) = name.extension() {
        out.push(".");
        out.push(ext);
    }
    PathBuf::from(out)
}

fn is_clean_output(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.ends_with(".clean"))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Write `data` to a temporary file beside `target`, then rename it into
/// place so an interrupted run never leaves a half-written file. The new file
/// takes the original's permissions but none of its extended attributes.
fn write_atomically(original: &Path, target: &Path, data: &[u8]) -> Result<()> {
    let dir = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let name = target.file_name().context("output path has no file name")?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".metascrub-{}.tmp", std::process::id()));
    let tmp = dir.join(tmp_name);

    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .with_context(|| format!("cannot create {}", tmp.display()))?;
        file.write_all(data)?;
        file.sync_all()?;
        if let Ok(meta) = fs::metadata(original) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, target).with_context(|| format!("cannot write {}", target.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Expand directory arguments into the files inside them. Hidden entries and
/// symbolic links inside directories are skipped; explicit arguments are not.
fn collect_inputs(paths: &[PathBuf], skip_clean_outputs: bool, ok: &mut bool) -> Vec<Input> {
    let mut inputs = Vec::new();
    for path in paths {
        if path.is_dir() {
            let base = path.file_name().map(PathBuf::from).unwrap_or_default();
            if let Err(e) = walk(path, &base, skip_clean_outputs, &mut inputs) {
                error(path, &e);
                *ok = false;
            }
        } else {
            inputs.push(Input {
                path: path.clone(),
                relative: None,
                explicit: true,
            });
        }
    }
    inputs
}

fn walk(
    dir: &Path,
    relative: &Path,
    skip_clean_outputs: bool,
    inputs: &mut Vec<Input>,
) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .with_context(|| "cannot read directory")?
        .collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            walk(&path, &relative.join(&name), skip_clean_outputs, inputs)?;
        } else if file_type.is_file() && !(skip_clean_outputs && is_clean_output(&path)) {
            inputs.push(Input {
                path,
                relative: Some(relative.join(&name)),
                explicit: false,
            });
        }
    }
    Ok(())
}

fn display_path(path: &Path) -> String {
    sanitize(&path.display().to_string(), 300)
}

/// Make untrusted text safe to print: metadata values and file names can
/// contain terminal escape sequences or bidi overrides that would otherwise
/// rewrite the screen or disguise the output.
fn sanitize(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (count, c) in text.chars().enumerate() {
        if count >= max_chars {
            out.push('…');
            break;
        }
        let bidi = matches!(
            c,
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}'
        );
        if c.is_control() || bidi {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// Machine-readable output. Values are emitted verbatim (JSON escaping
/// makes them safe), and paths are as given on the command line.
#[derive(Serialize)]
struct JsonField {
    group: String,
    name: String,
    value: String,
    category: Option<&'static str>,
}

impl From<&Field> for JsonField {
    fn from(field: &Field) -> Self {
        JsonField {
            group: field.group.clone(),
            name: field.name.clone(),
            value: field.value.clone(),
            category: field.category.map(Category::label),
        }
    }
}

#[derive(Serialize)]
struct JsonInspection {
    path: String,
    format: Option<String>,
    fields: Vec<JsonField>,
    notes: Vec<String>,
    error: Option<String>,
}

impl JsonInspection {
    fn new(
        path: &Path,
        format: metascrub::Format,
        inspection: &metascrub::Inspection,
        sensitive_only: bool,
    ) -> Self {
        JsonInspection {
            path: path.display().to_string(),
            format: Some(format.to_string()),
            fields: inspection
                .fields
                .iter()
                .filter(|f| !sensitive_only || f.is_sensitive())
                .map(JsonField::from)
                .collect(),
            notes: inspection.notes.clone(),
            error: None,
        }
    }

    fn error(path: &Path, err: &anyhow::Error) -> Self {
        JsonInspection {
            path: path.display().to_string(),
            format: None,
            fields: Vec::new(),
            notes: Vec::new(),
            error: Some(format!("{err:#}")),
        }
    }
}

#[derive(Serialize)]
struct JsonClean {
    input: String,
    /// `cleaned`, `would_clean` (dry run), `skipped` (unsupported file
    /// found in a directory) or `error`.
    status: &'static str,
    output: Option<String>,
    format: Option<String>,
    removed: Vec<String>,
    kept: Vec<String>,
    removed_fields: Vec<JsonField>,
    error: Option<String>,
}

impl JsonClean {
    fn empty(input: &Input, status: &'static str) -> Self {
        JsonClean {
            input: input.path.display().to_string(),
            status,
            output: None,
            format: None,
            removed: Vec::new(),
            kept: Vec::new(),
            removed_fields: Vec::new(),
            error: None,
        }
    }

    fn cleaned(
        input: &Input,
        target: &Path,
        report: &metascrub::CleanReport,
        dry_run: bool,
    ) -> Self {
        JsonClean {
            output: Some(target.display().to_string()),
            format: Some(report.format.to_string()),
            removed: report.removed.clone(),
            kept: report.kept.clone(),
            removed_fields: report.removed_fields.iter().map(JsonField::from).collect(),
            ..JsonClean::empty(input, if dry_run { "would_clean" } else { "cleaned" })
        }
    }

    fn skipped(input: &Input) -> Self {
        JsonClean::empty(input, "skipped")
    }

    fn error(input: &Input, err: &anyhow::Error) -> Self {
        JsonClean {
            error: Some(format!("{err:#}")),
            ..JsonClean::empty(input, "error")
        }
    }
}

fn print_json<T: Serialize>(records: &[T]) {
    let json = serde_json::to_string_pretty(records).expect("report serialises");
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{json}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_clean_copies() {
        assert_eq!(
            clean_name(Path::new("photo.jpg")),
            PathBuf::from("photo.clean.jpg")
        );
        assert_eq!(
            clean_name(Path::new("a.b.pdf")),
            PathBuf::from("a.b.clean.pdf")
        );
        assert_eq!(
            clean_name(Path::new("README")),
            PathBuf::from("README.clean")
        );
        assert!(is_clean_output(Path::new("dir/photo.clean.jpg")));
        assert!(!is_clean_output(Path::new("dir/photo.jpg")));
    }

    #[test]
    fn escapes_terminal_control_sequences() {
        assert_eq!(sanitize("a\x1b[31mb", 80), "a\\u{1b}[31mb");
        assert_eq!(sanitize("x\u{202E}y", 80), "x\\u{202e}y");
        assert_eq!(sanitize("abcdef", 3), "abc…");
    }
}
