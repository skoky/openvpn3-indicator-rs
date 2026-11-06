//! Small blocking client for the OpenVPN 3 Linux D-Bus services.
//!
//! This replaces the `openvpn3` Python module that ships with openvpn3-linux.
//! Everything goes through a `gio::DBusConnection` on the system bus, so method
//! calls are plain synchronous calls and signals are delivered as callbacks on
//! the GLib main loop – no async runtime involved.

use anyhow::{Context, Result, anyhow};
use gtk4::gio;
use gtk4::glib::{
    self, Variant, VariantTy,
    variant::{ObjectPath, ToVariant},
};

pub const CONFIG_SERVICE: &str = "net.openvpn.v3.configuration";
pub const CONFIG_ROOT: &str = "/net/openvpn/v3/configuration";
pub const CONFIG_IFACE: &str = "net.openvpn.v3.configuration";
pub const SESSION_SERVICE: &str = "net.openvpn.v3.sessions";
pub const SESSION_ROOT: &str = "/net/openvpn/v3/sessions";
pub const SESSION_IFACE: &str = "net.openvpn.v3.sessions";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
const PEER_IFACE: &str = "org.freedesktop.DBus.Peer";
const TIMEOUT_MS: i32 = 30_000;

// ---------------------------------------------------------------------------
// Constants (mirrors openvpn3/constants.py)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusMajor {
    Unset,
    CfgError,
    Connection,
    Session,
    Pkcs11,
    Process,
    Unknown(u32),
}

