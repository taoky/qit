#!/usr/bin/env python3
"""Bundle a native release build. Run separately on each target OS/architecture."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]

def run(*args, **kwargs):
    return subprocess.check_output(list(map(str, args)), text=True, encoding="utf-8", **kwargs).strip()

def copy(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)

def tree(source, destination):
    if source.exists():
        shutil.copytree(source, destination, dirs_exist_ok=True, symlinks=False, ignore_dangling_symlinks=True)

def linux_dependencies(binary):
    result = []
    for line in run("ldd", binary).splitlines():
        if "not found" in line:
            raise RuntimeError(line)
        match = re.search(r"=> (/\S+)", line)
        if match:
            path = Path(match[1])
            if not re.match(r"(lib(c|m|pthread|dl|rt|resolv)\.so|ld-linux)", path.name):
                result.append(path)
    return result

def mac_dependencies(binary):
    return [Path(line.strip().split(" (", 1)[0]) for line in run("otool", "-L", binary).splitlines()[1:]
            if line.strip().startswith(("/opt/", "/usr/local/"))]

def windows_dependencies(binary, prefix):
    output = run("objdump", "-p", binary)
    result = []
    for name in re.findall(r"DLL Name:\s*(\S+)", output):
        source = prefix / "bin" / name
        if not source.exists():
            source = next((p for p in (prefix / "bin").glob("*.dll") if p.name.lower() == name.lower()), source)
        if source.exists():
            result.append(source)
        elif not name.lower().startswith(("api-ms-", "ext-ms-")) and not (Path(os.environ["SystemRoot"]) / "System32" / name).exists():
            raise RuntimeError(f"Missing DLL {name} required by {binary}")
    return result

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", default="release")
    parser.add_argument("--prefix", type=Path, help="Native GTK dependency prefix (required on Windows)")
    parser.add_argument("--appimagetool", type=Path)
    parser.add_argument("--runtime-file", type=Path, help="Pre-downloaded AppImage type2 runtime")
    args = parser.parse_args()
    system = platform.system()
    arch = {"AMD64": "x86_64", "arm64": "aarch64", "ARM64": "aarch64"}.get(platform.machine(), platform.machine())
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    label = f"Qit-{version}-{system.lower()}-{arch}"
    out = ROOT / "dist"
    out.mkdir(exist_ok=True)
    stage = ROOT / ".build" / label
    if stage.exists():
        shutil.rmtree(stage)
    if system == "Darwin":
        bundle = stage / "Qit.app"
        bin_dir = bundle / "Contents/MacOS"
        lib_dir = bundle / "Contents/Frameworks"
        share_dir = bundle / "Contents/Resources/share"
        prefix = Path(run("brew", "--prefix"))
    elif system == "Windows":
        if not args.prefix:
            parser.error("--prefix is required on Windows")
        bundle = stage / "Qit"
        bin_dir = lib_dir = bundle
        share_dir = bundle / "share"
        prefix = args.prefix.resolve()
    else:
        bundle = stage / "Qit.AppDir"
        bin_dir = bundle / "usr/bin"
        lib_dir = bundle / "usr/lib"
        share_dir = bundle / "usr/share"
        prefix = args.prefix or Path("/usr")
    for directory in (bin_dir, lib_dir, share_dir):
        directory.mkdir(parents=True, exist_ok=True)
    suffix = ".exe" if system == "Windows" else ""
    queue = []
    inventory = {}
    for name in ("qit", "qit-cli"):
        source = ROOT / "target" / args.profile / (name + suffix)
        destination = bin_dir / (name + suffix)
        copy(source, destination)
        queue.append((source, destination))
    processed = set()
    while queue:
        source, destination = queue.pop()
        if source in processed:
            continue
        processed.add(source)
        if system == "Windows":
            dependencies = windows_dependencies(source, prefix)
        elif system == "Darwin":
            dependencies = mac_dependencies(source)
        else:
            dependencies = linux_dependencies(source)
        for dependency in dependencies:
            target = lib_dir / dependency.name
            if not target.exists():
                copy(dependency, target)
                queue.append((dependency, target))
                inventory[dependency.name] = dict(source=str(dependency), sha256=hashlib.sha256(dependency.read_bytes()).hexdigest())
            if system == "Darwin":
                run("install_name_tool", "-change", dependency, "@rpath/" + dependency.name, destination)
        if system == "Darwin":
            run("install_name_tool", "-add_rpath", "@executable_path/../Frameworks", destination)
            if destination.suffix == ".dylib":
                run("install_name_tool", "-id", "@rpath/" + destination.name, destination)
    for relative in ["glib-2.0/schemas", "icons/Adwaita", "icons/hicolor", "locale/zh_CN", "locale/en", "fontconfig"]:
        tree(prefix / "share" / relative, share_dir / relative)
    # macOS app roots must contain only Contents; configuration is a resource.
    config_dir = bundle / "Contents/Resources/etc" if system == "Darwin" else bundle / "etc"
    tree(prefix / "etc/fonts", config_dir / "fonts")
    copy(ROOT / "data/io.github.taoky.Qit.svg", share_dir / "icons/hicolor/scalable/apps/io.github.taoky.Qit.svg")
    copy(ROOT / "LICENSE", share_dir / "doc/qit/LICENSE")
    copy(ROOT / "THIRD_PARTY.md", share_dir / "doc/qit/THIRD_PARTY.md")
    # Include the distribution-provided notices, and an exact dependency inventory.
    if system == "Windows":
        tree(prefix / "share/licenses", share_dir / "licenses")
    elif system == "Linux" and shutil.which("pacman"):
        owners = set()
        for record in inventory.values():
            try:
                owners.add(run("pacman", "-Qoq", Path(record["source"]).resolve(), stderr=subprocess.DEVNULL))
            except subprocess.CalledProcessError:
                pass
        for owner in owners:
            tree(prefix / "share/licenses" / owner, share_dir / "licenses" / owner)
        tree(prefix / "share/licenses/common", share_dir / "licenses/common")
    if system == "Linux" and Path("/var/lib/dpkg/status").exists():
        for record in inventory.values():
            try:
                owner = run("dpkg-query", "-S", record["source"]).split(": ", 1)[0]
                tree(Path("/usr/share/doc") / owner.split(":", 1)[0], share_dir / "doc/native" / owner.replace(":", "-"))
            except subprocess.CalledProcessError:
                pass
    if system == "Darwin":
        cellar = Path(run("brew", "--cellar"))
        for record in inventory.values():
            resolved = Path(record["source"]).resolve()
            if resolved.is_relative_to(cellar):
                parts = resolved.relative_to(cellar).parts
                package_root = cellar / parts[0] / parts[1]
                for pattern in ("LICENSE*", "COPYING*", "AUTHORS*"):
                    for path in package_root.glob(pattern):
                        if path.is_file():
                            copy(path, share_dir / "licenses" / parts[0] / path.name)
    metadata = json.loads(run("cargo", "metadata", "--locked", "--offline", "--filter-platform", next(line.split(": ", 1)[1] for line in run("rustc", "-vV").splitlines() if line.startswith("host: ")), "--format-version", "1"))
    crate_licenses = []
    for package in metadata["packages"]:
        crate_licenses.append({key: package.get(key) for key in ("name", "version", "license", "repository")})
        parent = Path(package["manifest_path"]).parent
        for pattern in ("LICENSE*", "COPYING*"):
            for path in parent.glob(pattern):
                if path.is_file():
                    copy(path, share_dir / "licenses/rust" / f"{package['name']}-{package['version']}" / path.name)
    (share_dir / "doc/qit/dependencies.json").write_text(json.dumps(dict(native=inventory, rust=crate_licenses), indent=2))
    if system == "Darwin":
        plist = dict(CFBundleName="Qit", CFBundleIdentifier="io.github.taoky.Qit", CFBundleExecutable="qit", CFBundleShortVersionString=version, CFBundleVersion=version, CFBundlePackageType="APPL", LSMinimumSystemVersion="14.0", NSHighResolutionCapable=True)
        (bundle / "Contents/Info.plist").write_bytes(plistlib.dumps(plist))
        for path in lib_dir.iterdir():
            run("codesign", "--force", "--sign", "-", path)
        run("codesign", "--force", "--deep", "--sign", "-", bundle)
        run("hdiutil", "create", "-volname", "Qit", "-srcfolder", stage, "-ov", "-format", "UDZO", out / f"{label}.dmg")
    elif system == "Windows":
        # Dependency packages can carry timestamps outside ZIP's 1980–2107 range.
        # Clamp archive metadata without changing the staged files' timestamps.
        with zipfile.ZipFile(out / f"{label}.zip", "w", compression=zipfile.ZIP_DEFLATED,
                             strict_timestamps=False) as archive:
            for path in sorted(stage.rglob("*")):
                archive.write(path, path.relative_to(stage))
    else:
        copy(ROOT / "data/io.github.taoky.Qit.desktop", bundle / "io.github.taoky.Qit.desktop")
        copy(ROOT / "data/io.github.taoky.Qit.svg", bundle / "io.github.taoky.Qit.svg")
        launcher = bundle / "AppRun"
        launcher.write_text('''#!/bin/sh
set -eu
APPDIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
export LD_LIBRARY_PATH="$APPDIR/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export XDG_DATA_DIRS="$APPDIR/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export GSETTINGS_SCHEMA_DIR="$APPDIR/usr/share/glib-2.0/schemas"
if [ "${1-}" = "--cli" ]; then
    shift
    exec "$APPDIR/usr/bin/qit-cli" "$@"
fi
exec "$APPDIR/usr/bin/qit" "$@"
''')
        launcher.chmod(0o755)
        if args.appimagetool:
            run(args.appimagetool.resolve(), "--appimage-extract-and-run", *(["--runtime-file", args.runtime_file.resolve()] if args.runtime_file else []), bundle, out / f"{label}.AppImage", env=dict(os.environ, ARCH=arch))
        else:
            shutil.make_archive(str(out / label), "gztar", stage)
            print("AppDir tarball produced; use --appimagetool PATH to produce an AppImage.")
    print(f"Packaged {label} in {out}")

if __name__ == "__main__":
    main()
