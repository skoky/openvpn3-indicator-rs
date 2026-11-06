//! GTK 4 dialogs. All of them are non-modal and callback based so the main
//! loop (and with it D-Bus signal handling) keeps running while they are open.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{gio, glib};

pub const APP_ICON: &str = "openvpn3-indicator-rs";

type Answer = Option<(Vec<String>, bool)>;
type AnswerSlot = Rc<Cell<Option<Box<dyn FnOnce(Answer)>>>>;

/// One field in the credentials dialog.
#[derive(Debug, Clone)]
pub struct CredentialField {
    pub label: String,
    pub masked: bool,
    pub value: Option<String>,
    pub can_store: bool,
}

/// Handle to an open credentials dialog.
pub struct CredentialsDialog {
    window: gtk4::Window,
    silent: Rc<Cell<bool>>,
}

impl CredentialsDialog {
    /// Close without triggering the cancel callback (e.g. the session moved on).
    pub fn dismiss(&self) {
        self.silent.set(true);
        self.window.close();
    }
}

/// Ask for credentials. `on_done` receives `Some((values, store))` on Connect
/// and `None` when the user cancels or closes the window.
pub fn credentials(
    app: &gtk4::Application,
    session_name: &str,
    fields: Vec<CredentialField>,
    on_done: impl FnOnce(Answer) + 'static,
) -> CredentialsDialog {
    let window = gtk4::Window::builder()
        .application(app)
        .title("OpenVPN Credentials")
        .icon_name(APP_ICON)
        .resizable(false)
        .build();

    let grid = gtk4::Grid::builder()
        .row_spacing(10)
        .column_spacing(10)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(20)
        .margin_end(20)
        .build();
    let intro = gtk4::Label::new(Some(&format!(
        "Session {session_name} requires credentials"
    )));
    intro.set_margin_bottom(10);
    grid.attach(&intro, 0, 0, 2, 1);

    let mut entries = Vec::new();
    let mut focus: Option<gtk4::Entry> = None;
    let mut any_storable = false;
    let mut any_prefilled = false;
    for (row, f) in fields.iter().enumerate() {
        let row = row as i32 + 1;
        let l = gtk4::Label::builder()
            .label(&f.label)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let e = gtk4::Entry::builder()
            .hexpand(true)
            .visibility(!f.masked)
            .activates_default(true)
            .build();
        if f.masked {
            e.set_input_purpose(gtk4::InputPurpose::Password);
        }
        if let Some(v) = &f.value {
            e.set_text(v);
            any_prefilled = true;
        } else if focus.is_none() {
            focus = Some(e.clone());
        }
        any_storable |= f.can_store;
        grid.attach(&l, 0, row, 1, 1);
        grid.attach(&e, 1, row, 1, 1);
        entries.push(e);
    }
    let mut row = fields.len() as i32 + 1;

    let store = gtk4::CheckButton::with_label("Store credentials");
    store.set_active(any_prefilled);
    if any_storable {
        grid.attach(&store, 0, row, 2, 1);
        row += 1;
    }

    let buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    buttons.set_halign(gtk4::Align::End);
    buttons.set_margin_top(10);
    let cancel = gtk4::Button::with_label("Cancel");
    let connect = gtk4::Button::with_label("Connect");
    connect.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&connect);
    grid.attach(&buttons, 0, row, 2, 1);

    window.set_child(Some(&grid));
    window.set_default_widget(Some(&connect));

    // `on_done` must run exactly once, whichever way the window goes away.
    let on_done: AnswerSlot = Rc::new(Cell::new(Some(Box::new(on_done))));
    let silent = Rc::new(Cell::new(false));

    connect.connect_clicked(glib::clone!(
        #[strong]
        on_done,
        #[weak]
        window,
        #[weak]
        store,
        move |_| {
            if let Some(cb) = on_done.take() {
                let values = entries.iter().map(|e| e.text().to_string()).collect();
                cb(Some((values, any_storable && store.is_active())));
            }
            window.close();
        }
    ));
    cancel.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |_| window.close()
    ));
    window.connect_close_request(glib::clone!(
        #[strong]
        on_done,
        #[strong]
        silent,
        move |_| {
            if let Some(cb) = on_done.take()
                && !silent.get()
            {
                cb(None);
            }
            glib::Propagation::Proceed
        }
    ));

    window.present();
    if let Some(e) = focus {
        e.grab_focus();
    }
    CredentialsDialog { window, silent }
}

pub fn error(app: &gtk4::Application, title: &str, detail: &str) {
    let d = gtk4::AlertDialog::builder()
        .message(title)
        .detail(detail)
        .modal(false)
        .build();
    d.show(app.active_window().as_ref());
}

/// Yes/no confirmation; `on_yes` runs only if the destructive button is chosen.
pub fn confirm(
    app: &gtk4::Application,
    title: &str,
    detail: &str,
    yes: &str,
    on_yes: impl FnOnce() + 'static,
) {
    let d = gtk4::AlertDialog::builder()
        .message(title)
        .detail(detail)
        .buttons(["Cancel", yes])
        .cancel_button(0)
        .default_button(0)
        .modal(false)
        .build();
    d.choose(
        app.active_window().as_ref(),
        gio::Cancellable::NONE,
        move |res| {
            if res == Ok(1) {
                on_yes();
            }
        },
    );
}

/// Pick an OpenVPN profile; uses the desktop portal when available.
pub fn pick_profile(app: &gtk4::Application, on_pick: impl FnOnce(PathBuf) + 'static) {
    let filter = gtk4::FileFilter::new();
    filter.set_name(Some("OpenVPN profiles"));
    filter.add_mime_type("application/x-openvpn-profile");
    filter.add_suffix("ovpn");
    filter.add_suffix("conf");
    let all = gtk4::FileFilter::new();
    all.set_name(Some("All files"));
    all.add_pattern("*");
    let filters = gio::ListStore::new::<gtk4::FileFilter>();
    filters.append(&filter);
    filters.append(&all);

    let d = gtk4::FileDialog::builder()
        .title("Import OpenVPN Configuration")
        .modal(false)
        .filters(&filters)
        .default_filter(&filter)
        .build();
    d.open(
        app.active_window().as_ref(),
        gio::Cancellable::NONE,
        move |res| {
            if let Ok(file) = res
                && let Some(path) = file.path()
            {
                on_pick(path);
            }
        },
    );
}

pub fn about(app: &gtk4::Application) {
    let d = gtk4::AboutDialog::builder()
        .application(app)
        .program_name("OpenVPN3 Indicator (Rust)")
        .logo_icon_name(APP_ICON)
        .version(env!("CARGO_PKG_VERSION"))
        .comments("Simple tray indicator for OpenVPN 3 Linux.")
        .website(env!("CARGO_PKG_REPOSITORY"))
        .website_label("Source code on GitHub")
        .license_type(gtk4::License::Agpl30)
        .build();
    d.present();
}
