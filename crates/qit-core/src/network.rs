use crate::{Cancel, Progress, USER_AGENT, report};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    time::Duration,
};
use url::Url;

#[derive(Clone)]
pub struct Network {
    pub base: Url,
    client: Client,
    pub cancel: Cancel,
    pub progress: Progress,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checksum {
    pub algorithm: String,
    pub value: String,
}

impl Network {
    pub fn new(base: &str, cancel: Cancel, progress: Progress) -> Result<Self> {
        let base = Url::parse(&format!("{}/", base.trim_end_matches('/')))?;
        ensure!(
            matches!(base.scheme(), "http" | "https") && base.host_str().is_some(),
            "Mirror must be an HTTP(S) URL"
        );
        ensure!(
            base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "Mirror URL must not contain credentials, a query or fragment"
        );
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(45))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?;
        Ok(Self {
            base,
            client,
            cancel,
            progress,
        })
    }
    pub fn url(&self, relative: &str) -> Result<Url> {
        ensure!(
            !relative.starts_with('/') && !relative.contains('\\'),
            "Invalid repository path"
        );
        let url = self.base.join(relative)?;
        ensure!(
            url.origin() == self.base.origin() && url.path().starts_with(self.base.path()),
            "Repository URL escapes mirror root"
        );
        Ok(url)
    }
    async fn response(&self, url: &Url, range: Option<(u64, &str)>) -> Result<reqwest::Response> {
        for attempt in 0..4 {
            self.cancel.check()?;
            let mut request = self.client.get(url.clone());
            if let Some((offset, validator)) = range {
                request = request
                    .header(header::RANGE, format!("bytes={offset}-"))
                    .header(header::IF_RANGE, validator);
            }
            let response = tokio::select! { result = request.send() => result, _ = self.cancel.cancelled() => bail!("Cancelled") };
            match response {
                Ok(response) => {
                    if (response.status() == StatusCode::TOO_MANY_REQUESTS
                        || response.status().is_server_error())
                        && attempt < 3
                    {
                        let delay = response
                            .headers()
                            .get(header::RETRY_AFTER)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(1 << attempt)
                            .min(120);
                        self.wait(delay).await?;
                        continue;
                    }
                    report(
                        &self.progress,
                        "request",
                        response.url().to_string(),
                        0,
                        None,
                    );
                    return Ok(response);
                }
                Err(error) if attempt == 3 => return Err(error.into()),
                Err(_) => self.wait(1 << attempt).await?,
            }
        }
        unreachable!()
    }
    async fn wait(&self, seconds: u64) -> Result<()> {
        for _ in 0..seconds * 10 {
            self.cancel.check()?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }
    pub async fn optional_text(&self, path: &str) -> Result<Option<String>> {
        self.optional_text_with_timeout(path, Duration::from_secs(30))
            .await
    }
    async fn optional_text_with_timeout(
        &self,
        path: &str,
        timeout: Duration,
    ) -> Result<Option<String>> {
        let url = self.url(path)?;
        tokio::time::timeout(timeout, self.read_optional_text(path))
            .await
            .with_context(|| format!("Metadata request timed out: {url}"))?
    }
    async fn read_optional_text(&self, path: &str) -> Result<Option<String>> {
        let mut response = self.response(&self.url(path)?, None).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        response.error_for_status_ref()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = tokio::select! { result = response.chunk() => result?, _ = self.cancel.cancelled() => bail!("Cancelled") }
        {
            self.cancel.check()?;
            ensure!(
                bytes.len() + chunk.len() <= 32 * 1024 * 1024,
                "Metadata too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(Some(String::from_utf8(bytes)?))
    }
    pub async fn text(&self, path: &str) -> Result<String> {
        self.optional_text(path)
            .await?
            .with_context(|| format!("Not found on selected mirror: {path}"))
    }
    pub async fn checksum(&self, path: &str) -> Result<Checksum> {
        for (algorithm, suffix, length) in [("sha256", ".sha256", 64), ("sha1", ".sha1", 40)] {
            if let Some(text) = self.optional_text(&format!("{path}{suffix}")).await? {
                let value = text.split_whitespace().next().unwrap_or("").to_lowercase();
                ensure!(
                    value.len() == length && value.bytes().all(|b| b.is_ascii_hexdigit()),
                    "Invalid {algorithm} checksum for {path}"
                );
                return Ok(Checksum {
                    algorithm: algorithm.into(),
                    value,
                });
            }
        }
        bail!("No checksum available for {path}")
    }
    pub async fn download(&self, path: &str, destination: &Path, sum: &Checksum) -> Result<()> {
        self.cancel.check()?;
        fs::create_dir_all(destination.parent().context("Cache parent")?)?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(destination.with_extension("lock"))?;
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) => {
                    self.cancel.check()?;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
        if destination.is_file() && verify(destination, sum)? {
            report(&self.progress, "cached", path, 0, None);
            return Ok(());
        }
        fs::create_dir_all(destination.parent().context("Cache parent")?)?;
        let part = destination.with_extension("part");
        let validator_path = destination.with_extension("validator");
        let url = self.url(path)?;
        for attempt in 0..4 {
            self.cancel.check()?;
            let validator = fs::read_to_string(&validator_path).unwrap_or_default();
            let offset = if validator.is_empty() {
                0
            } else {
                fs::metadata(&part).map(|m| m.len()).unwrap_or(0)
            };
            let mut response = self
                .response(&url, (offset > 0).then_some((offset, validator.as_str())))
                .await?;
            if response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
                let _ = fs::remove_file(&part);
                let _ = fs::remove_file(&validator_path);
                continue;
            }
            response.error_for_status_ref()?;
            let append = response.status() == StatusCode::PARTIAL_CONTENT && offset > 0;
            if append {
                let range = response
                    .headers()
                    .get(header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                ensure!(
                    range.starts_with(&format!("bytes {offset}-")),
                    "Invalid Content-Range"
                );
            } else {
                ensure!(
                    response.status() == StatusCode::OK,
                    "Unexpected download status {}",
                    response.status()
                );
            }
            let validator = response
                .headers()
                .get(header::ETAG)
                .and_then(|v| v.to_str().ok())
                .filter(|s| !s.starts_with("W/"))
                .or_else(|| {
                    response
                        .headers()
                        .get(header::LAST_MODIFIED)
                        .and_then(|v| v.to_str().ok())
                })
                .unwrap_or("");
            fs::write(&validator_path, validator)?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .append(append)
                .truncate(!append)
                .open(&part)?;
            let mut done = if append { offset } else { 0 };
            let total = response.content_length().map(|length| length + done);
            let mut broken = false;
            loop {
                self.cancel.check()?;
                let chunk = tokio::select! { result = response.chunk() => result, _ = self.cancel.cancelled() => bail!("Cancelled") };
                match chunk {
                    Ok(Some(chunk)) => {
                        file.write_all(&chunk)?;
                        done += chunk.len() as u64;
                        report(&self.progress, "download", path, done, total);
                    }
                    Ok(None) => break,
                    Err(error) => {
                        if attempt == 3 {
                            return Err(error.into());
                        }
                        broken = true;
                        break;
                    }
                }
            }
            file.sync_all()?;
            drop(file);
            if broken {
                self.wait(1 << attempt).await?;
                continue;
            }
            if !verify(&part, sum)? {
                let _ = fs::remove_file(&part);
                bail!("Checksum mismatch: {path}");
            }
            if destination.exists() {
                fs::remove_file(destination)?;
            }
            fs::rename(&part, destination)?;
            let _ = fs::remove_file(validator_path);
            return Ok(());
        }
        bail!("Download retries exhausted: {path}")
    }
}
pub fn verify(path: &Path, sum: &Checksum) -> Result<bool> {
    Ok(hash(path, &sum.algorithm)? == sum.value)
}

pub fn hash(path: &Path, algorithm: &str) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut sha256 = Sha256::new();
    let mut sha1 = sha1::Sha1::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        match algorithm {
            "sha256" => sha256.update(&buffer[..count]),
            "sha1" => sha1.update(&buffer[..count]),
            _ => bail!("Unsupported hash algorithm"),
        }
    }
    Ok(if algorithm == "sha256" {
        hex::encode(sha256.finalize())
    } else {
        hex::encode(sha1.finalize())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, sync::Arc, thread};

    #[tokio::test]
    async fn metadata_deadline_covers_headers_body_and_retry_wait() {
        for response in [
            "",
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial",
            "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 120\r\nContent-Length: 0\r\n\r\n",
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 4096];
                assert!(stream.read(&mut request).unwrap() > 0);
                stream.write_all(response.as_bytes()).unwrap();
                thread::sleep(Duration::from_millis(500));
            });
            let mut net = Network::new(
                &format!("http://{address}/"),
                Cancel::default(),
                Arc::new(|_| {}),
            )
            .unwrap();
            net.client = Client::builder().no_proxy().build().unwrap();
            let error = net
                .optional_text_with_timeout("Updates.xml", Duration::from_millis(200))
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains("Metadata request timed out"),
                "{error:#}"
            );
            assert!(error.to_string().contains("Updates.xml"));
            server.join().unwrap();
        }
    }
}
