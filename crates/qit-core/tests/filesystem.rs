use qit_core::{
    Cancel, archive, install,
    network::{Checksum, hash, verify},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static COUNTER: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "qit-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn tar(path: &Path, name: &str, data: &[u8], link: Option<&str>) {
    let mut header = [0u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[100..108].copy_from_slice(b"0000755\0");
    header[124..136].copy_from_slice(
        format!("{:011o}\0", if link.is_some() { 0 } else { data.len() }).as_bytes(),
    );
    header[148..156].fill(b' ');
    header[156] = if link.is_some() { b'2' } else { b'0' };
    if let Some(link) = link {
        header[157..157 + link.len()].copy_from_slice(link.as_bytes());
    }
    let checksum: u32 = header.iter().map(|b| *b as u32).sum();
    header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    let mut bytes = header.to_vec();
    if link.is_none() {
        bytes.extend_from_slice(data);
    }
    bytes.resize(bytes.len().div_ceil(512) * 512 + 1024, 0);
    fs::write(path, bytes).unwrap();
}
#[test]
fn extraction_handles_unicode_paths_and_rejects_escape_and_collision() {
    let temp = Temp::new();
    let input = temp.0.join("archive.tar");
    let root = temp.0.join("安装 with spaces");
    tar(&input, "bin/tool", b"hello", None);
    archive::extract(&input, &root, &Cancel::default()).unwrap();
    assert_eq!(fs::read(root.join("bin/tool")).unwrap(), b"hello");
    archive::extract(&input, &root, &Cancel::default()).unwrap();
    tar(&input, "bin/tool", b"different", None);
    assert!(archive::extract(&input, &root, &Cancel::default()).is_err());
    assert_eq!(fs::read(root.join("bin/tool")).unwrap(), b"hello");
    tar(&input, "../escape", b"oops", None);
    assert!(archive::extract(&input, &root, &Cancel::default()).is_err());
    assert!(!temp.0.join("escape").exists());
    tar(&input, "link", b"", Some("../../escape"));
    assert!(archive::extract(&input, &root, &Cancel::default()).is_err());
}
#[cfg(unix)]
#[test]
fn extraction_never_writes_through_existing_symlink() {
    let temp = Temp::new();
    let input = temp.0.join("archive.tar");
    let root = temp.0.join("root");
    let outside = temp.0.join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("bin")).unwrap();
    tar(&input, "bin/tool", b"oops", None);
    assert!(archive::extract(&input, &root, &Cancel::default()).is_err());
    assert!(!outside.join("tool").exists());
}
#[test]
fn paths_are_patched_to_final_destination_not_staging_directory() {
    let temp = Temp::new();
    let stage = temp.0.join("stage");
    let final_root = temp.0.join("installed");
    let qt = stage.join("6.8.1/gcc_64");
    fs::create_dir_all(qt.join("mkspecs")).unwrap();
    fs::create_dir_all(qt.join("lib/pkgconfig")).unwrap();
    fs::write(qt.join("mkspecs/qconfig.pri"), "QT_EDITION = OpenSource\n").unwrap();
    fs::write(
        qt.join("lib/pkgconfig/Qt6Core.pc"),
        "prefix=/home/qt/work/install\n",
    )
    .unwrap();
    install::configure(&stage, &final_root, &Cancel::default()).unwrap();
    assert_eq!(
        fs::read_to_string(qt.join("bin/qt.conf")).unwrap(),
        "[Paths]\nPrefix=..\n"
    );
    assert_eq!(
        fs::read_to_string(qt.join("lib/pkgconfig/Qt6Core.pc")).unwrap(),
        format!("prefix={}/6.8.1/gcc_64\n", final_root.display()).replace('\\', "/")
    );
}
#[test]
fn cancellation_and_checksum_failure_are_not_success() {
    let temp = Temp::new();
    let path = temp.0.join("archive.tar");
    tar(&path, "test", b"content", None);
    let cancel = Cancel::default();
    cancel.cancel();
    assert!(archive::extract(&path, &temp.0.join("out"), &cancel).is_err());
    assert!(
        verify(
            &path,
            &Checksum {
                algorithm: "sha256".into(),
                value: hash(&path, "sha256").unwrap()
            }
        )
        .unwrap()
    );
    assert!(
        !verify(
            &path,
            &Checksum {
                algorithm: "sha256".into(),
                value: "0".repeat(64)
            }
        )
        .unwrap()
    );
}