impl From<u32> for StatusMajor {
    fn from(v: u32) -> Self {
        match v {
            0 => Self::Unset,
            1 => Self::CfgError,
            2 => Self::Connection,
            3 => Self::Session,
            4 => Self::Pkcs11,
            5 => Self::Process,
            n => Self::Unknown(n),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusMinor {
    Unset,
    CfgError,
    CfgOk,
    CfgInlineMissing,
    CfgRequireUser,
    ConnInit,
    ConnConnecting,
    ConnConnected,
    ConnDisconnecting,
    ConnDisconnected,
    ConnFailed,
    ConnAuthFailed,
    ConnReconnecting,
    ConnPausing,
    ConnPaused,
    ConnResuming,
    ConnDone,
    SessNew,
    SessBackendCompleted,
    SessRemoved,
    SessAuthUserpass,
    SessAuthChallenge,
    SessAuthUrl,
    Pkcs11Sign,
    Pkcs11Encrypt,
    Pkcs11Decrypt,
    Pkcs11Verify,
    ProcStarted,
    ProcStopped,
    ProcKilled,
    Unknown(u32),
}

impl From<u32> for StatusMinor {
    fn from(v: u32) -> Self {
        use StatusMinor::*;
        const TABLE: [StatusMinor; 30] = [
            Unset,
            CfgError,
            CfgOk,
            CfgInlineMissing,
            CfgRequireUser,
            ConnInit,
            ConnConnecting,
            ConnConnected,
            ConnDisconnecting,
            ConnDisconnected,
            ConnFailed,
            ConnAuthFailed,
            ConnReconnecting,
            ConnPausing,
            ConnPaused,
            ConnResuming,
            ConnDone,
            SessNew,
            SessBackendCompleted,
            SessRemoved,
            SessAuthUserpass,
            SessAuthChallenge,
            SessAuthUrl,
            Pkcs11Sign,
            Pkcs11Encrypt,
            Pkcs11Decrypt,
            Pkcs11Verify,
            ProcStarted,
            ProcStopped,
            ProcKilled,
        ];
        TABLE.get(v as usize).copied().unwrap_or(Unknown(v))
    }
}

/// `ClientAttentionType` values used by the user-input queue.
pub mod attention_type {
    pub const CREDENTIALS: u32 = 1;
}

/// `ClientAttentionGroup` values used by the user-input queue.
pub mod attention_group {
    pub const UNSET: u32 = 0;
    pub const USER_PASSWORD: u32 = 1;
    pub const HTTP_PROXY_CREDS: u32 = 2;
    pub const PK_PASSPHRASE: u32 = 3;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub major: StatusMajor,
    pub minor: StatusMinor,
    pub message: String,
}

impl Status {
    fn from_tuple((major, minor, message): (u32, u32, String)) -> Self {
        Self {
            major: major.into(),
            minor: minor.into(),
            message,
        }
    }

    pub fn is(&self, major: StatusMajor, minor: StatusMinor) -> bool {
        self.major == major && self.minor == minor
    }
}

/// One pending request for user input (username, password, OTP, ...).
#[derive(Debug, Clone)]
pub struct InputSlot {
    pub kind: u32,
    pub group: u32,
    pub id: u32,
    #[allow(dead_code)] // kept for debugging / future use
    pub var_name: String,
    pub label: String,
    pub masked: bool,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Client {
    conn: gio::DBusConnection,
}

impl Client {
    pub fn system() -> Result<Self> {
        let conn = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE)
            .context("cannot connect to the D-Bus system bus")?;
        Ok(Self { conn })
    }

    fn call(
        &self,
        service: &str,
        path: &str,
        iface: &str,
        method: &str,
        args: Option<Variant>,
        reply: &str,
    ) -> Result<Variant> {
        let reply_ty = VariantTy::new(reply).map_err(|e| anyhow!("bad reply type {reply}: {e}"))?;
        self.conn
            .call_sync(
                Some(service),
                path,
                iface,
                method,
                args.as_ref(),
                Some(reply_ty),
                gio::DBusCallFlags::NONE,
                TIMEOUT_MS,
                gio::Cancellable::NONE,
            )
            .map_err(|e| anyhow!("{iface}.{method} on {path}: {}", clean_dbus_error(&e)))
    }

    fn property(&self, service: &str, path: &str, iface: &str, name: &str) -> Result<Variant> {
        let v = self.call(
            service,
            path,
            PROPS_IFACE,
            "Get",
            Some((iface, name).to_variant()),
            "(v)",
        )?;
        v.child_value(0)
            .as_variant()
            .ok_or_else(|| anyhow!("property {name} is not a variant"))
    }

    fn string_property(
        &self,
        service: &str,
        path: &str,
        iface: &str,
        name: &str,
    ) -> Result<String> {
        let v = self.property(service, path, iface, name)?;
        v.str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("property {name} is not a string"))
    }

    fn object_list(v: Variant) -> Vec<String> {
        v.child_value(0)
            .iter()
            .filter_map(|o| o.str().map(str::to_owned))
            .collect()
    }

    /// Wake up the (D-Bus activated) configuration manager and return its version string.
    pub fn manager_version(&self) -> Result<String> {
        self.call(CONFIG_SERVICE, CONFIG_ROOT, PEER_IFACE, "Ping", None, "()")?;
        self.string_property(CONFIG_SERVICE, CONFIG_ROOT, CONFIG_IFACE, "version")
    }

    // -- configurations ------------------------------------------------------

    pub fn configs(&self) -> Result<Vec<String>> {
        let v = self.call(
            CONFIG_SERVICE,
            CONFIG_ROOT,
            CONFIG_IFACE,
            "FetchAvailableConfigs",
            None,
            "(ao)",
        )?;
        Ok(Self::object_list(v))
    }

    pub fn config_name(&self, config: &str) -> Result<String> {
        self.string_property(CONFIG_SERVICE, config, CONFIG_IFACE, "name")
    }

    pub fn import_config(
        &self,
        name: &str,
        profile: &str,
        single_use: bool,
        persistent: bool,
    ) -> Result<String> {
        let args = (name, profile, single_use, persistent).to_variant();
        let v = self.call(
            CONFIG_SERVICE,
            CONFIG_ROOT,
            CONFIG_IFACE,
            "Import",
            Some(args),
            "(o)",
        )?;
        v.child_value(0)
            .str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("Import returned no path"))
    }

    pub fn add_config_tag(&self, config: &str, tag: &str) -> Result<()> {
        self.call(
            CONFIG_SERVICE,
            config,
            CONFIG_IFACE,
            "AddTag",
            Some((tag,).to_variant()),
            "()",
        )?;
        Ok(())
    }

    pub fn validate_config(&self, config: &str) -> Result<()> {
        self.call(CONFIG_SERVICE, config, CONFIG_IFACE, "Validate", None, "()")?;
        Ok(())
    }

    pub fn remove_config(&self, config: &str) -> Result<()> {
        self.call(CONFIG_SERVICE, config, CONFIG_IFACE, "Remove", None, "()")?;
        Ok(())
    }

    // -- sessions ------------------------------------------------------------

    pub fn sessions(&self) -> Result<Vec<String>> {
        let v = self.call(
            SESSION_SERVICE,
            SESSION_ROOT,
            SESSION_IFACE,
            "FetchAvailableSessions",
            None,
            "(ao)",
        )?;
        Ok(Self::object_list(v))
    }

    pub fn new_tunnel(&self, config: &str) -> Result<String> {
        let path =
            ObjectPath::try_from(config).map_err(|_| anyhow!("invalid object path {config}"))?;
        let v = self.call(
            SESSION_SERVICE,
            SESSION_ROOT,
            SESSION_IFACE,
            "NewTunnel",
            Some((path,).to_variant()),
            "(o)",
        )?;
        v.child_value(0)
            .str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("NewTunnel returned no path"))
    }

