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

/// Build the optional D-Bus activation env update as data, never carrying key
/// content or secrets along for the ride.
#[allow(dead_code)] // wired into startup by e11s01t5
pub fn dbus_activation_command(identity: &SessionIdentity) -> std::process::Command {
    let mut command = std::process::Command::new("dbus-update-activation-environment");
    command.arg("--systemd");
    for key in identity.keys() {
        command.arg(key);
    }
    command
}

/// Give a child the same identity the compositor announced. Only the four
/// session keys are applied; any TERMINAL-style caller value survives.
#[allow(dead_code)] // wired into spawn by e11s01t5
pub fn apply_to_command<'a>(
    identity: &SessionIdentity,
    command: &'a mut std::process::Command,
) -> &'a mut std::process::Command {
    for key in identity.keys() {
        command.env(key, identity.get(key).unwrap_or_default());
    }
    command
}

/// Announce the identity to the user session. A missing tool, absent user bus
/// or explicit ESWM_NO_SD_VARS is a warning, never a startup failure.
#[allow(dead_code)] // wired into startup by e11s01t5
pub fn export_to_user_session(identity: &SessionIdentity) {
    if std::env::var_os("ESWM_NO_SD_VARS").is_some() {
        tracing::warn!("ESWM_NO_SD_VARS set: skipping systemd/D-Bus session export");
        return;
    }
    run_optional(
        systemd_import_command(identity),
        "systemd user environment",
    );
    run_optional(
        dbus_activation_command(identity),
        "D-Bus activation environment",
    );
}

fn run_optional(mut command: std::process::Command, label: &str) {
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::warn!(%status, "failed to update {label}"),
        Err(err) => tracing::warn!(?err, "skipping {label} update"),
    }
}

/// Owns the identity eswm installed in its own process and puts back only what
/// is still ours at shutdown. A user edit to a value while we run wins.
#[allow(dead_code)] // wired into startup by e11s01t5
pub struct SessionScope {
    identity: SessionIdentity,
    previous: BTreeMap<String, Option<String>>,
    restored: bool,
}

