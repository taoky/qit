use qit_core::{archive::safe_relative, catalog};

#[test]
fn actual_qt_metadata_preserves_per_archive_destinations() {
    let packages = catalog::parse(
        include_str!("../../../tests/fixtures/qt6-linux-6.8.1.xml"),
        "repo/",
    )
    .unwrap();
    let desktop = packages
        .iter()
        .find(|p| p.id == "qt.qt6.681.linux_gcc_64")
        .unwrap();
    assert_eq!(desktop.archives.len(), 7);
    assert!(
        desktop
            .archives
            .iter()
            .any(|a| a.destination == "6.8.1/gcc_64/lib"
                && a.path.ends_with("icu-linux-Rhel8.6-x86_64.7z"))
    );
    let plan = catalog::plan(
        "https://example.org/qt/",
        &packages,
        std::slice::from_ref(&desktop.id),
    )
    .unwrap();
    assert!(plan.notes.iter().any(|n| n.contains("qt.tools.qtcreator")));
    assert!(plan.packages.iter().any(|p| p.id == desktop.id));
}

#[test]
fn cmake_destination_and_license_are_not_lost() {
    let packages = catalog::parse(
        include_str!("../../../tests/fixtures/cmake-linux.xml"),
        "repo/",
    )
    .unwrap();
    assert_eq!(packages[0].archives[0].destination, "Tools/CMake");
    assert_eq!(packages[0].licenses, ["CMake License Agreement"]);
}

#[test]
fn listing_does_not_follow_parent_or_external_links() {
    assert_eq!(
        catalog::directories(
            r#"<a href="../">parent</a><a href="/else/">escape</a><a href="https://example.org/">external</a><a href="qt6_681/">Qt</a><a href="%2e%2e/">escape</a>"#
        ),
        ["qt6_681"]
    );
}

#[test]
fn unsafe_files_and_windows_aliases_are_rejected_on_every_platform() {
    for name in [
        "../outside",
        "/absolute",
        "C:/absolute",
        "a\\..\\b",
        "NUL.txt",
        "dir/CON",
        "trailing.",
        "trailing ",
        "a/../../b",
    ] {
        assert!(safe_relative(name).is_err(), "{name}");
    }
    for name in [
        "6.8.1/gcc_64/bin/qmake",
        "Tools/CMake/bin/cmake",
        "中文/with spaces",
    ] {
        assert!(safe_relative(name).is_ok(), "{name}");
    }
}

#[test]
fn unknown_operations_and_html_errors_fail_closed() {
    assert!(catalog::parse("<html>Access denied</html>", "repo/").is_err());
    let xml = "<Updates><PackageUpdate><Name>qt.tools.cmake</Name><Version>1.0.0</Version><DownloadableArchives>a.7z</DownloadableArchives><Operations><Operation name='Execute'><Argument>evil</Argument></Operation></Operations></PackageUpdate></Updates>";
    let packages = catalog::parse(xml, "repo/").unwrap();
    assert!(
        catalog::plan(
            "https://example.org/",
            &packages,
            &["qt.tools.cmake".into()]
        )
        .is_err()
    );
}

#[test]
fn yadex_absolute_links_are_resolved_only_inside_current_directory() {
    let base = url::Url::parse(
        "https://mirrors.ustc.edu.cn/qtproject/online/qtsdkrepository/linux_x64/desktop/",
    )
    .unwrap();
    let html = r#"<a href="/qtproject/online/qtsdkrepository/linux_x64/desktop/qt6_681/">Qt</a><a href="/qtproject/online/qtsdkrepository/linux_x64/">parent</a><a href="https://evil.example/qt6_681/">external</a><a href="/else/">other</a>"#;
    assert_eq!(catalog::directories_at(html, &base), ["qt6_681"]);
}

#[test]
fn real_cross_targets_add_the_matching_desktop_sdk() {
    let mut packages = catalog::parse(
        include_str!("../../../tests/fixtures/qt6-linux-6.8.1.xml"),
        "desktop/",
    )
    .unwrap();
    for xml in [
        include_str!("../../../tests/fixtures/qt6-android-6.8.1.xml"),
        include_str!("../../../tests/fixtures/qt6-wasm-6.8.1.xml"),
    ] {
        packages.extend(catalog::parse(xml, "cross/").unwrap());
    }
    for id in [
        "qt.qt6.681.android_arm64_v8a",
        "qt.qt6.681.wasm_singlethread",
    ] {
        let plan = catalog::plan("https://example.org/", &packages, &[id.into()]).unwrap();
        assert!(
            plan.packages
                .iter()
                .any(|p| p.id == "qt.qt6.681.linux_gcc_64")
        );
        assert!(!plan.packages.iter().any(|p| p.id.ends_with(".patcher")));
    }
}

