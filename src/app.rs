//! Application state and behaviour.
//!
//! Everything here runs on the GLib main thread. The state lives in a
//! thread-local and is reached through [`with_app`] from D-Bus signal
//! handlers, timers, dialog callbacks and tray commands.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use ksni::blocking::TrayMethods;

use crate::dialogs::{self, CredentialField, CredentialsDialog};
use crate::notify::Notifier;
use crate::openvpn3::{
    self as ov, Client, InputSlot, Status, StatusMajor as Major, StatusMinor as Minor,
};
use crate::secrets::SecretStore;
use crate::status::{self, Look};
use crate::tray::{Command, ConfigItem, SessionItem, Tray, TrayModel};

pub const APP_ID: &str = "net.openvpn.openvpn3_indicator_rs";
const SYSTEM_TAG: &str = "system:openvpn3-indicator-rs";
const MANAGER_VERSION_MINIMUM: u32 = 20;
const MANAGER_VERSION_RECOMMENDED: u32 = 21;
const REFRESH_SECONDS: u32 = 5;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Run `f` with the application state. If the state is already borrowed
/// (re-entrant call), the call is deferred to the next main loop iteration.
pub fn with_app(f: impl FnOnce(&mut App) + 'static) {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut app) => {
            if let Some(app) = app.as_mut() {
                f(app)
            }
        }
        Err(_) => {
            glib::idle_add_local_once(move || with_app(f));
        }
    })
}

/// Entry point for tray menu commands (called on the main thread).
pub fn dispatch(cmd: Command) {
    with_app(move |app| app.handle(cmd));
}

struct Session {
    config: Option<String>,
    name: String,
    status: Status,
}

pub struct App {
    gtk: gtk4::Application,
    _hold: gio::ApplicationHoldGuard,
    client: Client,
    manager_version: u32,
    tray: Option<ksni::blocking::Handle<Tray>>,
    icon_dir: Option<PathBuf>,
    /// config object path -> config name
    configs: BTreeMap<String, String>,
    /// session object path -> session
    sessions: BTreeMap<String, Session>,
    connect_sent: HashSet<String>,
    failed_auth: HashSet<String>,
    dialogs: HashMap<String, CredentialsDialog>,
    notifier: Notifier,
    secrets: SecretStore,
    _subscriptions: Vec<gio::SignalSubscription>,
    refresh_queued: bool,
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

pub fn run(args: Vec<String>) -> glib::ExitCode {
    let gtk = gtk4::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    gtk.connect_startup(startup);
    gtk.connect_activate(|_| log::debug!("activate"));
    gtk.connect_open(|_, files, _| {
        for path in files.iter().filter_map(|f| f.path()) {
            with_app(move |app| app.confirm_import(path));
        }
    });
    gtk.connect_shutdown(|_| {
        if let Some(app) = APP.with(|a| a.borrow_mut().take()) {
            app.shutdown();
        }
    });
    gtk.run_with_args(&args)
}

fn fatal(gtk: &gtk4::Application, title: &str, detail: &str) {
    log::error!("{title}: {detail}");
    let hold = gtk.hold();
    let d = gtk4::AlertDialog::builder()
        .message(title)
        .detail(detail)
        .buttons(["Quit"])
        .build();
    let gtk = gtk.clone();
    d.choose(gtk4::Window::NONE, gio::Cancellable::NONE, move |_| {
        drop(hold);
        gtk.quit();
    });
}

fn find_icon_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("OPENVPN3_INDICATOR_RS_ICONS") {
        return Some(dir.into());
    }
    // Debug builds use the icons straight from the source tree; release builds
    // expect them installed into the hicolor theme (`just install`).
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/icons");
    (cfg!(debug_assertions) && dev.is_dir()).then_some(dev)
}

