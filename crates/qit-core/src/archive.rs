//! A small libarchive binding: entry-by-entry extraction, never archive_write_disk.
use crate::Cancel;
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::{CStr, c_char, c_int, c_void},
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[link(name = "archive")]
unsafe extern "C" {
    fn archive_read_new() -> *mut c_void;
    fn archive_read_support_filter_all(a: *mut c_void) -> c_int;
    fn archive_read_support_format_all(a: *mut c_void) -> c_int;
    #[cfg(not(windows))]
    fn archive_read_open_filename(a: *mut c_void, filename: *const c_char, block: usize) -> c_int;
    #[cfg(windows)]
    fn archive_read_open_filename_w(a: *mut c_void, filename: *const u16, block: usize) -> c_int;
    fn archive_read_next_header(a: *mut c_void, entry: *mut *mut c_void) -> c_int;
    fn archive_read_data(a: *mut c_void, buffer: *mut c_void, size: usize) -> isize;
    fn archive_read_free(a: *mut c_void) -> c_int;
    fn archive_error_string(a: *mut c_void) -> *const c_char;
    fn archive_entry_pathname_utf8(e: *mut c_void) -> *const c_char;
    fn archive_entry_symlink_utf8(e: *mut c_void) -> *const c_char;
    fn archive_entry_hardlink_utf8(e: *mut c_void) -> *const c_char;
    fn archive_entry_filetype(e: *mut c_void) -> u32;
    fn archive_entry_perm(e: *mut c_void) -> u32;
}
struct Reader(*mut c_void);
impl Drop for Reader {
    fn drop(&mut self) {
        unsafe {
            archive_read_free(self.0);
        }
    }
}
fn string(pointer: *const c_char) -> Result<String> {
    ensure!(!pointer.is_null(), "Missing or invalid UTF-8 archive name");
    Ok(unsafe { CStr::from_ptr(pointer) }.to_str()?.into())
}
impl Reader {
    fn check(&self, result: isize) -> Result<()> {
        if result < 0 {
            bail!(
                "Archive error: {}",
                string(unsafe { archive_error_string(self.0) }).unwrap_or_default()
            );
        }
        Ok(())
    }
}
pub fn safe_relative(value: &str) -> Result<PathBuf> {
    ensure!(
        !value.is_empty() && !value.contains(['\\', ':', '\0']) && !value.starts_with('/'),
        "Unsafe relative path: {value}"
    );
    let path = Path::new(value);
    for part in path.components() {
        ensure!(
            matches!(part, Component::Normal(_) | Component::CurDir),
            "Unsafe relative path: {value}"
        );
    }
    for part in value.split('/') {
        ensure!(
            !part.ends_with([' ', '.']) || part == ".",
            "Ambiguous archive path: {value}"
        );
        let stem = part.split('.').next().unwrap_or("").to_uppercase();
        ensure!(
            ![
                "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
                "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
                "LPT9"
            ]
            .contains(&stem.as_str()),
            "Reserved archive path: {value}"
        );
    }
    Ok(path.into())
}
pub fn ensure_parents(root: &Path, relative: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    ensure!(
        !fs::symlink_metadata(root)?.file_type().is_symlink(),
        "Installation root is a symlink"
    );
    for part in relative.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) => ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "Unsafe archive parent: {}",
                current.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&current)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
pub fn extract(source: &Path, root: &Path, cancel: &Cancel) -> Result<()> {
    extract_inner(source, root, cancel, false).map(|_| ())
}

/// Overlay regular files in a private installation staging directory.
/// Remove the old entry before writing, so hard-linked files are not modified in place.
/// Returns replaced paths for the installation log. Other extraction stays strict.
pub fn extract_overlay(source: &Path, root: &Path, cancel: &Cancel) -> Result<Vec<PathBuf>> {
    extract_inner(source, root, cancel, true)
}