#[test]
fn windows_and_macos_real_metadata_parse() {
    for (xml, id, directory) in [
        (
            include_str!("../../../tests/fixtures/qt6-windows-6.8.1.xml"),
            "qt.qt6.681.win64_msvc2022_64",
            "6.8.1/msvc2022_64",
        ),
        (
            include_str!("../../../tests/fixtures/qt6-macos-6.8.1.xml"),
            "qt.qt6.681.clang_64",
            "6.8.1/macos",
        ),
    ] {
        let packages = catalog::parse(xml, "repo/").unwrap();
        let plan = catalog::plan("https://example.org/", &packages, &[id.into()]).unwrap();
        assert!(
            plan.packages
                .iter()
                .flat_map(|p| &p.archives)
                .any(|a| a.destination == directory)
        );
    }
}

#[test]
fn explicit_documentation_selection_keeps_its_documentation_dependencies() {
    let xml = "<Updates><PackageUpdate><Name>qt.qt6.681.doc</Name><Version>6.8.1</Version><Dependencies>qt.qt6.681.doc.qtcore</Dependencies></PackageUpdate><PackageUpdate><Name>qt.qt6.681.doc.qtcore</Name><Version>6.8.1</Version><DownloadableArchives>docs.7z</DownloadableArchives></PackageUpdate></Updates>";
    let packages = catalog::parse(xml, "docs/").unwrap();
    let plan = catalog::plan(
        "https://example.org/",
        &packages,
        &["qt.qt6.681.doc".into()],
    )
    .unwrap();
    assert_eq!(plan.packages.len(), 2);
}

#[test]
fn recommendations_include_only_the_selected_components_dependency_closure() {
    let mut packages = catalog::parse(
        include_str!("../../../tests/fixtures/qt6-linux-6.8.1.xml"),
        "repo/",
    )
    .unwrap();
    packages.extend(
        catalog::parse(
            include_str!("../../../tests/fixtures/qtcreator-linux.xml"),
            "creator/",
        )
        .unwrap(),
    );
    packages.extend(
        catalog::parse(
            include_str!("../../../tests/fixtures/qt6-common-6.8.1.xml"),
            "common/",
        )
        .unwrap(),
    );
    packages.extend(
        catalog::parse(
            include_str!("../../../tests/fixtures/tools-generic-linux.xml"),
            "generic/",
        )
        .unwrap(),
    );
    let selected = vec!["qt.qt6.681.linux_gcc_64".into()];
    let default = catalog::plan("https://example.org/", &packages, &selected).unwrap();
    let recommended =
        catalog::plan_with_recommendations("https://example.org/", &packages, &selected, true)
            .unwrap();
    assert_eq!(recommended.selected, selected);
    assert!(recommended.packages.len() > default.packages.len());
    for id in [
        "qt.tools.qtcreator",
        "qt.qt6.681.doc",
        "qt.qt6.681.examples",
    ] {
        assert!(
            recommended.packages.iter().any(|p| p.id == id),
            "missing {id}"
        );
        assert!(!default.packages.iter().any(|p| p.id == id));
    }
    assert!(
        !recommended
            .packages
            .iter()
            .any(|p| p.id == "qt.qt6.681.addons.qtcharts")
    );
    assert!(
        !recommended
            .notes
            .iter()
            .any(|n| n.starts_with("Optional recommendation:"))
    );
}