fn startup(gtk: &gtk4::Application) {
    log::info!("startup");
    let icon_dir = find_icon_dir();
    if let (Some(dir), Some(display)) = (&icon_dir, gtk4::gdk::Display::default()) {
        gtk4::IconTheme::for_display(&display).add_search_path(dir);
    }
    gtk4::Window::set_default_icon_name(dialogs::APP_ICON);

    let client = match Client::system() {
        Ok(c) => c,
        Err(e) => return fatal(gtk, "Cannot connect to the system D-Bus", &format!("{e:#}")),
    };

    let version = client.manager_version().or_else(|_| {
        // The configuration manager is D-Bus activated; give it a moment.
        std::thread::sleep(std::time::Duration::from_millis(500));
        client.manager_version()
    });
    let manager_version = match version {
        Ok(v) => ov::parse_version(&v),
        Err(e) => {
            return fatal(
                gtk,
                "OpenVPN 3 is not available",
                &format!(
                    "Could not reach the OpenVPN 3 configuration manager.\nIs openvpn3-linux installed?\n\n{e:#}"
                ),
            );
        }
    };
    log::info!("OpenVPN 3 manager version {manager_version}");

    let notifier = Notifier::new();
    if manager_version < MANAGER_VERSION_MINIMUM {
        notifier.show("version", "dialog-warning", "OpenVPN3 Indicator",
            &format!("OpenVPN 3 v{manager_version} is not supported. Please upgrade to v{MANAGER_VERSION_RECOMMENDED} or newer."));
    }

    let subscriptions = vec![
        client.on_session_manager_event(|path, kind| {
            log::info!("session manager event {kind} for {path}");
            with_app(App::queue_refresh);
        }),
        client.on_config_manager_event(|path| {
            log::info!("configuration manager event for {path}");
            with_app(App::queue_refresh);
        }),
        client.on_status_change(|path, st| {
            let path = path.to_owned();
            with_app(move |app| app.on_status(&path, st));
        }),
    ];

    let tray = Tray {
        model: TrayModel {
            icon: Look::Idle.icon(),
            tooltip: "Not connected".into(),
            configs: vec![],
        },
        icon_theme_path: icon_dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_default(),
    };
    let tray = match tray.spawn() {
        Ok(handle) => handle,
        Err(e) => {
            return fatal(
                gtk,
                "No system tray available",
                &format!(
                    "OpenVPN3 Indicator needs a StatusNotifierItem (AppIndicator) host.\n\
                          On GNOME, enable the \"AppIndicator and KStatusNotifierItem Support\" extension.\n\n{e}"
                ),
            );
        }
    };

    let app = App {
        gtk: gtk.clone(),
        _hold: gtk.hold(),
        client,
        manager_version,
        tray: Some(tray),
        icon_dir,
        configs: BTreeMap::new(),
        sessions: BTreeMap::new(),
        connect_sent: HashSet::new(),
        failed_auth: HashSet::new(),
        dialogs: HashMap::new(),
        notifier,
        secrets: SecretStore::default(),
        _subscriptions: subscriptions,
        refresh_queued: false,
    };
    APP.with(|a| *a.borrow_mut() = Some(app));
    with_app(App::refresh);

    // Quit cleanly on Ctrl+C / SIGTERM so log forwarding is switched off again.
    for signal in [SIGINT, SIGTERM] {
        let gtk = gtk.clone();
        glib_unix::unix_signal_add_local_once(signal, move || gtk.quit());
    }

    glib::timeout_add_seconds_local(REFRESH_SECONDS, || {
        with_app(App::refresh);
        glib::ControlFlow::Continue
    });
}

// ---------------------------------------------------------------------------
// State refresh
// ---------------------------------------------------------------------------

impl App {
    fn queue_refresh(&mut self) {
        if !self.refresh_queued {
            self.refresh_queued = true;
            glib::idle_add_local_once(|| with_app(App::refresh));
        }
    }

    fn refresh(&mut self) {
        self.refresh_queued = false;
        if let Err(e) = self.refresh_inner() {
            log::warn!("refresh failed: {e:#}");
        }
        self.update_tray();
    }

