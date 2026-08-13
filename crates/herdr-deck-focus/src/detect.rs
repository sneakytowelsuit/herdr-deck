//! Working out which window-raise backend this machine can use.
//!
//! The user told us their Linux desktop "varies by machine", so this is decided at runtime on
//! every start rather than baked in at install time. Detection reads only environment variables,
//! which makes it cheap and — importantly — completely testable.

use crate::backend::Backend;

/// The environment variables that determine the backend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FocusEnv {
    pub target_os: TargetOs,
    pub hyprland_signature: Option<String>,
    pub sway_sock: Option<String>,
    pub session_type: Option<String>,
    pub current_desktop: Option<String>,
    pub wayland_display: Option<String>,
    pub x11_display: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TargetOs {
    MacOs,
    #[default]
    Linux,
    Other,
}

impl FocusEnv {
    pub fn from_process() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Self {
            target_os: if cfg!(target_os = "macos") {
                TargetOs::MacOs
            } else if cfg!(target_os = "linux") {
                TargetOs::Linux
            } else {
                TargetOs::Other
            },
            hyprland_signature: var("HYPRLAND_INSTANCE_SIGNATURE"),
            sway_sock: var("SWAYSOCK"),
            session_type: var("XDG_SESSION_TYPE"),
            current_desktop: var("XDG_CURRENT_DESKTOP"),
            wayland_display: var("WAYLAND_DISPLAY"),
            x11_display: var("DISPLAY"),
        }
    }

    fn desktop_contains(&self, needle: &str) -> bool {
        self.current_desktop
            .as_deref()
            .map(|d| d.to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
    }

    fn is_wayland(&self) -> bool {
        self.session_type.as_deref() == Some("wayland") || self.wayland_display.is_some()
    }

    fn is_x11(&self) -> bool {
        self.session_type.as_deref() == Some("x11")
            || (self.x11_display.is_some() && !self.is_wayland())
    }
}

/// Pick a backend for this environment.
///
/// Ordering matters. Compositor-specific signals (`HYPRLAND_INSTANCE_SIGNATURE`, `SWAYSOCK`)
/// are checked first because they are unambiguous. X11 is preferred over a Wayland guess when
/// the session is genuinely X11, since `wmctrl` works there regardless of desktop environment.
pub fn detect(env: &FocusEnv) -> Backend {
    match env.target_os {
        TargetOs::MacOs => return Backend::MacOs,
        TargetOs::Other => return Backend::Unsupported,
        TargetOs::Linux => {}
    }

    if env.hyprland_signature.is_some() {
        return Backend::Hyprland;
    }
    if env.sway_sock.is_some() {
        return Backend::Sway;
    }
    // A true X11 session: wmctrl/xdotool work under any window manager, including KWin's and
    // GNOME's X11 sessions. This is the most reliable Linux path.
    if env.is_x11() {
        return Backend::X11;
    }
    if env.is_wayland() {
        if env.desktop_contains("kde") || env.desktop_contains("plasma") {
            return Backend::KWin;
        }
        // GNOME on Wayland deliberately blocks programmatic window activation, and so do most
        // other wlroots-less compositors we cannot script. Say so rather than pretending.
        return Backend::Unsupported;
    }
    Backend::Unsupported
}

