//! The StatusNotifierItem tray icon and its menu (via `ksni`).
//!
//! ksni runs the D-Bus side of the tray on its own background thread. The
//! tray only holds a plain snapshot of what to display ([`TrayModel`]); menu
//! clicks are turned into [`Command`]s and handed to the GLib main thread,
//! where all application state lives.

use gtk4::glib;
use ksni::menu::{MenuItem, StandardItem, SubMenu};

#[derive(Debug, Clone)]
pub enum Command {
    Connect(String),
    RemoveConfig(String),
    Pause(String),
    Resume(String),
    Restart(String),
    Disconnect(String),
    Import,
    About,
    Quit,
}

#[derive(Debug, Clone, Default)]
pub struct TrayModel {
    pub icon: String,
    pub tooltip: String,
    pub configs: Vec<ConfigItem>,
}

#[derive(Debug, Clone)]
pub struct ConfigItem {
    pub id: String,
    pub name: String,
    pub sessions: Vec<SessionItem>,
}

#[derive(Debug, Clone)]
pub struct SessionItem {
    pub id: String,
    pub description: String,
    pub can_pause: bool,
    pub can_resume: bool,
}

pub struct Tray {
    pub model: TrayModel,
    pub icon_theme_path: String,
}

/// Run a command on the main thread.
fn send(cmd: Command) {
    glib::MainContext::default().invoke(move || crate::app::dispatch(cmd));
}

/// DBusMenu treats `_` as a mnemonic marker.
fn label(s: &str) -> String {
    s.replace('_', "__")
}

fn item(text: &str, cmd: Command) -> MenuItem<Tray> {
    StandardItem {
        label: label(text),
        activate: Box::new(move |_: &mut Tray| send(cmd.clone())),
        ..Default::default()
    }
    .into()
}

fn heading(text: &str) -> MenuItem<Tray> {
    StandardItem {
        label: label(text),
        enabled: false,
        ..Default::default()
    }
    .into()
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "openvpn3-indicator-rs".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::SystemServices
    }

    fn title(&self) -> String {
        "OpenVPN3 Indicator".into()
    }

    fn icon_name(&self) -> String {
        self.model.icon.clone()
    }

    fn icon_theme_path(&self) -> String {
        self.icon_theme_path.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "OpenVPN3 Indicator".into(),
            description: self.model.tooltip.clone(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut menu = Vec::new();
        for cfg in &self.model.configs {
            if cfg.sessions.is_empty() {
                menu.push(
                    SubMenu {
                        label: label(&cfg.name),
                        submenu: vec![
                            item("Connect", Command::Connect(cfg.id.clone())),
                            item("Remove…", Command::RemoveConfig(cfg.id.clone())),
                        ],
                        ..Default::default()
                    }
                    .into(),
                );
                continue;
            }
            for s in &cfg.sessions {
                let mut sub = vec![heading(&s.description)];
                if s.can_pause {
                    sub.push(item("Pause", Command::Pause(s.id.clone())));
                }
                if s.can_resume {
                    sub.push(item("Resume", Command::Resume(s.id.clone())));
                }
                sub.push(item("Restart", Command::Restart(s.id.clone())));
                sub.push(item("Disconnect", Command::Disconnect(s.id.clone())));
                menu.push(
                    SubMenu {
                        label: label(&format!("{} — {}", cfg.name, s.description)),
                        submenu: sub,
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }
        if !menu.is_empty() {
            menu.push(MenuItem::Separator);
        }
        menu.push(item("Import Config…", Command::Import));
        menu.push(item("About", Command::About));
        menu.push(item("Quit", Command::Quit));
        menu
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        log::warn!("StatusNotifierWatcher went away ({reason:?}); waiting for it to return");
        true // keep running, ksni re-registers when the watcher comes back
    }
}