    fn refresh_inner(&mut self) -> Result<()> {
        let mut configs = BTreeMap::new();
        for path in self.client.configs()? {
            let name = self
                .client
                .config_name(&path)
                .unwrap_or_else(|_| "UNKNOWN".into());
            configs.insert(path, name);
        }
        self.configs = configs;

        let live: Vec<String> = self.client.sessions()?;
        let gone: Vec<String> = self
            .sessions
            .keys()
            .filter(|s| !live.contains(s))
            .cloned()
            .collect();
        for s in gone {
            log::info!("session {s} is gone");
            self.sessions.remove(&s);
            self.connect_sent.remove(&s);
            if let Some(d) = self.dialogs.remove(&s) {
                d.dismiss();
            }
            self.notifier.forget(&s);
        }

        for s in live {
            let status = match self.client.session_status(&s) {
                Ok(st) => st,
                Err(e) => {
                    log::debug!("status of {s}: {e:#}");
                    continue;
                }
            };
            match self.sessions.get(&s) {
                Some(known) if known.status == status => {}
                Some(_) => self.on_status(&s, status),
                None => {
                    if let Err(e) = self.client.log_forward(&s, true) {
                        log::debug!("LogForward on {s}: {e:#}");
                    }
                    let config = self.client.session_config_path(&s).ok();
                    let name = config
                        .as_ref()
                        .and_then(|c| self.configs.get(c).cloned())
                        .or_else(|| self.client.session_config_name(&s).ok())
                        .unwrap_or_else(|| "UNKNOWN".into());
                    log::info!("new session {s} ({name})");
                    // Placeholder status so on_status sees a change and reacts.
                    let placeholder = Status {
                        major: Major::Unset,
                        minor: Minor::Unset,
                        message: String::new(),
                    };
                    self.sessions.insert(
                        s.clone(),
                        Session {
                            config,
                            name,
                            status: placeholder,
                        },
                    );
                    self.on_status(&s, status);
                }
            }
        }
        Ok(())
    }

    fn update_tray(&self) {
        let Some(tray) = &self.tray else { return };
        let mut items: Vec<ConfigItem> = self
            .configs
            .iter()
            .map(|(id, name)| ConfigItem {
                id: id.clone(),
                name: name.clone(),
                sessions: vec![],
            })
            .collect();
        let mut looks = Vec::new();
        let mut tooltip = Vec::new();
        for (id, s) in &self.sessions {
            let (look, desc) = status::describe(&s.status);
            looks.push(look);
            tooltip.push(format!("{}: {desc}", s.name));
            let item = SessionItem {
                id: id.clone(),
                description: desc.to_owned(),
                can_pause: s.status.is(Major::Connection, Minor::ConnConnected),
                can_resume: s.status.is(Major::Connection, Minor::ConnPaused),
            };
            match items.iter_mut().find(|c| Some(&c.id) == s.config.as_ref()) {
                Some(c) => c.sessions.push(item),
                // Session of a removed / single-use profile.
                None => items.push(ConfigItem {
                    id: String::new(),
                    name: s.name.clone(),
                    sessions: vec![item],
                }),
            }
        }
        items.sort_by_key(|c| c.name.to_lowercase());
        let model = TrayModel {
            icon: status::aggregate(looks).icon(),
            tooltip: if tooltip.is_empty() {
                "Not connected".into()
            } else {
                tooltip.join("\n")
            },
            configs: items,
        };
        tray.update(move |t| t.model = model);
    }

    fn icon_ref(&self, icon: &str) -> String {
        // Notification daemons only know installed themes; hand them a file in dev builds.
        match &self.icon_dir {
            Some(dir) => {
                let f = dir.join(format!("hicolor/scalable/status/{icon}.svg"));
                if f.exists() {
                    f.display().to_string()
                } else {
                    icon.to_owned()
                }
            }
            None => icon.to_owned(),
        }
    }

    // -----------------------------------------------------------------------
    // Session state machine
    // -----------------------------------------------------------------------

