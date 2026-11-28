use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

/// The identity eswm publishes so portals stop inviting file pickers to the
/// wrong compositor's party.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionIdentity {
    values: BTreeMap<String, String>,
}

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

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }
}

/// Build the optional user-scope import as data so tests need no live bus.
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
pub fn apply_to_command<'a>(
    identity: &SessionIdentity,
    command: &'a mut std::process::Command,
) -> &'a mut std::process::Command {
    for key in identity.keys() {
        // Caller values are invited guests; eswm does not kick them out.
        if command.get_envs().any(|(name, _)| name.to_str() == Some(key)) {
            continue;
        }
        command.env(key, identity.get(key).unwrap_or_default());
    }
    command
}

/// Announce the identity to the user session and return the guard that undoes
/// the external part on shutdown. A missing tool, absent user bus or explicit
/// ESWM_NO_SD_VARS is a warning, never a startup failure.
pub fn export_to_user_session(identity: &SessionIdentity) -> Option<SessionExportScope> {
    if std::env::var_os("ESWM_NO_SD_VARS").is_some() {
        tracing::warn!("ESWM_NO_SD_VARS set: skipping systemd/D-Bus session export");
        return None;
    }
    Some(SessionExportScope::install(identity.clone()))
}

fn run_optional(mut command: std::process::Command, label: &str) {
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::warn!(%status, "failed to update {label}"),
        Err(err) => tracing::warn!(?err, "skipping {label} update"),
    }
}

fn parse_show_environment(
    bytes: &[u8],
    identity: &SessionIdentity,
) -> BTreeMap<String, Option<OsString>> {
    let mut parsed: BTreeMap<String, Option<OsString>> = identity
        .keys()
        .map(|key| (key.to_owned(), None))
        .collect();
    for line in bytes.split(|byte| *byte == b'\n') {
        let Some(eq) = line.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let Ok(key) = std::str::from_utf8(&line[..eq]) else {
            continue;
        };
        if !parsed.contains_key(key) {
            continue;
        }
        let mut value = line[eq + 1..].to_vec();
        if value.last() == Some(&b'\r') {
            value.pop();
        }
        parsed.insert(key.to_owned(), Some(OsString::from_vec(value)));
    }
    parsed
}

fn current_external_environment(
    identity: &SessionIdentity,
) -> Option<BTreeMap<String, Option<OsString>>> {
    let mut command = std::process::Command::new("systemctl");
    command.arg("--user").arg("show-environment");
    match command.output() {
        Ok(output) if output.status.success() => Some(parse_show_environment(&output.stdout, identity)),
        Ok(output) => {
            tracing::warn!(status = %output.status, "skipping external session env snapshot");
            None
        }
        Err(err) => {
            tracing::warn!(?err, "skipping external session env snapshot");
            None
        }
    }
}

fn external_restore_args(
    previous: &BTreeMap<String, Option<OsString>>,
    current: &BTreeMap<String, Option<OsString>>,
    identity: &SessionIdentity,
) -> (Vec<OsString>, Vec<OsString>, Vec<OsString>) {
    let mut dbus = Vec::new();
    let mut systemd_set = Vec::new();
    let mut systemd_unset = Vec::new();
    for (key, original) in previous {
        let ours = OsString::from(identity.get(key).unwrap_or_default());
        if current.get(key).and_then(|value| value.as_ref()) != Some(&ours) {
            continue; // somebody changed it after us; their value wins
        }
        match original {
            Some(value) => {
                let mut pair = OsString::from(key);
                pair.push("=");
                pair.push(value);
                dbus.push(pair.clone());
                systemd_set.push(pair);
            }
            None => {
                systemd_unset.push(OsString::from(key));
                let mut cleared = OsString::from(key);
                cleared.push("=");
                dbus.push(cleared);
            }
        }
    }
    (dbus, systemd_set, systemd_unset)
}

/// Snapshots the systemd user environment before export and puts it back when
/// eswm exits. Only values still owned by eswm are touched.
pub struct SessionExportScope {
    identity: SessionIdentity,
    previous: BTreeMap<String, Option<OsString>>,
    captured: bool,
    restored: bool,
}

impl SessionExportScope {
    fn install(identity: SessionIdentity) -> Self {
        let (previous, captured) = match current_external_environment(&identity) {
            Some(values) => (values, true),
            None => (
                identity.keys().map(|key| (key.to_owned(), None)).collect(),
                false,
            ),
        };
        let scope = Self {
            identity,
            previous,
            captured,
            restored: false,
        };
        run_optional(
            systemd_import_command(&scope.identity),
            "systemd user environment",
        );
        run_optional(
            dbus_activation_command(&scope.identity),
            "D-Bus activation environment",
        );
        scope
    }

    pub fn restore(&mut self) {
        if self.restored || !self.captured {
            return;
        }
        self.restored = true;
        let Some(current) = current_external_environment(&self.identity) else {
            return;
        };
        let (dbus, set, unset) = external_restore_args(&self.previous, &current, &self.identity);
        if !dbus.is_empty() {
            let mut command = std::process::Command::new("dbus-update-activation-environment");
            // D-Bus has no unset verb; an absent value gets swept to empty.
            command.args(&dbus);
            run_optional(command, "D-Bus activation environment restore");
        }
        if !set.is_empty() {
            let mut command = std::process::Command::new("systemctl");
            command.arg("--user").arg("set-environment").args(&set);
            run_optional(command, "systemd user environment restore");
        }
        if !unset.is_empty() {
            let mut command = std::process::Command::new("systemctl");
            command.arg("--user").arg("unset-environment").args(&unset);
            run_optional(command, "systemd user environment cleanup");
        }
    }
}

