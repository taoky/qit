# Validation record — 2026-10-16

## Passed locally

- Workspace debug/release builds, Rust formatting and Clippy with warnings denied.
- System-language i18n: 78 embedded English/Simplified Chinese messages; locale
  fallback, catalog key/placeholder consistency and substitution tests passed.
  The GTK fixture workflow passed in both English and Simplified Chinese; Chinese
  screenshots inspected. No in-app language selector or runtime catalog fetch.
- Install-page redesign: friendly version ordering, component grouping, common
  repository matching and folder validation tests. GTK workflow exercised in a
  private D-Bus session and Xvfb against a local HTTP fixture: automatic loading,
  cleared selections after version changes, rapid switching, common components,
  tool category and inline retry. Full-window screenshots inspected.
- Repository parsing against real Qt 6.8.1 Linux, Windows, macOS, Android and
  WebAssembly metadata, plus CMake tool metadata.
- Apache relative-link and USTC/yadex absolute-link directory parsing.
- Per-archive extraction destinations, required host Qt selection, optional
  recommendations and explicitly selected documentation dependencies.
- libarchive extraction with Unicode/space-containing directories, conflicting
  files, traversal, escaping links and pre-existing symlink parents.
- Cancellation, file digests, final-path configuration, preservation of modified
  and additional files during uninstall, journal-based recovery.
- Real Linux Qt 6.8.1 QtBase + ICU extraction/configuration, `qmake -query`, and a
  CMake-built C++ executable linked to QtCore. The QtBase archive matched the
  upstream SHA-1. Archives were obtained with curl: this validates Qit's extraction
  and configuration, **not its HTTP download implementation**.
- Real Qt 6.8.1 WebAssembly QtBase wrapper relocation: the installed wrapper
  invokes the matching host qmake and reports the cross SDK's prefix. This also
  runs with spaces in the installation path. It does not compile a WASM program.
- CMake license metadata archive checksum checked against its Updates.xml entry.
- MirrorZ CERNET configuration and published metadata parsed into 10 unique Qt
  entries, with 教育网联合镜像站 first. Checked deduplication, relative/absolute
  URL handling, USTC’s `qtproject` path and rejection of empty Qt snapshots.
  Nine configured sites lack published metadata; recorded in `missing_sites`.
- Linux x86_64 AppImage generation, with native library/notice collection. This
  local package uses the current development system's ABI, not the Ubuntu 24.04
  CI baseline.
- The final AppImage's `--appimage-extract-and-run --help` exits successfully;
  the bundled CLI runs against bundled libraries and returns all 10 mirrors in the updated snapshot.
  These verify the executable entry points, not interactive GUI behavior.

## Environment-blocked checks

`python3 tests/protocol.py` was attempted. Creating the local HTTP server fails
with `PermissionError: [Errno 1] Operation not permitted` when creating its socket.
The suite is provided and wired into all six CI jobs, but has not passed locally.

The original sandboxed GUI smoke could not bind its private D-Bus socket. The
redesign's GTK test subsequently passed with approved execution outside that
socket restriction, using `python3 scripts/run-test-session.py` and an explicit X11
virtual display. This validates the fixture-driven UI flow, not desktop portals,
accessibility, or a complete real-network GUI installation.

A real CLI install from USTC was attempted; DNS resolution from the application
failed in this environment. The complete application-managed network installation
has therefore not been validated here, despite successful separate curl fetches.

## Still required before a stable release

| Target | Build configuration | Native runtime / clean-system package validation |
| --- | --- | --- |
| Linux x86_64 | Local release build and AppImage produced | GTK fixture flow passed; HTTP/clean-system checks pending |
| Linux ARM64 | Ubuntu ARM64 CI job provided | Pending |
| Windows x86_64 | MSYS2 UCRT64 CI job provided | Pending |
| Windows ARM64 | MSYS2 CLANGARM64 CI job provided | Pending |
| macOS Intel | Native macOS CI job provided | Pending |
| macOS Apple Silicon | Native macOS CI job provided | Pending |

The workflow has not been run remotely. It uploads artifacts without publishing
a release. Qt 5.15, Creator and toolchain execution, Android builds, WASM builds,
and Qt 6 releases beyond the fixture/runtime cases above still require validation.
Windows symbolic-link archives require an environment that permits creating
symbolic links (for example, Developer Mode); failures are reported rather than
silently replacing links with incorrect files.