    fn on_status(&mut self, session: &str, status: Status) {
        let Some(s) = self.sessions.get_mut(session) else {
            // A session we have not seen yet; pick it up on the next refresh.
            self.queue_refresh();
            return;
        };
        if s.status == status {
            return;
        }
        log::info!(
            "{} {:?}/{:?} {}",
            s.name,
            status.major,
            status.minor,
            status.message
        );
        s.status = status.clone();
        let name = s.name.clone();
        let config = s.config.clone();

        if !status.is(Major::Connection, Minor::CfgRequireUser)
            && let Some(d) = self.dialogs.remove(session)
        {
            d.dismiss();
        }

        match (status.major, status.minor) {
            (Major::Connection, Minor::CfgOk) => {
                if !self.connect_sent.contains(session) {
                    self.start_connection(session);
                }
            }
            (Major::Session, Minor::SessAuthUrl) => {
                if let Err(e) = gio::AppInfo::launch_default_for_uri(
                    &status.message,
                    gio::AppLaunchContext::NONE,
                ) {
                    log::warn!("cannot open {}: {e}", status.message);
                }
            }
            (Major::Connection, Minor::ConnAuthFailed) => {
                self.disconnect(session);
                if let Some(c) = config {
                    // Retry and force the credentials dialog this time.
                    self.failed_auth.insert(c.clone());
                    self.connect_config(&c);
                }
            }
            (Major::Connection, Minor::ConnConnected) => {
                if let Some(c) = &config {
                    self.failed_auth.remove(c);
                }
            }
            (Major::Connection, Minor::ConnFailed) => self.disconnect(session),
            (Major::Connection, Minor::CfgRequireUser) => {
                if let Err(e) = self.request_credentials(session) {
                    log::warn!("credentials for {name}: {e:#}");
                    self.disconnect(session);
                }
            }
            _ => {}
        }

        let (look, desc) = status::describe(&status);
        let icon = self.icon_ref(&look.icon());
        self.notifier
            .show(session, &icon, &format!("OpenVPN: {name}"), desc);
        self.update_tray();
    }

    fn start_connection(&mut self, session: &str) {
        let res = self
            .client
            .ready(session)
            .and_then(|_| self.client.connect(session));
        match res {
            Ok(()) => {
                self.connect_sent.insert(session.to_owned());
            }
            Err(e) => log::debug!("connect {session}: {e:#}"),
        }
    }

    // -----------------------------------------------------------------------
    // Credentials
    // -----------------------------------------------------------------------

    fn can_store(slot: &InputSlot) -> bool {
        use ov::attention_group::*;
        slot.kind == ov::attention_type::CREDENTIALS
            && matches!(
                slot.group,
                UNSET | USER_PASSWORD | HTTP_PROXY_CREDS | PK_PASSPHRASE
            )
    }

    fn request_credentials(&mut self, session: &str) -> Result<()> {
        let slots: Vec<InputSlot> = self
            .client
            .input_slots(session)?
            .into_iter()
            .filter(|s| s.kind == ov::attention_type::CREDENTIALS)
            .collect();
        if slots.is_empty() {
            return Ok(());
        }
        let (config, name) = {
            let s = &self.sessions[session];
            (s.config.clone(), s.name.clone())
        };
        let stored = match &config {
            Some(c) => self.secrets.get_all(c).unwrap_or_else(|e| {
                log::warn!("secret storage: {e:#}");
                HashMap::new()
            }),
            None => HashMap::new(),
        };
        let force_ui = config
            .as_ref()
            .is_some_and(|c| self.failed_auth.contains(c));
        if !force_ui && slots.iter().all(|s| stored.contains_key(&s.label)) {
            let values = slots.iter().map(|s| stored[&s.label].clone()).collect();
            self.provide_credentials(session, &slots, values);
            return Ok(());
        }

        let fields = slots
            .iter()
            .map(|s| CredentialField {
                label: s.label.clone(),
                masked: s.masked,
                value: if Self::can_store(s) {
                    stored.get(&s.label).cloned()
                } else {
                    None
                },
                can_store: Self::can_store(s),
            })
            .collect();
        let session_id = session.to_owned();
        let dialog = dialogs::credentials(&self.gtk, &name, fields, move |answer| {
            with_app(move |app| app.credentials_answered(&session_id, config, slots, answer));
        });
        if let Some(old) = self.dialogs.insert(session.to_owned(), dialog) {
            old.dismiss();
        }
        Ok(())
    }

