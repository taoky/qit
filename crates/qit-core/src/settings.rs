use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Mirror {
    pub country: String,
    pub operator: String,
    pub url: String,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct Mirrors {
    pub source: String,
    pub fetched_at: String,
    pub mirrors: Vec<Mirror>,
}
pub fn mirrors() -> Mirrors {
    serde_json::from_str(include_str!("../../../data/mirrors.json"))
        .expect("bundled mirror snapshot")
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub mirror: Option<String>,
    pub install_root: Option<PathBuf>,
    pub language: Option<String>,
}
pub fn data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("QIT_DATA_DIR") {
        return path.into();
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or(home)
            .join("Qit");
    }
    if cfg!(target_os = "macos") {
        return home.join("Library/Application Support/Qit");
    }
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))
        .join("qit")
}
pub fn default_root() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(data_dir)
        .join("Qit")
}
impl Settings {
    pub fn load() -> Result<Self> {
        let path = data_dir().join("settings.json");
        if !path.exists() {
            return Ok(Self::default());
        }
        serde_json::from_slice(&fs::read(path)?).context("Read settings")
    }
    pub fn save(&self) -> Result<()> {
        atomic_json(&data_dir().join("settings.json"), self)
    }
}
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    fs::create_dir_all(path.parent().context("Missing parent")?)?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp)?;
    serde_json::to_writer_pretty(&file, value)?;
    file.sync_all()?;
    drop(file);
    #[cfg(not(windows))]
    fs::rename(&temp, path)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(source: *const u16, target: *const u16, flags: u32) -> i32;
        }
        let source: Vec<_> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0x9) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}
