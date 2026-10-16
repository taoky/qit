use qit_core::catalog::{Package, Repository};

pub fn repository_name(repo: &Repository) -> String {
    if let Some((series, code)) = repo.name.split_once('_') {
        let code = code.split('_').next().unwrap_or(code);
        if matches!(series, "qt5" | "qt6")
            && code.len() >= 3
            && code.bytes().all(|b| b.is_ascii_digit())
        {
            let minor_len =
                if code.starts_with("515") || (code.starts_with("61") && code.len() >= 4) {
                    2
                } else {
                    1
                };
            let (minor, patch) = code[1..].split_at(minor_len);
            let suffix = repo
                .name
                .strip_prefix(&format!("{series}_{code}"))
                .unwrap_or("")
                .replace('_', " ");
            return format!("Qt {}.{}.{}{}", &code[..1], minor, patch, suffix);
        }
    }
    repo.name
        .replace("tools_", "")
        .replace("qtcreator", "Qt Creator")
        .replace("cmake", "CMake")
        .replace("ninja", "Ninja")
        .replace("mingw", "MinGW")
        .replace("openssl", "OpenSSL")
        .replace('_', " ")
}

pub fn sort_repositories(items: &mut [Repository]) {
    items.sort_by(|a, b| {
        fn key(repo: &Repository) -> (bool, Vec<u32>, String) {
            let name = repository_name(repo);
            let stable = !["preview", "beta", "alpha", "rc"]
                .iter()
                .any(|tag| repo.name.contains(tag));
            let version = name
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .split('.')
                .filter_map(|n| n.parse().ok())
                .collect();
            (stable, version, name)
        }
        key(b).cmp(&key(a))
    });
}

pub fn common_repository(version: &str, name: &str, host: &str) -> bool {
    (name == version || name.starts_with(&format!("{version}_")))
        && !(host.starts_with("windows") && name.contains("unix_line_endings"))
        && !(!host.starts_with("windows") && name.contains("windows_line_endings"))
}

pub fn component_name(package: &Package) -> String {
    if group(package) != 0 {
        return package.name.clone();
    }
    let arch = package.id.rsplit('.').next().unwrap_or("");
    let friendly = match arch {
        "gcc_64" | "linux_gcc_64" => "GCC · x86_64",
        "gcc_arm64" | "linux_gcc_arm64" => "GCC · ARM64",
        "clang_64" | "macos" => "macOS",
        "win64_msvc2022_64" => "MSVC 2022 · x86_64",
        "win64_msvc2022_arm64_cross_compiled" => "MSVC 2022 · ARM64",
        "win64_mingw" => "MinGW · x86_64",
        _ => return package.name.clone(),
    };
    format!("{} · {friendly}", package.name)
}

pub fn group(package: &Package) -> usize {
    let id = &package.id;
    if id.starts_with("qt.tools.") {
        5
    } else if id.contains(".debug") {
        4
    } else if id.contains(".doc") || id.contains(".examples") {
        3
    } else if id.contains(".src") {
        2
    } else if id.split('.').count() == 4 {
        0
    } else {
        1
    }
}

pub fn folder_name(repo: &Repository, target: &str) -> String {
    format!(
        "{}-{target}",
        repository_name(repo).to_lowercase().replace(' ', "-")
    )
}

pub fn folder_name_for_host(repo: &Repository, target: &str, host: &str) -> String {
    let base = folder_name(repo, target);
    if host == qit_core::catalog::host() {
        return base;
    }
    let suffix = match host {
        "windows_x86" => "windows-x86_64",
        "windows_arm64" => "windows-arm64",
        "linux_x64" => "linux-x86_64",
        "linux_arm64" => "linux-arm64",
        "mac_x64" => "macos",
        other => other,
    };
    format!("{base}-{suffix}")
}

pub fn valid_folder(root: &str, name: &str) -> bool {
    !root.trim().is_empty()
        && !name.is_empty()
        && !name.starts_with('.')
        && !name.contains('/')
        && qit_core::archive::safe_relative(name).is_ok()
        && {
            let path = std::path::Path::new(root).join(name);
            std::fs::symlink_metadata(&path)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                || qit_core::install::managed_destination(&path)
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_names_and_directory_validation() {
        for (raw, expected) in [
            ("qt6_681", "Qt 6.8.1"),
            ("qt6_6100", "Qt 6.10.0"),
            ("qt5_5152", "Qt 5.15.2"),
            ("qt5_51518", "Qt 5.15.18"),
            ("tools_qtcreator", "Qt Creator"),
        ] {
            assert_eq!(
                repository_name(&Repository {
                    name: raw.into(),
                    path: String::new()
                }),
                expected
            );
        }
        assert!(!valid_folder("/tmp", "../escape"));
        assert!(!valid_folder("/tmp", "."));
        assert!(!valid_folder("", "qt"));
    }
    #[test]
    fn shared_repositories_match_exact_version_and_line_endings() {
        assert!(common_repository(
            "qt6_681",
            "qt6_681_unix_line_endings_src",
            "linux_x64"
        ));
        assert!(!common_repository(
            "qt6_681",
            "qt6_681_windows_line_endings_src",
            "linux_x64"
        ));
        assert!(common_repository(
            "qt6_681",
            "qt6_681_windows_line_endings_src",
            "windows_x86"
        ));
        assert!(!common_repository(
            "qt6_681",
            "qt6_6810_unix_line_endings_src",
            "linux_x64"
        ));
    }
    #[test]
    fn newest_stable_version_first() {
        let mut items =
            ["qt6_690", "qt6_6100", "qt6_6110_preview", "qt5_51518"].map(|name| Repository {
                name: name.into(),
                path: String::new(),
            });
        sort_repositories(&mut items);
        assert_eq!(
            items.map(|r| r.name),
            ["qt6_6100", "qt6_690", "qt5_51518", "qt6_6110_preview"]
        );
    }
    #[test]
    fn real_components_are_grouped() {
        let packages = qit_core::catalog::parse(
            include_str!("../../../tests/fixtures/qt6-linux-6.8.1.xml"),
            "fixture/",
        )
        .unwrap();
        assert_eq!(
            group(
                packages
                    .iter()
                    .find(|p| p.id.ends_with("681.linux_gcc_64"))
                    .unwrap()
            ),
            0
        );
        assert_eq!(
            group(
                packages
                    .iter()
                    .find(|p| p.id.ends_with("addons.qtcharts.linux_gcc_64"))
                    .unwrap()
            ),
            1
        );
        assert_eq!(
            group(
                packages
                    .iter()
                    .find(|p| p.id.contains("debug_information"))
                    .unwrap()
            ),
            4
        );
    }
}

#[test]
fn foreign_host_uses_a_separate_default_folder() {
    let repo = Repository {
        name: "qt6_6120".into(),
        path: String::new(),
    };
    assert_eq!(
        folder_name_for_host(&repo, "desktop", qit_core::catalog::host()),
        "qt-6.12.0-desktop"
    );
    for host in [
        "windows_x86",
        "windows_arm64",
        "linux_x64",
        "linux_arm64",
        "mac_x64",
    ] {
        if host != qit_core::catalog::host() {
            assert_ne!(
                folder_name_for_host(&repo, "desktop", host),
                "qt-6.12.0-desktop"
            );
        }
    }
}
