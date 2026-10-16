use crate::{
    Cancel, Progress, archive,
    catalog::Plan,
    network::{Checksum, Network, hash},
    report,
    settings::{atomic_json, data_dir},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub sha256: Option<String>,
    pub link: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Installation {
    pub schema: u32,
    pub id: String,
    pub path: PathBuf,
    pub plan: Plan,
    pub files: Vec<FileRecord>,
    pub checksums: BTreeMap<String, Checksum>,
    pub state: String,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    id: String,
    root: PathBuf,
    destination: PathBuf,
    #[serde(default)]
    previous: Option<Installation>,
}
struct StageGuard {
    stage: PathBuf,
    journal: PathBuf,
}
impl Drop for StageGuard {
    fn drop(&mut self) {
        if self.stage.exists() {
            let _ = fs::remove_dir_all(&self.stage);
            let _ = fs::remove_file(&self.journal);
        }
    }
}
pub fn recover(root: &Path) -> Result<Vec<String>> {
    let _lock = lock(root)?;
    recover_locked(&fs::canonicalize(root)?)
}
fn recover_locked(root: &Path) -> Result<Vec<String>> {
    let journals = data_dir().join("transactions");
    let mut recovered = Vec::new();
    if !journals.exists() {
        return Ok(recovered);
    }
    for entry in fs::read_dir(journals)? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let journal: Journal = serde_json::from_slice(&fs::read(&path)?)?;
        if journal.root != root {
            continue;
        }
        archive::safe_relative(&journal.id)?;
        ensure!(
            !journal.id.contains('/') && journal.destination.parent() == Some(root),
            "Invalid recovery journal"
        );
        let stage = root.join(format!(".qit-stage-{}", journal.id));
        let record = data_dir()
            .join("installations")
            .join(format!("{}.json", journal.id));
        if let Some(previous) = &journal.previous {
            let backup = root.join(format!(".qit-backup-{}", journal.id));
            ensure!(
                previous.id == journal.id && previous.path == journal.destination,
                "Invalid extension recovery journal"
            );
            if backup.try_exists()? {
                ensure!(
                    !fs::symlink_metadata(&backup)?.file_type().is_symlink(),
                    "Backup directory was replaced by a symlink"
                );
                ensure!(
                    fs::read_to_string(backup.join(".qit-owner"))? == journal.id,
                    "Backup ownership marker does not match"
                );
                if journal.destination.try_exists()? {
                    ensure!(
                        fs::read_to_string(journal.destination.join(".qit-owner"))? == journal.id
                            && !fs::symlink_metadata(&journal.destination)?
                                .file_type()
                                .is_symlink(),
                        "Installation directory was replaced during recovery"
                    );
                    fs::remove_dir_all(&journal.destination)?;
                }
                fs::rename(backup, &journal.destination)?;
            }
            atomic_json(&record, previous)?;
            if stage.try_exists()? {
                fs::remove_dir_all(&stage)?;
            }
            fs::remove_file(path)?;
            recovered.push(journal.id);
            continue;
        }
        if fs::read_to_string(journal.destination.join(".qit-owner"))
            .ok()
            .as_deref()
            == Some(&journal.id)
            && record.exists()
        {
            let mut installation: Installation = serde_json::from_slice(&fs::read(&record)?)?;
            installation.state = "installed".into();
            atomic_json(&record, &installation)?;
        } else if !journal.destination.exists() && record.exists() {
            fs::remove_file(&record)?;
        }
        if let Ok(meta) = fs::symlink_metadata(&stage) {
            ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "Staging directory was replaced"
            );
            fs::remove_dir_all(stage)?;
        }
        fs::remove_file(path)?;
        recovered.push(journal.id);
    }
    Ok(recovered)
}
pub fn list() -> Result<Vec<Installation>> {
    let directory = data_dir().join("installations");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for file in fs::read_dir(directory)? {
        let path = file?.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            result.push(
                serde_json::from_slice(&fs::read(&path)?)
                    .with_context(|| format!("Invalid installation record: {}", path.display()))?,
            );
        }
    }
    Ok(result)
}
fn walk(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(root, &path, output)?;
        } else {
            output.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}