    pub fn session_status(&self, session: &str) -> Result<Status> {
        let v = self.property(SESSION_SERVICE, session, SESSION_IFACE, "status")?;
        v.get::<(u32, u32, String)>()
            .map(Status::from_tuple)
            .ok_or_else(|| anyhow!("unexpected status type {}", v.type_()))
    }

    pub fn session_config_path(&self, session: &str) -> Result<String> {
        self.string_property(SESSION_SERVICE, session, SESSION_IFACE, "config_path")
    }

    pub fn session_config_name(&self, session: &str) -> Result<String> {
        self.string_property(SESSION_SERVICE, session, SESSION_IFACE, "config_name")
    }

    fn session_call(&self, session: &str, method: &str, args: Option<Variant>) -> Result<()> {
        self.call(SESSION_SERVICE, session, SESSION_IFACE, method, args, "()")?;
        Ok(())
    }

    pub fn ready(&self, session: &str) -> Result<()> {
        self.session_call(session, "Ready", None)
    }
    pub fn connect(&self, session: &str) -> Result<()> {
        self.session_call(session, "Connect", None)
    }
    pub fn disconnect(&self, session: &str) -> Result<()> {
        self.session_call(session, "Disconnect", None)
    }
    pub fn restart(&self, session: &str) -> Result<()> {
        self.session_call(session, "Restart", None)
    }
    pub fn resume(&self, session: &str) -> Result<()> {
        self.session_call(session, "Resume", None)
    }
    pub fn pause(&self, session: &str, reason: &str) -> Result<()> {
        self.session_call(session, "Pause", Some((reason,).to_variant()))
    }

    /// Ask the log service to forward this session's `StatusChange`/`Log`
    /// signals to us. Without it the indicator never sees status updates.
    pub fn log_forward(&self, session: &str, enable: bool) -> Result<()> {
        self.session_call(session, "LogForward", Some((enable,).to_variant()))
    }

    pub fn input_slots(&self, session: &str) -> Result<Vec<InputSlot>> {
        let groups = self.call(
            SESSION_SERVICE,
            session,
            SESSION_IFACE,
            "UserInputQueueGetTypeGroup",
            None,
            "(a(uu))",
        )?;
        let groups: Vec<(u32, u32)> = groups.child_value(0).get().unwrap_or_default();
        let mut slots = Vec::new();
        for (kind, group) in groups {
            let ids = self.call(
                SESSION_SERVICE,
                session,
                SESSION_IFACE,
                "UserInputQueueCheck",
                Some((kind, group).to_variant()),
                "(au)",
            )?;
            let ids: Vec<u32> = ids.child_value(0).get().unwrap_or_default();
            for id in ids {
                let s = self.call(
                    SESSION_SERVICE,
                    session,
                    SESSION_IFACE,
                    "UserInputQueueFetch",
                    Some((kind, group, id).to_variant()),
                    "(uuussb)",
                )?;
                let (kind, group, id, var_name, label, masked): (
                    u32,
                    u32,
                    u32,
                    String,
                    String,
                    bool,
                ) = s
                    .get()
                    .ok_or_else(|| anyhow!("unexpected UserInputQueueFetch reply"))?;
                slots.push(InputSlot {
                    kind,
                    group,
                    id,
                    var_name,
                    label,
                    masked,
                });
            }
        }
        Ok(slots)
    }

