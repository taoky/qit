use std::{env, path::PathBuf, process::Command};

fn main() {
    let data = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../data");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("qit.gresource");
    for file in ["qit.gresource.xml", "io.github.taoky.Qit.svg"] {
        println!("cargo:rerun-if-changed={}", data.join(file).display());
    }
    let status = Command::new("glib-compile-resources")
        .arg(data.join("qit.gresource.xml"))
        .arg("--sourcedir")
        .arg(&data)
        .arg("--target")
        .arg(output)
        .status()
        .expect("glib-compile-resources is required to embed the application icon");
    assert!(status.success(), "Failed to compile Qit resources");
}