fn files(root: &Path) -> Result<Vec<FileRecord>> {
    let mut paths = Vec::new();
    walk(root, root, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|relative| {
            let path = root.join(&relative);
            let metadata = fs::symlink_metadata(&path)?;
            let link = metadata.file_type().is_symlink();
            ensure!(
                link || metadata.is_file(),
                "Unsupported file type: {}",
                path.display()
            );
            Ok(FileRecord {
                path: relative
                    .to_str()
                    .context("Non UTF-8 install path")?
                    .replace('\\', "/"),
                sha256: if link {
                    None
                } else {
                    Some(hash(&path, "sha256")?)
                },
                link: if link {
                    Some(
                        fs::read_link(&path)?
                            .to_str()
                            .context("Non UTF-8 link")?
                            .into(),
                    )
                } else {
                    None
                },
            })
        })
        .collect()
}
fn lock(root: &Path) -> Result<fs::File> {
    fs::create_dir_all(root)?;
    let path = root.join(".qit.lock");
    ensure!(
        !fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false),
        "Invalid installation lock"
    );
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock()
        .context("Another Qit operation is using this directory")?;
    Ok(file)
}
pub async fn install(
    plan: Plan,
    root: &Path,
    name: &str,
    cancel: Cancel,
    progress: Progress,
) -> Result<Installation> {
    archive::safe_relative(name)?;
    ensure!(
        !name.contains('/') && !name.starts_with('.'),
        "Installation name must be one visible directory name"
    );
    let _lock = lock(root)?;
    let root = fs::canonicalize(root)?;
    recover_locked(&root)?;
    let destination = root.join(name);
    ensure!(
        !destination.exists() && fs::symlink_metadata(&destination).is_err(),
        "Destination already exists: {}",
        destination.display()
    );
    let id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let stage = root.join(format!(".qit-stage-{id}"));
    fs::create_dir(&stage)?;
    let journal = data_dir().join("transactions").join(format!("{id}.json"));
    let _stage_guard = StageGuard {
        stage: stage.clone(),
        journal: journal.clone(),
    };
    atomic_json(
        &journal,
        &Journal {
            id: id.clone(),
            root: root.clone(),
            destination: destination.clone(),
            previous: None,
        },
    )?;
    let result = install_inner(
        &plan,
        &stage,
        &destination,
        cancel.clone(),
        progress.clone(),
    )
    .await;
    let checksums = match result {
        Ok(sums) => sums,
        Err(error) => {
            let _ = fs::remove_dir_all(&stage);
            return Err(error);
        }
    };
    if let Err(error) = cancel.check() {
        fs::remove_dir_all(&stage)?;
        return Err(error);
    }
    report(&progress, "record", "Recording installed files", 0, None);
    let records = match files(&stage) {
        Ok(records) => records,
        Err(error) => {
            let _ = fs::remove_dir_all(&stage);
            return Err(error);
        }
    };
    cancel.check()?;
    let mut installation = Installation {
        schema: 1,
        id: id.clone(),
        path: destination.clone(),
        plan,
        files: records,
        checksums,
        state: "pending".into(),
    };
    let record = data_dir().join("installations").join(format!("{id}.json"));
    atomic_json(&record, &installation)?;
    fs::write(stage.join(".qit-owner"), &id)?;
    if let Err(error) = fs::rename(&stage, &destination) {
        let _ = fs::remove_file(&record);
        let _ = fs::remove_dir_all(stage);
        return Err(error.into());
    }
    installation.state = "installed".into();
    atomic_json(&record, &installation)?;
    fs::remove_file(&journal)?;
    report(
        &progress,
        "complete",
        destination.display().to_string(),
        1,
        Some(1),
    );
    Ok(installation)
}
async fn install_inner(
    plan: &Plan,
    stage: &Path,
    destination: &Path,
    cancel: Cancel,
    progress: Progress,
) -> Result<BTreeMap<String, Checksum>> {
    let net = Network::new(&plan.mirror, cancel.clone(), progress.clone())?;
    let cache = data_dir().join("cache");
    fs::create_dir_all(&cache)?;
    let mut jobs = tokio::task::JoinSet::new();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    for package in &plan.packages {
        ensure!(
            package.unsupported.is_none(),
            "Unsupported package: {}",
            package.id
        );
        for item in &package.archives {
            let item = item.clone();
            let net = net.clone();
            let semaphore = semaphore.clone();
            let cache = cache.clone();
            jobs.spawn(async move {
                let _permit = semaphore.acquire_owned().await?;
                net.cancel.check()?;
                let sum = net.checksum(&item.path).await?;
                let key = hex::encode(Sha256::digest(
                    format!("{}:{}:{}", net.base, item.path, sum.value).as_bytes(),
                ));
                let path = cache.join(key);
                net.download(&item.path, &path, &sum).await?;
                Ok::<_, anyhow::Error>((item, path, sum))
            });
        }
    }
    let mut downloaded = Vec::new();
    while let Some(job) = jobs.join_next().await {
        downloaded.push(job??);
    }
    // Stable overlay precedence, independent of download completion order.
    downloaded.sort_by(|a, b| a.0.path.cmp(&b.0.path));
    let mut sums = BTreeMap::new();
    for (index, (item, file, sum)) in downloaded.iter().enumerate() {
        cancel.check()?;
        report(
            &progress,
            "extract",
            &item.path,
            index as u64,
            Some(downloaded.len() as u64),
        );
        let relative = if item.destination.is_empty() {
            PathBuf::new()
        } else {
            archive::safe_relative(&item.destination)?
        };
        archive::ensure_parents(stage, &relative)?;
        for replaced in archive::extract_overlay(file, &stage.join(relative), &cancel)? {
            report(
                &progress,
                "extract",
                format!(
                    "{} replaced {}",
                    item.path,
                    replaced.strip_prefix(stage).unwrap_or(&replaced).display()
                ),
                index as u64,
                Some(downloaded.len() as u64),
            );
        }
        sums.insert(item.path.clone(), sum.clone());
    }
    report(&progress, "configure", "Configuring Qt paths", 0, None);
    configure(stage, destination, &cancel)?;
    Ok(sums)
}

