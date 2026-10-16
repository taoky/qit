use super::*;
use std::time::{Duration, Instant};

fn widgets(root: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
    let root = root.as_ref();
    let mut result = vec![root.clone()];
    let mut child = root.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        result.extend(widgets(&widget));
    }
    result
}
fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        while glib::MainContext::default().pending() {
            glib::MainContext::default().iteration(false);
        }
        if condition() {
            return;
        }
        assert!(Instant::now() < deadline, "UI condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn dropdown(window: &adw::ApplicationWindow, first: &str) -> gtk::DropDown {
    widgets(window)
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::DropDown>().ok())
        .find(|d| {
            d.model()
                .and_then(|m| m.item(0))
                .and_then(|i| i.downcast::<gtk::StringObject>().ok())
                .is_some_and(|i| i.string() == i18n::message(first))
        })
        .unwrap()
}
fn button(window: &adw::ApplicationWindow, label: &str) -> gtk::Button {
    widgets(window)
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| b.label().is_some_and(|s| s == i18n::message(label)))
        .unwrap()
}
fn component_rows(window: &adw::ApplicationWindow) -> Vec<adw::ActionRow> {
    widgets(window)
        .into_iter()
        .filter_map(|w| w.downcast::<adw::ActionRow>().ok())
        .filter(|r| r.widget_name().contains("qt."))
        .collect()
}

