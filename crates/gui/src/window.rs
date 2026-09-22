use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gio, glib};
use yaccgl_core::api::{self, GamePackage};
use yaccgl_core::install::{self, Progress, Status};
use yaccgl_core::settings::Settings;
use yaccgl_core::space::{self, human};
use yaccgl_core::steam::{self, ShortcutSpec, Steam, User, process};
use yaccgl_core::{Error, GAME_NAME, http};

use crate::APP_ID;

const WINDOW_SIZES: [((u32, u32), &str); 4] = [
    ((1280, 800), "1280 × 800 (Deck)"),
    ((1920, 1080), "1920 × 1080"),
    ((2560, 1440), "2560 × 1440"),
    ((3840, 2160), "3840 × 2160"),
];

const BUILTIN_TOOLS: [(&str, &str); 3] = [
    ("proton_10", "Proton 10.0"),
    ("proton_experimental", "Proton Experimental"),
    ("proton_9", "Proton 9.0"),
];

enum Latest {
    Checking,
    Known(GamePackage),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Task {
    Install,
    Steam,
    /// The game is running from the Log In button.
    Game,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    None,
    Retry,
    Install,
    Update,
    AddToSteam,
}

/// Throughput over the current stage, for speed and ETA display.
struct Rate {
    stage: &'static str,
    since: Instant,
    from: u64,
}

struct State {
    settings: Settings,
    latest: Latest,
    task: Option<Task>,
    cancel: Arc<AtomicBool>,
    steam: Option<Steam>,
    users: Vec<User>,
    tools: Vec<(String, String)>,
    action: Action,
    rate: Option<Rate>,
}

struct Ui {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    banner: adw::Banner,
    status_title: gtk::Label,
    status_detail: gtk::Label,
    progress: gtk::ProgressBar,
    progress_label: gtk::Label,
    primary: gtk::Button,
    cancel: gtk::Button,
    refresh: gtk::Button,
    dir_row: adw::ActionRow,
    dir_button: gtk::Button,
    version_row: adw::ActionRow,
    repair: gtk::Button,
    user_row: adw::ComboRow,
    tool_row: adw::ComboRow,
    artwork_row: adw::SwitchRow,
    launch_row: adw::EntryRow,
    single_row: adw::SwitchRow,
    size_row: adw::ComboRow,
    shortcut_row: adw::ActionRow,
    steam_button: gtk::Button,
    remove_button: gtk::Button,
    login_row: adw::ActionRow,
    login_button: gtk::Button,
    prefs: adw::PreferencesDialog,
}

pub struct Inner {
    ui: Ui,
    state: RefCell<State>,
    /// Set while combo models are rebuilt, so their change handlers don't fire.
    populating: Cell<bool>,
}

#[derive(Clone)]
pub struct App(Rc<Inner>);

impl std::ops::Deref for App {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

pub fn build(application: &adw::Application) {
    let ui = build_ui(application);
    let app = App(Rc::new(Inner {
        ui,
        state: RefCell::new(State {
            settings: Settings::load(),
            latest: Latest::Checking,
            task: None,
            cancel: Arc::new(AtomicBool::new(false)),
            steam: None,
            users: Vec::new(),
            tools: Vec::new(),
            action: Action::None,
            rate: None,
        }),
        populating: Cell::new(false),
    }));
    app.connect_signals();
    app.reload_steam();
    app.apply_prefix_settings(false);
    app.refresh();
    app.ui.window.present();
    app.check_latest();
}

fn build_ui(application: &adw::Application) -> Ui {
    let menu = gio::Menu::new();
    menu.append(Some("Preferences"), Some("win.preferences"));
    menu.append(Some("Open Install Folder"), Some("win.open-folder"));
    menu.append(Some("About"), Some("win.about"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .primary(true)
        .tooltip_text("Main Menu")
        .build();
    let refresh = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("Check for Updates")
        .build();
    let header = adw::HeaderBar::new();
    header.pack_start(&refresh);
    header.pack_end(&menu_button);

    let banner = adw::Banner::new("");

    // Status and the one "next step" button.
    let status_title = gtk::Label::builder().label(GAME_NAME).css_classes(["title-2"]).build();
    let status_detail = gtk::Label::builder()
        .css_classes(["dim-label"])
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    let progress = gtk::ProgressBar::builder().visible(false).margin_top(6).build();
    let progress_label = gtk::Label::builder()
        .css_classes(["caption", "dim-label", "numeric"])
        .visible(false)
        .build();
    let primary = gtk::Button::builder()
        .label("Install")
        .css_classes(["pill", "suggested-action"])
        .build();
    let cancel = gtk::Button::builder().label("Cancel").css_classes(["pill"]).visible(false).build();
    let buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .halign(gtk::Align::Center)
        .margin_top(6)
        .build();
    buttons.append(&primary);
    buttons.append(&cancel);
    let hero = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
    for w in [
        status_title.upcast_ref::<gtk::Widget>(),
        status_detail.upcast_ref(),
        progress.upcast_ref(),
        progress_label.upcast_ref(),
        buttons.upcast_ref(),
    ] {
        hero.append(w);
    }

    // Section 1: the game.
    let dir_row = adw::ActionRow::builder().title("Install location").build();
    let dir_button = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .valign(gtk::Align::Center)
        .tooltip_text("Choose Install Location")
        .css_classes(["flat"])
        .build();
    dir_row.add_suffix(&dir_button);
    dir_row.set_activatable_widget(Some(&dir_button));
    let version_row = adw::ActionRow::builder().title("Version").build();
    let game_group = adw::PreferencesGroup::builder().title("Game").build();
    game_group.add(&dir_row);
    game_group.add(&version_row);

    // Section 2: Steam.
    let shortcut_row = adw::ActionRow::builder().title("Library shortcut").build();
    let remove_button = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .valign(gtk::Align::Center)
        .tooltip_text("Remove from Steam")
        .css_classes(["flat"])
        .build();
    let steam_button = gtk::Button::builder().label("Add").valign(gtk::Align::Center).build();
    shortcut_row.add_suffix(&remove_button);
    shortcut_row.add_suffix(&steam_button);
    let login_row = adw::ActionRow::builder()
        .title("Log in to Aniimo")
        .subtitle("Opens FunPlus login here so you can type your email")
        .build();
    let login_button = gtk::Button::builder().label("Log In").valign(gtk::Align::Center).build();
    login_row.add_suffix(&login_button);
    let steam_group = adw::PreferencesGroup::builder().title("Steam").build();
    steam_group.add(&shortcut_row);
    steam_group.add(&login_row);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_start(16)
        .margin_end(16)
        .margin_top(12)
        .margin_bottom(18)
        .build();
    content.append(&hero);
    content.append(&game_group);
    content.append(&steam_group);

    let clamp = adw::Clamp::builder().maximum_size(600).child(&content).build();
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&scroller));

    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.add_top_bar(&banner);
    view.set_content(Some(&toasts));

    // Settings live in their own dialog to keep the main window short.
    let user_row = adw::ComboRow::builder().title("Steam account").build();
    let tool_row = adw::ComboRow::builder()
        .title("Proton version")
        .subtitle("Aniimo currently only runs on Proton 10")
        .build();
    let artwork_row = adw::SwitchRow::builder()
        .title("Library artwork")
        .subtitle("Use the game's official Steam artwork")
        .build();
    let launch_row = adw::EntryRow::builder().title("Launch options").show_apply_button(true).build();
    let shortcut_prefs = adw::PreferencesGroup::builder()
        .title("Steam shortcut")
        .description("Applied when you add or update the shortcut.")
        .build();
    shortcut_prefs.add(&user_row);
    shortcut_prefs.add(&tool_row);
    shortcut_prefs.add(&artwork_row);
    shortcut_prefs.add(&launch_row);

    let single_row = adw::SwitchRow::builder()
        .title("Single window")
        .subtitle("Runs the game inside one fixed-size window")
        .build();
    let size_row = adw::ComboRow::builder()
        .title("Window size")
        .model(&gtk::StringList::new(&WINDOW_SIZES.map(|(_, label)| label)))
        .build();
    let game_mode_prefs = adw::PreferencesGroup::builder()
        .title("Window")
        .description("Takes effect the next time Aniimo starts.")
        .build();
    game_mode_prefs.add(&single_row);
    game_mode_prefs.add(&size_row);

    let repair_row = adw::ActionRow::builder()
        .title("Repair game files")
        .subtitle("Download and unpack the game again")
        .build();
    let repair = gtk::Button::builder().label("Repair").valign(gtk::Align::Center).build();
    repair_row.add_suffix(&repair);
    let files_prefs = adw::PreferencesGroup::builder().title("Game files").build();
    files_prefs.add(&repair_row);

    let prefs_page = adw::PreferencesPage::new();
    prefs_page.add(&shortcut_prefs);
    prefs_page.add(&game_mode_prefs);
    prefs_page.add(&files_prefs);
    let prefs = adw::PreferencesDialog::builder().title("Preferences").build();
    prefs.add(&prefs_page);

    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("Yet Another Creature Collector Game Launcher")
        .icon_name(APP_ID)
        .default_width(640)
        .default_height(600)
        .content(&view)
        .build();

    Ui {
        window,
        toasts,
        banner,
        status_title,
        status_detail,
        progress,
        progress_label,
        primary,
        cancel,
        refresh,
        dir_row,
        dir_button,
        version_row,
        repair,
        user_row,
        tool_row,
        artwork_row,
        launch_row,
        single_row,
        size_row,
        shortcut_row,
        steam_button,
        remove_button,
        login_row,
        login_button,
        prefs,
    }
}

impl App {
    fn connect_signals(&self) {
        let ui = &self.ui;
        let a = self.clone();
        ui.primary.connect_clicked(move |_| a.on_primary());
        let a = self.clone();
        ui.cancel.connect_clicked(move |_| a.state.borrow().cancel.store(true, Ordering::Relaxed));
        let a = self.clone();
        ui.refresh.connect_clicked(move |_| {
            a.reload_steam();
            a.check_latest();
        });
        let a = self.clone();
        ui.dir_button.connect_clicked(move |_| {
            let a = a.clone();
            glib::spawn_future_local(async move { a.choose_dir().await });
        });
        let a = self.clone();
        ui.repair.connect_clicked(move |_| {
            a.ui.prefs.close();
            a.start_install(true);
        });
        let a = self.clone();
        ui.steam_button.connect_clicked(move |_| {
            let a = a.clone();
            glib::spawn_future_local(async move { a.add_to_steam().await });
        });
        let a = self.clone();
        ui.login_button.connect_clicked(move |_| {
            let a = a.clone();
            glib::spawn_future_local(async move { a.log_in().await });
        });
        let a = self.clone();
        ui.remove_button.connect_clicked(move |_| {
            let a = a.clone();
            glib::spawn_future_local(async move { a.remove_from_steam().await });
        });

        let a = self.clone();
        ui.user_row.connect_selected_notify(move |row| {
            if a.populating.get() {
                return;
            }
            let id = a.state.borrow().users.get(row.selected() as usize).map(|u| u.account_id);
            a.update_settings(|s| s.steam_user = id);
        });
        let a = self.clone();
        ui.tool_row.connect_selected_notify(move |row| {
            if a.populating.get() {
                return;
            }
            let tool = a.state.borrow().tools.get(row.selected() as usize).map(|t| t.0.clone());
            if let Some(tool) = tool {
                a.update_settings(|s| s.compat_tool = tool);
                a.hint_update_shortcut();
            }
        });
        let a = self.clone();
        ui.artwork_row.connect_active_notify(move |row| {
            if !a.populating.get() {
                a.update_settings(|s| s.artwork = row.is_active());
                a.hint_update_shortcut();
            }
        });
        let a = self.clone();
        ui.single_row.connect_active_notify(move |row| {
            if !a.populating.get() {
                a.update_settings(|s| s.single_window = row.is_active());
                a.apply_prefix_settings(true);
            }
        });
        let a = self.clone();
        ui.size_row.connect_selected_notify(move |row| {
            if a.populating.get() {
                return;
            }
            if let Some(((w, h), _)) = WINDOW_SIZES.get(row.selected() as usize) {
                a.update_settings(|s| s.window_size = yaccgl_core::settings::format_size((*w, *h)));
                a.apply_prefix_settings(true);
            }
        });
        let a = self.clone();
        ui.launch_row.connect_apply(move |row| {
            let text = row.text().to_string();
            a.update_settings(|s| s.launch_options = text);
            a.hint_update_shortcut();
        });

        let prefs = gio::SimpleAction::new("preferences", None);
        let a = self.clone();
        prefs.connect_activate(move |_, _| a.ui.prefs.present(Some(&a.ui.window)));
        ui.window.add_action(&prefs);
        if let Some(app) = ui.window.application() {
            app.set_accels_for_action("win.preferences", &["<Control>comma"]);
        }

        let open = gio::SimpleAction::new("open-folder", None);
        let a = self.clone();
        open.connect_activate(move |_, _| {
            let dir = a.state.borrow().settings.install_dir.clone();
            if !dir.is_dir() {
                a.toast("The install folder doesn't exist yet.");
                return;
            }
            gtk::FileLauncher::new(Some(&gio::File::for_path(&dir))).launch(
                Some(&a.ui.window),
                gio::Cancellable::NONE,
                |_| {},
            );
        });
        ui.window.add_action(&open);

        let about = gio::SimpleAction::new("about", None);
        let a = self.clone();
        about.connect_activate(move |_, _| {
            adw::AboutDialog::builder()
                .application_name("Yet Another Creature Collector Game Launcher")
                .application_icon(APP_ID)
                .version(env!("CARGO_PKG_VERSION"))
                .license_type(gtk::License::Gpl30)
                .comments(
                    "Installs the standalone version of Aniimo from its official servers and adds it to Steam.\n\n\
                     This is an unofficial community tool, not affiliated with Pawprint Interactive or FunPlus.",
                )
                .build()
                .present(Some(&a.ui.window));
        });
        ui.window.add_action(&about);
    }

    fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        let result = {
            let mut st = self.state.borrow_mut();
            f(&mut st.settings);
            st.settings.save()
        };
        if let Err(e) = result {
            self.toast(&format!("Couldn't save settings: {e}"));
        }
        self.refresh();
    }

    /// After a shortcut setting changes, point at the button that applies it.
    fn hint_update_shortcut(&self) {
        let (_, appid) = self.target();
        let exists = {
            let st = self.state.borrow();
            st.steam.as_ref().zip(self.selected_user()).is_some_and(|(s, u)| s.shortcut_exists(&u, appid))
        };
        if exists {
            self.ui.prefs.add_toast(
                adw::Toast::builder()
                    .title("Saved. Press Update next to the Steam shortcut to apply it.")
                    .timeout(4)
                    .build(),
            );
        }
    }

    fn toast(&self, msg: &str) {
        self.ui.toasts.add_toast(adw::Toast::builder().title(msg).timeout(5).build());
    }

    fn error(&self, heading: &str, body: &str) {
        let d = adw::AlertDialog::new(Some(heading), Some(body));
        d.add_response("ok", "OK");
        d.present(Some(&self.ui.window));
    }

    async fn confirm(&self, heading: &str, body: &str, accept: &str, destructive: bool) -> bool {
        let d = adw::AlertDialog::new(Some(heading), Some(body));
        d.add_responses(&[("cancel", "Cancel"), ("accept", accept)]);
        d.set_response_appearance(
            "accept",
            if destructive { adw::ResponseAppearance::Destructive } else { adw::ResponseAppearance::Suggested },
        );
        d.set_default_response(Some("accept"));
        d.set_close_response("cancel");
        d.choose_future(Some(&self.ui.window)).await == "accept"
    }

