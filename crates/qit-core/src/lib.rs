pub mod archive;
pub mod catalog;
pub mod install;
pub mod network;
pub mod settings;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const USER_AGENT: &str = concat!("Qit/", env!("CARGO_PKG_VERSION"));
pub const ISSUE_URL: &str = "https://qt-project.atlassian.net/browse/QTBUG-129057";

use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Default)]
pub struct Cancel(pub Arc<AtomicBool>);
impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn check(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.0.load(Ordering::Relaxed), "Cancelled");
        Ok(())
    }
    pub async fn cancelled(&self) {
        while self.check().is_ok() {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub phase: String,
    pub message: String,
    pub done: u64,
    pub total: Option<u64>,
}
pub type Progress = Arc<dyn Fn(Event) + Send + Sync>;
pub fn report(
    p: &Progress,
    phase: &str,
    message: impl Into<String>,
    done: u64,
    total: Option<u64>,
) {
    p(Event {
        phase: phase.into(),
        message: message.into(),
        done,
        total,
    });
}
