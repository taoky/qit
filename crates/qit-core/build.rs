fn main() {
    pkg_config::Config::new()
        .atleast_version("3.6")
        .probe("libarchive")
        .expect("libarchive development files are required");
}