    /// Re-detect Steam, its users and compat tools, and rebuild the combo rows.
    fn reload_steam(&self) {
        let steam = Steam::detect().into_iter().find(|s| !s.users().is_empty()).or_else(|| Steam::detect().into_iter().next());
        let users = steam.as_ref().map(Steam::users).unwrap_or_default();
        let mut tools: Vec<(String, String)> =
            BUILTIN_TOOLS.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        if let Some(s) = &steam {
            tools.extend(s.custom_compat_tools());
        }

        let mut st = self.state.borrow_mut();
        if !tools.iter().any(|t| t.0 == st.settings.compat_tool) {
            tools.push((st.settings.compat_tool.clone(), st.settings.compat_tool.clone()));
        }
        self.populating.set(true);
        let user_names: Vec<String> = users
            .iter()
            .map(|u| match &u.persona {
                Some(p) => format!("{p} ({})", u.account_id),
                None => u.account_id.to_string(),
            })
            .collect();
        let user_names: Vec<&str> = user_names.iter().map(String::as_str).collect();
        self.ui.user_row.set_model(Some(&gtk::StringList::new(&user_names)));
        let selected_user = st
            .settings
            .steam_user
            .and_then(|id| users.iter().position(|u| u.account_id == id))
            .unwrap_or(0);
        self.ui.user_row.set_selected(selected_user as u32);

        let tool_names: Vec<&str> = tools.iter().map(|t| t.1.as_str()).collect();
        self.ui.tool_row.set_model(Some(&gtk::StringList::new(&tool_names)));
        let selected_tool = tools.iter().position(|t| t.0 == st.settings.compat_tool).unwrap_or(0);
        self.ui.tool_row.set_selected(selected_tool as u32);
        self.ui.artwork_row.set_active(st.settings.artwork);
        self.ui.single_row.set_active(st.settings.single_window);
        let size = st.settings.single_window_size().unwrap_or(steam::DEFAULT_WINDOW_SIZE);
        let size_index = WINDOW_SIZES.iter().position(|(s, _)| *s == size).unwrap_or(0);
        self.ui.size_row.set_selected(size_index as u32);
        self.ui.launch_row.set_text(&st.settings.launch_options);
        self.populating.set(false);

        st.steam = steam;
        st.users = users;
        st.tools = tools;
    }

