//! Window-raise backends.
//!
//! # Why backends produce commands instead of running them
//!
//! Each backend returns a list of [`CommandSpec`]s and the engine executes them. That keeps
//! every "which flags does `hyprctl` take" decision unit-testable on a machine with no
//! compositor, which matters because the one thing we cannot do in CI is attach a real desktop.
//!
//! # The title-marker trick
//!
//! herdr can stamp an arbitrary string into its terminal's window title
//! (`client.window_title.set`). We set a unique marker and then ask the window manager for the
//! window whose title contains it. This finds the *exact* window hosting herdr rather than just
//! any window of the terminal application — which matters if you keep several terminals open.
//! Backends that cannot match on title fall back to matching the application.

use serde::{Deserialize, Serialize};

/// A command to run, as program plus arguments. Never passed through a shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    pub fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }
}

/// What we are trying to bring forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowTarget {
    /// macOS bundle id, or Linux window class / `app_id`.
    pub app_id: String,
    /// A unique string herdr has stamped into the window title, if enabled.
    pub title_marker: Option<String>,
}

/// Which mechanism is being used to raise windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    MacOs,
    Hyprland,
    Sway,
    KWin,
    X11,
    /// No usable mechanism in this environment.
    Unsupported,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::MacOs => "macos",
            Backend::Hyprland => "hyprland",
            Backend::Sway => "sway",
            Backend::KWin => "kwin",
            Backend::X11 => "x11",
            Backend::Unsupported => "unsupported",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "macos" => Some(Backend::MacOs),
            "hyprland" => Some(Backend::Hyprland),
            "sway" => Some(Backend::Sway),
            "kwin" => Some(Backend::KWin),
            "x11" => Some(Backend::X11),
            "unsupported" | "none" => Some(Backend::Unsupported),
            _ => None,
        }
    }

    /// Can this backend target a specific window by title, or only the application?
    pub fn supports_title_matching(self) -> bool {
        matches!(
            self,
            Backend::Hyprland | Backend::Sway | Backend::KWin | Backend::X11
        )
    }

    /// Why this environment cannot raise windows, if it cannot.
    ///
    /// This message is user-facing — it appears in `herdr-deck doctor` and on the deck itself —
    /// so it names the limitation and the workaround rather than just failing.
    pub fn unsupported_reason(self) -> Option<&'static str> {
        match self {
            Backend::Unsupported => Some(
                "No supported way to raise windows in this session. GNOME on Wayland blocks \
                 programmatic window activation; focusing will still switch herdr's active \
                 pane, but you will need to alt-tab to the terminal. See the docs page on \
                 focus backends for workarounds.",
            ),
            _ => None,
        }
    }

    /// The commands to try, in order. The first that exits zero wins.
    pub fn raise_commands(self, target: &WindowTarget) -> Vec<CommandSpec> {
        match self {
            // `open -b` activates the app without needing Automation (TCC) permission, which
            // `osascript ... activate` would prompt for. macOS gives us no supported way to
            // raise one specific window of another app, so app-level is the ceiling here.
            Backend::MacOs => vec![CommandSpec::new("open", &["-b", &target.app_id])],

            Backend::Hyprland => {
                let mut cmds = Vec::new();
                if let Some(marker) = &target.title_marker {
                    cmds.push(CommandSpec::new(
                        "hyprctl",
                        &[
                            "dispatch",
                            "focuswindow",
                            &format!("title:^(.*{marker}.*)$"),
                        ],
                    ));
                }
                cmds.push(CommandSpec::new(
                    "hyprctl",
                    &[
                        "dispatch",
                        "focuswindow",
                        &format!("class:^({})$", target.app_id),
                    ],
                ));
                cmds
            }

            Backend::Sway => {
                let mut cmds = Vec::new();
                if let Some(marker) = &target.title_marker {
                    cmds.push(CommandSpec::new(
                        "swaymsg",
                        &[&format!("[title=\"{marker}\"] focus")],
                    ));
                }
                cmds.push(CommandSpec::new(
                    "swaymsg",
                    &[&format!("[app_id=\"{}\"] focus", target.app_id)],
                ));
                cmds
            }

            // KWin activates windows through KWin scripting and nothing else. Its own D-Bus
            // object `/KWin` offers `killWindow`, `queryWindowInfo` and friends but nothing
            // that raises, and no tool shipping with Plasma raises a window from a command
            // line. The supported route is: write a JavaScript file, `loadScript` it on
            // `/Scripting` (`org.kde.kwin.Scripting`), then `run` it on the object the load
            // returned — a temp file plus two D-Bus calls where the second needs the first
            // one's return value. That cannot be expressed as independent commands tried until
            // one succeeds, so we delegate to `kdotool`, which performs exactly that dance in
            // a single invocation. It is third-party and unlikely to be installed, which is
            // why `doctor` names it: on a Plasma Wayland session it is the whole ballgame.
            //
            // One dishonesty we cannot fix from here: kdotool exits zero when its search
            // matched nothing, so once kdotool is installed the first command below always
            // "succeeds" and the rest are only reached when kdotool is absent entirely.
            //
            // This backend is only ever picked for a *Wayland* Plasma session — a Plasma X11
            // session detects as `x11` — so wmctrl comes last and is a long shot rather than
            // the fallback it looks like. KWin still runs an X11 window manager for Xwayland
            // under Wayland and honours `_NET_ACTIVE_WINDOW` from tools, so wmctrl does raise
            // an Xwayland terminal; it simply cannot see a native Wayland one, which is what
            // most terminals now are.
            Backend::KWin => {
                let mut cmds = Vec::new();
                if let Some(marker) = &target.title_marker {
                    // `--limit 1` because without it kdotool activates every match in turn,
                    // leaving whichever came last in front.
                    cmds.push(CommandSpec::new(
                        "kdotool",
                        &[
                            "search",
                            "--limit",
                            "1",
                            "--name",
                            &kdotool_pattern(marker),
                            // `search` alone only prints window ids; activation is a chained
                            // step and has to be asked for explicitly.
                            "windowactivate",
                        ],
                    ));
                }
                cmds.push(CommandSpec::new(
                    "kdotool",
                    &[
                        "search",
                        "--limit",
                        "1",
                        "--class",
                        // Anchored, so a configured `kitty` cannot activate `kitty-scratchpad`.
                        &format!("^{}$", kdotool_pattern(&target.app_id)),
                        "windowactivate",
                    ],
                ));
                if let Some(marker) = &target.title_marker {
                    cmds.push(CommandSpec::new("wmctrl", &["-a", marker]));
                }
                cmds.push(CommandSpec::new("wmctrl", &["-x", "-a", &target.app_id]));
                cmds
            }

            Backend::X11 => {
                let mut cmds = Vec::new();
                if let Some(marker) = &target.title_marker {
                    // -F would demand an exact full-title match; we want a substring.
                    cmds.push(CommandSpec::new("wmctrl", &["-a", marker]));
                }
                cmds.push(CommandSpec::new("wmctrl", &["-x", "-a", &target.app_id]));
                cmds.push(CommandSpec::new(
                    "xdotool",
                    &["search", "--class", &target.app_id, "windowactivate", "%1"],
                ));
                cmds
            }

            Backend::Unsupported => vec![],
        }
    }

    /// External programs this backend needs, for `doctor` to check.
    pub fn required_tools(self) -> &'static [&'static str] {
        match self {
            Backend::MacOs => &["open"],
            Backend::Hyprland => &["hyprctl"],
            Backend::Sway => &["swaymsg"],
            // Deliberately not `wmctrl`: doctor treats this list as alternatives and goes green
            // when any one of them is present, and a green tick for wmctrl alone would be a
            // lie on the Wayland session this backend is chosen for.
            Backend::KWin => &["kdotool"],
            Backend::X11 => &["wmctrl", "xdotool"],
            Backend::Unsupported => &[],
        }
    }
}

