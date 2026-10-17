use crate::{archive::safe_relative, network::Network};
use anyhow::{Context, Result, bail, ensure};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Repository {
    pub name: String,
    pub path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Archive {
    pub path: String,
    pub destination: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub repository: String,
    pub archives: Vec<Archive>,
    pub dependencies: Vec<String>,
    pub auto_dependencies: Vec<String>,
    pub compressed_size: u64,
    pub installed_size: u64,
    pub unsupported: Option<String>,
    pub licenses: Vec<String>,
    #[serde(default)]
    pub license_files: Vec<String>,
    #[serde(default)]
    pub metadata_sha1: String,
}
pub fn host() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "aarch64") => "linux_arm64",
        ("linux", _) => "linux_x64",
        ("windows", "aarch64") => "windows_arm64",
        ("windows", _) => "windows_x86",
        _ => "mac_x64",
    }
}
pub fn directories(html: &str) -> Vec<String> {
    directories_at(
        html,
        &url::Url::parse("https://listing.invalid/directory/").unwrap(),
    )
}
pub fn directories_at(html: &str, base: &url::Url) -> Vec<String> {
    let doc = Html::parse_document(html);
    let selector = Selector::parse("a[href]").unwrap();
    let mut result = BTreeSet::new();
    for link in doc.select(&selector) {
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        if href.contains(['%', '\\']) || href.split('/').any(|p| p == "..") {
            continue;
        }
        let Ok(url) = base.join(href) else {
            continue;
        };
        if url.origin() != base.origin()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.path().ends_with('/')
        {
            continue;
        }
        let Some(relative) = url.path().strip_prefix(base.path()) else {
            continue;
        };
        let name = relative.trim_end_matches('/');
        if crate::listing::valid_name(name) {
            result.insert(name.to_string());
        }
    }
    result.into_iter().collect()
}
pub async fn repositories(net: &Network, host: &str, target: &str) -> Result<Vec<Repository>> {
    for value in [host, target] {
        ensure!(
            crate::listing::valid_name(value),
            "Invalid repository group"
        );
    }
    let hosts: Vec<&str> = match target {
        "wasm" | "qt" => vec!["all_os"],
        "android" if host != "all_os" => vec![host, "all_os"],
        _ => vec![host],
    };
    let mut result = Vec::new();
    for effective_host in hosts {
        let root = format!("online/qtsdkrepository/{effective_host}/{target}/");
        if let Some(names) = crate::listing::directories(net, &root).await? {
            result.extend(names.into_iter().map(|name| Repository {
                path: format!("{root}{name}/"),
                name,
            }));
        }
    }
    result.sort_by(|a, b| b.name.cmp(&a.name));
    ensure!(
        !result.is_empty(),
        "No repositories for {host}/{target} on this mirror"
    );
    Ok(result)
}