#[test]
#[ignore = "Needs QIT_QTBASE_ARCHIVE downloaded from the Qt repository"]
fn real_qtbase_archive_relocates_and_runs_qmake() {
    let input = PathBuf::from(std::env::var_os("QIT_QTBASE_ARCHIVE").expect("QIT_QTBASE_ARCHIVE"));
    let temp = Temp::new();
    let sdk_root = temp.0.join("SDK with spaces");
    let qt = sdk_root.join("6.8.1/gcc_64");
    fs::create_dir_all(&qt).unwrap();
    archive::extract(&input, &qt, &Cancel::default()).unwrap();
    if let Some(icu) = std::env::var_os("QIT_ICU_ARCHIVE") {
        archive::extract(Path::new(&icu), &qt.join("lib"), &Cancel::default()).unwrap();
    }
    install::configure(&sdk_root, &sdk_root, &Cancel::default()).unwrap();
    let result = std::process::Command::new(qt.join("bin/qmake"))
        .args(["-query", "QT_INSTALL_PREFIX"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8(result.stdout).unwrap().trim(),
        qt.to_str().unwrap()
    );
    if let Some(wasm) = std::env::var_os("QIT_WASM_ARCHIVE") {
        let cross = sdk_root.join("6.8.1/wasm_singlethread");
        archive::extract(Path::new(&wasm), &cross, &Cancel::default()).unwrap();
        install::configure(&sdk_root, &sdk_root, &Cancel::default()).unwrap();
        let result = std::process::Command::new(cross.join("bin/qmake"))
            .args(["-query", "QT_INSTALL_PREFIX"])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            String::from_utf8(result.stdout).unwrap().trim(),
            cross.to_str().unwrap()
        );
    }
    let source = temp.0.join("sample");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("CMakeLists.txt"), "cmake_minimum_required(VERSION 3.16)\nproject(qit_smoke LANGUAGES CXX)\nfind_package(Qt6 REQUIRED COMPONENTS Core)\nadd_executable(qit_smoke main.cpp)\ntarget_link_libraries(qit_smoke PRIVATE Qt6::Core)\n").unwrap();
    fs::write(source.join("main.cpp"), "#include <QtCore/QString>\n#include <iostream>\nint main() { std::cout << QStringLiteral(\"Qit Qt smoke passed\").toStdString(); }\n").unwrap();
    let build = source.join("build");
    assert!(
        std::process::Command::new("cmake")
            .arg("-S")
            .arg(&source)
            .arg("-B")
            .arg(&build)
            .arg(format!("-DCMAKE_PREFIX_PATH={}", qt.display()))
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("cmake")
            .arg("--build")
            .arg(&build)
            .status()
            .unwrap()
            .success()
    );
    let result = std::process::Command::new(build.join("qit_smoke"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"Qit Qt smoke passed");
}

#[test]
fn staging_overlay_replaces_regular_files_but_rejects_type_conflicts() {
    let temp = Temp::new();
    let input = temp.0.join("archive.tar");
    let root = temp.0.join("stage");
    tar(&input, "CMakeLists.txt", b"long original contents", None);
    archive::extract_overlay(&input, &root, &Cancel::default()).unwrap();
    tar(&input, "CMakeLists.txt", b"new", None);
    let replaced = archive::extract_overlay(&input, &root, &Cancel::default()).unwrap();
    assert_eq!(replaced, [root.join("CMakeLists.txt")]);
    assert_eq!(fs::read(&replaced[0]).unwrap(), b"new");
    tar(&input, "CMakeLists.txt/child", b"bad", None);
    assert!(archive::extract_overlay(&input, &root, &Cancel::default()).is_err());
    fs::create_dir(root.join("directory")).unwrap();
    tar(&input, "directory", b"bad", None);
    assert!(archive::extract_overlay(&input, &root, &Cancel::default()).is_err());
    tar(&input, "../escape", b"bad", None);
    assert!(archive::extract_overlay(&input, &root, &Cancel::default()).is_err());
    assert!(!temp.0.join("escape").exists());
}

#[cfg(unix)]
#[test]
fn staging_overlay_does_not_follow_links_or_modify_hardlink_targets() {
    let temp = Temp::new();
    let input = temp.0.join("archive.tar");
    let root = temp.0.join("stage");
    fs::create_dir(&root).unwrap();
    let outside = temp.0.join("outside");
    fs::write(&outside, b"keep").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    tar(&input, "link", b"changed", None);
    assert!(archive::extract_overlay(&input, &root, &Cancel::default()).is_err());
    fs::hard_link(&outside, root.join("hard")).unwrap();
    tar(&input, "hard", b"changed", None);
    archive::extract_overlay(&input, &root, &Cancel::default()).unwrap();
    assert_eq!(fs::read(&outside).unwrap(), b"keep");
    assert_eq!(fs::read(root.join("hard")).unwrap(), b"changed");
    std::os::unix::fs::symlink(&temp.0, root.join("parent")).unwrap();
    tar(&input, "parent/outside", b"changed", None);
    assert!(archive::extract_overlay(&input, &root, &Cancel::default()).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"keep");
}

#[test]
#[ignore = "Requires QIT_EXAMPLE_ARCHIVES: ordered paths to real Qt example archives"]
fn real_example_archives_overlay_successfully() {
    let paths = std::env::var_os("QIT_EXAMPLE_ARCHIVES").unwrap();
    let paths = std::env::split_paths(&paths).collect::<Vec<_>>();
    assert!(paths.len() >= 2);
    let temp = Temp::new();
    let root = temp.0.join("Examples/Qt");
    let mut replaced = Vec::new();
    for input in &paths {
        replaced.extend(archive::extract_overlay(input, &root, &Cancel::default()).unwrap());
    }
    assert!(replaced.contains(&root.join("CMakeLists.txt")));
    let last = temp.0.join("last");
    archive::extract(paths.last().unwrap(), &last, &Cancel::default()).unwrap();
    assert_eq!(
        fs::read(root.join("CMakeLists.txt")).unwrap(),
        fs::read(last.join("CMakeLists.txt")).unwrap()
    );
    println!(
        "Extracted {} real archives; {} regular-file overlays",
        paths.len(),
        replaced.len()
    );
}