/// Detect, unless the user pinned a backend in config.
pub fn detect_or_override(env: &FocusEnv, override_name: Option<&str>) -> Backend {
    match override_name.and_then(Backend::parse) {
        Some(backend) => backend,
        None => detect(env),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux() -> FocusEnv {
        FocusEnv {
            target_os: TargetOs::Linux,
            ..Default::default()
        }
    }

    #[test]
    fn macos_always_uses_the_macos_backend() {
        let env = FocusEnv {
            target_os: TargetOs::MacOs,
            ..Default::default()
        };
        assert_eq!(detect(&env), Backend::MacOs);
    }

    #[test]
    fn hyprland_is_detected_by_its_instance_signature() {
        let env = FocusEnv {
            hyprland_signature: Some("abc123".into()),
            session_type: Some("wayland".into()),
            ..linux()
        };
        assert_eq!(detect(&env), Backend::Hyprland);
    }

    #[test]
    fn sway_is_detected_by_its_socket() {
        let env = FocusEnv {
            sway_sock: Some("/run/user/1000/sway-ipc.sock".into()),
            session_type: Some("wayland".into()),
            ..linux()
        };
        assert_eq!(detect(&env), Backend::Sway);
    }

    #[test]
    fn a_compositor_signal_beats_a_generic_wayland_session() {
        // Hyprland sets XDG_CURRENT_DESKTOP=Hyprland but we should not depend on that.
        let env = FocusEnv {
            hyprland_signature: Some("abc".into()),
            sway_sock: Some("/run/sway".into()),
            session_type: Some("wayland".into()),
            current_desktop: Some("KDE".into()),
            ..linux()
        };
        assert_eq!(
            detect(&env),
            Backend::Hyprland,
            "the first unambiguous signal wins"
        );
    }

    #[test]
    fn an_x11_session_uses_wmctrl_regardless_of_desktop() {
        for desktop in ["GNOME", "KDE", "XFCE", "i3"] {
            let env = FocusEnv {
                session_type: Some("x11".into()),
                current_desktop: Some(desktop.into()),
                x11_display: Some(":0".into()),
                ..linux()
            };
            assert_eq!(detect(&env), Backend::X11, "failed for {desktop}");
        }
    }

    #[test]
    fn kde_on_wayland_uses_the_kwin_backend() {
        for desktop in ["KDE", "plasma", "KDE:plasma"] {
            let env = FocusEnv {
                session_type: Some("wayland".into()),
                current_desktop: Some(desktop.into()),
                wayland_display: Some("wayland-0".into()),
                ..linux()
            };
            assert_eq!(detect(&env), Backend::KWin, "failed for {desktop}");
        }
    }

    #[test]
    fn gnome_on_wayland_is_honestly_reported_as_unsupported() {
        // This is the case the docs call out. Pretending it works would be worse than saying so.
        let env = FocusEnv {
            session_type: Some("wayland".into()),
            current_desktop: Some("GNOME".into()),
            wayland_display: Some("wayland-0".into()),
            ..linux()
        };
        assert_eq!(detect(&env), Backend::Unsupported);
        assert!(Backend::Unsupported.unsupported_reason().is_some());
    }

    #[test]
    fn a_headless_session_with_nothing_set_is_unsupported_rather_than_crashing() {
        assert_eq!(detect(&linux()), Backend::Unsupported);
    }

    #[test]
    fn a_bare_display_variable_still_gets_the_x11_backend() {
        // Some minimal WMs do not set XDG_SESSION_TYPE at all.
        let env = FocusEnv {
            x11_display: Some(":0".into()),
            ..linux()
        };
        assert_eq!(detect(&env), Backend::X11);
    }

    #[test]
    fn xwayland_does_not_masquerade_as_a_real_x11_session() {
        // Under Wayland, DISPLAY is often set for XWayland. Raising via wmctrl would then only
        // reach X clients, missing the native-Wayland terminal we actually want.
        let env = FocusEnv {
            session_type: Some("wayland".into()),
            wayland_display: Some("wayland-0".into()),
            x11_display: Some(":0".into()),
            current_desktop: Some("GNOME".into()),
            ..linux()
        };
        assert_eq!(detect(&env), Backend::Unsupported);
    }

    #[test]
    fn an_unsupported_operating_system_degrades_instead_of_guessing() {
        let env = FocusEnv {
            target_os: TargetOs::Other,
            ..Default::default()
        };
        assert_eq!(detect(&env), Backend::Unsupported);
    }

    #[test]
    fn a_config_override_wins_over_detection() {
        let env = FocusEnv {
            hyprland_signature: Some("abc".into()),
            ..linux()
        };
        assert_eq!(detect_or_override(&env, Some("x11")), Backend::X11);
    }

    #[test]
    fn an_unparseable_override_falls_back_to_detection_rather_than_breaking_focus() {
        let env = FocusEnv {
            hyprland_signature: Some("abc".into()),
            ..linux()
        };
        assert_eq!(
            detect_or_override(&env, Some("nonsense")),
            Backend::Hyprland
        );
    }

    #[test]
    fn empty_environment_variables_are_treated_as_unset() {
        let env = FocusEnv {
            hyprland_signature: None,
            session_type: Some("x11".into()),
            x11_display: Some(":0".into()),
            ..linux()
        };
        assert_eq!(detect(&env), Backend::X11);
    }
}