impl Drop for SessionExportScope {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Owns the identity eswm installed in its own process and puts back only what
/// is still ours at shutdown. A user edit to a value while we run wins.
pub struct SessionScope {
    identity: SessionIdentity,
    previous: BTreeMap<String, Option<OsString>>,
    restored: bool,
}

impl SessionScope {
    pub fn install(identity: SessionIdentity) -> Self {
        let mut previous = BTreeMap::new();
        for key in identity.keys() {
            previous.insert(key.to_owned(), std::env::var_os(key));
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
            let ours = OsString::from(self.identity.get(key).unwrap_or_default());
            if std::env::var_os(key).as_deref() != Some(ours.as_os_str()) {
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
        let originals: Vec<(&str, Option<std::ffi::OsString>)> = keys
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
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
        let originals: Vec<(&str, Option<std::ffi::OsString>)> = keys
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
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

    fn os(value: &str) -> std::ffi::OsString {
        std::ffi::OsString::from(value)
    }

    #[test]
    fn parse_show_environment_extracts_only_identity_keys() {
        let bytes = b"LANG=C\nWAYLAND_DISPLAY=hyprland-1\nXDG_CURRENT_DESKTOP=Tacit:Hyprland\nXDG_SESSION_DESKTOP=Tacit\nXDG_SESSION_TYPE=x11\nOTHER=nope\n";
        let identity = super::SessionIdentity::new("eswm-test");
        let parsed = super::parse_show_environment(bytes, &identity);
        assert_eq!(parsed.len(), 4);
        assert_eq!(
            parsed.get("WAYLAND_DISPLAY").and_then(|value| value.as_deref()),
            Some(std::ffi::OsStr::new("hyprland-1"))
        );
        assert_eq!(
            parsed.get("XDG_CURRENT_DESKTOP").and_then(|value| value.as_deref()),
            Some(std::ffi::OsStr::new("Tacit:Hyprland"))
        );
        assert!(parsed.contains_key("XDG_SESSION_TYPE"));
        assert!(parsed.contains_key("XDG_SESSION_DESKTOP"));
    }

    #[test]
    fn external_restore_args_are_ownership_safe() {
        let identity = super::SessionIdentity::new("eswm-test");
        let previous = [
            ("WAYLAND_DISPLAY".to_owned(), Some(os("wayland-0"))),
            ("XDG_CURRENT_DESKTOP".to_owned(), None),
            ("XDG_SESSION_TYPE".to_owned(), Some(os("x11"))),
            ("XDG_SESSION_DESKTOP".to_owned(), Some(os("Hyprland"))),
        ]
        .into_iter()
        .collect();
        let current = [
            ("WAYLAND_DISPLAY".to_owned(), Some(os("eswm-test"))),
            ("XDG_CURRENT_DESKTOP".to_owned(), Some(os("eswm"))),
            ("XDG_SESSION_TYPE".to_owned(), Some(os("user-tweaked"))),
            ("XDG_SESSION_DESKTOP".to_owned(), Some(os("eswm"))),
        ]
        .into_iter()
        .collect();

        let (dbus, set, unset) = super::external_restore_args(&previous, &current, &identity);
        assert_eq!(
            dbus,
            vec![
                os("WAYLAND_DISPLAY=wayland-0"),
                os("XDG_CURRENT_DESKTOP="),
                os("XDG_SESSION_DESKTOP=Hyprland")
            ]
        );
        assert_eq!(
            set,
            vec![os("WAYLAND_DISPLAY=wayland-0"), os("XDG_SESSION_DESKTOP=Hyprland")]
        );
        assert_eq!(unset, vec![os("XDG_CURRENT_DESKTOP")]);
    }

    #[test]
    fn opt_out_skips_external_export() {
        let _guard = SESSION_ENV_LOCK.lock().unwrap();
        let original = std::env::var_os("ESWM_NO_SD_VARS");
        // SAFETY: SESSION_ENV_LOCK serialises env mutation across tests.
        unsafe { std::env::set_var("ESWM_NO_SD_VARS", "1") };
        let identity = super::SessionIdentity::new("eswm-test");
        let scope = super::export_to_user_session(&identity);
        assert!(scope.is_none(), "opt-out must not create an export scope");
        match original {
            Some(value) => unsafe { std::env::set_var("ESWM_NO_SD_VARS", value) },
            None => unsafe { std::env::remove_var("ESWM_NO_SD_VARS") },
        }
    }

    #[test]
    fn optional_export_paths_do_not_panic() {
        let command = std::process::Command::new("true");
        super::run_optional(command, "test success");
        let command = std::process::Command::new("false");
        super::run_optional(command, "test failure");
        let command = std::process::Command::new("eswm-coverage-missing-tool");
        super::run_optional(command, "test missing tool");
    }
}
