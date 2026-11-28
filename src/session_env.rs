use std::collections::BTreeMap;

/// The identity eswm publishes so portals stop inviting file pickers to the
/// wrong compositor's party.
#[allow(dead_code)] // wired into startup by e11s01t5
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionIdentity {
    values: BTreeMap<String, String>,
}

#[allow(dead_code)] // wired into startup by e11s01t5
impl SessionIdentity {
    pub fn new(wayland_display: &str) -> Self {
        let mut values = BTreeMap::new();
        values.insert("WAYLAND_DISPLAY".into(), wayland_display.into());
        values.insert("XDG_CURRENT_DESKTOP".into(), "eswm".into());
        values.insert("XDG_SESSION_TYPE".into(), "wayland".into());
        values.insert("XDG_SESSION_DESKTOP".into(), "eswm".into());
        Self { values }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sets_eswm_identity_vars() {
        let identity = super::SessionIdentity::new("eswm-test");
        assert_eq!(identity.get("WAYLAND_DISPLAY"), Some("eswm-test"));
        assert_eq!(identity.get("XDG_CURRENT_DESKTOP"), Some("eswm"));
        assert_eq!(identity.get("XDG_SESSION_TYPE"), Some("wayland"));
        assert_eq!(identity.get("XDG_SESSION_DESKTOP"), Some("eswm"));
    }
}