    /// Bring the game's Proton prefix in line with the settings: the game drive (see
    /// `Steam::map_game_drive`) and single-window mode. Covers shortcuts added by older
    /// versions and settings changed while the game was running. None of this touches
    /// Steam's own files, so Steam doesn't need to restart. `explicit` is true when the
    /// user just changed a setting, so every outcome is reported.
    fn apply_prefix_settings(&self, explicit: bool) {
        let (exe, appid) = self.target();
        let Some(user) = self.selected_user() else { return };
        let (steam, dir, size, tool) = {
            let st = self.state.borrow();
            let Some(steam) = st.steam.clone() else { return };
            (steam, st.settings.install_dir.clone(), st.settings.single_window_size(), st.settings.compat_tool.clone())
        };
        if !steam.shortcut_exists(&user, appid) {
            return;
        }
        if !steam.game_drive_mapped(appid, &dir) {
            match steam.map_game_drive(appid, &dir) {
                Ok(()) => self.toast("Fixed the game's free-space check. Restart Aniimo if it's running."),
                Err(e) => self.toast(&format!("Couldn't set up the game drive: {e}")),
            }
        }

        let ready = steam.prefix_ready(appid);
        if steam.single_window(appid) == size && (ready || size.is_none()) {
            return;
        }
        let exe_name = exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match process::game_running(&exe_name) {
            Ok(false) => {}
            Ok(true) => {
                if explicit {
                    self.pref_toast("Close Aniimo first. The change will be applied the next time you open this launcher.");
                }
                return;
            }
            Err(e) => {
                if explicit {
                    self.pref_toast(&e.to_string());
                }
                return;
            }
        }
        match steam.set_single_window(appid, size, Some(&tool)) {
            Ok(steam::PrefixChange::Applied) if explicit => self.pref_toast("Saved. It takes effect the next time Aniimo starts."),
            Ok(steam::PrefixChange::Applied) if size.is_some() => {
                self.toast("Turned on single-window mode for logging in from Game Mode")
            }
            Ok(steam::PrefixChange::Applied) => {}
            Ok(steam::PrefixChange::Pending) if explicit => {
                self.pref_toast("Saved. It will be set up after Aniimo's first launch, when you next open this launcher.")
            }
            Ok(steam::PrefixChange::Pending) => {}
            Err(e) => self.toast(&format!("Couldn't change the game's window setting: {e}")),
        }
    }

    /// A toast that's visible whether or not the Preferences dialog is open.
    fn pref_toast(&self, msg: &str) {
        if self.ui.prefs.is_mapped() {
            self.ui.prefs.add_toast(adw::Toast::builder().title(msg).timeout(5).build());
        } else {
            self.toast(msg);
        }
    }