pub fn configure(stage: &Path, final_path: &Path, cancel: &Cancel) -> Result<()> {
    let mut paths = Vec::new();
    walk(stage, stage, &mut paths)?;
    let qt_roots: Vec<_> = paths
        .iter()
        .filter(|p| p.ends_with("mkspecs/qconfig.pri"))
        .map(|p| p.parent().unwrap().parent().unwrap().to_path_buf())
        .collect();
    for relative in &qt_roots {
        cancel.check()?;
        let prefix = stage.join(relative);
        let final_prefix = qt_path(&final_path.join(relative));
        let cross = prefix.join("bin/target_qt.conf").exists();
        if !cross {
            fs::create_dir_all(prefix.join("bin"))?;
            fs::write(prefix.join("bin/qt.conf"), "[Paths]\nPrefix=..\n")?;
        }
        let host = if cross {
            qt_roots
                .iter()
                .find(|r| {
                    r.parent() == relative.parent()
                        && !stage.join(r).join("bin/target_qt.conf").exists()
                })
                .cloned()
                .context("Cross target has no matching host Qt")?
        } else {
            relative.clone()
        };
        let host_prefix = qt_path(&final_path.join(&host));
        let mut local = Vec::new();
        walk(&prefix, &prefix, &mut local)?;
        for file in local {
            cancel.check()?;
            let path = prefix.join(&file);
            if fs::symlink_metadata(&path)?.file_type().is_symlink() {
                continue;
            }
            let extension = path.extension().and_then(|v| v.to_str()).unwrap_or("");
            let basename = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
            if !["pc", "prl", "la", "cmake", "pri", "conf", "bat", "sh"].contains(&extension)
                && !["qmake", "qmake6", "qtpaths", "qtpaths6"].contains(&basename)
                && !basename.starts_with("qt-")
            {
                continue;
            }
            if fs::metadata(&path)?.len() > 4 * 1024 * 1024 {
                continue;
            }
            let Ok(original) = fs::read_to_string(&path) else {
                continue;
            };
            if original.contains('\0') {
                continue;
            }
            let mut text = original.clone();
            for build_prefix in [
                "/home/qt/work/install",
                "/Users/qt/work/install",
                "C:/Users/qt/work/install",
                "c:/Users/qt/work/install",
                "\\home\\qt\\work\\install",
                "\\Users\\qt\\work\\install",
            ] {
                text = text.replace(build_prefix, &final_prefix);
            }
            if cross
                && (basename.starts_with("qmake")
                    || basename.starts_with("qtpaths")
                    || basename.starts_with("host-qmake")
                    || basename.starts_with("host-qtpaths"))
            {
                let program = basename
                    .trim_start_matches("host-")
                    .trim_end_matches(".bat");
                if ["qmake", "qmake6", "qtpaths", "qtpaths6"].contains(&program) {
                    if basename.ends_with(".bat") {
                        text = format!(
                            "@echo off\r\n\"{host_prefix}/bin/{program}.exe\" -qtconf \"%~dp0target_qt.conf\" %*\r\n"
                        );
                    } else {
                        let executable =
                            format!("{host_prefix}/bin/{program}").replace('\'', "'\\''");
                        text = format!(
                            "#!/bin/sh\nscript_dir=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\nexec '{executable}' -qtconf \"$script_dir/target_qt.conf\" \"$@\"\n"
                        );
                    }
                }
            }
            if basename == "target_qt.conf" {
                text = text
                    .lines()
                    .map(|line| {
                        if line.starts_with("HostPrefix=") {
                            format!("HostPrefix={host_prefix}")
                        } else if line.starts_with("HostData=") {
                            format!("HostData={final_prefix}")
                        } else if line.starts_with("HostLibraryExecutables=") {
                            format!(
                                "HostLibraryExecutables={}",
                                if cfg!(windows) { "bin" } else { "libexec" }
                            )
                        } else {
                            line.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n";
            }
            if relative.to_string_lossy().starts_with("5.15") && basename == "qconfig.pri" {
                text = text
                    .lines()
                    .map(|line| {
                        if line.starts_with("QT_EDITION =") {
                            "QT_EDITION = OpenSource"
                        } else if line.starts_with("QT_LICHECK =") {
                            "QT_LICHECK ="
                        } else {
                            line
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n";
            }
            if original != text {
                fs::write(&path, &text)?;
            }
            #[cfg(unix)]
            if file.starts_with("bin") && text.starts_with("#!") {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(&path)?.permissions().mode();
                fs::set_permissions(&path, fs::Permissions::from_mode(mode | 0o111))?;
            }
        }
    }
    fs::write(stage.join("QIT-ENVIRONMENT.txt"), qt_roots.iter().map(|p| format!("Qt prefix: {}\nAdd its bin directory to PATH, and use it as CMAKE_PREFIX_PATH.\n", final_path.join(p).display())).collect::<Vec<_>>().join("\n"))?;
    Ok(())
}
fn qt_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    if let Some(unc) = path.strip_prefix("\\\\?\\UNC\\") {
        format!("//{}", unc.replace('\\', "/"))
    } else {
        path.strip_prefix("\\\\?\\")
            .unwrap_or(&path)
            .replace('\\', "/")
    }
}
pub fn uninstall(id: &str, cancel: &Cancel, progress: &Progress) -> Result<Vec<String>> {
    let installation = list()?
        .into_iter()
        .find(|i| i.id == id)
        .context("Unknown installation")?;
    let root = &installation.path;
    cancel.check()?;
    // Check before locking: lock() creates the parent directory, which may
    // itself have been deleted along with this installation.
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::remove_file(
                data_dir()
                    .join("installations")
                    .join(format!("{}.json", installation.id)),
            )?;
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.into()),
        Ok(_) => (),
    }
    let _lock = lock(root.parent().context("Invalid installation root")?)?;
    ensure!(
        !fs::symlink_metadata(root)?.file_type().is_symlink(),
        "Installation directory was replaced by a symlink"
    );
    ensure!(
        fs::read_to_string(root.join(".qit-owner"))? == installation.id,
        "Installation ownership marker does not match"
    );
    let mut preserved = Vec::new();
    for (index, file) in installation.files.iter().enumerate() {
        cancel.check()?;
        let relative = archive::safe_relative(&file.path)?;
        let mut current = root.to_path_buf();
        let mut unsafe_parent = false;
        for component in relative.parent().unwrap_or(Path::new("")).components() {
            current.push(component);
            if fs::symlink_metadata(&current)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                unsafe_parent = true;
                break;
            }
        }
        if unsafe_parent {
            preserved.push(file.path.clone());
            continue;
        }
        let path = root.join(relative);
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let unchanged = if let Some(link) = &file.link {
            meta.file_type().is_symlink() && fs::read_link(&path)?.to_str() == Some(link)
        } else {
            meta.is_file()
                && !meta.file_type().is_symlink()
                && Some(hash(&path, "sha256")?) == file.sha256
        };
        if unchanged {
            fs::remove_file(&path)?;
        } else {
            preserved.push(file.path.clone());
        }
        report(
            progress,
            "uninstall",
            &file.path,
            index as u64 + 1,
            Some(installation.files.len() as u64),
        );
    }
    fs::remove_file(root.join(".qit-owner"))?;
    remove_empty(root)?;
    fs::remove_file(
        data_dir()
            .join("installations")
            .join(format!("{}.json", installation.id)),
    )?;
    if root.exists() {
        preserved.push(format!("Additional files remain in {}", root.display()));
    }
    Ok(preserved)
}
fn remove_empty(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_empty(&entry.path())?;
        }
    }
    match fs::remove_dir(root) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => (),
        Err(e) => return Err(e.into()),
    };
    Ok(())
}