/// Make `text` safe to hand to `kdotool search`.
///
/// kdotool bakes the search term into the KWin script it generates as
/// ``new RegExp(String.raw`<term>`)``, so the term is read twice: once as a JavaScript template
/// literal, then as a regular expression. A bare backtick or `${` closes that literal and the
/// script no longer parses — and a script that does not parse raises nothing while kdotool
/// still exits zero, which is the worst failure we could ship. Backslash-escaping every regex
/// metacharacter closes both holes at once: the character becomes literal to the regex, and
/// `` \` `` / `\$` no longer terminate the template literal. `String.raw` is what makes this
/// work, since it hands our backslashes to the regex engine untouched.
fn kdotool_pattern(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '.' | '*'
                | '+'
                | '?'
                | '^'
                | '$'
                | '{'
                | '}'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '\\'
                | '/'
                | '`'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_with_marker() -> WindowTarget {
        WindowTarget {
            app_id: "com.mitchellh.ghostty".into(),
            title_marker: Some("herdr-deck:9f3a".into()),
        }
    }

    fn target_without_marker() -> WindowTarget {
        WindowTarget {
            app_id: "com.mitchellh.ghostty".into(),
            title_marker: None,
        }
    }

    #[test]
    fn macos_uses_open_dash_b_to_avoid_a_tcc_automation_prompt() {
        let cmds = Backend::MacOs.raise_commands(&target_with_marker());
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].program, "open");
        assert_eq!(cmds[0].args, vec!["-b", "com.mitchellh.ghostty"]);
        // osascript would work but prompts for Automation permission on first use.
        assert!(!cmds.iter().any(|c| c.program == "osascript"));
    }

    #[test]
    fn title_marker_is_tried_before_falling_back_to_the_application() {
        for backend in [
            Backend::Hyprland,
            Backend::Sway,
            Backend::KWin,
            Backend::X11,
        ] {
            let cmds = backend.raise_commands(&target_with_marker());
            assert!(
                cmds.len() >= 2,
                "{backend:?} should try the marker then fall back"
            );
            let first = format!("{:?}", cmds[0]);
            assert!(
                first.contains("herdr-deck:9f3a"),
                "{backend:?} should try the title marker first, got {first}"
            );
        }
    }

    #[test]
    fn without_a_marker_backends_fall_straight_through_to_app_matching() {
        for backend in [
            Backend::Hyprland,
            Backend::Sway,
            Backend::KWin,
            Backend::X11,
        ] {
            let cmds = backend.raise_commands(&target_without_marker());
            for cmd in &cmds {
                let rendered = format!("{cmd:?}");
                assert!(
                    !rendered.contains("herdr-deck:"),
                    "{backend:?} emitted a marker command with no marker set"
                );
            }
            assert!(
                !cmds.is_empty(),
                "{backend:?} still needs an app-level path"
            );
        }
    }

    #[test]
    fn hyprland_anchors_its_regexes_so_a_substring_class_cannot_match_the_wrong_window() {
        let cmds = Backend::Hyprland.raise_commands(&target_without_marker());
        let class_arg = cmds[0].args.last().unwrap();
        assert!(class_arg.starts_with("class:^("));
        assert!(class_arg.ends_with(")$"));
    }

    #[test]
    fn sway_uses_its_criteria_syntax() {
        let cmds = Backend::Sway.raise_commands(&target_without_marker());
        assert_eq!(cmds[0].program, "swaymsg");
        assert_eq!(cmds[0].args[0], "[app_id=\"com.mitchellh.ghostty\"] focus");
    }

    #[test]
    fn kwin_tells_kdotool_to_activate_the_window_rather_than_merely_print_its_id() {
        // `kdotool search …` on its own only lists matching windows. Activation is a chained
        // step, and leaving it off is a raise that reports success and moves nothing.
        let cmds = Backend::KWin.raise_commands(&target_with_marker());
        for cmd in cmds.iter().filter(|c| c.program == "kdotool") {
            assert!(
                cmd.args.contains(&"windowactivate".to_string()),
                "kdotool invocation does nothing: {cmd:?}"
            );
        }
    }

    #[test]
    fn kwin_reaches_for_kdotool_before_wmctrl_because_wmctrl_is_blind_to_wayland_windows() {
        // The kwin backend is only chosen for a Plasma *Wayland* session, where wmctrl can
        // only reach Xwayland clients. Preferring it would half-work at best.
        let cmds = Backend::KWin.raise_commands(&target_with_marker());
        let first_wmctrl = cmds.iter().position(|c| c.program == "wmctrl");
        let last_kdotool = cmds.iter().rposition(|c| c.program == "kdotool");
        assert!(last_kdotool < first_wmctrl, "{cmds:?}");
    }

    #[test]
    fn kwin_anchors_the_class_pattern_so_a_prefix_cannot_activate_a_different_terminal() {
        // kdotool's --class is a substring regex, so an unanchored `kitty` would also match
        // `kitty-scratchpad`.
        let cmds = Backend::KWin.raise_commands(&target_without_marker());
        let class_pattern = cmds[0].args.iter().find(|a| a.contains("ghostty")).unwrap();
        assert_eq!(class_pattern, r"^com\.mitchellh\.ghostty$");
    }

    #[test]
    fn a_kwin_search_term_cannot_break_out_of_the_javascript_it_is_interpolated_into() {
        // kdotool builds a KWin script containing new RegExp(String.raw`<term>`). A raw
        // backtick would end that literal, and a script that fails to parse raises nothing
        // while still exiting zero — a silent no-op, the one failure we refuse to ship.
        let hostile = "`+workspace.windowList()[0].closeWindow()+`${x}";
        let escaped = kdotool_pattern(hostile);
        for (i, ch) in escaped.char_indices() {
            if matches!(ch, '`' | '$' | '(' | ')' | '[' | ']') {
                assert!(escaped[..i].ends_with('\\'), "unescaped {ch} in {escaped}");
            }
        }

        // And the escaped form is what actually reaches the command line — only the anchors
        // we add ourselves stay live.
        let target = WindowTarget {
            app_id: "ghostty".into(),
            title_marker: Some(hostile.to_string()),
        };
        let cmds = Backend::KWin.raise_commands(&target);
        assert_eq!(cmds[0].args[4], escaped);
    }

    #[test]
    fn kwin_asks_doctor_only_for_kdotool_because_wmctrl_alone_would_be_false_reassurance() {
        // doctor goes green when any one of the required tools is present. wmctrl is a
        // last-resort Xwayland long shot here, not an alternative to kdotool.
        assert_eq!(Backend::KWin.required_tools(), &["kdotool"]);
        assert!(Backend::KWin
            .raise_commands(&target_with_marker())
            .iter()
            .any(|c| c.program == "wmctrl"));
    }

    #[test]
    fn an_unsupported_environment_produces_no_commands_and_explains_itself() {
        let cmds = Backend::Unsupported.raise_commands(&target_with_marker());
        assert!(cmds.is_empty());
        let reason = Backend::Unsupported.unsupported_reason().unwrap();
        assert!(reason.contains("GNOME"), "should name the common cause");
        assert!(
            reason.contains("herdr's active pane"),
            "should reassure that focusing still half-works"
        );
    }

    #[test]
    fn supported_backends_have_no_unsupported_reason() {
        for backend in [
            Backend::MacOs,
            Backend::Hyprland,
            Backend::Sway,
            Backend::KWin,
            Backend::X11,
        ] {
            assert!(backend.unsupported_reason().is_none(), "{backend:?}");
            assert!(!backend.raise_commands(&target_with_marker()).is_empty());
        }
    }

    #[test]
    fn macos_cannot_target_a_single_window_so_the_marker_buys_nothing_there() {
        assert!(!Backend::MacOs.supports_title_matching());
        assert!(Backend::Hyprland.supports_title_matching());
    }

    #[test]
    fn backend_names_round_trip() {
        for backend in [
            Backend::MacOs,
            Backend::Hyprland,
            Backend::Sway,
            Backend::KWin,
            Backend::X11,
            Backend::Unsupported,
        ] {
            assert_eq!(Backend::parse(backend.name()), Some(backend));
        }
        assert_eq!(Backend::parse("nonsense"), None);
    }

    #[test]
    fn no_command_is_ever_routed_through_a_shell() {
        // Shelling out with a window title in it would be a command-injection hole, since
        // titles come from arbitrary terminal output.
        let nasty = WindowTarget {
            app_id: "ghostty; rm -rf /".into(),
            title_marker: Some("$(whoami)".into()),
        };
        for backend in [
            Backend::MacOs,
            Backend::Hyprland,
            Backend::Sway,
            Backend::KWin,
            Backend::X11,
        ] {
            for cmd in backend.raise_commands(&nasty) {
                assert!(
                    !matches!(cmd.program.as_str(), "sh" | "bash" | "zsh" | "/bin/sh"),
                    "{backend:?} must not invoke a shell"
                );
            }
        }
    }
}