    fn selected_user(&self) -> Option<User> {
        let st = self.state.borrow();
        match st.settings.steam_user {
            Some(id) => st.users.iter().find(|u| u.account_id == id).cloned(),
            None => st.users.first().cloned(),
        }
    }

    /// Exe path and shortcut app id for the current install directory.
    fn target(&self) -> (PathBuf, u32) {
        let dir = self.state.borrow().settings.install_dir.clone();
        let exe_name = install::read_state(&dir)
            .map(|s| s.package.exe_name().to_owned())
            .unwrap_or_else(|| yaccgl_core::DEFAULT_EXE.into());
        let exe = dir.join(exe_name);
        let appid = steam::appid_for(&exe, GAME_NAME);
        (exe, appid)
    }

    fn check_latest(&self) {
        self.state.borrow_mut().latest = Latest::Checking;
        self.refresh();
        let a = self.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(|| api::latest_game(&http::agent()).map_err(|e| e.to_string())).await;
            a.state.borrow_mut().latest = match result {
                Ok(Ok(pkg)) => Latest::Known(pkg),
                Ok(Err(e)) => Latest::Failed(e),
                Err(_) => Latest::Failed("internal error".into()),
            };
            a.refresh();
        });
    }

    /// Bring every label and button in line with the current state.
    fn refresh(&self) {
        let ui = &self.ui;
        let (exe, appid) = self.target();
        let user = self.selected_user();
        let mut st = self.state.borrow_mut();
        let dir = st.settings.install_dir.clone();

        ui.dir_row.set_subtitle(&match space::available(&dir) {
            Some(free) => format!("{} · {} free", dir.display(), human(free)),
            None => dir.display().to_string(),
        });

        let installed = install::read_state(&dir);
        let status = match &st.latest {
            Latest::Known(pkg) => Some(install::status(&dir, pkg)),
            _ => None,
        };
        let shortcut = match (&st.steam, &user) {
            (Some(s), Some(u)) => s.shortcut_exists(u, appid),
            _ => false,
        };

        let (title, detail, action) = match (&status, &st.latest, &installed) {
            (Some(Status::NotInstalled), Latest::Known(pkg), _) => (
                "Not installed".to_string(),
                format!(
                    "Version {} · {} download, about {} after the game's first start",
                    pkg.version_number,
                    pkg.ext.archive_size.map_or("unknown size".into(), human),
                    pkg.package_file_size.map_or("40 GB".into(), human),
                ),
                Action::Install,
            ),
            (Some(Status::UpdateAvailable { installed, latest }), _, _) => (
                "Update available".to_string(),
                format!("{} → {}", installed.package.version_number, latest.version_number),
                Action::Update,
            ),
            (Some(Status::UpToDate(_)), _, _) => (
                if shortcut { "Ready to play".to_string() } else { "Installed".to_string() },
                if shortcut {
                    "Launch it from your Steam library".to_string()
                } else {
                    "Add it to Steam to play".to_string()
                },
                if shortcut { Action::None } else { Action::AddToSteam },
            ),
            (None, Latest::Checking, _) => ("Checking for updates…".to_string(), String::new(), Action::None),
            (None, Latest::Failed(e), Some(s)) => (
                "Installed".to_string(),
                format!("Version {} · couldn't check for updates: {e}", s.package.version_number),
                if shortcut { Action::None } else { Action::AddToSteam },
            ),
            (None, Latest::Failed(e), None) => {
                ("Can't reach the update server".to_string(), e.clone(), Action::Retry)
            }
            _ => (String::new(), String::new(), Action::None),
        };
        let title = match (st.task, action) {
            (Some(Task::Install), Action::Update) => "Updating…".to_string(),
            (Some(Task::Install), Action::Install) => "Installing…".to_string(),
            (Some(Task::Install), _) => "Repairing…".to_string(),
            _ => title,
        };
        ui.status_title.set_label(&title);
        ui.status_detail.set_label(&detail);
        ui.status_detail.set_visible(!detail.is_empty());
        st.action = action;
        let (label, suggested) = match action {
            Action::None => ("Install", true),
            Action::Retry => ("Retry", false),
            Action::Install => ("Install", true),
            Action::Update => ("Update", true),
            Action::AddToSteam => ("Add to Steam", true),
        };
        ui.primary.set_label(label);
        if suggested {
            ui.primary.add_css_class("suggested-action");
        } else {
            ui.primary.remove_css_class("suggested-action");
        }

        ui.version_row.set_subtitle(&match (&installed, &st.latest) {
            (Some(s), Latest::Known(l)) if s.package.id != l.id => {
                format!("{} installed, {} available", s.package.version_number, l.version_number)
            }
            (Some(s), _) => format!("{} installed", s.package.version_number),
            (None, Latest::Known(l)) => format!("{} available", l.version_number),
            (None, _) => "Not installed".into(),
        });

        let tool = st.steam.as_ref().and_then(|s| s.compat_tool(appid));
        let tool_label = |t: &str| {
            st.tools.iter().find(|x| x.0 == t).map_or(t.to_string(), |x| x.1.clone())
        };
        ui.shortcut_row.set_subtitle(&match (&st.steam, shortcut) {
            (None, _) => "Steam was not found".into(),
            (Some(_), false) if !exe.is_file() => "Install the game first".into(),
            (Some(_), false) => "Not added yet".into(),
            (Some(_), true) => match &tool {
                Some(t) => format!("Added · {}", tool_label(t)),
                None => "Added · Steam's default Proton".into(),
            },
        });
        // Before the shortcut exists, the main button is the way to add it.
        ui.steam_button.set_label("Update");
        ui.steam_button.set_tooltip_text(Some("Apply the current preferences to the Steam shortcut"));
        ui.steam_button.set_visible(shortcut);

        let busy = st.task.is_some();
        let has_steam = st.steam.is_some() && user.is_some();
        ui.primary.set_sensitive(!busy && action != Action::None);
        // Nothing to do (up to date and in Steam, or still checking): no button.
        ui.primary.set_visible(st.task != Some(Task::Install) && action != Action::None);
        ui.cancel.set_visible(st.task == Some(Task::Install));
        ui.refresh.set_sensitive(!busy);
        ui.dir_button.set_sensitive(!busy);
        ui.repair.set_sensitive(!busy && installed.is_some() && matches!(st.latest, Latest::Known(_)));
        ui.steam_button.set_sensitive(!busy && has_steam && exe.is_file());
        ui.remove_button.set_visible(shortcut);
        ui.remove_button.set_sensitive(!busy);
        ui.login_row.set_visible(shortcut && exe.is_file());
        ui.login_button.set_sensitive(!busy && !process::in_game_mode());
        ui.login_button.set_label(if st.task == Some(Task::Game) { "Running…" } else { "Log In" });
        ui.user_row.set_sensitive(!busy && st.users.len() > 1);
        ui.size_row.set_sensitive(st.settings.single_window);

        let game_mode = process::in_game_mode();
        ui.banner.set_title(if game_mode {
            "You're in Game Mode. Switch to Desktop Mode to add or update the Steam shortcut."
        } else {
            ""
        });
        ui.banner.set_revealed(game_mode);
    }

    fn on_primary(&self) {
        let action = self.state.borrow().action;
        match action {
            Action::None => {}
            Action::Retry => self.check_latest(),
            Action::Install | Action::Update => self.start_install(false),
            Action::AddToSteam => {
                let a = self.clone();
                glib::spawn_future_local(async move { a.add_to_steam().await });
            }
        }
    }

    fn start_install(&self, repair: bool) {
        let (dir, pkg, cancel) = {
            let mut st = self.state.borrow_mut();
            let Latest::Known(pkg) = &st.latest else { return };
            let pkg = pkg.clone();
            if st.task.is_some() {
                return;
            }
            st.task = Some(Task::Install);
            st.cancel = Arc::new(AtomicBool::new(false));
            st.rate = None;
            (st.settings.install_dir.clone(), pkg, st.cancel.clone())
        };
        let was_installed = install::read_state(&dir).is_some();
        self.ui.progress.set_visible(true);
        self.ui.progress_label.set_visible(true);
        self.ui.progress.set_fraction(0.0);
        self.ui.progress_label.set_label("Starting…");
        self.refresh();

        let (tx, rx) = async_channel::unbounded::<Progress>();
        let a = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(p) = rx.recv().await {
                a.show_progress(p);
            }
        });

        let a = self.clone();
        let worker_dir = dir.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                let mut last = Instant::now() - Duration::from_secs(1);
                let mut last_kind = None;
                install::install(&http::agent(), &worker_dir, &pkg, &cancel, &mut |p| {
                    // Throttle UI updates, but always deliver stage changes.
                    let kind = std::mem::discriminant(&p);
                    if last_kind != Some(kind) || last.elapsed() >= Duration::from_millis(100) {
                        last = Instant::now();
                        last_kind = Some(kind);
                        let _ = tx.send_blocking(p);
                    }
                })
            })
            .await;

            a.state.borrow_mut().task = None;
            a.ui.progress.set_visible(false);
            a.ui.progress_label.set_visible(false);
            if let Err(e) = a.state.borrow().settings.save() {
                a.toast(&format!("Couldn't save settings: {e}"));
            }
            a.refresh();
            match result {
                Ok(Ok(state)) => {
                    let v = &state.package.version_number;
                    a.toast(&if repair {
                        format!("Aniimo {v} repaired")
                    } else if was_installed {
                        format!("Aniimo updated to {v}")
                    } else {
                        format!("Aniimo {v} installed")
                    });
                    let (_, appid) = a.target();
                    let has_shortcut = {
                        let st = a.state.borrow();
                        st.steam.as_ref().zip(a.selected_user()).is_some_and(|(s, u)| s.shortcut_exists(&u, appid))
                    };
                    let can_add = a.state.borrow().steam.is_some() && !process::in_game_mode();
                    if !has_shortcut
                        && can_add
                        && a
                            .confirm(
                                "Add Aniimo to Steam?",
                                "Aniimo will appear in your Steam library as a non-Steam game, set to run with the selected Proton version.",
                                "Add to Steam",
                                false,
                            )
                            .await
                    {
                        a.add_to_steam().await;
                    }
                }
                Ok(Err(Error::Cancelled)) => a.toast("Cancelled. The download will resume where it left off."),
                Ok(Err(e)) => a.error("Installation failed", &e.to_string()),
                Err(_) => a.error("Installation failed", "The installer crashed unexpectedly."),
            }
        });
    }

    fn show_progress(&self, p: Progress) {
        let (stage, done, total) = match p {
            Progress::Downloading { done, total } => ("Downloading", done, total),
            Progress::Verifying { done, total } => ("Verifying", done, Some(total)),
            Progress::Extracting { done, total } => ("Unpacking", done, Some(total)),
        };
        let now = Instant::now();
        let mut st = self.state.borrow_mut();
        if st.rate.as_ref().is_none_or(|r| r.stage != stage) {
            st.rate = Some(Rate { stage, since: now, from: done });
        }
        let rate = st.rate.as_ref().unwrap();
        let elapsed = (now - rate.since).as_secs_f64();
        let speed = if elapsed > 1.0 { (done.saturating_sub(rate.from)) as f64 / elapsed } else { 0.0 };

        let mut text = format!("{stage}… {}", human(done));
        if let Some(t) = total.filter(|&t| t > 0) {
            self.ui.progress.set_fraction((done as f64 / t as f64).clamp(0.0, 1.0));
            text.push_str(&format!(" of {}", human(t)));
            if speed > 0.0 {
                let left = t.saturating_sub(done) as f64 / speed;
                text.push_str(&format!(" · {}/s · {} left", human(speed as u64), eta(left)));
            }
        } else {
            self.ui.progress.pulse();
        }
        self.ui.progress_label.set_label(&text);
    }

    async fn choose_dir(&self) {
        let current = self.state.borrow().settings.install_dir.clone();
        let start = current.ancestors().find(|p| p.is_dir()).map(Path::to_path_buf);
        let dialog = gtk::FileDialog::builder().title("Choose Where to Install Aniimo").modal(true).build();
        if let Some(start) = start {
            dialog.set_initial_folder(Some(&gio::File::for_path(start)));
        }
        let Ok(folder) = dialog.select_folder_future(Some(&self.ui.window)).await else { return };
        let Some(picked) = folder.path() else {
            self.error("Unsupported location", "Please choose a local folder.");
            return;
        };
        let dir = resolve_install_dir(&picked);
        self.update_settings(|s| s.install_dir = dir.clone());
        self.toast(&format!("Aniimo will be installed in {}", dir.display()));
    }

    /// Steam rewrites its config on exit, so it must be closed while we edit it.
    /// Returns whether it's OK to go ahead (closing Steam if it's running).
    async fn confirm_steam_change(&self) -> bool {
        if process::in_game_mode() {
            self.error(
                "Can't change Steam right now",
                "Steam can't be modified from Game Mode. Switch to Desktop Mode and try again.",
            );
            return false;
        }
        match gio::spawn_blocking(process::is_running).await {
            Ok(Ok(false)) => true,
            Ok(Ok(true)) => {
                self.confirm(
                    "Steam needs to restart",
                    "Steam overwrites its list of shortcuts when it exits, so it has to be closed while this change is made. It will be started again afterwards.",
                    "Restart Steam",
                    false,
                )
                .await
            }
            Ok(Err(e)) => {
                self.error("Can't change Steam right now", &e.to_string());
                false
            }
            Err(_) => false,
        }
    }

    /// Run a Steam config change on a worker thread with Steam closed, showing progress.
    async fn run_steam_change<T: Send + 'static>(
        &self,
        steam: Steam,
        busy_text: &str,
        change: impl FnOnce(&Steam) -> yaccgl_core::Result<T> + Send + 'static,
        is_applied: impl Fn(&Steam) -> bool + Send + 'static,
    ) -> Result<steam::Applied<T>, String> {
        self.state.borrow_mut().task = Some(Task::Steam);
        self.ui.progress_label.set_label(busy_text);
        self.ui.progress_label.set_visible(true);
        self.refresh();
        let result = gio::spawn_blocking(move || {
            steam.with_closed(true, || change(&steam), || is_applied(&steam)).map_err(|e| e.to_string())
        })
        .await;
        self.state.borrow_mut().task = None;
        self.ui.progress_label.set_visible(false);
        self.refresh();
        let applied = result.map_err(|_| "An unexpected error occurred.".to_string())??;
        if applied.survived_restart == Some(false) {
            return Err("Steam overwrote the change when it restarted, which means another Steam process was still                         running. Exit Steam completely (Steam menu > Exit), then try again."
                .into());
        }
        Ok(applied)
    }

    async fn add_to_steam(&self) {
        let (exe, appid) = self.target();
        let (steam, user, spec) = {
            let st = self.state.borrow();
            let Some(steam) = st.steam.clone() else { return };
            let spec = ShortcutSpec {
                name: GAME_NAME.into(),
                exe,
                start_dir: st.settings.install_dir.clone(),
                launch_options: st.settings.launch_options.clone(),
                compat_tool: Some(st.settings.compat_tool.clone()),
                artwork: st.settings.artwork,
                single_window: st.settings.single_window_size(),
            };
            (steam, self.selected_user(), spec)
        };
        let Some(user) = user else {
            self.error("No Steam account", "Log in to Steam at least once, then try again.");
            return;
        };
        if !spec.exe.is_file() {
            self.error("Game not installed", "Install Aniimo before adding it to Steam.");
            return;
        }
        if !self.confirm_steam_change().await {
            return;
        }

        let tool = spec.compat_tool.clone().unwrap_or_default();
        let check_user = user.clone();
        let result = self
            .run_steam_change(
                steam,
                "Adding Aniimo to Steam…",
                move |s| s.register(&http::agent(), &user, &spec),
                move |s| s.shortcut_exists(&check_user, appid),
            )
            .await;
        match result {
            Ok(applied) => {
                let reg = applied.value;
                let name = self.state.borrow().tools.iter().find(|t| t.0 == tool).map_or(tool.clone(), |t| t.1.clone());
                let verb = if reg.created { "Added to Steam" } else { "Steam shortcut updated" };
                self.toast(&format!("{verb} · {name}"));
                if let Some(e) = reg.artwork_error {
                    self.toast(&format!("Artwork couldn't be downloaded: {e}"));
                }
                if reg.single_window_pending {
                    self.error(
                        "One more step for Game Mode",
                        "Proton hasn't set up this game yet, so single-window mode can't be turned on. \
                         Start Aniimo once from Steam here in Desktop Mode and close it (you can log in while you're there), \
                         then open this launcher again. After that, the login window works in Game Mode too.",
                    );
                }
            }
            Err(e) => self.error("Couldn't add Aniimo to Steam", &e),
        }
    }

    /// Open the FunPlus login dialog via the FPX host, using the shortcut's Proton
    /// prefix so the session is kept for launches from Steam.
    async fn log_in(&self) {
        let (game_exe, appid) = self.target();
        let install_dir = self.state.borrow().settings.install_dir.clone();
        let (steam, tool) = {
            let st = self.state.borrow();
            let Some(steam) = st.steam.clone() else { return };
            (steam, st.settings.compat_tool.clone())
        };

        let login_exe = match gio::spawn_blocking(move || yaccgl_core::fpx_login::ensure_installed(&install_dir)).await {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => return self.error("Couldn't prepare login", &e.to_string()),
            Err(_) => return self.error("Couldn't prepare login", "An unexpected error occurred."),
        };

        let game_name = game_exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let login_name = login_exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let already = gio::spawn_blocking(move || {
            Ok::<_, yaccgl_core::Error>(process::game_running(&game_name)? || process::game_running(&login_name)?)
        })
        .await;
        if already.is_ok_and(|r| r.unwrap_or(false)) {
            self.error("Aniimo is already running", "Close the game (or the login window) first, then press Log In again.");
            return;
        }

        let launch = match steam.game_launch(appid, &login_exe, &tool) {
            Ok(l) => l,
            Err(e) => return self.error("Couldn't start login", &e.to_string()),
        };

        self.state.borrow_mut().task = Some(Task::Game);
        self.ui.progress_label.set_label("Login window is open. Sign in — it may close on its own when you're done.");
        self.ui.progress_label.set_visible(true);
        self.refresh();
        let started = Instant::now();
        let result = gio::spawn_blocking(move || process::run_game(&launch)).await;
        self.state.borrow_mut().task = None;
        self.ui.progress_label.set_visible(false);
        // The first launch creates the prefix, so pending prefix settings can go in now.
        self.apply_prefix_settings(false);
        self.refresh();

        match result {
            Ok(Ok(_)) if started.elapsed() < Duration::from_secs(5) => self.error(
                "Login closed straight away",
                "The login window exited within a few seconds. Check that Proton 10 is installed, then try again.",
            ),
            Ok(Ok(_)) => self.error(
                "All set",
                "If you logged in, Aniimo should remember it when you start it from Steam, in Game Mode too.",
            ),
            Ok(Err(e)) => self.error("Couldn't start login", &e.to_string()),
            Err(_) => self.error("Couldn't start login", "An unexpected error occurred."),
        }
    }

    async fn remove_from_steam(&self) {
        let (_, appid) = self.target();
        let Some(steam) = self.state.borrow().steam.clone() else { return };
        let Some(user) = self.selected_user() else { return };
        if !self
            .confirm(
                "Remove Aniimo from Steam?",
                "The shortcut, its Proton setting and artwork are removed. Game files are kept.",
                "Remove",
                true,
            )
            .await
        {
            return;
        }
        if !self.confirm_steam_change().await {
            return;
        }
        let check_user = user.clone();
        match self
            .run_steam_change(
                steam,
                "Removing Aniimo from Steam…",
                move |s| s.unregister(&user, appid),
                move |s| !s.shortcut_exists(&check_user, appid),
            )
            .await
        {
            Ok(_) => self.toast("Removed from Steam"),
            Err(e) => self.error("Couldn't remove the shortcut", &e),
        }
    }
}

/// Where to install when the user picks `picked`: use it directly if it already
/// holds the game, is empty, or is named after the game; otherwise use a subfolder.
fn resolve_install_dir(picked: &Path) -> PathBuf {
    let has_game = picked.join(yaccgl_core::DEFAULT_EXE).is_file() || picked.join(".yaccgl").is_dir();
    let is_empty = std::fs::read_dir(picked).map_or(true, |mut d| d.next().is_none());
    let named = picked.file_name().is_some_and(|n| n.eq_ignore_ascii_case(GAME_NAME));
    if has_game || is_empty || named { picked.to_path_buf() } else { picked.join(GAME_NAME) }
}

fn eta(secs: f64) -> String {
    let s = secs.round() as u64;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}
