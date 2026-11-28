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

    #[allow(dead_code)] // wired into startup by e11s01t5
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }
}

/// Build the optional user-scope import as data so tests need no live bus.
#[allow(dead_code)] // wired into startup by e11s01t5
pub fn systemd_import_command(identity: &SessionIdentity) -> std::process::Command {
    let mut command = std::process::Command::new("systemctl");
    command.arg("--user").arg("import-environment");
    for key in identity.keys() {
        command.arg(key);
    }
    command
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

    #[test]
    fn systemd_import_command_uses_user_scope() {
        let identity = super::SessionIdentity::new("eswm-test");
        let command = super::systemd_import_command(&identity);
        assert_eq!(command.get_program(), "systemctl");
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--user",
                "import-environment",
                "WAYLAND_DISPLAY",
                "XDG_CURRENT_DESKTOP",
                "XDG_SESSION_DESKTOP",
                "XDG_SESSION_TYPE",
            ]
        );
    }

    #[test]
    fn dbus_activation_command_includes_vars() {
        let identity = super::SessionIdentity::new("eswm-test");
        let command = super::dbus_activation_command(&identity);
        assert_eq!(command.get_program(), "dbus-update-activation-environment");
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--systemd",
                "WAYLAND_DISPLAY",
                "XDG_CURRENT_DESKTOP",
                "XDG_SESSION_DESKTOP",
                "XDG_SESSION_TYPE",
            ]
        );
        // Key material does not get a free ride to the activation environment.
        assert!(args.iter().all(|arg| !arg.contains("KEY") && !arg.contains("SECRET")));
    }
}