#[test]
#[ignore = "Requires local HTTP sockets"]
fn recommendations_fetch_creator_from_selected_mirror() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mirror = format!("http://{}/qt/", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let server = std::thread::spawn(move || {
        for body in [
            "<a href=\"tools_qtcreator/\">Creator</a>",
            "<Updates><PackageUpdate><Name>qt.tools.qtcreator</Name><Version>1.0.0</Version><Dependencies>qt.tools</Dependencies><DownloadableArchives>creator.7z</DownloadableArchives></PackageUpdate></Updates>",
            "<Updates><PackageUpdate><Name>qt.tools</Name><Version>1.0.0</Version></PackageUpdate></Updates>",
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
            }
            seen.lock().unwrap().push(String::from_utf8(bytes).unwrap());
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    let packages = catalog::parse("<Updates><PackageUpdate><Name>qt.qt6.681.linux_gcc_64</Name><Version>6.8.1</Version><Dependencies>qt.tools.qtcreator</Dependencies><DownloadableArchives>qt.7z</DownloadableArchives></PackageUpdate></Updates>", "online/qtsdkrepository/linux_x64/desktop/qt6_681/").unwrap();
    let net =
        qit_core::network::Network::new(&mirror, Default::default(), Arc::new(|_| {})).unwrap();
    let plan = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(catalog::prepare_plan_with_recommendations(
            &net,
            packages,
            &["qt.qt6.681.linux_gcc_64".into()],
            true,
        ))
        .unwrap();
    server.join().unwrap();
    assert_eq!(plan.packages.len(), 3);
    let requests = requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /qt/online/qtsdkrepository/linux_x64/desktop/ HTTP/1.1"));
    assert!(requests[1].starts_with(
        "GET /qt/online/qtsdkrepository/linux_x64/desktop/tools_qtcreator/Updates.xml HTTP/1.1"
    ));
}

#[test]
#[ignore = "Requires local HTTP sockets"]
fn required_windows_compilers_load_without_optional_recommendations() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::Arc,
    };
    for (id, repo, xml) in [
        (
            "qt.tools.win64_llvm_mingw2217",
            "tools_llvm_mingw2217",
            include_str!("../../../tests/fixtures/llvm-mingw-windows.xml"),
        ),
        (
            "qt.tools.win64_mingw1310",
            "tools_mingw1310",
            include_str!("../../../tests/fixtures/mingw-windows.xml"),
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mirror = format!("http://{}/qt/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(
                request.starts_with(&format!(
                    "GET /qt/online/qtsdkrepository/windows_x86/desktop/{repo}/Updates.xml HTTP/1.1"
                )),
                "{request}"
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
                xml.len()
            )
            .unwrap();
        });
        let packages = catalog::parse(&format!("<Updates><PackageUpdate><Name>qt.qt6.6120.win64_llvm_mingw</Name><Version>6.12.0</Version><Dependencies>{id}</Dependencies><DownloadableArchives>qt.7z</DownloadableArchives></PackageUpdate></Updates>"), "online/qtsdkrepository/windows_x86/desktop/qt6_6120/").unwrap();
        let net =
            qit_core::network::Network::new(&mirror, Default::default(), Arc::new(|_| {})).unwrap();
        let plan = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog::prepare_plan(
                &net,
                packages,
                &["qt.qt6.6120.win64_llvm_mingw".into()],
            ))
            .unwrap();
        server.join().unwrap();
        let compiler = plan.packages.iter().find(|p| p.id == id).unwrap();
        assert!(compiler.unsupported.is_none());
        if repo.contains("llvm") {
            assert_eq!(compiler.archives[0].destination, "Tools/llvm-mingw2217_64");
        }
        assert!(!plan.packages.iter().any(|p| p.id == "qt.tools.qtcreator"));
        assert_eq!(plan.packages.len(), 2);
    }
}

#[test]
fn extending_an_installation_cannot_mix_host_platforms() {
    let xml = "<Updates><PackageUpdate><Name>qt.qt6.6120.kit</Name><Version>6.12.0</Version><DownloadableArchives>qt.7z</DownloadableArchives></PackageUpdate></Updates>";
    let old_packages =
        catalog::parse(xml, "online/qtsdkrepository/linux_x64/desktop/qt6_6120/").unwrap();
    let new_packages = catalog::parse(
        &xml.replace(".kit", ".windowskit"),
        "online/qtsdkrepository/windows_x86/desktop/qt6_6120/",
    )
    .unwrap();
    let old = qit_core::install::Installation {
        schema: 1,
        id: "test".into(),
        path: "/tmp/sdk".into(),
        plan: catalog::plan(
            "https://example.org/",
            &old_packages,
            &[old_packages[0].id.clone()],
        )
        .unwrap(),
        files: Vec::new(),
        checksums: Default::default(),
        state: "installed".into(),
    };
    let new = catalog::plan(
        "https://example.org/",
        &new_packages,
        &[new_packages[0].id.clone()],
    )
    .unwrap();
    assert!(
        qit_core::install::merge_plan(&old, new)
            .unwrap_err()
            .to_string()
            .contains("different system")
    );
}