fn extract_inner(
    source: &Path,
    root: &Path,
    cancel: &Cancel,
    overlay: bool,
) -> Result<Vec<PathBuf>> {
    let mut replaced = Vec::new();
    fs::create_dir_all(root)?;
    let reader = Reader(unsafe { archive_read_new() });
    ensure!(!reader.0.is_null(), "Cannot allocate archive reader");
    unsafe {
        archive_read_support_filter_all(reader.0);
        archive_read_support_format_all(reader.0);
    }
    #[cfg(not(windows))]
    {
        let name =
            std::ffi::CString::new(source.to_str().context("Archive cache path is not UTF-8")?)?;
        reader.check(
            unsafe { archive_read_open_filename(reader.0, name.as_ptr(), 65536) } as isize,
        )?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let name: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        reader.check(
            unsafe { archive_read_open_filename_w(reader.0, name.as_ptr(), 65536) } as isize,
        )?;
    }
    let mut links = Vec::new();
    loop {
        cancel.check()?;
        let mut entry = std::ptr::null_mut();
        let result = unsafe { archive_read_next_header(reader.0, &mut entry) };
        if result == 1 {
            break;
        }
        reader.check(result as isize)?;
        let name = string(unsafe { archive_entry_pathname_utf8(entry) })?;
        let relative = safe_relative(name.trim_end_matches('/'))?;
        ensure!(
            !relative
                .components()
                .any(|p| p.as_os_str().to_string_lossy().starts_with(".qit")),
            "Archive contains a reserved Qit file"
        );
        ensure_parents(root, relative.parent().unwrap_or(Path::new("")))?;
        let target = root.join(&relative);
        let kind = unsafe { archive_entry_filetype(entry) };
        let hard = unsafe { archive_entry_hardlink_utf8(entry) };
        if kind == 0o120000 || !hard.is_null() {
            let link = string(if hard.is_null() {
                unsafe { archive_entry_symlink_utf8(entry) }
            } else {
                hard
            })?;
            ensure!(
                !link.contains(['\\', ':', '\0']) && !link.starts_with('/'),
                "Unsafe archive link"
            );
            let mut resolved = if hard.is_null() {
                relative.parent().unwrap_or(Path::new("")).to_path_buf()
            } else {
                PathBuf::new()
            };
            for component in Path::new(&link).components() {
                match component {
                    Component::ParentDir => {
                        ensure!(resolved.pop(), "Archive link escapes destination");
                    }
                    Component::Normal(p) => resolved.push(p),
                    Component::CurDir => (),
                    _ => bail!("Unsafe archive link"),
                }
            }
            links.push((relative, link, resolved, !hard.is_null()));
        } else if kind == 0o040000 {
            ensure_parents(root, &relative)?;
        } else if kind == 0o100000 {
            ensure!(
                !fs::symlink_metadata(&target)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false),
                "Refusing to follow archive symlink"
            );
            let mut existing = target.exists();
            if existing && overlay {
                ensure!(
                    fs::symlink_metadata(&target)?.file_type().is_file(),
                    "Conflicting archive entry type: {}",
                    target.display()
                );
                fs::remove_file(&target)?;
                replaced.push(target.clone());
                existing = false;
            }
            let mut output = if existing {
                fs::File::open(&target)?
            } else {
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)?
            };
            let mut buffer = [0u8; 65536];
            loop {
                cancel.check()?;
                let count = unsafe {
                    archive_read_data(reader.0, buffer.as_mut_ptr().cast(), buffer.len())
                };
                reader.check(count)?;
                if count == 0 {
                    break;
                }
                if existing {
                    let mut old = vec![0u8; count as usize];
                    output
                        .read_exact(&mut old)
                        .context("Conflicting archive files")?;
                    ensure!(
                        old == buffer[..count as usize],
                        "Conflicting archive file: {}",
                        target.display()
                    );
                } else {
                    output.write_all(&buffer[..count as usize])?;
                }
            }
            if existing {
                let mut extra = [0];
                ensure!(
                    output.read(&mut extra)? == 0,
                    "Conflicting archive file length"
                );
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    &target,
                    fs::Permissions::from_mode(unsafe { archive_entry_perm(entry) } & 0o777),
                )?;
            }
        } else {
            bail!("Unsupported archive entry type: {name}");
        }
    }
    for (relative, link, resolved, hard) in links {
        cancel.check()?;
        ensure_parents(root, relative.parent().unwrap_or(Path::new("")))?;
        let path = root.join(relative);
        if fs::symlink_metadata(&path).is_ok() {
            ensure!(
                fs::read_link(&path).ok().as_deref() == Some(Path::new(&link)),
                "Conflicting archive link"
            );
            continue;
        }
        if hard {
            ensure_parents(root, resolved.parent().unwrap_or(Path::new("")))?;
            let original = root.join(resolved);
            ensure!(
                fs::symlink_metadata(&original)?.is_file(),
                "Invalid hardlink target"
            );
            fs::hard_link(original, path)?;
        } else {
            #[cfg(unix)]
            std::os::unix::fs::symlink(link, path)?;
            #[cfg(windows)]
            {
                let original = root.join(resolved);
                if original.is_dir() {
                    std::os::windows::fs::symlink_dir(&link, &path)?;
                } else {
                    std::os::windows::fs::symlink_file(&link, &path)
                        .context("This archive needs Windows Developer Mode for symbolic links")?;
                }
            }
        }
    }
    Ok(replaced)
}
