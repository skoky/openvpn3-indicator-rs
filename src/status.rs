//! Mapping of session status to icons and human readable descriptions.

use crate::openvpn3::{Status, StatusMinor};

pub const ICON_PREFIX: &str = "openvpn3-indicator-rs";

/// Visual state of a session, ordered by how much it should dominate the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Look {
    Idle,
    IdleError,
    Paused,
    Active,
    Loading,
}

impl Look {
    pub fn icon(self) -> String {
        let suffix = match self {
            Look::Idle => "idle",
            Look::IdleError => "idle-error",
            Look::Paused => "paused",
            Look::Active => "active",
            Look::Loading => "loading",
        };
        format!("{ICON_PREFIX}-{suffix}")
    }
}

pub fn describe(status: &Status) -> (Look, &'static str) {
    use Look::*;
    use StatusMinor::*;
    match status.minor {
        Unset => (Idle, "Unknown"),
        CfgError | CfgInlineMissing => (IdleError, "Configuration error"),
        CfgOk | ConnInit | ConnConnecting | ConnReconnecting | ConnResuming | SessNew
        | SessBackendCompleted => (Loading, "Connecting"),
        CfgRequireUser | SessAuthUserpass | SessAuthChallenge | SessAuthUrl | Pkcs11Sign
        | Pkcs11Encrypt | Pkcs11Decrypt | Pkcs11Verify => (Loading, "Authentication required"),
        ConnConnected => (Active, "Connected"),
        ConnDisconnecting => (Loading, "Disconnecting"),
        ConnDisconnected | ConnDone | SessRemoved => (Idle, "Disconnected"),
        ConnFailed => (IdleError, "Connection failed"),
        ConnAuthFailed => (IdleError, "Authentication failed"),
        ConnPausing => (Loading, "Pausing"),
        ConnPaused => (Paused, "Paused"),
        ProcStarted => (Loading, "Process started"),
        ProcStopped => (IdleError, "Process stopped"),
        ProcKilled => (IdleError, "Process killed"),
        StatusMinor::Unknown(_) => (Idle, "Unknown"),
    }
}

/// Icon for the single tray icon given the looks of all sessions.
pub fn aggregate<I: IntoIterator<Item = Look>>(looks: I) -> Look {
    let mut looks: Vec<Look> = looks.into_iter().collect();
    looks.sort();
    match looks.last() {
        None => Look::Idle,
        // Connected but something else failed: still show "active".
        Some(l) => *l,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_prefers_busy_then_active() {
        assert_eq!(aggregate([]), Look::Idle);
        assert_eq!(aggregate([Look::Idle, Look::Active]), Look::Active);
        assert_eq!(aggregate([Look::Active, Look::Loading]), Look::Loading);
        assert_eq!(aggregate([Look::IdleError, Look::Idle]), Look::IdleError);
    }
}
