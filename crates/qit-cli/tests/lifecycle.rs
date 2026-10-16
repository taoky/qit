use qit_core::{
    catalog::Plan,
    install::{FileRecord, Installation},
    network::hash,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn cli(data: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_qit-cli"))
        .env("QIT_DATA_DIR", data)
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn uninstall_preserves_user_changes_and_recovery_only_cleans_owned_staging() {
    let temp = Temp(std::env::temp_dir().join(format!("qit-cli-test-{}", std::process::id())));
    let data = temp.0.join("data");
    let root = temp.0.join("root");
    let sdk = root.join("sdk");
    fs::create_dir_all(&sdk).unwrap();
    fs::create_dir_all(data.join("installations")).unwrap();
    fs::write(sdk.join("unchanged"), b"original").unwrap();
    fs::write(sdk.join("changed"), b"original").unwrap();
    fs::write(sdk.join(".qit-owner"), "test").unwrap();
    fs::write(sdk.join("extra"), "user file").unwrap();
    let files = ["unchanged", "changed"]
        .iter()
        .map(|name| FileRecord {
            path: name.to_string(),
            sha256: Some(hash(&sdk.join(name), "sha256").unwrap()),
            link: None,
        })
        .collect();
    let installation = Installation {
        schema: 1,
        id: "test".into(),
        path: sdk.clone(),
        plan: Plan {
            mirror: "https://example.org/".into(),
            selected: Vec::new(),
            packages: Vec::new(),
            notes: Vec::new(),
            license_texts: BTreeMap::new(),
        },
        files,
        checksums: BTreeMap::new(),
        state: "installed".into(),
    };
    fs::write(
        data.join("installations/test.json"),
        serde_json::to_vec(&installation).unwrap(),
    )
    .unwrap();
    fs::write(sdk.join("changed"), "modified").unwrap();
    let output = cli(&data, &["uninstall", "test", "--yes"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!sdk.join("unchanged").exists());
    assert_eq!(fs::read_to_string(sdk.join("changed")).unwrap(), "modified");
    assert!(sdk.join("extra").exists());
    assert_eq!(
        String::from_utf8(cli(&data, &["list"]).stdout)
            .unwrap()
            .trim(),
        "[]"
    );
    let root = fs::canonicalize(root).unwrap();
    let stage = root.join(".qit-stage-test");
    let unrelated = root.join(".qit-stage-unrelated");
    fs::create_dir(&stage).unwrap();
    fs::create_dir(&unrelated).unwrap();
    fs::write(stage.join("partial"), "partial").unwrap();
    fs::create_dir_all(data.join("transactions")).unwrap();
    fs::write(
        data.join("transactions/test.json"),
        serde_json::to_vec(
            &serde_json::json!({"id":"test", "root":root, "destination": root.join("new-sdk")}),
        )
        .unwrap(),
    )
    .unwrap();
    let output = cli(&data, &["recover", "--root", root.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!stage.exists());
    assert!(unrelated.exists());
}