/// Recognize a Qit destination without parsing the full installation record on each UI tick.
pub fn managed_destination(path: &Path) -> bool {
    if fs::symlink_metadata(path).is_err()
        || fs::symlink_metadata(path).is_ok_and(|m| !m.is_dir() || m.file_type().is_symlink())
    {
        return false;
    }
    let Ok(id) = fs::read_to_string(path.join(".qit-owner")) else {
        return false;
    };
    archive::safe_relative(&id).is_ok()
        && !id.contains('/')
        && data_dir()
            .join("installations")
            .join(format!("{id}.json"))
            .is_file()
}

pub fn at_path(path: &Path) -> Result<Option<Installation>> {
    if fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(None);
    }
    ensure!(
        managed_destination(path),
        "This directory is not a Qit-managed installation"
    );
    let id = fs::read_to_string(path.join(".qit-owner"))?;
    let installation: Installation = serde_json::from_slice(&fs::read(
        data_dir().join("installations").join(format!("{id}.json")),
    )?)?;
    ensure!(
        installation.id == id
            && installation.state == "installed"
            && installation.path == fs::canonicalize(path)?,
        "Installation record does not match destination"
    );
    Ok(Some(installation))
}

/// Existing packages are pinned: adding components is not an implicit upgrade.
pub fn merge_plan(old: &Installation, mut new: Plan) -> Result<Plan> {
    fn platform(package: &crate::catalog::Package) -> Option<&str> {
        package
            .repository
            .strip_prefix("online/qtsdkrepository/")?
            .split('/')
            .next()
            .filter(|host| *host != "all_os")
    }
    let hosts: std::collections::BTreeSet<_> =
        old.plan.packages.iter().filter_map(platform).collect();
    ensure!(
        hosts.is_empty()
            || new
                .packages
                .iter()
                .filter_map(platform)
                .all(|host| hosts.contains(host)),
        "These components target a different system than the existing installation; choose a separate folder"
    );
    for package in &old.plan.packages {
        if let Some(candidate) = new.packages.iter().find(|p| p.id == package.id) {
            ensure!(
                candidate.version == package.version
                    && candidate
                        .archives
                        .iter()
                        .map(|a| (&a.path, &a.destination))
                        .eq(package.archives.iter().map(|a| (&a.path, &a.destination))),
                "Installed component {} differs from the requested version; use a separate installation",
                package.id
            );
        } else {
            new.packages.push(package.clone());
        }
    }
    for id in &old.plan.selected {
        if !new.selected.contains(id) {
            new.selected.push(id.clone());
        }
    }
    new.packages.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(new)
}