    pub fn provide_input(&self, session: &str, slot: &InputSlot, value: &str) -> Result<()> {
        self.session_call(
            session,
            "UserInputProvide",
            Some((slot.kind, slot.group, slot.id, value).to_variant()),
        )
    }

    // -- signals -------------------------------------------------------------

    /// `SessionManagerEvent(o path, q type, u owner)`: sessions created/destroyed.
    pub fn on_session_manager_event(
        &self,
        f: impl Fn(&str, u16) + 'static,
    ) -> gio::SignalSubscription {
        self.conn.subscribe_to_signal(
            Some(SESSION_SERVICE),
            Some(SESSION_IFACE),
            Some("SessionManagerEvent"),
            Some(SESSION_ROOT),
            None,
            gio::DBusSignalFlags::NONE,
            move |sig| {
                if let Some((path, kind, _owner)) = sig.parameters.get::<(ObjectPath, u16, u32)>() {
                    f(path.as_str(), kind);
                }
            },
        )
    }

    /// `ConfigurationManagerEvent(o path, q type, u owner)`: configs imported/removed.
    pub fn on_config_manager_event(&self, f: impl Fn(&str) + 'static) -> gio::SignalSubscription {
        self.conn.subscribe_to_signal(
            Some(CONFIG_SERVICE),
            Some(CONFIG_IFACE),
            Some("ConfigurationManagerEvent"),
            Some(CONFIG_ROOT),
            None,
            gio::DBusSignalFlags::NONE,
            move |sig| {
                if let Some((path, _kind, _owner)) = sig.parameters.get::<(ObjectPath, u16, u32)>()
                {
                    f(path.as_str());
                }
            },
        )
    }

    /// `StatusChange(u major, u minor, s message)` for any session.
    ///
    /// The log service re-emits it on the `net.openvpn.v3.backends` interface
    /// (that is what the Python module listens to), and newer releases also
    /// emit it on the session object itself, so match on the member name only
    /// and use the object path to identify the session.
    pub fn on_status_change(&self, f: impl Fn(&str, Status) + 'static) -> gio::SignalSubscription {
        self.conn.subscribe_to_signal(
            None,
            None,
            Some("StatusChange"),
            None,
            None,
            gio::DBusSignalFlags::NONE,
            move |sig| {
                if !sig.object_path.starts_with(SESSION_ROOT) {
                    return;
                }
                if let Some(t) = sig.parameters.get::<(u32, u32, String)>() {
                    f(sig.object_path, Status::from_tuple(t));
                }
            },
        )
    }
}

/// Turn `GDBus.Error:net.openvpn.v3.error.foo: Something` into `Something`.
pub fn clean_dbus_error(e: &glib::Error) -> String {
    let msg = e.message();
    match msg.find("GDBus.Error:") {
        Some(i) => {
            let rest = &msg[i..];
            rest.split_once(char::is_whitespace)
                .map(|(_, m)| m.trim())
                .unwrap_or(rest)
                .to_owned()
        }
        None => msg.to_owned(),
    }
}

/// Parse `v27`, `v22_dev`, `git:...` into a number (dev builds count as "very new").
pub fn parse_version(v: &str) -> u32 {
    if v.starts_with("git:") {
        return 9999;
    }
    v.strip_prefix('v')
        .map(|s| {
            s.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v27"), 27);
        assert_eq!(parse_version("v22_dev"), 22);
        assert_eq!(parse_version("git:abc"), 9999);
        assert_eq!(parse_version("garbage"), 0);
    }

    #[test]
    fn minor_from_u32() {
        assert_eq!(StatusMinor::from(7), StatusMinor::ConnConnected);
        assert_eq!(StatusMinor::from(29), StatusMinor::ProcKilled);
        assert_eq!(StatusMinor::from(99), StatusMinor::Unknown(99));
    }
}
