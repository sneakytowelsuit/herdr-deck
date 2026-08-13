//! `herdr-plugin.toml` must satisfy the schema herdr actually parses.
//!
//! This file is consumed by a *different program*, so nothing else in this repository would
//! notice it drifting — and it shipped broken: the actions used `name` where herdr requires
//! `title`, and `herdr plugin install` failed with "missing field title" at the first
//! `[[actions]]`. Every other manifest in this project is validated by tooling (Cargo checks
//! `Cargo.toml`, `streamdeck validate` checks the Stream Deck manifest); this one had nothing.
//!
//! The structs below mirror `RawPluginManifest` in herdr's
//! `src/app/api/plugins/manifest.rs` as of herdr 0.8.x — same field names, same required-versus-
//! optional split. `deny_unknown_fields` is deliberately stricter than herdr, which ignores
//! unknown keys: a field we invent is far more likely to be a typo we want to hear about than a
//! forward-compatible addition.
//!
//! If herdr changes its manifest schema, this test will pass while the real thing breaks. That
//! is the limit of what can be checked without herdr installed, and it is still a great deal
//! better than the nothing it replaces.

use serde::Deserialize;

/// Several fields here are never read, and must stay anyway: `deny_unknown_fields` rejects any
/// key the struct does not declare, so omitting a field herdr allows would make this test fail on
/// a perfectly valid manifest. They are part of the schema being asserted, not dead weight.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct Manifest {
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    min_herdr_version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    platforms: Option<Vec<Platform>>,
    #[serde(default)]
    build: Vec<Build>,
    #[serde(default)]
    startup: Vec<Build>,
    #[serde(default)]
    actions: Vec<Action>,
}

/// Several fields here are never read, and must stay anyway: `deny_unknown_fields` rejects any
/// key the struct does not declare, so omitting a field herdr allows would make this test fail on
/// a perfectly valid manifest. They are part of the schema being asserted, not dead weight.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct Build {
    #[serde(default)]
    platforms: Option<Vec<Platform>>,
    command: Vec<String>,
}

/// Several fields here are never read, and must stay anyway: `deny_unknown_fields` rejects any
/// key the struct does not declare, so omitting a field herdr allows would make this test fail on
/// a perfectly valid manifest. They are part of the schema being asserted, not dead weight.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct Action {
    id: String,
    /// herdr calls this `title`. Calling it `name` is exactly the bug this file exists to catch.
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    contexts: Vec<Context>,
    #[serde(default)]
    platforms: Option<Vec<Platform>>,
    command: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Platform {
    Linux,
    Macos,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Context {
    Global,
    Workspace,
    Tab,
    Pane,
    Selection,
}

fn manifest() -> Manifest {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../herdr-plugin.toml");
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("herdr-plugin.toml must exist at the repository root: {e}"));
    toml::from_str(&text).unwrap_or_else(|e| {
        panic!("herdr-plugin.toml does not match the schema herdr parses:\n{e}")
    })
}

#[test]
fn the_plugin_manifest_parses_as_herdr_would_parse_it() {
    let manifest = manifest();
    assert_eq!(manifest.id, "sneakytowelsuit.herdr-deck");
    assert!(
        !manifest.actions.is_empty(),
        "the manifest declares actions"
    );
}

#[test]
fn every_action_has_the_title_herdr_requires() {
    // The shipped bug: actions carried `name`, so `herdr plugin install` died on the first one
    // with "missing field title". Deserialization above already enforces presence; this also
    // catches an empty string, which parses fine and shows as a blank entry in herdr's UI.
    for action in manifest().actions {
        assert!(
            !action.title.trim().is_empty(),
            "action `{}` has a blank title",
            action.id
        );
    }
}

#[test]
fn action_ids_are_unique_and_use_only_the_characters_herdr_allows() {
    // herdr qualifies action ids as `plugin.id.action`, so a dot inside a local id would make
    // that ambiguous — it rejects them for that reason.
    let actions = manifest().actions;
    let mut seen = std::collections::HashSet::new();
    for action in &actions {
        assert!(
            seen.insert(action.id.clone()),
            "duplicate action id `{}`",
            action.id
        );
        assert!(
            !action.id.contains('.'),
            "action id `{}` contains a dot, which herdr does not allow in local ids",
            action.id
        );
        assert!(
            action
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-')),
            "action id `{}` uses characters herdr does not allow",
            action.id
        );
    }
}

#[test]
fn every_action_names_a_context_so_herdr_knows_where_to_offer_it() {
    // These actions manage the daemon and diagnose it; none is scoped to a workspace, tab or
    // pane. herdr defaults `contexts` to empty, and an action offered in no context at all is
    // one nobody can find.
    for action in manifest().actions {
        assert!(
            !action.contexts.is_empty(),
            "action `{}` declares no context",
            action.id
        );
        assert!(
            action.contexts.contains(&Context::Global),
            "action `{}` manages the daemon, so it belongs in the global context",
            action.id
        );
    }
}

#[test]
fn every_command_points_at_a_binary_the_build_step_produces() {
    // The build step compiles two binaries into target/release. An action pointing anywhere else
    // fails only at the moment a user presses it, long after install appeared to succeed.
    let manifest = manifest();
    let builds: Vec<String> = manifest
        .build
        .iter()
        .flat_map(|b| b.command.clone())
        .collect();
    assert!(
        builds.iter().any(|arg| arg == "herdr-deck-cli"),
        "the build step must produce the binary the actions invoke: {builds:?}"
    );

    for action in &manifest.actions {
        let program = action
            .command
            .first()
            .unwrap_or_else(|| panic!("action `{}` has an empty command", action.id));
        assert!(
            program.starts_with("target/release/"),
            "action `{}` runs `{program}`, which the build step does not produce",
            action.id
        );
    }
}