fn same_file(a: &FileRecord, b: &FileRecord) -> bool {
    a.sha256 == b.sha256 && a.link == b.link
}

fn copy_user_entry(source: &Path, target: &Path) -> Result<()> {
    if fs::symlink_metadata(source)?.file_type().is_symlink() {
        let link = fs::read_link(source)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(link, target)?;
        #[cfg(windows)]
        if source.is_dir() {
            std::os::windows::fs::symlink_dir(link, target)?;
        } else {
            std::os::windows::fs::symlink_file(link, target)?;
        }
    } else {
        ensure!(
            fs::symlink_metadata(source)?.is_file(),
            "Unsupported user file type"
        );
        fs::copy(source, target)?;
    }
    Ok(())
}

fn preserve_directories(
    source: &Path,
    stage: &Path,
    relative: &Path,
    cancel: &Cancel,
) -> Result<()> {
    for entry in fs::read_dir(source.join(relative))? {
        cancel.check()?;
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let child = relative.join(entry.file_name());
            archive::ensure_parents(stage, &child)?;
            preserve_directories(source, stage, &child, cancel)?;
        }
    }
    Ok(())
}

// Preserve user additions, edits and deletions without claiming ownership of them.
fn preserve_user_files(
    old: &Installation,
    stage: &Path,
    baseline: &[FileRecord],
    cancel: &Cancel,
) -> Result<()> {
    let current = files(&old.path)?;
    let originals: BTreeMap<_, _> = old.files.iter().map(|p| (p.path.as_str(), p)).collect();
    let fresh_files: BTreeMap<_, _> = baseline.iter().map(|p| (p.path.as_str(), p)).collect();
    let current_files: BTreeMap<_, _> = current.iter().map(|p| (p.path.as_str(), p)).collect();
    for file in &current {
        cancel.check()?;
        if file.path == ".qit-owner" {
            continue;
        }
        let original = originals.get(file.path.as_str()).copied();
        if original.is_some_and(|p| same_file(p, file)) {
            continue;
        }
        let fresh = fresh_files.get(file.path.as_str()).copied();
        ensure!(
            fresh.is_none() || original.zip(fresh).is_some_and(|(a, b)| same_file(a, b)),
            "New components conflict with a user-modified or untracked file: {}",
            file.path
        );
        let relative = archive::safe_relative(&file.path)?;
        archive::ensure_parents(stage, relative.parent().unwrap_or(Path::new("")))?;
        let target = stage.join(&relative);
        if fs::symlink_metadata(&target).is_ok() {
            ensure!(
                !fs::symlink_metadata(&target)?.is_dir(),
                "User file conflicts with a new directory: {}",
                file.path
            );
            fs::remove_file(&target)?;
        }
        copy_user_entry(&old.path.join(relative), &target)?;
    }
    for file in &old.files {
        cancel.check()?;
        if current_files.contains_key(file.path.as_str()) {
            continue;
        }
        if let Some(fresh) = fresh_files.get(file.path.as_str()).copied() {
            ensure!(
                same_file(file, fresh),
                "New components conflict with a user-deleted file: {}",
                file.path
            );
            let relative = archive::safe_relative(&file.path)?;
            // Do not follow a user symlink installed above.
            let mut parent = stage.to_path_buf();
            let mut safe = true;
            for part in relative.parent().unwrap_or(Path::new("")).components() {
                parent.push(part);
                if fs::symlink_metadata(&parent).is_ok_and(|m| m.file_type().is_symlink()) {
                    safe = false;
                    break;
                }
            }
            if safe && fs::symlink_metadata(stage.join(&relative)).is_ok() {
                fs::remove_file(stage.join(relative))?;
            }
        }
    }
    preserve_directories(&old.path, stage, Path::new(""), cancel)?;
    ensure!(
        current == files(&old.path)?,
        "Installation files changed while adding components; close Qt tools and retry"
    );
    Ok(())
}

