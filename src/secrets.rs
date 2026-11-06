//! Credential storage in the desktop Secret Service (GNOME Keyring, KWallet, ...).
//!
//! Items use the same attributes as the Python openvpn3-indicator
//! (`application`, `config`, `key`), so credentials saved by either
//! application are visible to the other.

use std::collections::HashMap;

use anyhow::{Context, Result};
use dbus_secret_service::{EncryptionType, SecretService};

const APPLICATION: &str = "openvpn3-indicator";
const LABEL_PREFIX: &str = "OpenVPN3 Indicator";

#[derive(Default)]
pub struct SecretStore {
    service: Option<SecretService>,
}

impl SecretStore {
    fn service(&mut self) -> Result<&SecretService> {
        if self.service.is_none() {
            self.service = Some(
                SecretService::connect(EncryptionType::Plain)
                    .context("cannot reach Secret Service")?,
            );
        }
        Ok(self.service.as_ref().expect("just set"))
    }

    fn attrs<'a>(config: &'a str, key: Option<&'a str>) -> HashMap<&'a str, &'a str> {
        let mut a = HashMap::from([("application", APPLICATION), ("config", config)]);
        if let Some(k) = key {
            a.insert("key", k);
        }
        a
    }

    /// All stored `key -> secret` pairs for one configuration.
    pub fn get_all(&mut self, config: &str) -> Result<HashMap<String, String>> {
        let svc = self.service()?;
        let coll = svc.get_default_collection()?;
        coll.ensure_unlocked()?;
        let mut out = HashMap::new();
        for item in coll.search_items(Self::attrs(config, None))? {
            let Some(key) = item.get_attributes()?.remove("key") else {
                continue;
            };
            let secret = String::from_utf8_lossy(&item.get_secret()?).into_owned();
            out.insert(key, secret);
        }
        Ok(out)
    }

    pub fn set(&mut self, config: &str, key: &str, value: &str) -> Result<()> {
        let svc = self.service()?;
        let coll = svc.get_default_collection()?;
        coll.ensure_unlocked()?;
        let label = format!("{LABEL_PREFIX} {config} {key}");
        coll.create_item(
            &label,
            Self::attrs(config, Some(key)),
            value.as_bytes(),
            true,
            "text/plain",
        )?;
        log::info!("stored secret {label}");
        Ok(())
    }

    pub fn delete(&mut self, config: &str, key: &str) -> Result<()> {
        let svc = self.service()?;
        let coll = svc.get_default_collection()?;
        coll.ensure_unlocked()?;
        for item in coll.search_items(Self::attrs(config, Some(key)))? {
            item.delete()?;
        }
        Ok(())
    }
}
