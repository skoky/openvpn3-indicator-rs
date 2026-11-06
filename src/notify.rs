//! Desktop notifications via `org.freedesktop.Notifications`.
//!
//! Notifications are keyed (one per session), so a status change replaces the
//! previous bubble instead of stacking, and repeating the same text is muted.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk4::gio;
use gtk4::glib::{self, variant::ToVariant};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// (icon, title, body) of the last bubble shown for a key.
type Content = (String, String, String);

#[derive(Clone)]
pub struct Notifier {
    conn: Option<gio::DBusConnection>,
    /// key -> (notification id, last (icon, title, body))
    sent: Rc<RefCell<HashMap<String, (u32, Content)>>>,
}

impl Notifier {
    pub fn new() -> Self {
        let conn = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
            .inspect_err(|e| log::warn!("no session bus for notifications: {e}"))
            .ok();
        Self {
            conn,
            sent: Default::default(),
        }
    }

    pub fn show(&self, key: &str, icon: &str, title: &str, body: &str) {
        let Some(conn) = &self.conn else { return };
        let content = (icon.to_owned(), title.to_owned(), body.to_owned());
        let replaces = match self.sent.borrow().get(key) {
            Some((_, last)) if *last == content => return, // muted repetition
            Some((id, _)) => *id,
            None => 0,
        };
        // A HashMap serialises as a{sv}; a VariantDict inside a tuple would be boxed as `v`.
        let hints: HashMap<String, glib::Variant> = HashMap::from([
            (
                "desktop-entry".to_owned(),
                "net.openvpn.openvpn3_indicator_rs".to_variant(),
            ),
            ("transient".to_owned(), true.to_variant()),
        ]);
        let args = (
            "OpenVPN3 Indicator",
            replaces,
            icon,
            title,
            body,
            Vec::<String>::new(),
            hints,
            5000i32,
        )
            .to_variant();
        let sent = self.sent.clone();
        let key = key.to_owned();
        conn.call(
            Some(SERVICE),
            PATH,
            SERVICE,
            "Notify",
            Some(&args),
            Some(glib::VariantTy::new("(u)").expect("valid")),
            gio::DBusCallFlags::NONE,
            -1,
            gio::Cancellable::NONE,
            move |res| match res {
                Ok(v) => {
                    let id = v.child_value(0).get::<u32>().unwrap_or(0);
                    sent.borrow_mut().insert(key, (id, content));
                }
                Err(e) => log::warn!("notification failed: {e}"),
            },
        );
    }

    /// Forget a key so the next message for it is shown even if identical.
    pub fn forget(&self, key: &str) {
        self.sent.borrow_mut().remove(key);
    }
}
