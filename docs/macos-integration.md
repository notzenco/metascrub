# Plan: metascrub in the Finder and the share sheet

Status: proposal. Nothing here is implemented yet.

## Goal

Let someone on a Mac clean a file without opening a terminal:

1. **Quick Action**: select files in the Finder, choose *Quick Actions →
   Remove Metadata*, and get `photo.clean.jpg` next to each original.
2. **Share extension**: in any app's share sheet, choose *Share Without
   Metadata*. The extension cleans a copy and then offers the usual share
   targets (Messages, Mail, AirDrop) for the clean copy.

Both wrap the existing `metascrub` CLI rather than reimplementing it, so
format support, the lossless guarantees and the "refuse if anything
sensitive survives" check stay in one tested place.

## Phase 1: Quick Action through Shortcuts (no app)

The quickest way to get value and test the workflow.

- A Shortcut with *Receive Files from Quick Actions* (Finder), then *Run Shell
  Script* with input passed as arguments:
  `metascrub clean --color=never "$@"`.
- The shortcut shows the CLI's summary in a notification, or an alert when
  the exit status is non-zero.
- Requirements: `metascrub` installed on `PATH`, for example with
  `cargo install`. Shortcuts runs shell scripts with a minimal `PATH`, so the
  script calls the binary by absolute path (`~/.cargo/bin/metascrub`).
- Ship it as a signed `.shortcut` file in the repository with a short setup
  section in the README.

Limitations: it needs the CLI installed separately, the first run asks for
permission to run scripts, and there is no share-sheet entry. Phase 2
removes all three.

## Phase 2: a native app with extensions

A small SwiftUI app, **Metascrub.app**, that exists mainly to carry two
extensions and the CLI binary.

### Bundle layout

```text
Metascrub.app/
  Contents/
    MacOS/Metascrub                   SwiftUI host app (drop zone, settings, about)
    Helpers/metascrub                 the Rust CLI, universal (arm64 + x86_64)
    PlugIns/
      CleanAction.appex               Finder Quick Action (Action extension)
      ShareWithoutMetadata.appex      Share extension
```

- Build the CLI for `aarch64-apple-darwin` and `x86_64-apple-darwin` and
  merge them with `lipo`, from an Xcode *Run Script* build phase that calls
  `cargo build --release --locked`.
- Sign the helper with the app's team ID and the entitlements
  `com.apple.security.app-sandbox` and `com.apple.security.inherit`, so it
  runs inside the sandbox of whichever process launches it. A sandboxed app
  may only launch executables inside its own bundle, which is why the helper
  is bundled rather than found on `PATH`.

### Calling the CLI

A Swift `MetascrubRunner` wraps `Process`:

- arguments: `clean --color=never --json --out-dir <container temp dir> <inputs>`;
- reads stdout and stderr fully, enforces a timeout (for example 60 s per
  batch) and maps exit status 1 to per-file errors;
- never passes file names through a shell, so names need no quoting.

This needs one CLI addition first, tracked separately: **`--json`**, a
machine-readable report per file (`input`, `output`, `format`, `removed`,
`kept`, `removed_fields`, `error`). The extensions show this in their UI
instead of parsing human-oriented text. It should come with its own tests
in this repository, and the human output stays the default.

### Quick Action (Action extension)

- `NSExtensionPointIdentifier`: `com.apple.ui-services`, with
  `NSExtensionServiceAllowsFinderPreviewItem` and
  `NSExtensionServiceAllowsTouchBarItem` as appropriate, and an activation
  rule limited to the supported content types: `public.jpeg`, `public.png`,
  `org.webmproject.webp`, `public.heic`, `public.heif`, `public.avif`,
  `com.adobe.pdf` and the three Office Open XML types.
- Input arrives as `NSItemProvider`s. Load each as a file URL; the system
  grants access to the selected files through security-scoped URLs.
- Default behaviour mirrors the CLI: write `name.clean.ext` next to the
  original. Writing beside the original from a sandboxed extension needs
  user consent, so the first run presents an `NSSavePanel`/`NSOpenPanel`
  for the destination folder and stores a security-scoped bookmark. A
  setting offers "Ask every time" or "Save to Downloads".
- The extension shows a compact sheet: one row per file with the removed
  categories (location, identity, device, time) and any error.

### Share extension

- `NSExtensionPointIdentifier`: `com.apple.share-services`, with the same
  activation rule.
- Clean the shared items into the extension's temporary directory, show the
  summary, then present `NSSharingServicePicker` with the cleaned files so
  the user continues to Messages, Mail or AirDrop with the clean copies.
- Remove the temporary files once the sharing service finishes or the sheet
  is dismissed.

### Host app

- A drop zone that runs the same flow for dragged files.
- Settings: destination for Quick Action output, whether to reveal output in
  the Finder, and a link to the README section on what is not removed.
- First-launch screen explaining how to enable the extensions in *System
  Settings → Login Items & Extensions*.

## Privacy and security requirements

- No network entitlement for the app, the extensions or the helper. Nothing
  leaves the Mac.
- No logging of file names or metadata values. Errors shown in the UI use
  the CLI's escaped output.
- Never modify originals from the extensions. `--in-place` is not exposed in
  the UI.
- The extension refuses to share when the CLI reports an error for a file,
  rather than falling back to sharing the original.

## Distribution

- Developer ID signing with the hardened runtime, notarised with
  `notarytool` and stapled, distributed as a DMG from GitHub releases.
- The Mac App Store is possible because the design already fits the
  sandbox; consider it after the Developer ID build has been in use.
- A Homebrew cask for the app, and a formula for the CLI on its own.

## Testing

- Unit tests for `MetascrubRunner` against a stub executable that emits
  canned JSON and exit codes.
- UI tests for the host app's drop zone.
- A manual checklist per release: Quick Action and Share on each supported
  format, a mixed selection with an unsupported file, a read-only folder, a
  file in iCloud Drive that is not downloaded yet, and a 200 MB PDF to check
  extension memory use.
- Use the repository's existing fixtures as the test files.

## Milestones

1. CLI: add `--json` with tests. Publish a release with universal macOS binaries.
2. Phase 1 Shortcut, documented in the README.
3. Host app with the bundled, sandboxed helper and the drop zone.
4. Quick Action extension.
5. Share extension.
6. Signing, notarisation, DMG and Homebrew cask.

## Open questions

- Is the Quick Action's default output "beside the original" (needs folder
  consent) or "Downloads" (no prompt, but less discoverable)?
- Should the share extension remember a per-app choice, or always show the
  summary before handing off?
- Would linking the Rust code as a library through a C ABI (or UniFFI) be
  worth it later? It would avoid process launches and allow progress
  reporting, at the cost of a second integration surface. The CLI wrapper is
  the simpler start.