#[test]
#[ignore = "Run through tests/ui-smoke.py under an isolated display/session"]
fn install_page_workflow() {
    assert!(std::env::var_os("QIT_UI_FIXTURE").is_some());
    adw::init().unwrap();
    let app = adw::Application::builder()
        .application_id("io.github.taoky.Qit.Test")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build(&app);
    let window = app
        .active_window()
        .unwrap()
        .downcast::<adw::ApplicationWindow>()
        .unwrap();
    until(|| !component_rows(&window).is_empty());
    let recommendations = widgets(&window)
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::CheckButton>().ok())
        .find(|b| {
            b.label()
                .is_some_and(|s| s == tr("Install optional recommended components for me"))
        })
        .unwrap();
    assert!(!recommendations.is_active());
    recommendations.set_active(true);
    assert!(recommendations.is_active());
    let next = button(&window, "Next");
    assert!(!next.is_sensitive());
    let row = component_rows(&window)
        .into_iter()
        .find(|r| r.widget_name().contains("681.linux_gcc_64"))
        .unwrap();
    let check = widgets(&row)
        .into_iter()
        .find_map(|w| w.downcast::<gtk::CheckButton>().ok())
        .unwrap();
    check.set_active(true);
    until(|| next.is_sensitive());
    assert!(
        widgets(&window)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Label>().ok())
            .any(|l| l.text().contains("qt-6.8.1-desktop"))
    );
    // An existing managed SDK must remain selectable for additional components.
    let root = settings::Settings::load().unwrap().install_root.unwrap();
    let sdk = root.join("qt-6.8.1-desktop");
    std::fs::create_dir_all(&sdk).unwrap();
    std::fs::write(sdk.join(".qit-owner"), "ui-existing").unwrap();
    let record = settings::data_dir().join("installations/ui-existing.json");
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    let all = catalog::parse(
        include_str!("../../../tests/fixtures/qt6-linux-6.8.1.xml"),
        &format!(
            "online/qtsdkrepository/{}/desktop/qt6_681/",
            catalog::host()
        ),
    )
    .unwrap();
    let old_plan = catalog::plan(
        "http://localhost/",
        &all,
        &["qt.qt6.681.linux_gcc_64".into()],
    )
    .unwrap();
    let old = install::Installation {
        schema: 1,
        id: "ui-existing".into(),
        path: std::fs::canonicalize(&sdk).unwrap(),
        plan: old_plan,
        files: Vec::new(),
        checksums: Default::default(),
        state: "installed".into(),
    };
    check.set_active(false);
    std::fs::write(&record, serde_json::to_vec(&old).unwrap()).unwrap();
    until(|| {
        widgets(&window)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Label>().ok())
            .any(|label| {
                label.text().contains(tr(
                    " · Additional components will be added to this installation",
                ))
            })
    });
    assert!(next.is_sensitive());
    until(|| check.is_active() && !check.is_sensitive());
    assert!(row.subtitle().unwrap().contains(tr(" · Installed")));
    recommendations.set_active(false);
    until(|| !next.is_sensitive());
    let folder = widgets(&window)
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Entry>().ok())
        .find(|entry| {
            entry
                .placeholder_text()
                .is_some_and(|s| s == tr("Installation folder name"))
        })
        .unwrap();
    folder.set_text("another-sdk");
    until(|| !check.is_active() && check.is_sensitive());
    folder.set_text("qt-6.8.1-desktop");
    until(|| check.is_active() && !check.is_sensitive());
    let addon = component_rows(&window)
        .into_iter()
        .find(|r| r.widget_name().contains("addons.qt3d.linux_gcc_64"))
        .unwrap();
    widgets(&addon)
        .into_iter()
        .find_map(|w| w.downcast::<gtk::CheckButton>().ok())
        .unwrap()
        .set_active(true);
    until(|| next.is_sensitive());
    next.emit_clicked();
    until(|| window.visible_dialog().is_some());
    let dialog = window
        .visible_dialog()
        .unwrap()
        .downcast::<adw::AlertDialog>()
        .unwrap();
    assert_eq!(
        dialog.heading().as_deref(),
        Some(tr("Review additional components"))
    );
    assert!(dialog.body().contains(tr(" (new component)")));
    assert!(dialog.body().contains(tr(" (already installed)")));
    assert!(!dialog.is_response_enabled("install"));
    dialog.close();
    until(|| window.visible_dialog().is_none());
    let reload_versions = dropdown(&window, "Qt 6.8.1");
    reload_versions.set_selected(1);
    until(|| {
        component_rows(&window)
            .iter()
            .any(|r| r.widget_name().contains("680.linux_gcc_64"))
    });
    reload_versions.set_selected(0);
    until(|| {
        component_rows(&window).iter().any(|r| {
            r.widget_name().contains("681.linux_gcc_64")
                && widgets(r)
                    .into_iter()
                    .filter_map(|w| w.downcast::<gtk::CheckButton>().ok())
                    .any(|c| c.is_active() && !c.is_sensitive())
        })
    });
    // Keep one new component selected for the unmanaged/new-directory checks below.
    let addon = component_rows(&window)
        .into_iter()
        .find(|r| r.widget_name().contains("addons.qt3d.linux_gcc_64"))
        .unwrap();
    widgets(&addon)
        .into_iter()
        .find_map(|w| w.downcast::<gtk::CheckButton>().ok())
        .unwrap()
        .set_active(true);
    // Unknown existing directories are still blocked.
    std::fs::remove_file(sdk.join(".qit-owner")).unwrap();
    until(|| !next.is_sensitive());
    std::fs::remove_dir(&sdk).unwrap();
    std::fs::remove_file(record).unwrap();
    until(|| next.is_sensitive());
    let versions = dropdown(&window, "Qt 6.8.1");
    versions.set_selected(1);
    until(|| {
        !component_rows(&window).is_empty()
            && component_rows(&window)
                .iter()
                .all(|r| !r.widget_name().contains("681."))
    });
    until(|| !next.is_sensitive());
    assert!(
        component_rows(&window)
            .iter()
            .any(|r| r.widget_name().contains("680.doc"))
    );
    // Two fast changes must leave only the last request's components.
    versions.set_selected(0);
    versions.set_selected(1);
    until(|| {
        component_rows(&window)
            .iter()
            .any(|r| r.widget_name().contains("680.linux_gcc_64"))
    });
    if let Ok(path) = std::env::var("QIT_UI_SCREENSHOT") {
        let frame = Instant::now();
        until(|| frame.elapsed() > Duration::from_millis(300));
        assert!(
            std::process::Command::new("import")
                .args(["-window", "root", &path.replace(".png", "-qt.png")])
                .status()
                .unwrap()
                .success()
        );
    }
    let target = dropdown(&window, "Desktop Qt");
    target.set_selected(3);
    let retry = button(&window, "Retry");
    until(|| retry.is_visible() && retry.is_sensitive());
    retry.emit_clicked();
    until(|| {
        component_rows(&window)
            .iter()
            .any(|r| r.widget_name().contains("qt.tools.cmake"))
    });
    assert!(
        component_rows(&window)
            .iter()
            .all(|r| r.widget_name().contains("qt.tools."))
    );
    assert!(!next.is_sensitive());
    // Leave a real rendered frame for manual inspection.
    if let Ok(path) = std::env::var("QIT_UI_SCREENSHOT") {
        // Flush a frame before asking X11 for a screenshot.
        let frame = Instant::now();
        until(|| frame.elapsed() > Duration::from_millis(300));
        assert!(
            std::process::Command::new("import")
                .args(["-window", "root", &path])
                .status()
                .unwrap()
                .success()
        );
    }
    button(&window, "Change").emit_clicked();
    let mirror = widgets(&window)
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Entry>().ok())
        .find(|e| e.text().starts_with("http://127.0.0.1:"))
        .unwrap();
    mirror.set_text("http://127.0.0.1:1/still-typing");
    let editing = Instant::now();
    until(|| editing.elapsed() > Duration::from_millis(350));
    assert!(
        mirror.is_sensitive(),
        "Editing a mirror must not start a request and lock Settings"
    );
    window.close();
}
