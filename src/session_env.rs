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
