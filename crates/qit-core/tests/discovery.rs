use qit_core::{Cancel, catalog, listing, network::Network};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

#[test]
fn captured_listing_formats_and_unsafe_names() {
    for name in ["html", "nginx", "caddy", "mirror-index", "redirect"] {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/listing");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join(format!("{name}.json"))).unwrap())
                .unwrap();
        let text = std::fs::read_to_string(root.join(format!("{name}.body"))).unwrap();
        let (names, _) = listing::parse(
            &text,
            &metadata["final_url"].as_str().unwrap().parse().unwrap(),
            "",
        )
        .unwrap();
        assert!(names.contains(&"qt6_681".to_owned()), "{name}");
    }
    let base = "https://mirror.test/qt/".parse().unwrap();
    let (names, _) = listing::parse(r#"<a href="child/">ok</a><a href="https://mirror.test/qt/absolute/">ok</a><a href="../qt/escape/">bad</a><a href="https://elsewhere/qt/foreign/">bad</a><a href="%2e%2e/">bad</a><a href="child/">duplicate</a>"#, &base, "text/html").unwrap();
    assert_eq!(names, ["absolute", "child"]);
    let (names, _) = listing::parse(r#"[{"name":"../","type":"directory"},{"name":".","type":"directory"},{"name":"good","type":"directory"},{"name":"evil%2f","type":"directory"}]"#, &base, "application/json").unwrap();
    assert_eq!(names, ["good"]);
    assert!(listing::parse("<html>Access denied</html>", &base, "text/html").is_err());
    assert!(listing::parse("{}", &base, "application/json").is_err());
}

struct Fixture {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}
impl Fixture {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let thread = thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(std::time::Duration::from_millis(2));
                    continue;
                };
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                }
                let request = String::from_utf8(bytes).unwrap();
                let path = request.split_whitespace().nth(1).unwrap();
                seen.lock().unwrap().push(path.to_owned());
                assert!(!request.to_lowercase().contains("authorization:"));
                if path.ends_with('/') {
                    assert!(
                        request
                            .to_lowercase()
                            .contains("accept: application/json, text/html;q=0.9")
                    );
                }
                let (status, body, headers) = match path {
                    "/qt/root/" => (
                        200,
                        r#"<div id="autoindex-list"></div><script>const index=".mirror-index/";</script>"#,
                        "",
                    ),
                    "/redirect/" => (302, "", "Location: /final/\r\n"),
                    "/final/" => (
                        200,
                        r#"<a href="/final/child/">child</a>"#,
                        "Content-Type: text/html\r\n",
                    ),
                    "/broken/Updates.xml" => (200, "broken XML", ""),
                    "/denied/" => (403, "Denied", ""),
                    "/unknown/" => (200, "<html>Service unavailable</html>", ""),
                    _ if path.ends_with("/kit/Updates.xml") => (
                        200,
                        "<Updates><PackageUpdate><Name>qt.qt6.681.gcc_64</Name><Version>6.8.1</Version></PackageUpdate></Updates>",
                        "",
                    ),
                    _ if path.ends_with("Updates.xml") => (404, "", ""),
                    _ if path.ends_with("desktop/") => (
                        200,
                        r#"[{"name":"qt6_681","type":"directory"}]"#,
                        "Content-Type: application/json\r\n",
                    ),
                    _ => (
                        200,
                        r#"[{"name":"kit","is_dir":true,"url":"kit/"}]"#,
                        "Content-Type: application/json\r\n",
                    ),
                };
                write!(stream, "HTTP/1.1 {status} Test\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        Self {
            base,
            requests,
            stop,
            thread: Some(thread),
        }
    }
    fn net(&self) -> Network {
        Network::new(&self.base, Cancel::default(), Arc::new(|_| {})).unwrap()
    }
}

#[tokio::test]
async fn listing_discovers_versions_and_nested_components() {
    let fixture = Fixture::new();
    let net = fixture.net();
    let repos = catalog::repositories(&net, "linux_x64", "desktop")
        .await
        .unwrap();
    assert_eq!(repos[0].name, "qt6_681");
    assert_eq!(
        catalog::packages(&net, &repos[0].path).await.unwrap().len(),
        1
    );
    assert_eq!(fixture.requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn redirected_links_and_same_site_mirror_index() {
    let fixture = Fixture::new();
    let mut net = fixture.net();
    assert_eq!(
        listing::directories(&net, "redirect/")
            .await
            .unwrap()
            .unwrap(),
        ["child"]
    );
    net.base = format!("{}qt/", fixture.base).parse().unwrap();
    assert_eq!(
        listing::directories(&net, "root/").await.unwrap().unwrap(),
        ["kit"]
    );
    assert_eq!(
        *fixture.requests.lock().unwrap(),
        [
            "/redirect/",
            "/final/",
            "/qt/root/",
            "/qt/.mirror-index/root/"
        ]
    );
}

#[tokio::test]
async fn direct_xml_and_errors_do_not_trigger_other_discovery() {
    let fixture = Fixture::new();
    let net = fixture.net();
    assert_eq!(catalog::packages(&net, "kit/").await.unwrap().len(), 1);
    assert!(catalog::packages(&net, "broken/").await.is_err());
    assert_eq!(fixture.requests.lock().unwrap().len(), 2);
    assert!(listing::directories(&net, "denied/").await.is_err());
    assert!(listing::directories(&net, "unknown/").await.is_err());
    let count = fixture.requests.lock().unwrap().len();
    assert!(
        catalog::repositories(&net, "../escape", "desktop")
            .await
            .is_err()
    );
    net.cancel.cancel();
    assert!(
        catalog::repositories(&net, "linux_x64", "desktop")
            .await
            .unwrap_err()
            .to_string()
            .contains("Cancelled")
    );
    assert_eq!(fixture.requests.lock().unwrap().len(), count);
}