pub async fn packages(net: &Network, repository: &str) -> Result<Vec<Package>> {
    ensure!(
        repository
            .trim_end_matches('/')
            .split('/')
            .all(crate::listing::valid_name),
        "Invalid repository path"
    );
    let root = format!("{}/", repository.trim_end_matches('/'));
    if let Some(xml) = net.optional_text(&format!("{root}Updates.xml")).await? {
        return parse(&xml, &root);
    }
    let children = crate::listing::directories(net, &root)
        .await?
        .with_context(|| format!("No component metadata in {root}"))?;
    let mut result = Vec::new();
    for child in children {
        let path = format!("{root}{child}/");
        if let Some(xml) = net.optional_text(&format!("{path}Updates.xml")).await? {
            result.extend(parse(&xml, &path)?);
        }
    }
    ensure!(!result.is_empty(), "No component metadata in {root}");
    Ok(result)
}
pub fn parse(xml: &str, repository: &str) -> Result<Vec<Package>> {
    let doc = roxmltree::Document::parse(xml).context("Invalid Updates.xml")?;
    ensure!(
        doc.root_element().has_tag_name("Updates"),
        "Expected Updates.xml, received a different document"
    );
    let mut result = Vec::new();
    for node in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("PackageUpdate"))
    {
        let field = |key| {
            node.children()
                .find(|n| n.has_tag_name(key))
                .and_then(|n| n.text())
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let split = |key| {
            field(key)
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect::<Vec<_>>()
        };
        let id = field("Name");
        let version = field("Version");
        ensure!(
            !id.is_empty() && !version.is_empty(),
            "Component missing name/version"
        );
        safe_relative(&id)?;
        safe_relative(&version)?;
        ensure!(
            !id.contains('/') && !version.contains('/'),
            "Invalid package identity"
        );
        let mut unsupported = None;
        let mut destinations = BTreeMap::new();
        for operation in node.descendants().filter(|n| n.has_tag_name("Operation")) {
            let args: Vec<_> = operation
                .children()
                .filter(|n| n.has_tag_name("Argument"))
                .map(|n| n.text().unwrap_or(""))
                .collect();
            if operation.attribute("name") == Some("Extract") && args.len() == 2 {
                let target = args[0]
                    .strip_prefix("@TargetDir@/")
                    .or_else(|| (args[0] == "@TargetDir@").then_some(""))
                    .unwrap_or("@UNSUPPORTED@");
                if target.contains('@') || (!target.is_empty() && safe_relative(target).is_err()) {
                    unsupported = Some(format!("Unsupported extraction destination: {}", args[0]));
                }
                destinations.insert(args[1].to_string(), target.to_string());
            } else {
                unsupported = Some(format!(
                    "Unsupported installer operation: {}",
                    operation.attribute("name").unwrap_or("unknown")
                ));
            }
        }
        // Legacy Qt archives contain their own version/architecture directory tree.
        let archives = split("DownloadableArchives")
            .into_iter()
            .map(|archive| {
                safe_relative(&archive)?;
                ensure!(!archive.contains('/'), "Invalid archive name");
                Ok(Archive {
                    path: format!("{repository}{id}/{version}{archive}"),
                    destination: destinations.remove(&archive).unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let supported_family = (id == "qt.tools" && archives.is_empty())
            || id.starts_with("qt.qt5.515")
            || id.starts_with("qt.qt6.")
            || compiler_repository(&id).is_some()
            || [
                "qt.tools.qtcreator",
                "qt.tools.cmake",
                "qt.tools.ninja",
                "qt.tools.mingw",
                "qt.tools.openssl",
            ]
            .iter()
            .any(|prefix| id.starts_with(prefix));
        if !supported_family {
            unsupported = Some("This component has no Qit install adapter yet".into());
        }
        let update = node.children().find(|n| n.has_tag_name("UpdateFile"));
        let size = |name| {
            update
                .and_then(|n| n.attribute(name))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        };
        result.push(Package {
            id,
            name: field("DisplayName"),
            description: field("Description"),
            version,
            repository: repository.into(),
            archives,
            dependencies: split("Dependencies"),
            auto_dependencies: split("AutoDependOn"),
            compressed_size: size("CompressedSize"),
            installed_size: size("UncompressedSize"),
            unsupported,
            licenses: node
                .descendants()
                .filter(|n| n.has_tag_name("License"))
                .map(|n| {
                    n.attribute("name")
                        .unwrap_or("Qt component license")
                        .to_string()
                })
                .collect(),
            license_files: node
                .descendants()
                .filter(|n| n.has_tag_name("License"))
                .filter_map(|n| n.attribute("file"))
                .map(String::from)
                .collect(),
            metadata_sha1: field("SHA1"),
        });
    }
    Ok(result)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub mirror: String,
    pub selected: Vec<String>,
    pub packages: Vec<Package>,
    pub notes: Vec<String>,
    #[serde(default)]
    pub license_texts: BTreeMap<String, String>,
}
fn dependency(dep: &str) -> (&str, Option<&str>) {
    if let Some((id, _)) = dep.split_once("-!=") {
        return (id, Some(&dep[id.len() + 1..]));
    }
    for op in ["->=", "-<=", "->", "-<", "-="] {
        if let Some((id, _)) = dep.split_once(op) {
            return (id, Some(&dep[id.len() + 1..]));
        }
    }
    (dep, None)
}
fn compiler_repository(id: &str) -> Option<String> {
    for (prefix, repository) in [
        ("qt.tools.win64_llvm_mingw", "tools_llvm_mingw"),
        ("qt.tools.win64_mingw", "tools_mingw"),
    ] {
        if let Some(version) = id.strip_prefix(prefix)
            && !version.is_empty()
            && version.bytes().all(|b| b.is_ascii_digit())
        {
            return Some(format!("{repository}{version}"));
        }
    }
    None
}

fn optional_recommendation(id: &str) -> bool {
    (id == "qt.tools.qtcreator" || id == "qt.tools.qtcreator_gui")
        || id.contains(".doc")
        || id.contains(".examples")
}
#[derive(Debug)]
struct MissingDependency(String);
impl std::fmt::Display for MissingDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Missing dependency {}; load its repository too", self.0)
    }
}
impl std::error::Error for MissingDependency {}

pub fn plan(mirror: &str, all: &[Package], selected: &[String]) -> Result<Plan> {
    plan_with_recommendations(mirror, all, selected, false)
}

pub fn plan_with_recommendations(
    mirror: &str,
    all: &[Package],
    selected: &[String],
    include_recommendations: bool,
) -> Result<Plan> {
    ensure!(!selected.is_empty(), "Select at least one component");
    let index: BTreeMap<_, _> = all.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut wanted: BTreeSet<String> = selected.iter().cloned().collect();
    let mut notes = BTreeSet::new();
    loop {
        let before = wanted.len();
        for id in wanted.clone() {
            let p = index
                .get(id.as_str())
                .ok_or_else(|| MissingDependency(id.clone()))?;
            if let Some(reason) = &p.unsupported {
                bail!("{}: {reason}", p.id);
            }
            for dep in &p.dependencies {
                let (name, constraint) = dependency(dep);
                if name.ends_with(".patcher") {
                    notes.insert("Qt path relocation is supplied by Qit; the official installer patcher is not required.".into());
                    continue;
                }
                if !include_recommendations
                    && optional_recommendation(name)
                    && !wanted.contains(name)
                    && !p.id.contains(".doc")
                    && !p.id.contains(".examples")
                {
                    notes.insert(format!("Optional recommendation: {name}"));
                    continue;
                }
                if let Some(constraint) = constraint {
                    let actual = index
                        .get(name)
                        .ok_or_else(|| MissingDependency(name.to_string()))?;
                    let required = semver::VersionReq::parse(constraint)
                        .with_context(|| format!("Unsupported dependency constraint: {dep}"))?;
                    let version =
                        semver::Version::parse(actual.version.split('-').next().unwrap_or(""))?;
                    ensure!(
                        required.matches(&version),
                        "Dependency version mismatch: {dep}"
                    );
                }
                wanted.insert(name.into());
            }
            if p.id.contains(".android")
                || p.id.contains(".wasm")
                || p.id.ends_with("_cross_compiled")
            {
                let parts: Vec<_> = p.id.split('.').collect();
                if parts.len() >= 4 && parts[1] == "qt6" {
                    let prefix = parts[..3].join(".");
                    let candidates: Vec<_> = all
                        .iter()
                        .filter(|candidate| {
                            candidate.id.starts_with(&format!("{prefix}."))
                                && candidate.id.split('.').count() == 4
                                && [
                                    "gcc_64",
                                    "gcc_arm64",
                                    "clang_64",
                                    "macos",
                                    "msvc2022_64",
                                    "msvc2019_64",
                                    "msvc2022_arm64",
                                ]
                                .iter()
                                .any(|s| candidate.id.ends_with(s))
                        })
                        .collect();
                    let preferred = if host().ends_with("arm64") {
                        "arm64"
                    } else {
                        "64"
                    };
                    let candidate = candidates
                        .iter()
                        .find(|p| p.id.ends_with(preferred))
                        .or(candidates.first())
                        .with_context(|| {
                            format!("Load the matching desktop repository for {prefix}")
                        })?;
                    wanted.insert(candidate.id.clone());
                    notes.insert("Android requires a compatible JDK/SDK/NDK; WebAssembly requires a matching Emscripten SDK. External dependencies are not installed by Qit.".into());
                }
            }
        }
        for p in all {
            if !p.auto_dependencies.is_empty()
                && p.auto_dependencies
                    .iter()
                    .all(|d| wanted.contains(dependency(d).0))
            {
                wanted.insert(p.id.clone());
            }
        }
        if before == wanted.len() {
            break;
        }
    }
    let packages = wanted
        .into_iter()
        .map(|id| {
            index
                .get(id.as_str())
                .map(|p| (*p).clone())
                .with_context(|| format!("Missing component: {id}"))
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        packages.iter().any(|p| !p.archives.is_empty()),
        "Selection has no downloadable components"
    );
    Ok(Plan {
        mirror: mirror.into(),
        selected: selected.to_vec(),
        packages,
        notes: notes.into_iter().collect(),
        license_texts: BTreeMap::new(),
    })
}

/// Resolve host Qt automatically and fetch checksum-verified license texts before confirmation.
pub async fn prepare_plan(net: &Network, all: Vec<Package>, selected: &[String]) -> Result<Plan> {
    prepare_plan_with_recommendations(net, all, selected, false).await
}

pub async fn prepare_plan_with_recommendations(
    net: &Network,
    mut all: Vec<Package>,
    selected: &[String],
    include_recommendations: bool,
) -> Result<Plan> {
    let mut versions = BTreeSet::new();
    for id in selected {
        if id.starts_with("qt.qt6.")
            && (id.contains(".android") || id.contains(".wasm") || id.ends_with("_cross_compiled"))
            && let Some(version) = id.split('.').nth(2)
        {
            versions.insert(version.to_string());
        }
    }
    for version in versions {
        let prefix = format!("qt.qt6.{version}.");
        let has_host = all.iter().any(|p| {
            p.id.starts_with(&prefix)
                && p.id.split('.').count() == 4
                && [
                    "gcc_64",
                    "gcc_arm64",
                    "clang_64",
                    "msvc2022_64",
                    "msvc2019_64",
                    "msvc2022_arm64",
                ]
                .iter()
                .any(|a| p.id.ends_with(a))
        });
        if !has_host {
            all.extend(
                packages(
                    net,
                    &format!("online/qtsdkrepository/{}/desktop/qt6_{version}/", host()),
                )
                .await?,
            );
        }
    }
    // Load only repositories needed by the dependency closure, using the same mirror.
    let selected_host = all
        .iter()
        .filter(|p| selected.contains(&p.id))
        .filter_map(|p| p.repository.strip_prefix("online/qtsdkrepository/"))
        .filter_map(|path| path.split('/').next())
        .find(|value| *value != "all_os")
        .unwrap_or(host())
        .to_string();
    let mut attempted = BTreeSet::new();
    let mut result = loop {
        match plan_with_recommendations(net.base.as_str(), &all, selected, include_recommendations)
        {
            Ok(plan) => break plan,
            Err(error) => {
                let Some(missing) = error.downcast_ref::<MissingDependency>() else {
                    return Err(error);
                };
                let compiler_repo = compiler_repository(&missing.0);
                if missing.0 != "qt.tools"
                    && compiler_repo.is_none()
                    && (!include_recommendations || !optional_recommendation(&missing.0))
                {
                    return Err(error);
                }
                let mut candidates = if let Some(repository) = compiler_repo {
                    vec![Repository {
                        path: format!(
                            "online/qtsdkrepository/{selected_host}/desktop/{repository}/"
                        ),
                        name: repository,
                    }]
                } else if missing.0 == "qt.tools" {
                    vec![Repository {
                        name: "tools_generic".into(),
                        path: format!(
                            "online/qtsdkrepository/{selected_host}/desktop/tools_generic/"
                        ),
                    }]
                } else if missing.0.starts_with("qt.tools.qtcreator") {
                    repositories(net, &selected_host, "desktop")
                        .await?
                        .into_iter()
                        .filter(|r| {
                            matches!(r.name.as_str(), "tools_qtcreator" | "tools_qtcreator_gui")
                        })
                        .collect::<Vec<_>>()
                } else {
                    let parts = missing.0.split('.').collect::<Vec<_>>();
                    ensure!(parts.len() >= 3, "Invalid recommendation: {}", missing.0);
                    let version = format!("{}_{}", parts[1], parts[2]);
                    repositories(net, "all_os", "qt")
                        .await?
                        .into_iter()
                        .filter(|r| {
                            (r.name == version || r.name.starts_with(&format!("{version}_")))
                                && !(selected_host.starts_with("windows")
                                    && r.name.contains("unix_line_endings"))
                                && !(!selected_host.starts_with("windows")
                                    && r.name.contains("windows_line_endings"))
                        })
                        .collect::<Vec<_>>()
                };
                candidates.sort_by(|a, b| a.name.cmp(&b.name));
                let mut found = false;
                for repo in candidates {
                    if !attempted.insert(repo.path.clone()) {
                        continue;
                    }
                    let loaded = packages(net, &repo.path).await?;
                    found = loaded.iter().any(|p| p.id == missing.0);
                    for package in loaded {
                        if !all.iter().any(|p| p.id == package.id) {
                            all.push(package);
                        }
                    }
                    if found {
                        break;
                    }
                }
                if !found {
                    return Err(error.context("Required component is unavailable from this mirror"));
                }
            }
        }
    };
    for package in &result.packages {
        if package.license_files.is_empty() {
            continue;
        }
        ensure!(
            package.metadata_sha1.len() == 40
                && package.metadata_sha1.bytes().all(|b| b.is_ascii_hexdigit()),
            "License metadata has no valid checksum: {}",
            package.id
        );
        let cache = crate::settings::data_dir().join("licenses");
        std::fs::create_dir_all(&cache)?;
        let archive = cache.join(&package.metadata_sha1);
        let path = format!(
            "{}{}/{}meta.7z",
            package.repository, package.id, package.version
        );
        net.download(
            &path,
            &archive,
            &crate::network::Checksum {
                algorithm: "sha1".into(),
                value: package.metadata_sha1.to_lowercase(),
            },
        )
        .await?;
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let extracted = cache.join(format!("read-{}-{unique}", std::process::id()));
        let read_result = (|| -> Result<()> {
            crate::archive::extract(&archive, &extracted, &net.cancel)?;
            for (index, file) in package.license_files.iter().enumerate() {
                let relative = safe_relative(file)?;
                let path = extracted.join(&package.id).join(&relative);
                let path = if path.is_file() {
                    path
                } else {
                    extracted.join(relative)
                };
                ensure!(
                    std::fs::metadata(&path)?.len() <= 4 * 1024 * 1024,
                    "License text is too large"
                );
                let title = package.licenses.get(index).unwrap_or(file);
                result.license_texts.insert(
                    format!("{} — {title}", package.name),
                    std::fs::read_to_string(path)?,
                );
            }
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(extracted);
        read_result?;
    }
    let cross_android = selected.iter().any(|id| id.contains(".android"));
    let cross_wasm = selected.iter().any(|id| id.contains(".wasm"));
    if cross_android || cross_wasm {
        for tool in if cross_android {
            vec!["java", "javac"]
        } else {
            vec!["emcc"]
        } {
            let extensions = if cfg!(windows) {
                vec![".exe", ".bat", ".cmd", ""]
            } else {
                vec![""]
            };
            let found = std::env::var_os("PATH")
                .into_iter()
                .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
                .flat_map(|path| {
                    extensions
                        .iter()
                        .map(move |ext| path.join(format!("{tool}{ext}")))
                })
                .find(|path| path.is_file());
            result.notes.push(match found { Some(path) => format!("External prerequisite found: {}. Check its version against the selected Qt release.", path.display()), None => format!("External prerequisite not found in PATH: {tool}") });
        }
        if cross_android {
            for variable in ["ANDROID_SDK_ROOT", "ANDROID_NDK_ROOT", "JAVA_HOME"] {
                result.notes.push(format!(
                    "{variable}: {}",
                    std::env::var(variable).unwrap_or_else(|_| "not configured".into())
                ));
            }
        }
    }
    Ok(result)
}
