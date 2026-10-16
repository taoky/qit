#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod i18n;
mod presentation;
use i18n::{tr, tr_args};

use adw::prelude::*;
use gtk::{gio, glib};
use qit_core::{
    Cancel, Event, Progress,
    catalog::{self, Package, Plan, Repository},
    install,
    network::Network,
    settings::{self, Settings},
};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn main() -> glib::ExitCode {
    // Set bundle lookup paths before GTK or worker threads are started.
    if let Ok(exe) = std::env::current_exe()
        && let Some(bin) = exe.parent()
    {
        let share = if cfg!(target_os = "macos") {
            bin.join("../Resources/share")
        } else {
            bin.join("share")
        };
        if share.is_dir() {
            unsafe {
                std::env::set_var("XDG_DATA_DIRS", &share);
                std::env::set_var("GSETTINGS_SCHEMA_DIR", share.join("glib-2.0/schemas"));
            }
        }
    }
    let app = adw::Application::builder()
        .application_id("io.github.taoky.Qit")
        .build();
    app.connect_activate(build);
    app.run()
}

enum Message {
    Event(Event),
    Repositories(u64, Vec<Repository>),
    Packages(u64, Vec<Package>),
    CatalogError(u64, String),
    Review(Plan, PathBuf, String, Option<Box<install::Installation>>),
    Installed,
    Removed(Vec<String>),
    Error(String),
    Done,
}
type Work = Box<dyn FnOnce(Cancel, Progress) -> anyhow::Result<Message> + Send>;
#[derive(Clone)]
struct Worker {
    sender: async_channel::Sender<Message>,
    cancel: Rc<RefCell<Cancel>>,
    busy: Arc<AtomicBool>,
}
impl Worker {
    fn run(&self, job: Work) {
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let cancel = Cancel::default();
        *self.cancel.borrow_mut() = cancel.clone();
        let sender = self.sender.clone();
        let busy = self.busy.clone();
        std::thread::spawn(move || {
            let event_sender = sender.clone();
            let last = std::sync::Mutex::new(std::time::Instant::now());
            let progress: Progress = Arc::new(move |e| {
                if e.phase == "download" {
                    let mut time = last.lock().unwrap();
                    if time.elapsed().as_millis() < 100 {
                        return;
                    }
                    *time = std::time::Instant::now();
                }
                let _ = event_sender.send_blocking(Message::Event(e));
            });
            let result =
                job(cancel, progress).unwrap_or_else(|error| Message::Error(format!("{error:#}")));
            let _ = sender.send_blocking(result);
            busy.store(false, Ordering::SeqCst);
            let _ = sender.send_blocking(Message::Done);
        });
    }
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("worker runtime")
}
fn margins(widget: &impl IsA<gtk::Widget>, n: i32) {
    widget.set_margin_top(n);
    widget.set_margin_bottom(n);
    widget.set_margin_start(n);
    widget.set_margin_end(n);
}
fn beginner_guidance(host: &str) -> String {
    let tools = if host.starts_with("windows") {
        tr(
            "MinGW/LLVM-MinGW is installed when required. For MSVC, install Visual Studio or Build Tools with C++ tools and the Windows SDK separately.",
        )
    } else if host.starts_with("mac") {
        tr("Install Xcode development tools separately.")
    } else {
        tr(
            "Install GCC/G++ and other system development tools using your distribution’s package manager.",
        )
    };
    format!(
        "{} {tools}",
        tr(
            "New to Qt? Select a base development kit and enable “Install optional recommended components for me”."
        )
    )
}
fn notice(parent: &adw::ApplicationWindow, title: &str, text: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(title)
        .body(text)
        .build();
    dialog.add_response("ok", tr("OK"));
    dialog.present(Some(parent));
}
fn build(app: &adw::Application) {
    static RESOURCES: std::sync::Once = std::sync::Once::new();
    RESOURCES.call_once(|| {
        gio::resources_register_include!("qit.gresource").expect("embedded Qit resources");
    });
    let settings_error;
    let saved = match Settings::load() {
        Ok(s) => {
            settings_error = None;
            s
        }
        Err(e) => {
            settings_error = Some(e.to_string());
            Settings::default()
        }
    };
    i18n::init();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Qit")
        .default_width(1050)
        .default_height(960)
        .width_request(1050)
        .height_request(960)
        .build();
    gtk::IconTheme::for_display(&WidgetExt::display(&window))
        .add_resource_path("/io/github/taoky/Qit/icons");
    gtk::Window::set_default_icon_name("io.github.taoky.Qit");
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let title = adw::WindowTitle::new("Qit", tr("A GTK installer for Qt"));
    header.set_title_widget(Some(&title));
    let about = gtk::Button::from_icon_name("help-about-symbolic");
    about.set_tooltip_text(Some(tr("About Qit")));
    header.pack_end(&about);
    toolbar.add_top_bar(&header);
    window.set_content(Some(&toolbar));
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.set_widget_name("main-body");
    let stack = gtk::Stack::builder()
        .hexpand(true)
        .vexpand(true)
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    let sidebar = gtk::StackSidebar::new();
    sidebar.set_stack(&stack);
    sidebar.set_size_request(155, -1);
    body.append(&sidebar);
    body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    // Let long pages scroll instead of forcing the window to fit every label.
    // This also isolates GtkBoxLayout from the stack's width-dependent height.
    let pages = gtk::ScrolledWindow::builder()
        .name("pages-scroll")
        .hscrollbar_policy(gtk::PolicyType::Never)
        .hexpand(true)
        .vexpand(true)
        .child(&stack)
        .build();
    body.append(&pages);
    toolbar.set_content(Some(&body));

    let snapshot = settings::mirrors();
    let mut mirror_names: Vec<String> = vec![tr("Choose a mirror…").into()];
    mirror_names.extend(
        snapshot
            .mirrors
            .iter()
            .map(|m| format!("{} · {}", m.country, m.operator)),
    );
    let mirrors =
        gtk::DropDown::from_strings(&mirror_names.iter().map(String::as_str).collect::<Vec<_>>());
    let mirror_url = gtk::Entry::builder()
        .placeholder_text("https://mirror.example/qt/")
        .hexpand(true)
        .build();
    if let Some(url) = saved
        .mirror
        .as_ref()
        .or_else(|| snapshot.mirrors.first().map(|mirror| &mirror.url))
    {
        mirror_url.set_text(url);
        if let Some(index) = snapshot.mirrors.iter().position(|m| &m.url == url) {
            mirrors.set_selected(index as u32 + 1);
        }
    }
    let mirror_url_clone = mirror_url.clone();
    mirrors.connect_selected_notify(move |dropdown| {
        if let Some(mirror) = snapshot
            .mirrors
            .get(dropdown.selected().wrapping_sub(1) as usize)
        {
            mirror_url_clone.set_text(&mirror.url);
        }
    });

    let settings_page = gtk::Box::new(gtk::Orientation::Vertical, 14);
    margins(&settings_page, 24);
    let mirror_title = gtk::Label::new(Some(tr("Download mirror")));
    mirror_title.add_css_class("title-1");
    mirror_title.set_xalign(0.0);
    settings_page.append(&mirror_title);
    settings_page.append(&mirrors);
    settings_page.append(&mirror_url);
    let explanation = gtk::Label::new(Some(tr(
        "Bundled from MirrorZ, with CERNET selected by default. CERNET redirects downloads to a mirror; Qit uses your chosen service.",
    )));
    explanation.set_wrap(true);
    explanation.set_xalign(0.0);
    settings_page.append(&explanation);
    let root = gtk::Entry::builder()
        .hexpand(true)
        .text(
            saved
                .install_root
                .unwrap_or_else(settings::default_root)
                .to_string_lossy(),
        )
        .build();
    settings_page.append(&gtk::Label::new(Some(tr("Installation directory"))));
    let directory_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    directory_row.append(&root);
    let browse = gtk::Button::with_label(tr("Browse…"));
    directory_row.append(&browse);
    settings_page.append(&directory_row);
    {
        let window = window.clone();
        let root = root.clone();
        browse.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder()
                .title(tr("Select installation directory"))
                .modal(true)
                .build();
            let path = PathBuf::from(root.text().as_str());
            // The default installation folder may not exist yet.
            if let Some(folder) = path.ancestors().find(|p| p.is_dir()) {
                dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
            }
            let root = root.clone();
            let parent = window.clone();
            dialog.select_folder(Some(&window), None::<&gio::Cancellable>, move |result| {
                match result {
                    Ok(folder) => {
                        if let Some(path) = folder.path() {
                            root.set_text(&path.to_string_lossy());
                        } else {
                            notice(
                                &parent,
                                tr("Settings error"),
                                tr("Choose a local directory."),
                            );
                        }
                    }
                    Err(error)
                        if error.matches(gtk::DialogError::Dismissed)
                            || error.matches(gtk::DialogError::Cancelled) => {}
                    Err(error) => notice(&parent, tr("Settings error"), &error.to_string()),
                }
            });
        });
    }
    let save = gtk::Button::with_label(tr("Save settings"));
    save.set_halign(gtk::Align::Start);
    settings_page.append(&save);
    stack.add_titled(&settings_page, Some("settings"), tr("Settings"));
    {
        let window = window.clone();
        let root = root.clone();
        let mirror_url = mirror_url.clone();
        save.connect_clicked(move |_| {
            let settings = Settings {
                mirror: Some(mirror_url.text().to_string()),
                install_root: Some(PathBuf::from(root.text().as_str())),
                language: saved.language.clone(),
            };
            let result = Network::new(
                mirror_url.text().as_str(),
                Cancel::default(),
                Arc::new(|_| {}),
            )
            .and_then(|_| settings.save());
            match result {
                Ok(()) => notice(
                    &window,
                    tr("Saved"),
                    tr("The selected mirror will be used for downloads."),
                ),
                Err(e) => notice(&window, tr("Settings error"), &e.to_string()),
            }
        });
    }

    let install_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    margins(&install_page, 24);
    let heading = gtk::Label::new(Some(tr("Install Qt")));
    heading.add_css_class("title-1");
    heading.set_xalign(0.0);
    install_page.append(&heading);
    let source_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let source_label = gtk::Label::new(None);
    source_label.set_xalign(0.0);
    source_label.set_hexpand(true);
    source_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    let change_source = gtk::Button::with_label(tr("Change"));
    change_source.add_css_class("flat");
    source_row.append(&source_label);
    source_row.append(&change_source);
    install_page.append(&source_row);
    {
        let stack = stack.clone();
        change_source.connect_clicked(move |_| stack.set_visible_child_name("settings"));
    }
    let host_names = [
        "linux_x64",
        "linux_arm64",
        "windows_x86",
        "windows_arm64",
        "mac_x64",
    ];
    let host = gtk::DropDown::from_strings(&[
        "Linux · x86_64",
        "Linux · ARM64",
        "Windows · x86_64",
        "Windows · ARM64",
        "macOS · Intel / Apple Silicon",
    ]);
    host.set_selected(
        host_names
            .iter()
            .position(|h| *h == catalog::host())
            .unwrap_or(0) as u32,
    );
    let advanced = gtk::Expander::builder()
        .label(tr("Advanced · host system"))
        .child(&host)
        .build();
    let target = gtk::DropDown::from_strings(&[
        tr("Desktop Qt"),
        "Android",
        "WebAssembly",
        tr("Qt Creator & tools"),
    ]);
    let target_keys = ["desktop", "android", "wasm", "tools"];
    let repositories = gtk::DropDown::from_strings(&[]);
    repositories.set_hexpand(true);
    let selectors = gtk::Grid::builder()
        .column_spacing(12)
        .row_spacing(8)
        .build();
    for (index, (title, control)) in [
        (tr("Build for"), &target),
        (tr("Version / tool"), &repositories),
    ]
    .iter()
    .enumerate()
    {
        let label = gtk::Label::new(Some(title));
        label.set_xalign(0.0);
        selectors.attach(&label, 0, index as i32, 1, 1);
        selectors.attach(*control, 1, index as i32, 1, 1);
    }
    let lifecycle = gtk::Label::new(None);
    lifecycle.set_markup(
        &glib::markup_escape_text(tr(
            "Mirrors may not sync Qt versions that are no longer supported. See endoflife.date.",
        ))
        .replace(
            "endoflife.date",
            "<a href=\"https://endoflife.date/qt\">endoflife.date</a>",
        ),
    );
    lifecycle.set_xalign(0.0);
    lifecycle.set_wrap(true);
    lifecycle.set_tooltip_text(Some(tr("View Qt release and support dates")));
    selectors.attach(&lifecycle, 1, 2, 1, 1);
    target.connect_selected_notify(move |dropdown| {
        lifecycle.set_visible(dropdown.selected() != 3);
    });
    install_page.append(&selectors);
    install_page.append(&advanced);
    let catalog_status = gtk::Label::new(Some(tr("Loading versions…")));
    catalog_status.set_xalign(0.0);
    catalog_status.set_wrap(true);
    let retry = gtk::Button::with_label(tr("Retry"));
    retry.set_visible(false);
    retry.set_halign(gtk::Align::Start);
    install_page.append(&catalog_status);
    install_page.append(&retry);
    let beginner_hint = gtk::Label::new(Some(&beginner_guidance(
        host_names[host.selected() as usize],
    )));
    beginner_hint.set_xalign(0.0);
    beginner_hint.set_wrap(true);
    beginner_hint.add_css_class("dim-label");
    beginner_hint.set_visible(target.selected() == 0);
    install_page.append(&beginner_hint);
    {
        let hint = beginner_hint.clone();
        host.connect_selected_notify(move |dropdown| {
            if let Some(host) = host_names.get(dropdown.selected() as usize) {
                hint.set_text(&beginner_guidance(host));
            }
        });
    }
    target.connect_selected_notify(move |dropdown| {
        beginner_hint.set_visible(dropdown.selected() == 0);
    });
    let search = gtk::SearchEntry::builder()
        .placeholder_text(tr("Search components"))
        .build();
    install_page.append(&search);
    let component_list = gtk::ListBox::new();
    component_list.set_selection_mode(gtk::SelectionMode::None);
    component_list.add_css_class("boxed-list");
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .min_content_height(180)
        .child(&component_list)
        .build();
    install_page.append(&scroll);
    let destination = gtk::Label::new(Some(tr("Install to")));
    destination.set_xalign(0.0);
    let name = gtk::Entry::builder()
        .placeholder_text(tr("Installation folder name"))
        .build();
    name.set_hexpand(true);
    let destination_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    destination_row.append(&destination);
    destination_row.append(&name);
    install_page.append(&destination_row);
    let full_path = gtk::Label::new(None);
    full_path.set_xalign(0.0);
    full_path.set_selectable(true);
    full_path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    install_page.append(&full_path);
    let recommendations =
        gtk::CheckButton::with_label(tr("Install optional recommended components for me"));
    recommendations.set_tooltip_text(Some(tr("Include recommended Qt Creator, documentation and examples. Review the full list and download size before installing.")));
    install_page.append(&recommendations);
    let summary = gtk::Label::new(None);
    summary.set_xalign(0.0);
    summary.set_hexpand(true);
    let review = gtk::Button::with_label(tr("Next"));
    review.add_css_class("suggested-action");
    review.set_sensitive(false);
    let action_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    action_row.append(&summary);
    action_row.append(&review);
    install_page.append(&action_row);
    stack.add_titled(&install_page, Some("install"), tr("Install Qt"));

    let installed_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    margins(&installed_page, 24);
    let component_removal_hint = gtk::Label::new(Some(tr(
        "You can add components or uninstall an entire installation. Removing individual components is not supported.",
    )));
    component_removal_hint.set_xalign(0.0);
    component_removal_hint.set_wrap(true);
    component_removal_hint.add_css_class("dim-label");
    installed_page.append(&component_removal_hint);
    let installed_list = gtk::ListBox::new();
    installed_list.set_selection_mode(gtk::SelectionMode::None);
    installed_list.add_css_class("boxed-list");
    let installed_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .child(&installed_list)
        .build();
    installed_page.append(&installed_scroll);
    let reload = gtk::Button::with_label(tr("Refresh installations"));
    installed_page.append(&reload);
    stack.add_titled(&installed_page, Some("installed"), tr("Installed"));

    let footer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    margins(&footer, 12);
    let status = gtk::Label::new(Some(tr("Ready")));
    status.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    status.set_xalign(0.0);
    let progress_bar = gtk::ProgressBar::new();
    let cancel_button = gtk::Button::with_label(tr("Cancel"));
    cancel_button.set_sensitive(false);
    cancel_button.set_halign(gtk::Align::End);
    let log = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    let log_scroll = gtk::ScrolledWindow::builder()
        .height_request(110)
        .child(&log)
        .build();
    let expander = gtk::Expander::builder()
        .label(tr("Details"))
        .child(&log_scroll)
        .build();
    footer.append(&status);
    footer.append(&progress_bar);
    footer.append(&cancel_button);
    footer.append(&expander);
    toolbar.add_bottom_bar(&footer);
    let (sender, receiver) = async_channel::bounded(256);
    let worker = Worker {
        sender,
        cancel: Rc::new(RefCell::new(Cancel::default())),
        busy: Arc::new(AtomicBool::new(false)),
    };
    {
        let worker = worker.clone();
        cancel_button.connect_clicked(move |_| worker.cancel.borrow().cancel());
    }
    let repo_state = Rc::new(RefCell::new(Vec::<Repository>::new()));
    let repo_sources = Rc::new(RefCell::new(Vec::<Repository>::new()));
    let package_state = Rc::new(RefCell::new(Vec::<Package>::new()));
    let choices = Rc::new(RefCell::new(Vec::<(String, gtk::CheckButton)>::new()));
    let clear: Rc<dyn Fn()> = {
        let choices = choices.clone();
        let packages = package_state.clone();
        let list = component_list.clone();
        Rc::new(move || {
            choices.borrow_mut().clear();
            packages.borrow_mut().clear();
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
        })
    };
    // Requests are serialized through the worker. A generation prevents stale
    // results from repopulating a page after a selection or mirror change.
    let generation = Rc::new(Cell::new(0u64));
    let pending = Rc::new(Cell::new(Some(false))); // false: versions, true: components
    let last_request = Rc::new(Cell::new(false));
    let reset: Rc<dyn Fn(bool)> = {
        let clear = clear.clone();
        let generation = generation.clone();
        let pending = pending.clone();
        let repo_state = repo_state.clone();
        let repositories = repositories.clone();
        let catalog_status = catalog_status.clone();
        let retry = retry.clone();
        let search = search.clone();
        let review = review.clone();
        Rc::new(move |components| {
            review.set_sensitive(false);
            generation.set(generation.get() + 1);
            clear();
            search.set_text("");
            retry.set_visible(false);
            if !components {
                repo_state.borrow_mut().clear();
                repositories.set_model(Some(&gtk::StringList::new(&[])));
            }
            catalog_status.set_text(if components {
                tr("Loading components… Previous selections have been cleared.")
            } else {
                tr("Loading versions…")
            });
            pending.set(Some(components));
        })
    };
    {
        let reset = reset.clone();
        mirror_url.connect_changed(move |_| reset(false));
    }
    {
        let reset = reset.clone();
        host.connect_selected_notify(move |_| reset(false));
    }
    {
        let reset = reset.clone();
        target.connect_selected_notify(move |_| reset(false));
    }
    {
        let reset = reset.clone();
        let state = repo_state.clone();
        repositories.connect_selected_notify(move |dropdown| {
            if (dropdown.selected() as usize) < state.borrow().len() {
                reset(true);
            }
        });
    }
    {
        let reset = reset.clone();
        let last_request = last_request.clone();
        retry.connect_clicked(move |_| reset(last_request.get()));
    }
    {
        let list = component_list.clone();
        search.connect_search_changed(move |search| {
            let query = search.text().to_lowercase();
            let mut child = list.first_child();
            while let Some(row) = child {
                child = row.next_sibling();
                row.set_visible(row.widget_name().to_lowercase().contains(&query));
            }
        });
    }
    {
        let worker = worker.clone();
        let choices = choices.clone();
        let state = package_state.clone();
        let mirror_url = mirror_url.clone();
        let root = root.clone();
        let name = name.clone();
        let recommendations = recommendations.clone();
        review.connect_clicked(move |_| {
            let selected = choices
                .borrow()
                .iter()
                .filter(|(_, button)| button.is_active())
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            if selected.is_empty()
                || !presentation::valid_folder(root.text().as_str(), name.text().as_str())
            {
                return;
            }
            let include_recommendations = recommendations.is_active();
            let packages = state.borrow().clone();
            let mirror = mirror_url.text().to_string();
            let root = PathBuf::from(root.text().as_str());
            let name = name.text().to_string();
            worker.run(Box::new(move |cancel, progress| {
                let net = Network::new(&mirror, cancel, progress)?;
                let existing = install::at_path(&root.join(&name))?;
                let mut packages = packages;
                if let Some(old) = &existing {
                    packages.retain(|p| !old.plan.packages.iter().any(|o| o.id == p.id));
                    packages.extend(old.plan.packages.clone());
                }
                let mut plan = runtime().block_on(catalog::prepare_plan_with_recommendations(
                    &net,
                    packages,
                    &selected,
                    include_recommendations,
                ))?;
                if let Some(old) = &existing {
                    plan = install::merge_plan(old, plan)?;
                    anyhow::ensure!(
                        plan.packages.iter().any(|p| !old
                            .plan
                            .packages
                            .iter()
                            .any(|o| o.id == p.id)),
                        tr("All selected components are already installed.")
                    );
                }
                Ok(Message::Review(plan, root, name, existing.map(Box::new)))
            }));
        });
    }
    {
        let worker = worker.clone();
        let window = window.clone();
        let list = installed_list.clone();
        reload.connect_clicked(move |_| render_installed(&list, &window, &worker));
    }
    render_installed(&installed_list, &window, &worker);
    {
        let window = window.clone();
        about.connect_clicked(move |_| {
        let dialog = adw::AboutDialog::builder().application_name("Qit").application_icon("io.github.taoky.Qit").version(qit_core::VERSION).developer_name("Qit contributors").license_type(gtk::License::MitX11)
            .comments(tr("A GTK installer for Qt.\nWe identify our downloads so mirror operators don’t have to guess. Independent of The Qt Company.")).issue_url(qit_core::ISSUE_URL).build(); dialog.present(Some(&window));
    });
    }
    {
        let busy = worker.busy.clone();
        let cancel = worker.cancel.clone();
        let window_clone = window.clone();
        window.connect_close_request(move |_| {
            if busy.load(Ordering::SeqCst) {
                cancel.borrow().cancel();
                notice(
                    &window_clone,
                    tr("Cancelling"),
                    tr("Wait for the operation to stop safely, then close the window."),
                );
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    let busy = worker.busy.clone();
    let cancel_tick = cancel_button.clone();
    let progress_tick = progress_bar.clone();
    let review_tick = review.clone();
    {
        let worker = worker.clone();
        let pending = pending.clone();
        let last_request = last_request.clone();
        let generation = generation.clone();
        let repo_state = repo_state.clone();
        let repositories = repositories.clone();
        let mirror_url = mirror_url.clone();
        let choices = choices.clone();
        let package_state = package_state.clone();
        let root = root.clone();
        let name = name.clone();
        let retry = retry.clone();
        let catalog_status = catalog_status.clone();
        let component_list = component_list.clone();
        let repo_sources = repo_sources.clone();
        let mirror_snapshot = settings::mirrors();
        let stack = stack.clone();
        let mut selection_context = None;
        let mut installed_ids = std::collections::BTreeSet::<String>::new();
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            if !busy.load(Ordering::SeqCst)
                && stack.visible_child_name().as_deref() == Some("install")
                && let Some(components) = pending.take()
            {
                let url = mirror_url.text().to_string();
                let host_key = host_names[host.selected() as usize].to_string();
                let target_key = target_keys[target.selected() as usize].to_string();
                let repo = repo_state
                    .borrow()
                    .get(repositories.selected() as usize)
                    .cloned();
                let matching_repos = repo
                    .as_ref()
                    .map(|selected| {
                        repo_sources
                            .borrow()
                            .iter()
                            .filter(|r| r.name == selected.name)
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let epoch = generation.get();
                last_request.set(components);
                if components && let Some(repo) = &repo {
                    name.set_text(&presentation::folder_name_for_host(
                        repo,
                        &target_key,
                        &host_key,
                    ));
                }
                worker.run(Box::new(move |cancel, progress| {
                    let result = (|| -> anyhow::Result<Message> {
                        let net = Network::new(&url, cancel, progress)?;
                        runtime().block_on(async {
                            if components {
                                let repo =
                                    repo.ok_or_else(|| anyhow::anyhow!("Select a version first"))?;
                                let mut packages: Vec<Package> = Vec::new();
                                for source in &matching_repos {
                                    for package in catalog::packages(&net, &source.path).await? {
                                        if !packages.iter().any(|p| p.id == package.id) {
                                            packages.push(package);
                                        }
                                    }
                                }
                                // Discover version-matched common repositories, including
                                // source archives with platform-specific line endings.
                                if target_key != "tools" {
                                    let common_root = "online/qtsdkrepository/all_os/qt/";
                                    if let Some(html) = net.optional_text(common_root).await? {
                                        for name in
                                            catalog::directories_at(&html, &net.url(common_root)?)
                                        {
                                            if !presentation::common_repository(
                                                &repo.name, &name, &host_key,
                                            ) {
                                                continue;
                                            }
                                            for p in catalog::packages(
                                                &net,
                                                &format!("{common_root}{name}/"),
                                            )
                                            .await?
                                            {
                                                if !packages
                                                    .iter()
                                                    .any(|existing| existing.id == p.id)
                                                {
                                                    packages.push(p);
                                                }
                                            }
                                        }
                                    }
                                }
                                Ok(Message::Packages(epoch, packages))
                            } else {
                                let group = if target_key == "tools" {
                                    "desktop"
                                } else {
                                    &target_key
                                };
                                let mut items =
                                    catalog::repositories(&net, &host_key, group).await?;
                                items.retain(|r| {
                                    if target_key == "tools" {
                                        r.name.starts_with("tools_")
                                    } else {
                                        r.name.starts_with("qt6_") || r.name.starts_with("qt5_515")
                                    }
                                });
                                presentation::sort_repositories(&mut items);
                                Ok(Message::Repositories(epoch, items))
                            }
                        })
                    })();
                    Ok(result.unwrap_or_else(|e| Message::CatalogError(epoch, format!("{e:#}"))))
                }));
            }
            let active = busy.load(Ordering::SeqCst);
            settings_page.set_sensitive(!active);
            selectors.set_sensitive(!active);
            advanced.set_sensitive(!active);
            change_source.set_sensitive(!active);
            component_list.set_sensitive(!active);
            recommendations.set_sensitive(!active);
            name.set_sensitive(!active);
            cancel_tick.set_sensitive(active);
            cancel_tick.set_visible(active);
            progress_tick.set_visible(active);
            retry.set_sensitive(!active);
            let path = PathBuf::from(root.text().as_str()).join(name.text().as_str());
            let owner = std::fs::read_to_string(path.join(".qit-owner")).ok();
            let record_stamp = owner
                .as_ref()
                .filter(|id| qit_core::archive::safe_relative(id).is_ok() && !id.contains('/'))
                .and_then(|id| {
                    std::fs::metadata(
                        settings::data_dir()
                            .join("installations")
                            .join(format!("{id}.json")),
                    )
                    .ok()
                })
                .map(|meta| (meta.modified().ok(), meta.len()));
            let context = (
                path.clone(),
                owner,
                record_stamp,
                generation.get(),
                choices.borrow().len(),
            );
            if !active && selection_context.as_ref() != Some(&context) {
                let previous = std::mem::take(&mut installed_ids);
                if let Ok(Some(existing)) = install::at_path(&path) {
                    installed_ids.extend(existing.plan.packages.into_iter().map(|p| p.id));
                }
                for (id, check) in choices.borrow().iter() {
                    let installed = installed_ids.contains(id);
                    if installed || previous.contains(id) {
                        check.set_active(installed);
                    }
                    if let Some(package) = package_state.borrow().iter().find(|p| &p.id == id) {
                        check.set_sensitive(!installed && package.unsupported.is_none());
                        check.set_tooltip_text(installed.then(|| {
                            tr("Already installed. Individual component removal is not supported.")
                        }));
                        if let Some(row) = check
                            .ancestor(adw::ActionRow::static_type())
                            .and_then(|widget| widget.downcast::<adw::ActionRow>().ok())
                        {
                            row.set_subtitle(&format!(
                                "{} · {:.1} MiB{}",
                                package
                                    .version
                                    .split('-')
                                    .next()
                                    .unwrap_or(&package.version),
                                package.compressed_size as f64 / 1048576.0,
                                if installed {
                                    tr(" · Installed")
                                } else if package.unsupported.is_some() {
                                    tr(" · Unavailable")
                                } else {
                                    ""
                                }
                            ));
                        }
                    }
                }
                selection_context = Some(context);
            }
            let selected: Vec<String> = choices
                .borrow()
                .iter()
                .filter(|(_, b)| b.is_active())
                .map(|(id, _)| id.clone())
                .collect();
            let size: u64 = package_state
                .borrow()
                .iter()
                .filter(|p| selected.contains(&p.id) && !installed_ids.contains(&p.id))
                .map(|p| p.compressed_size)
                .sum();
            summary.set_text(&tr_args(
                if installed_ids.is_empty() {
                    "{count} selected · {size} MiB (before dependencies)"
                } else {
                    "{count} new components selected · {size} MiB (before dependencies)"
                },
                &[
                    (
                        "count",
                        &selected
                            .iter()
                            .filter(|id| !installed_ids.contains(*id))
                            .count()
                            .to_string(),
                    ),
                    ("size", &format!("{:.1}", size as f64 / 1048576.0)),
                ],
            ));
            let valid = presentation::valid_folder(root.text().as_str(), name.text().as_str());
            full_path.set_text(&format!(
                "{}{}",
                path.display(),
                if valid && install::managed_destination(&path) {
                    tr(" · Additional components will be added to this installation")
                } else if valid {
                    ""
                } else {
                    tr(" · Choose a new, valid folder")
                }
            ));
            full_path.set_tooltip_text(Some(&path.to_string_lossy()));
            review_tick.set_sensitive(
                !active
                    && pending.get().is_none()
                    && !selected.is_empty()
                    && valid
                    && (selected.iter().any(|id| !installed_ids.contains(id))
                        || recommendations.is_active()),
            );
            let url = mirror_url.text();
            let source = mirror_snapshot
                .mirrors
                .iter()
                .find(|m| m.url == url)
                .map(|m| m.operator.as_str())
                .unwrap_or(url.as_str());
            source_label.set_text(&tr_args("Download source: {source}", &[("source", source)]));
            source_label.set_tooltip_text(Some(url.as_str()));
            if url.is_empty() && !active {
                catalog_status.set_text(tr("Choose a download source in Settings."));
            }
            glib::ControlFlow::Continue
        });
    }
    let window_receiver = window.clone();
    let worker_receiver = worker.clone();
    stack.set_visible_child_name(if mirror_url.text().is_empty() {
        "settings"
    } else {
        "install"
    });
    glib::spawn_future_local(async move {
        let mut last_download = (String::new(), 0u64, std::time::Instant::now());
        while let Ok(message) = receiver.recv().await {
            match message {
                Message::Event(e) => {
                    let mut text =
                        format!("{} · {}", i18n::phase(&e.phase), i18n::message(&e.message));
                    if e.phase == "download" {
                        if last_download.0 == e.message {
                            let elapsed = last_download.2.elapsed().as_secs_f64();
                            if elapsed > 0.0 {
                                text = format!(
                                    "{text} · {:.1} MiB · {:.1} MiB/s",
                                    e.done as f64 / 1048576.0,
                                    e.done.saturating_sub(last_download.1) as f64
                                        / elapsed
                                        / 1048576.0
                                );
                            }
                        }
                        last_download = (e.message, e.done, std::time::Instant::now());
                    } else {
                        let buffer = log.buffer();
                        buffer.insert(&mut buffer.end_iter(), &format!("{text}\n"));
                        if buffer.line_count() > 1500 {
                            let mut start = buffer.start_iter();
                            if let Some(mut end) = buffer.iter_at_line(200) {
                                buffer.delete(&mut start, &mut end);
                            }
                        }
                    }
                    status.set_text(&text);
                    if let Some(total) = e.total.filter(|n| *n > 0) {
                        progress_bar.set_fraction((e.done as f64 / total as f64).min(1.0));
                    } else {
                        progress_bar.pulse();
                    }
                }
                Message::Repositories(epoch, mut items) => {
                    if epoch != generation.get() {
                        continue;
                    }
                    *repo_sources.borrow_mut() = items.clone();
                    items.dedup_by(|a, b| a.name == b.name);
                    let labels = items
                        .iter()
                        .map(presentation::repository_name)
                        .collect::<Vec<_>>();
                    *repo_state.borrow_mut() = items;
                    repositories.set_model(Some(&gtk::StringList::new(
                        &labels.iter().map(String::as_str).collect::<Vec<_>>(),
                    )));
                    if repo_state.borrow().is_empty() {
                        catalog_status.set_text(tr(
                            "No matching versions on this mirror. Try another source.",
                        ));
                        retry.set_visible(true);
                    } else {
                        repositories.set_selected(0);
                        reset(true);
                    }
                }
                Message::Packages(epoch, mut items) => {
                    if epoch != generation.get() {
                        continue;
                    }
                    items.sort_by_key(|p| (presentation::group(p), p.name.clone()));
                    let groups = [
                        tr("Development kits"),
                        tr("Additional modules"),
                        tr("Source code"),
                        tr("Documentation & examples"),
                        tr("Debug symbols"),
                        tr("Developer tools"),
                    ];
                    let mut previous_group = None;
                    for item in &items {
                        if item.archives.is_empty() {
                            continue;
                        }
                        let group = presentation::group(item);
                        if previous_group != Some(group) {
                            let label = gtk::Label::new(Some(groups[group]));
                            label.set_xalign(0.0);
                            label.add_css_class("heading");
                            margins(&label, 12);
                            component_list.append(&label);
                            label.parent().unwrap().set_widget_name(groups[group]);
                            previous_group = Some(group);
                        }
                        let check = gtk::CheckButton::new();
                        check.set_valign(gtk::Align::Center);
                        check.set_sensitive(item.unsupported.is_none());
                        let row = adw::ActionRow::builder()
                            .title(presentation::component_name(item))
                            .subtitle(format!(
                                "{} · {:.1} MiB{}",
                                item.version.split('-').next().unwrap_or(&item.version),
                                item.compressed_size as f64 / 1048576.0,
                                if item.unsupported.is_some() {
                                    tr(" · Unavailable")
                                } else {
                                    ""
                                }
                            ))
                            .build();
                        row.set_use_markup(false);
                        row.add_prefix(&check);
                        row.set_activatable_widget(Some(&check));
                        row.set_tooltip_text(Some(&format!(
                            "{}\n{}",
                            item.id,
                            item.unsupported.as_deref().unwrap_or(&item.description)
                        )));
                        row.set_widget_name(&format!(
                            "{} {} {}",
                            item.name, item.id, groups[group]
                        ));
                        component_list.append(&row);
                        choices.borrow_mut().push((item.id.clone(), check));
                    }
                    let empty = choices.borrow().is_empty();
                    let tools_only = items.iter().all(|p| p.id.starts_with("qt.tools."));
                    *package_state.borrow_mut() = items;
                    catalog_status.set_text(if empty { tr("No installable components in this version.") } else if tools_only { tr("Select the tools to install. Dependencies are checked at the next step.") } else { tr("Select a development kit and optional modules. Required dependencies are added at the next step. Changing version clears selections.") });
                }
                Message::CatalogError(epoch, error) => {
                    if epoch != generation.get() {
                        continue;
                    }
                    catalog_status
                        .set_text(&format!("{}\n{error}", tr("Could not load the catalog.")));
                    retry.set_visible(true);
                }
                Message::Review(plan, root, name, existing) => confirm(
                    &window_receiver,
                    &worker_receiver,
                    plan,
                    root,
                    name,
                    existing,
                ),
                Message::Installed => {
                    render_installed(&installed_list, &window_receiver, &worker_receiver);
                    status.set_text(tr("Installation complete"));
                    progress_bar.set_fraction(1.0);
                    stack.set_visible_child_name("installed");
                }
                Message::Removed(files) => {
                    render_installed(&installed_list, &window_receiver, &worker_receiver);
                    notice(
                        &window_receiver,
                        tr("Uninstalled"),
                        &if files.is_empty() {
                            tr("Installation removed.").into()
                        } else {
                            format!(
                                "{}\n{}",
                                tr("Preserved modified or additional files:"),
                                files.join("\n")
                            )
                        },
                    );
                }
                Message::Error(error) => {
                    status.set_text(&error);
                    notice(&window_receiver, tr("Operation stopped"), &error);
                }
                Message::Done => status.set_text(tr("Ready")),
            }
        }
    });
    window.present();
    if let Some(error) = settings_error {
        notice(&window, tr("Settings error"), &error);
    }
    if let Ok(seconds) = std::env::var("QIT_SMOKE_EXIT_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or(())
    {
        let app = app.clone();
        glib::timeout_add_local_once(std::time::Duration::from_secs(seconds), move || app.quit());
    }
}
fn confirm(
    window: &adw::ApplicationWindow,
    worker: &Worker,
    plan: Plan,
    root: PathBuf,
    name: String,
    existing: Option<Box<install::Installation>>,
) {
    let size: u64 = plan.packages.iter().map(|p| p.compressed_size).sum();
    let text = format!(
        "{}\n{}\n\n{}\n\n{}",
        root.join(&name).display(),
        plan.mirror,
        plan.packages
            .iter()
            .map(|p| {
                let suffix = if existing
                    .as_ref()
                    .is_some_and(|old| old.plan.packages.iter().any(|o| o.id == p.id))
                {
                    tr(" (already installed)")
                } else if existing.is_some() {
                    tr(" (new component)")
                } else if plan.selected.contains(&p.id) {
                    ""
                } else {
                    tr(" (automatically included)")
                };
                format!("{} ({}){suffix}", p.name, p.id)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        plan.notes
            .iter()
            .map(|note| i18n::plan_note(note))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let dialog = adw::AlertDialog::builder()
        .heading(tr(if existing.is_some() {
            "Review additional components"
        } else {
            "Review installation"
        }))
        .body(format!(
            "{}\n{text}",
            if existing.is_some() {
                tr_args(
                    "Combined archive size: {size} MiB (cached files reused)",
                    &[("size", &format!("{:.1}", size as f64 / 1048576.0))],
                )
            } else {
                format!("{:.1} MiB", size as f64 / 1048576.0)
            }
        ))
        .build();
    let extra = gtk::Box::new(gtk::Orientation::Vertical, 8);
    if existing.is_some() {
        let hint = gtk::Label::new(Some(tr(
            "Existing components are kept. Qit prepares the combined installation in a temporary folder using cached downloads; enough space for another copy is required. Close Qt tools before continuing.",
        )));
        hint.set_wrap(true);
        extra.append(&hint);
    }
    extra.append(&gtk::LinkButton::with_label(
        "https://www.qt.io/licensing/",
        tr("Review Qt licensing"),
    ));
    if !plan.license_texts.is_empty() {
        let text = gtk::TextView::builder()
            .editable(false)
            .wrap_mode(gtk::WrapMode::WordChar)
            .build();
        text.buffer().set_text(
            &plan
                .license_texts
                .iter()
                .map(|(name, text)| format!("{name}\n\n{text}"))
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
        let scroll = gtk::ScrolledWindow::builder()
            .height_request(180)
            .child(&text)
            .build();
        extra.append(&scroll);
    }
    let accept = gtk::CheckButton::with_label(tr("I accept the applicable component licenses"));
    extra.append(&accept);
    dialog.set_extra_child(Some(&extra));
    dialog.add_response("cancel", tr("Cancel"));
    dialog.add_response(
        "install",
        tr(if existing.is_some() {
            "Add components"
        } else {
            "Install"
        }),
    );
    dialog.set_response_appearance("install", adw::ResponseAppearance::Suggested);
    dialog.set_response_enabled("install", false);
    {
        let dialog = dialog.clone();
        accept.connect_toggled(move |button| {
            dialog.set_response_enabled("install", button.is_active())
        });
    }
    let worker = worker.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "install" {
            let plan = plan.clone();
            let root = root.clone();
            let name = name.clone();
            let existing = existing.clone();
            worker.run(Box::new(move |cancel, progress| {
                if let Some(old) = existing {
                    runtime().block_on(install::extend(plan, &old, cancel, progress))?;
                } else {
                    runtime().block_on(install::install(plan, &root, &name, cancel, progress))?;
                }
                Ok(Message::Installed)
            }));
        }
    });
    dialog.present(Some(window));
}
fn render_installed(list: &gtk::ListBox, window: &adw::ApplicationWindow, worker: &Worker) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    match install::list() {
        Err(error) => notice(window, tr("Cannot read installations"), &error.to_string()),
        Ok(items) if items.is_empty() => {
            let empty = gtk::Label::new(Some(tr("No Qt installations yet")));
            margins(&empty, 24);
            list.append(&empty);
        }
        Ok(items) => {
            for item in items {
                let row = adw::ActionRow::builder()
                    .title(item.path.file_name().unwrap_or_default().to_string_lossy())
                    .subtitle(item.path.to_string_lossy())
                    .build();
                let open = gtk::Button::from_icon_name("folder-open-symbolic");
                open.set_valign(gtk::Align::Center);
                open.set_tooltip_text(Some(tr("Open directory")));
                {
                    let path = item.path.clone();
                    open.connect_clicked(move |_| {
                        let file = gio::File::for_path(&path);
                        let _ = gio::AppInfo::launch_default_for_uri(
                            file.uri().as_str(),
                            None::<&gio::AppLaunchContext>,
                        );
                    });
                }
                let copy = gtk::Button::from_icon_name("edit-copy-symbolic");
                copy.set_valign(gtk::Align::Center);
                copy.set_tooltip_text(Some(tr("Copy environment instructions")));
                {
                    let path = item.path.clone();
                    copy.connect_clicked(move |button| {
                        if let Ok(text) = std::fs::read_to_string(path.join("QIT-ENVIRONMENT.txt"))
                        {
                            button.clipboard().set_text(&text);
                        }
                    });
                }
                let remove = gtk::Button::from_icon_name("user-trash-symbolic");
                remove.set_valign(gtk::Align::Center);
                remove.set_tooltip_text(Some(tr("Uninstall entire installation")));
                {
                    let window = window.clone();
                    let worker = worker.clone();
                    remove.connect_clicked(move |_| {
                        let dialog = adw::AlertDialog::builder()
                            .heading(tr("Uninstall this entire Qt installation?"))
                            .body(item.path.to_string_lossy())
                            .build();
                        dialog.add_response("cancel", tr("Cancel"));
                        dialog.add_response("remove", tr("Uninstall"));
                        dialog.set_response_appearance(
                            "remove",
                            adw::ResponseAppearance::Destructive,
                        );
                        let worker = worker.clone();
                        let id = item.id.clone();
                        dialog.connect_response(None, move |_, response| {
                            if response == "remove" {
                                let id = id.clone();
                                worker.run(Box::new(move |cancel, progress| {
                                    Ok(Message::Removed(install::uninstall(
                                        &id, &cancel, &progress,
                                    )?))
                                }));
                            }
                        });
                        dialog.present(Some(&window));
                    });
                }
                row.add_suffix(&open);
                row.add_suffix(&copy);
                row.add_suffix(&remove);
                list.append(&row);
            }
        }
    }
}

#[cfg(test)]
mod ui_tests;
