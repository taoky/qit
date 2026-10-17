# Qit

**A GTK installer for Qt.** Built with Rust, GTK4 and libadwaita.

Qit identifies every download. Mirror operators should not need a
special exception for a Qt installer pretending to be `Mozilla/5.0`.
The background is [QTBUG-129057](https://qt-project.atlassian.net/browse/QTBUG-129057).

This is an independent project, not an official Qt product.

## Current state

This is a development preview. The Rust core, GTK interface, CLI, bundled mirror
list and six-platform build workflow are implemented. Local Linux tests verify
real Qt repository metadata, safe extraction, relocation, uninstall preservation
and crash recovery. A real Qt 6.8.1 QtBase + ICU extraction has passed `qmake
-query` and a CMake build/run linked against QtCore.

**This is not yet a validated six-platform release.** The current execution
sandbox restricts listener sockets; the local HTTP integration suite and isolated
GTK smoke test passed with the required permissions. Windows/macOS runtime,
Android/WASM compilation and clean-system distribution tests still need
their respective runners. See [VALIDATION.md](VALIDATION.md) for the exact boundary.

## Build and run

### Publishing a release

Set `[workspace.package].version` in `Cargo.toml`, update `Cargo.lock` with
`cargo check --workspace`, and commit the version change together with the
release workflow. Push a matching tag to build and publish:

```sh
git tag -a v0.1.0 -m "Qit 0.1.0"
git push origin v0.1.0
```

Tag pushes matching `v*` publish a GitHub Release only after all six native
builds succeed: Linux AppImage, macOS DMG and Windows ZIP, each for x86_64 and
ARM64. The release includes `SHA256SUMS` and automatically generated release
notes. Tags must match the Cargo version exactly, prefixed with `v`;
versions such as `v0.2.0-rc.1` are marked as prereleases. Ordinary branch pushes,
pull requests and manual workflow runs build artifacts without publishing.

The workflow uses the built-in `GITHUB_TOKEN`; no additional token is needed.
New releases remain drafts until all assets have uploaded. Rerunning the tag
workflow reuses the release and replaces assets with matching names.

### Local builds

Use Rust 1.97.1 or newer, GTK >= 4.14, libadwaita >= 1.5, libarchive >= 3.6 and
pkg-config, plus GLib's `glib-compile-resources` to embed the application icon.
Runtime networking and archive extraction do not require Python,
aqtinstall or a preinstalled Qt.

Ubuntu 24.04 build dependencies:

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libarchive-dev
cargo build --locked --release --workspace
./target/release/qit
```

On macOS, install `gtk4 libadwaita libarchive pkg-config` through Homebrew and
include `$(brew --prefix libarchive)/lib/pkgconfig` in `PKG_CONFIG_PATH`.
On Windows use MSYS2 UCRT64 (x86_64) or CLANGARM64 (ARM64), with that environment's
Rust, GTK4, libadwaita, libarchive and pkgconf packages. Exact build commands are
in [.github/workflows/build.yml](.github/workflows/build.yml).

The GUI defaults to 教育网联合镜像站. To use another mirror, change it in
**Settings** and save it, then use **Install Qt**:

1. Choose Desktop Qt, Android, WebAssembly, or Qt Creator & tools. Versions and
   components load automatically; the host system is detected and can be changed
   under **Advanced**. Stable Qt versions appear before previews, newest first.
   A link to [endoflife.date](https://endoflife.date/qt) beside the version selector
   provides support-lifecycle information; Qit does not infer EOL from repository
   availability. Qt 5.15 remains available where the selected mirror carries it.
2. Choose a version and select a development kit and optional modules. Components
   are grouped; internal IDs are available in tooltips and search. Source packages
   with matching versions and line endings are discovered automatically. Changing
   version, host, target or mirror clears the previous selection.
3. Check the generated folder name and full installation path. **Next** is enabled
   after selecting a component and a new, valid folder. The page estimates the
   selected download size; dependency-inclusive totals and licenses are shown in
   the installation review before installing. Loading failures offer **Retry**.
4. Use **Installed** to open an installation, copy environment instructions or
   uninstall it. Modified and extra files are preserved.

Qt repository IDs and filenames are discovered at
runtime, not hardcoded to one release. Newer nested layouts and both Apache and
absolute-link directory listings are supported.

The interface follows the system language preferences reported by GLib
(`LANGUAGE`, locale environment variables and platform defaults). English and
Simplified Chinese are bundled; unsupported languages fall back to English.
There is no in-app language selector. Restart Qit after changing the system
language. Qt component metadata, licenses and raw diagnostics retain their
original language. See [translation resources](data/i18n/README.md) to contribute.

## Installation behavior

- All HTTP requests, including metadata, license archives, checksums, redirects
  and resumed downloads, use the same product UA. No telemetry is sent.
- The bundled mirrors come from [MirrorZ](https://mirrors.cernet.edu.cn/list/qt),
  with 教育网联合镜像站 (`https://mirrors.cernet.edu.cn/qt/`) selected by default
  when no saved mirror exists. Official (qt.io) is the second option; CQU is excluded
  from the snapshot and refresh script. Download failures never silently switch to another
  mirror or the official origin. HTTP redirects issued by the selected server are followed and logged.
- Versions and components are discovered directly from the selected source's
  directory listings and Updates.xml files. Direct leaf XML and parent repository
  arguments are both supported. No official installer API or credentials are used.
- Mirror directories support HTML, Caddy/nginx JSON content negotiation and ZJU's
  `.mirror-index` without executing JavaScript. Redirected directory URLs resolve
  links, but only safe immediate child names map back onto the selected mirror.
- Four archives download concurrently, with bounded retries and cancellation.
  Resume requires a server validator and a valid Content-Range; otherwise the
  archive is restarted. SHA-256 is preferred, with SHA-1 accepted for historical
  repositories. These checks detect corruption, not repository authenticity.
- Each installation has its own directory and host Qt when needed. Selecting an
  existing Qit-managed directory adds components after review; arbitrary existing
  directories remain protected. Adding components keeps installed package versions,
  prepares a combined SDK using cached archives, and swaps directories with recovery
  support. This needs space for another SDK copy. User edits and extra files are
  preserved; conflicting additions stop before changing the existing installation.
  Qit does not modify global PATH or require administrator access.
- Explicit archive destinations are honored, including ICU's separate `lib`
  directory. Legacy archives retain their embedded directory tree. Qt config and
  build paths are rewritten to the final installation location.
- Qt 5.15/6, Creator, CMake, Ninja, MinGW and OpenSSL package families have adapters.
  Unknown package families or explicit installer operations fail before download.
  Qit does not execute IFW JavaScript. This is not a claim that every historical
  package in an admitted family has been runtime-tested.
- Creator, documentation and examples declared as SDK dependencies are treated
  as optional recommendations; select them explicitly to include them. Required
  SDK/module dependencies and auto-dependent architecture packages are resolved.
  The official installer `patcher` dependency is replaced by Qit's relocation.
- Qt 6 cross targets add a matching host Qt. JDK/NDK/Android SDK and Emscripten
  remain external prerequisites: Qit reports PATH/environment detection, but
  does not install them or guarantee their version compatibility. Creator's
  global kit registry is not modified; configure the installed kit in Creator.
- In-progress installations have a journal. The next install in the same root
  cleans abandoned Qit staging directories; `qit-cli recover --root DIR` performs
  this explicitly. Verified downloads remain cached. Untracked directories are
  never treated as Qit staging directories merely because of their names.

## CLI

Linux AppImages built from this revision support `--cli` as the first application
argument (the published 0.1.0 AppImages do not have this launcher option):

```sh
chmod +x Qit-*.AppImage
./Qit-<version>-linux-<arch>.AppImage --cli --help
./Qit-<version>-linux-<arch>.AppImage --cli --mirror https://mirrors.ustc.edu.cn/qtproject/ repositories
./Qit-<version>-linux-<arch>.AppImage --cli list
```

Replace the filename with the downloaded AppImage's actual name. No manual
extraction or library-path configuration is needed. Without `--cli`, the
AppImage starts the GUI. For the examples below, replace `qit-cli` with
`./Qit-<version>-linux-<arch>.AppImage --cli` when using an AppImage.
On systems without working FUSE, put `--appimage-extract-and-run` before `--cli`.

Commands emit JSON on stdout. Progress and errors go to stderr; `--json` also
makes progress events JSON. Ctrl-C requests cancellation.

```sh
qit-cli mirrors
qit-cli --mirror https://mirrors.ustc.edu.cn/qtproject/ repositories
qit-cli --mirror https://mirrors.ustc.edu.cn/qtproject/ packages \
  --repo online/qtsdkrepository/linux_x64/desktop/qt6_681/
qit-cli --mirror https://mirrors.ustc.edu.cn/qtproject/ plan \
  --repo online/qtsdkrepository/linux_x64/desktop/qt6_681/ \
  qt.qt6.681.linux_gcc_64
qit-cli --mirror https://mirrors.ustc.edu.cn/qtproject/ install \
  --repo online/qtsdkrepository/linux_x64/desktop/qt6_681/ \
  --root "$HOME/Qit" --name qt-6.8.1 --accept-licenses \
  qt.qt6.681.linux_gcc_64
qit-cli list
qit-cli uninstall INSTALLATION_ID --yes
```

Repeat `--repo` to include another repository. Use `plan` to inspect the expanded
selection and available license texts before accepting licenses. Without
`--mirror`, the CLI uses the mirror saved by the GUI. `QIT_DATA_DIR` overrides the
application's settings/cache/record directory for tests and portable use.

## Tests and packaging

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --workspace
python3 tests/protocol.py
QIT_DATA_DIR=/tmp/qit-smoke QIT_SMOKE_EXIT_SECONDS=3 \
  python3 scripts/run-test-session.py xvfb-run -a target/debug/qit
```

The HTTP tests run a real local server that accepts only Qit's UA, exercises
redirects, interrupted/resumed transfers, ignored Range, throttling, corruption,
unsafe archives, missing checksums, uninstall preservation and staging cleanup.

Run `python3 scripts/package.py` after a release build on each target platform.
Linux produces an AppDir tarball by default; supply `--appimagetool PATH` for an
AppImage. Windows requires `--prefix` pointing at its MSYS2 dependency prefix and
produces a portable ZIP. macOS produces an ad-hoc-signed `.app` in a DMG; it is not
Developer ID signed or notarized. Native libraries, dependency inventories and
available license notices are bundled. Build on the CI baseline rather than a
newer development distribution when targeting older systems.

For packaging without network access, also pass `--runtime-file PATH` with a
pre-downloaded AppImage type2 runtime of the matching architecture.

The workflow uploads build artifacts; it does not publish releases. Dependency
versions used by the runner are recorded in each bundle, while Cargo.lock pins
Rust dependencies. MSYS2/Homebrew system dependency updates still need review.

To update the mirror snapshot intentionally:

```sh
python3 scripts/update-mirrors.py
```

The generator reads MirrorZ’s CERNET configuration and published
`mirrorz-json-legacy` metadata snapshot, selects `qt` entries and deduplicates
URLs (including TUNA’s multiple servers). It preserves paths such as USTC’s
`qtproject`. Sites absent from the published snapshot are reported and recorded
in `missing_sites`; this is not a live availability check. CERNET’s first entry
is a redirect service that selects a downstream mirror. Existing saved choices
are preserved. Official qt.io stays second and CQU is deliberately excluded.

Review the resulting `data/mirrors.json` diff before committing. The app never
refreshes the list at startup. For offline regeneration, pass `--input` with the
MirrorZ metadata tar.gz and `--config` with the CERNET configuration JSON.

For the GTK catalog/selection workflow test on Linux (local fixture server only):

```sh
GDK_BACKEND=x11 GTK_A11Y=none python3 scripts/run-test-session.py \
  xvfb-run -a -s '-screen 0 1280x1024x24' python3 tests/ui-smoke.py
```

The test checks automatic loading, version switching, selection reset, shared
metadata and inline retry. It does not download or install an SDK.

On the Install Qt page, **Install optional recommended components for me** is
unchecked by default. Enable it to include the Qt Creator, documentation and
examples recommended by the selected packages' metadata. Qit fetches missing
metadata from the selected mirror and marks automatically included components
in the review dialog, alongside the total download size and licenses. If a
required recommendation is unavailable, planning stops with an error; disable
the option to continue without optional recommendations.

To add components from the CLI, use `qit-cli install --add-components` with the
same `--root` and `--name` as the existing installation, plus the repositories,
new package IDs and `--accept-licenses`. This adds packages; it does not upgrade
or remove installed ones. Close Qt tools before adding components.

### Explicit discovery validation

```sh
cargo test --locked --workspace
cargo test --locked -p qit-core --test catalog -- --ignored
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo run --locked -p qit-core --example audit-mirrors > mirror-audit.json
```

The live mirror audit uses the shared snapshot and production Rust network/listing
parser on every source. It covers all host directories, Android/WASM, shared
sources, nested repositories and representative component XML. JSON output records
time, final URLs, formats, package counts, expected parent XML 404s, optional missing
directories and failures. Only connection/timeouts are marked skipped; HTTP errors
and unrecognized pages fail. Captured listing fixtures live in `tests/fixtures/listing`.
The local discovery fixture verifies content negotiation, redirects, mirror-index,
parent/leaf component loading, malformed metadata and cancellation.

Live verification on 2026-10-17: after removing CQU, all ten configured sources
(including qt.io) passed the Rust listing/component audit. Each covered 19 parsed
directories and 13 component XMLs, with Windows ARM64's missing Android directory
recorded separately and shared Android supplied by `all_os`. Workspace, HTTP and
isolated GTK fixture checks passed on Linux; this does not validate other OS
runtimes or GNOME portal integration.