#[allow(dead_code)] // wired into startup by e11s01t5
impl SessionScope {
    pub fn install(identity: SessionIdentity) -> Self {
        let mut previous = BTreeMap::new();
        for key in identity.keys() {
            previous.insert(key.to_owned(), std::env::var(key).ok());
            // SAFETY: installation happens once on the compositor thread before
            // any Wayland clients are accepted; tests serialise with a mutex.
            unsafe { std::env::set_var(key, identity.get(key).unwrap_or_default()) };
        }
        Self {
            identity,
            previous,
            restored: false,
        }
    }

    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        for (key, original) in &self.previous {
            let ours = self.identity.get(key).unwrap_or_default();
            if std::env::var(key).ok().as_deref() != Some(ours) {
                continue; // somebody changed it after us; their value wins
            }
            // SAFETY: paired with install(), on the compositor thread during
            // shutdown; tests serialise with a mutex.
            match original {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

#[allow(dead_code)] // wired into startup by e11s01t5
impl Drop for SessionScope {
    fn drop(&mut self) {
        self.restore();
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

    static SESSION_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn restore_only_owned_vars() {
        let _guard = SESSION_ENV_LOCK.lock().unwrap();
        let keys = [
            "WAYLAND_DISPLAY",
            "XDG_CURRENT_DESKTOP",
            "XDG_SESSION_TYPE",
            "XDG_SESSION_DESKTOP",
        ];
        let originals: Vec<(&str, Option<String>)> = keys
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();

        // Plant known pre-eswm values so restoration is exact.
        for key in keys {
            // SAFETY: SESSION_ENV_LOCK serialises env mutation across tests.
            unsafe { std::env::set_var(key, "pre-existing") };
        }

        let identity = super::SessionIdentity::new("eswm-restore-test");
        let mut scope = super::SessionScope::install(identity);
        assert_eq!(std::env::var("XDG_SESSION_TYPE").unwrap(), "wayland");

        // A user edit while eswm runs must win over shutdown cleanup.
        // SAFETY: SESSION_ENV_LOCK serialises env mutation across tests.
        unsafe { std::env::set_var("XDG_SESSION_TYPE", "user-tweaked") };
        scope.restore();

        assert_eq!(std::env::var("XDG_SESSION_TYPE").unwrap(), "user-tweaked");
        assert_eq!(std::env::var("WAYLAND_DISPLAY").unwrap(), "pre-existing");
        assert_eq!(std::env::var("XDG_CURRENT_DESKTOP").unwrap(), "pre-existing");
        assert_eq!(std::env::var("XDG_SESSION_DESKTOP").unwrap(), "pre-existing");

        // Idempotent: a second restore changes nothing.
        scope.restore();
        assert_eq!(std::env::var("XDG_SESSION_TYPE").unwrap(), "user-tweaked");

        // Put the test process back the way it was.
        for (key, original) in originals {
            // SAFETY: SESSION_ENV_LOCK serialises env mutation across tests.
            match original {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }

    #[test]
    fn session_scope_drops_once() {
        let _guard = SESSION_ENV_LOCK.lock().unwrap();
        let keys = [
            "WAYLAND_DISPLAY",
            "XDG_CURRENT_DESKTOP",
            "XDG_SESSION_TYPE",
            "XDG_SESSION_DESKTOP",
        ];
        let originals: Vec<(&str, Option<String>)> = keys
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();
        for key in keys {
            // SAFETY: SESSION_ENV_LOCK serialises env mutation across tests.
            unsafe { std::env::set_var(key, "pre-existing") };
        }

        // An un-restored scope puts everything back when it drops.
        {
            let _scope = super::SessionScope::install(super::SessionIdentity::new("eswm-drop-test"));
            assert_eq!(std::env::var("XDG_SESSION_TYPE").unwrap(), "wayland");
        }
        assert_eq!(std::env::var("XDG_SESSION_TYPE").unwrap(), "pre-existing");

        // An explicit restore makes the eventual drop a no-op: a later value
        // must survive. Drop is not allowed to restore twice.
        let mut scope = super::SessionScope::install(super::SessionIdentity::new("eswm-drop-test-2"));
        scope.restore();
        // SAFETY: SESSION_ENV_LOCK serialises env mutation across tests.
        unsafe { std::env::set_var("XDG_CURRENT_DESKTOP", "after-restore") };
        drop(scope);
        assert_eq!(std::env::var("XDG_CURRENT_DESKTOP").unwrap(), "after-restore");

        for (key, original) in originals {
            // SAFETY: SESSION_ENV_LOCK serialises env mutation across tests.
            match original {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }

    #[test]
    fn session_identity_applies_to_child_command() {
        let identity = super::SessionIdentity::new("eswm-test");
        let mut command = std::process::Command::new("true");
        super::apply_to_command(&identity, &mut command);
        let envs: std::collections::BTreeMap<&str, &str> = command
            .get_envs()
            .filter_map(|(key, value)| Some((key.to_str()?, value?.to_str()?)))
            .collect();
        assert_eq!(envs.get("WAYLAND_DISPLAY"), Some(&"eswm-test"));
        assert_eq!(envs.get("XDG_CURRENT_DESKTOP"), Some(&"eswm"));
        assert_eq!(envs.get("XDG_SESSION_TYPE"), Some(&"wayland"));
        assert_eq!(envs.get("XDG_SESSION_DESKTOP"), Some(&"eswm"));
    }

    #[test]
    fn caller_env_values_are_preserved() {
        let identity = super::SessionIdentity::new("eswm-test");
        let mut command = std::process::Command::new("true");
        command.env("WAYLAND_DISPLAY", "caller-owned-wayland");
        super::apply_to_command(&identity, &mut command);
        let envs: std::collections::BTreeMap<&str, &str> = command
            .get_envs()
            .filter_map(|(key, value)| Some((key.to_str()?, value?.to_str()?)))
            .collect();
        assert_eq!(envs.get("WAYLAND_DISPLAY"), Some(&"caller-owned-wayland"));
        assert_eq!(envs.get("XDG_CURRENT_DESKTOP"), Some(&"eswm"));
    }
}
