//! Mirror directory formats. Names are always mapped back onto the selected mirror.
use crate::{archive::safe_relative, catalog::directories_at, network::Network};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::collections::BTreeSet;
use url::Url;

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && !name.contains(['/', '\\', '%', '?', '#', ':', '*', '"', '<', '>', '|'])
        && !name.chars().any(char::is_control)
        && safe_relative(name).is_ok()
}

pub fn parse(text: &str, base: &Url, content_type: &str) -> Result<(Vec<String>, &'static str)> {
    if content_type.contains("json") || text.trim_start().starts_with('[') {
        let rows: Vec<Value> = serde_json::from_str(text).context("Invalid directory JSON")?;
        let mut names = BTreeSet::new();
        for row in rows {
            let name = row["name"]
                .as_str()
                .context("Directory JSON missing name")?;
            let directory = if let Some(value) = row["is_dir"].as_bool() {
                value
            } else {
                match row["type"].as_str() {
                    Some("directory") => true,
                    Some("file" | "link") => false,
                    _ => anyhow::bail!("Unknown directory JSON entry"),
                }
            };
            let name = name.trim_end_matches('/');
            if !directory || !valid_name(name) {
                continue;
            }
            if let Some(href) = row["url"].as_str() {
                if href.contains(['%', '\\']) || href.split('/').any(|p| p == "..") {
                    continue;
                }
                let url = base.join(href)?;
                if url.origin() != base.origin()
                    || url.query().is_some()
                    || url.fragment().is_some()
                    || url.path() != format!("{}{name}/", base.path())
                {
                    continue;
                }
            }
            names.insert(name.to_owned());
        }
        return Ok((names.into_iter().collect(), "json"));
    }
    let names = directories_at(text, base);
    ensure!(
        !names.is_empty() || text.contains("Index of ") || text.contains("Directory listing for "),
        "Unrecognized directory listing at {base}"
    );
    Ok((names, "html"))
}

pub async fn directories(net: &Network, path: &str) -> Result<Option<Vec<String>>> {
    let Some(mut metadata) = net.metadata(path).await? else {
        return Ok(None);
    };
    let mut adapter = false;
    if metadata.text.contains("id=\"autoindex-list\"") && metadata.text.contains(".mirror-index/") {
        // ZJU's published autoindex layout: /qt/<path> -> /qt/.mirror-index/<path>.
        // Stay on the final listing origin, including when a redirect service selected it.
        // Never execute script or follow an endpoint extracted from page content.
        let relative = metadata
            .url
            .path()
            .strip_prefix("/qt/")
            .context("Unsupported autoindex root")?
            .to_owned();
        let mut index_net = net.clone();
        index_net.base = metadata.url.join("/qt/")?;
        metadata = index_net
            .metadata(&format!(".mirror-index/{relative}"))
            .await?
            .context("Missing mirror index")?;
        adapter = true;
    }
    let (names, format) = parse(&metadata.text, &metadata.url, &metadata.content_type)?;
    crate::report(
        &net.progress,
        "listing",
        format!(
            "{} {}",
            if adapter { "mirror-index" } else { format },
            metadata.url
        ),
        0,
        None,
    );
    Ok(Some(names))
}