/// Rebuild the merged SDK in staging using cached archives, then swap directories.
/// The old SDK and record are restored by recovery if the swap is interrupted.
pub async fn extend(
    plan: Plan,
    expected: &Installation,
    cancel: Cancel,
    progress: Progress,
) -> Result<Installation> {
    let root = expected
        .path
        .parent()
        .context("Invalid installation path")?;
    let _lock = lock(root)?;
    recover_locked(root)?;
    let old = at_path(&expected.path)?.context("Installation no longer exists")?;
    ensure!(
        serde_json::to_vec(&old)? == serde_json::to_vec(expected)?,
        "Installation changed since review; prepare the plan again"
    );
    let plan = merge_plan(&old, plan)?;
    ensure!(
        plan.packages
            .iter()
            .any(|p| !old.plan.packages.iter().any(|o| o.id == p.id)),
        "All selected components are already installed"
    );
    let stage = root.join(format!(".qit-stage-{}", old.id));
    let backup = root.join(format!(".qit-backup-{}", old.id));
    ensure!(
        !stage.try_exists()? && !backup.try_exists()?,
        "Unrecovered extension staging or backup exists"
    );
    let journal = data_dir()
        .join("transactions")
        .join(format!("{}.json", old.id));
    let record = data_dir()
        .join("installations")
        .join(format!("{}.json", old.id));
    fs::create_dir(&stage)?;
    if let Err(error) = atomic_json(
        &journal,
        &Journal {
            id: old.id.clone(),
            root: root.into(),
            destination: old.path.clone(),
            previous: Some(old.clone()),
        },
    ) {
        let _ = fs::remove_dir_all(&stage);
        return Err(error);
    }
    let prepared = async {
        let checksums =
            install_inner(&plan, &stage, &old.path, cancel.clone(), progress.clone()).await?;
        for (path, expected) in &old.checksums {
            if let Some(actual) = checksums.get(path) {
                ensure!(
                    actual.algorithm == expected.algorithm && actual.value == expected.value,
                    "Previously installed archive changed: {path}"
                );
            }
        }
        let baseline = files(&stage)?;
        preserve_user_files(&old, &stage, &baseline, &cancel)?;
        fs::write(stage.join(".qit-owner"), &old.id)?;
        cancel.check()?;
        Ok::<_, anyhow::Error>(Installation {
            schema: old.schema,
            id: old.id.clone(),
            path: old.path.clone(),
            plan,
            files: baseline,
            checksums,
            state: "installed".into(),
        })
    }
    .await;
    let updated = match prepared {
        Ok(value) => value,
        Err(error) => {
            recover_locked(root).context("Failed to clean extension staging")?;
            return Err(error);
        }
    };
    let committed = (|| -> Result<()> {
        fs::rename(&old.path, &backup)?;
        fs::rename(&stage, &old.path)?;
        atomic_json(&record, &updated)?;
        fs::remove_file(&journal)?; // Commit marker: recovery rolls back until this succeeds.
        Ok(())
    })();
    if let Err(error) = committed {
        recover_locked(root).context("Extension failed; automatic rollback also failed")?;
        return Err(error);
    }
    // A cleanup failure must not report a successfully committed extension as failed.
    if let Err(error) = fs::remove_dir_all(&backup) {
        report(
            &progress,
            "cleanup",
            format!("Could not remove backup {}: {error}", backup.display()),
            0,
            None,
        );
    }
    report(
        &progress,
        "complete",
        old.path.display().to_string(),
        1,
        Some(1),
    );
    Ok(updated)
}