    fn credentials_answered(
        &mut self,
        session: &str,
        config: Option<String>,
        slots: Vec<InputSlot>,
        answer: Option<(Vec<String>, bool)>,
    ) {
        self.dialogs.remove(session);
        let Some((values, store)) = answer else {
            let waiting = self
                .sessions
                .get(session)
                .is_some_and(|s| s.status.is(Major::Connection, Minor::CfgRequireUser));
            if waiting {
                self.disconnect(session);
            }
            return;
        };
        if let Some(c) = &config {
            for (slot, value) in slots.iter().zip(&values) {
                let res = if store && Self::can_store(slot) && !value.is_empty() {
                    self.secrets.set(c, &slot.label, value)
                } else if !store {
                    self.secrets.delete(c, &slot.label)
                } else {
                    Ok(())
                };
                if let Err(e) = res {
                    log::warn!("secret storage: {e:#}");
                }
            }
        }
        self.provide_credentials(session, &slots, values);
    }

    fn provide_credentials(&mut self, session: &str, slots: &[InputSlot], values: Vec<String>) {
        let res = slots
            .iter()
            .zip(&values)
            .try_for_each(|(slot, v)| self.client.provide_input(session, slot, v))
            .and_then(|_| self.client.ready(session))
            .and_then(|_| self.client.connect(session));
        match res {
            Ok(()) => {
                self.connect_sent.insert(session.to_owned());
            }
            Err(e) => {
                log::warn!("providing credentials failed: {e:#}");
                self.disconnect(session);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Commands
    // -----------------------------------------------------------------------

    fn handle(&mut self, cmd: Command) {
        log::info!("command {cmd:?}");
        let session_op = |app: &mut App, s: &str, r: Result<()>| {
            if let Err(e) = r {
                log::warn!("{e:#}");
                app.notifier.forget(s);
            }
            app.queue_refresh();
        };
        match cmd {
            Command::Connect(c) => self.connect_config(&c),
            Command::RemoveConfig(c) => self.confirm_remove(&c),
            Command::Pause(s) => {
                let r = self.client.pause(&s, "User request");
                session_op(self, &s, r)
            }
            Command::Resume(s) => {
                let r = self.client.resume(&s);
                session_op(self, &s, r)
            }
            Command::Restart(s) => {
                let r = self.client.restart(&s);
                session_op(self, &s, r)
            }
            Command::Disconnect(s) => self.disconnect(&s),
            Command::Import => {
                dialogs::pick_profile(&self.gtk, |path| with_app(move |app| app.import(&path)))
            }
            Command::About => dialogs::about(&self.gtk),
            Command::Quit => self.gtk.quit(),
        }
    }

    fn connect_config(&mut self, config: &str) {
        match self.client.new_tunnel(config) {
            Ok(session) => {
                log::info!("started session {session}");
                self.queue_refresh();
            }
            Err(e) => {
                let name = self.configs.get(config).cloned().unwrap_or_default();
                dialogs::error(
                    &self.gtk,
                    &format!("Cannot start {name}"),
                    &format!("{e:#}"),
                );
            }
        }
    }

    fn disconnect(&mut self, session: &str) {
        if let Err(e) = self.client.disconnect(session) {
            log::debug!("disconnect {session}: {e:#}");
        }
        self.connect_sent.remove(session);
        self.queue_refresh();
    }

    fn confirm_remove(&mut self, config: &str) {
        let name = self.configs.get(config).cloned().unwrap_or_default();
        let config = config.to_owned();
        dialogs::confirm(
            &self.gtk,
            &format!("Remove configuration {name}?"),
            "The profile will be deleted from OpenVPN 3. This cannot be undone.",
            "Remove",
            move || {
                with_app(move |app| {
                    if let Err(e) = app.client.remove_config(&config) {
                        dialogs::error(
                            &app.gtk,
                            "Removing the configuration failed",
                            &format!("{e:#}"),
                        );
                    }
                    app.queue_refresh();
                })
            },
        );
    }

    fn confirm_import(&mut self, path: PathBuf) {
        let name = profile_name(&path);
        dialogs::confirm(
            &self.gtk,
            &format!("Import configuration {name}?"),
            &format!("{}", path.display()),
            "Import",
            move || with_app(move |app| app.import(&path)),
        );
    }

    fn import(&mut self, path: &Path) {
        if let Err(msg) = self.try_import(path) {
            dialogs::error(&self.gtk, "Configuration Import Failed", &msg);
        }
    }

    fn try_import(&mut self, path: &Path) -> std::result::Result<(), String> {
        use std::io::ErrorKind;
        let shown = path.display();
        let profile = std::fs::read_to_string(path).map_err(|e| match e.kind() {
            ErrorKind::NotFound => format!("Configuration file not found: {shown}"),
            ErrorKind::PermissionDenied => format!("Permission denied accessing file: {shown}"),
            ErrorKind::InvalidData => format!(
                "File encoding error: {shown}\nUnable to read file as text. \
                 Please check if this is a valid OpenVPN configuration file."
            ),
            _ => format!("Error reading file: {shown}\n{e}"),
        })?;
        let name = profile_name(path);
        let config = self
            .client
            .import_config(&name, &profile, false, true)
            .map_err(|e| format!("Failed to import configuration {name}:\n{e:#}"))?;
        if self.manager_version > 20
            && let Err(e) = self.client.add_config_tag(&config, SYSTEM_TAG)
        {
            log::warn!("tagging {config}: {e:#}");
        }
        if self.manager_version >= 22
            && let Err(e) = self.client.validate_config(&config)
        {
            let _ = self.client.remove_config(&config);
            return Err(format!(
                "Configuration {name} imported from {shown} failed validation:\n{e:#}"
            ));
        }
        let icon = self.icon_ref(&Look::Active.icon());
        self.notifier.forget("import");
        self.notifier.show(
            "import",
            &icon,
            "OpenVPN3 Indicator",
            &format!("Imported configuration {name}"),
        );
        self.queue_refresh();
        Ok(())
    }

    fn shutdown(mut self) {
        log::info!("shutdown");
        for s in self.sessions.keys() {
            let _ = self.client.log_forward(s, false);
        }
        for (_, d) in self.dialogs.drain() {
            d.dismiss();
        }
        if let Some(tray) = self.tray.take() {
            tray.shutdown().wait();
        }
    }
}

fn profile_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "profile".into())
}

// ---------------------------------------------------------------------------
// --dump: print what the indicator sees, without any UI
// ---------------------------------------------------------------------------

pub fn dump() -> glib::ExitCode {
    let run = || -> Result<()> {
        let c = Client::system()?;
        println!("OpenVPN 3 manager: {}", c.manager_version()?);
        println!("\nConfigurations:");
        for p in c.configs()? {
            println!("  {p}  {}", c.config_name(&p).unwrap_or_default());
        }
        println!("\nSessions:");
        for s in c.sessions()? {
            let st = c.session_status(&s)?;
            let (look, desc) = status::describe(&st);
            println!(
                "  {s}  {}  {desc} ({:?}/{:?}) icon={} {}",
                c.session_config_name(&s).unwrap_or_default(),
                st.major,
                st.minor,
                look.icon(),
                st.message
            );
        }
        Ok(())
    };
    match run() {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            glib::ExitCode::FAILURE
        }
    }
}