## Repeat the real archive test

Supply the Qt 6.8.1 Linux QtBase and ICU archives and optionally its shared
WebAssembly QtBase archive:

```sh
QIT_QTBASE_ARCHIVE=/path/to/qtbase.7z \
QIT_ICU_ARCHIVE=/path/to/icu.7z \
QIT_WASM_ARCHIVE=/path/to/wasm-qtbase.7z \
cargo test -p qit-core --test filesystem real_qtbase_archive -- --ignored --nocapture
```

The test uses temporary directories, verifies the configured prefix, compiles
and runs the QtCore sample, and removes its temporary installation afterward.

## Optional recommendations

Added real Qt 6.8.1 shared metadata, Qt Creator and generic-tools metadata fixtures.
Dependency-plan coverage checks opt-in recommendations and excludes unrelated
modules. A local HTTP integration test checks Creator and generic-tools metadata
hydration from the selected mirror:

```sh
cargo test --locked -p qit-core recommendations_fetch_creator_from_selected_mirror -- --ignored
```

The GTK smoke test also checks the translated recommendation checkbox and its
default-off state. This does not validate a complete Qt Creator installation.

## Example archive overlays

Installation extraction now overlays regular files in its private staging
folder, in archive-path lexical order, logging replacement paths and source
archives. Metadata extraction retains strict collision checks. Replacement
unlinks the previous entry first to avoid modifying a hard-link target.

Workspace tests and Clippy pass. Regression tests cover shorter replacements,
file/directory conflicts, traversal, symlinks and hard-link targets. The five
cached Qt 6.12.0 qtbase, qtdeclarative, qtdoc, qtsvg and qttools example archives
were extracted through Qit's overlay implementation: all passed, with eight
regular-file replacements. The final top-level CMakeLists.txt matched the last
archive. This verifies real example extraction, not a complete SDK installation.

To repeat with already downloaded archives, set `QIT_EXAMPLE_ARCHIVES` to an
ordered platform-path-separator-delimited list of archive paths and run:

```sh
cargo test --locked -p qit-core real_example_archives_overlay_successfully -- --ignored --nocapture
```

## Adding components to an existing installation

The GUI recognizes Qit-owned destinations and reviews additions separately from
installed packages. Existing package versions remain pinned. The CLI exposes the
same operation with `install --add-components`. A combined SDK is built in staging
from cached archives, with original archive checksum consistency checks. User
edits, untracked files (including symlinks), empty directories and deletions are
preserved. Conflicting new content aborts before the swap. The ownership manifest
retains vendor baselines so later uninstall still preserves user edits.

`tests/extend.py` uses a temporary local HTTP repository to test first installation,
component addition, cache reuse, duplicate requests, user-file conflicts, corrupt
archives, recovery at four directory-swap phases, and uninstall after addition.
It runs in the Unix CI jobs. The GTK fixture test clicks Next for a managed existing
destination and verifies the addition review, installed/new labels and license
gate in English and Chinese; unmanaged existing directories remain blocked.
Workspace tests and Clippy also pass. This is fixture-based lifecycle validation,
not an extension of the user's real SDK. Close Qt tools before extending and allow
space for a second SDK copy.

Installed component selection is restored from the current destination's manifest
when components load, the destination changes, or the installation record changes.
Installed rows are checked, labeled and locked because individual removal is not
supported. Only new selections contribute to the size summary. The bilingual GTK
workflow verifies manifest recognition, destination changes, catalog reloads and
adding another component with the installed selection retained.

## Windows compiler dependencies

Required `qt.tools.win64_llvm_mingw<version>` and `qt.tools.win64_mingw<version>`
packages resolve to version-specific repositories on the selected host, even
with optional recommendations disabled. Real LLVM-MinGW 22.1.7 and MinGW 13.1.0
metadata fixtures cover accepted adapters and extraction paths. The local HTTP
test verifies that a Windows package requests Windows tool metadata on a Linux
runner, and does not pull optional Qt Creator when recommendations are disabled.

Non-native host selections use a distinct default directory suffix. Extending a
managed installation with packages for a different host is rejected. Unit tests
cover naming and the cross-host merge check. No Windows SDK/toolchain runtime
installation was performed for this change.
