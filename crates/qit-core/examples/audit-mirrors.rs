//! Explicit live audit: cargo run -p qit-core --example audit-mirrors > report.json
use qit_core::{Cancel, catalog, listing, network::Network, settings};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

fn connection(error: &anyhow::Error) -> bool {
    error.chain().any(|e| {
        e.downcast_ref::<reqwest::Error>()
            .is_some_and(|e| e.is_connect() || e.is_timeout())
            || e.is::<tokio::time::error::Elapsed>()
    })
}
#[tokio::main]
async fn main() {
    let mut jobs = Vec::new();
    for mirror in settings::mirrors().mirrors {
        jobs.push(tokio::spawn(async move {
            let events = Arc::new(Mutex::new(Vec::new()));
            let sink = events.clone();
            let net = Network::new(&mirror.url, Cancel::default(), Arc::new(move |e| sink.lock().unwrap().push(e))).unwrap();
            let mut rows = Vec::<Value>::new();
            for group in ["linux_x64/desktop", "linux_x64/android", "linux_arm64/desktop", "linux_arm64/android", "windows_x86/desktop", "windows_x86/android", "windows_arm64/desktop", "windows_arm64/android", "mac_x64/desktop", "mac_x64/android", "all_os/android", "all_os/wasm", "all_os/qt"] {
                let root = format!("online/qtsdkrepository/{group}/");
                let start = Instant::now();
                events.lock().unwrap().clear();
                let result = listing::directories(&net, &root).await;
                let mut names = Vec::new();
                let status = match &result {
                    Ok(Some(value)) => { names = value.clone(); "pass" },
                    Ok(None) => "missing",
                    Err(e) if connection(e) => "skipped-connection",
                    Err(_) => "failed",
                };
                rows.push(json!({"path":root,"kind":"listing","status":status,"elapsed_ms":start.elapsed().as_millis(),"directories":names.len(),"error":result.as_ref().err().map(|e|format!("{e:#}")),"events":*events.lock().unwrap()}));
                if status == "skipped-connection" && group == "linux_x64/desktop" { break; }
                let choices = if group == "all_os/qt" { ["qt6_681_unix_line_endings_src", "qt6_681"] } else if group.starts_with("linux_arm64") || group.starts_with("windows_arm64") { ["qt6_6120", "qt6_681"] } else { ["qt6_681", "qt6_6120"] };
                let mut samples: Vec<_> = choices.into_iter().filter(|n| names.iter().any(|v|v==n)).take(1).collect();
                if group == "linux_x64/desktop" && names.iter().any(|v|v=="tools_cmake") { samples.push("tools_cmake"); }
                for name in samples {
                    let path = format!("{root}{name}/");
                    let start = Instant::now();
                    events.lock().unwrap().clear();
                    let result = async {
                        if let Some(xml) = net.optional_text(&format!("{path}Updates.xml")).await? {
                            return Ok::<_, anyhow::Error>(catalog::parse(&xml, &path)?.len());
                        }
                        rows.push(json!({"path":format!("{path}Updates.xml"),"status":"expected-parent-404"}));
                        let children = listing::directories(&net, &path).await?.ok_or_else(||anyhow::anyhow!("Missing parent directory"))?;
                        let mut count = 0;
                        for child in children {
                            let leaf = format!("{path}{child}/");
                            count += catalog::parse(&net.text(&format!("{leaf}Updates.xml")).await?, &leaf)?.len();
                        }
                        anyhow::ensure!(count > 0, "No packages");
                        Ok(count)
                    }.await;
                    rows.push(json!({"path":path,"kind":"packages","status":match &result {Ok(_) => "pass",Err(e) if connection(e)=>"skipped-connection",Err(_)=>"failed"},"packages":result.as_ref().ok(),"elapsed_ms":start.elapsed().as_millis(),"error":result.err().map(|e|format!("{e:#}")),"events":*events.lock().unwrap()}));
                }
            }
            eprintln!("{}: {} checks", mirror.operator, rows.len());
            json!({"mirror":mirror,"checks":rows})
        }));
    }
    let mut results = Vec::new();
    for job in jobs {
        results.push(job.await.unwrap());
    }
    println!("{}", serde_json::to_string_pretty(&json!({"timestamp":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),"results":results})).unwrap());
}
