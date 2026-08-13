//! Golden-image tests for the tile renderer.
//!
//! `render.rs`'s own tests only check "it produced a PNG" and "the same tile renders to the same
//! bytes twice" — neither of those notices if a theme tweak, a layout constant, or the wrapping
//! maths quietly makes a tile unreadable (wrong colour, text pushed off the edge, lines
//! overlapping). The font is vendored specifically so rendering is byte-identical across
//! machines (see the module doc on `render.rs`); these tests are what spends that guarantee.
//!
//! Each test renders one representative tile and compares the PNG bytes against a fixture
//! committed under `tests/golden/`. Together they cover every [`Tile`] variant, every
//! [`AgentStatus`], the focused and unfocused rings, more than one key size, a touchstrip
//! segment, and the three awkward wrapping cases the wrap code exists for (a long kebab-case
//! name, a name with no separators, and a name with XML metacharacters).
//!
//! # Regenerating fixtures
//!
//! When a change to the theme or layout is *intentional*, rewrite the fixtures instead of
//! hand-editing PNGs:
//!
//! ```sh
//! UPDATE_GOLDEN=1 cargo test --offline -p herdr-deck-core --test golden
//! ```
//!
//! That rewrites every fixture under `tests/golden/` from the current renderer output and always
//! passes (there is nothing to assert against while writing). Look at the diff — with an image
//! viewer, not just `git diff --stat` — before committing; `UPDATE_GOLDEN` will happily "fix" a
//! genuine regression by baking it in as the new expectation.

use herdr_deck_core::render::{Tile, TileRenderer};
use herdr_deck_core::state::WaitBucket;
use herdr_deck_core::theme::Theme;
use herdr_deck_herdr::wire::AgentStatus;
use std::path::PathBuf;

fn renderer() -> TileRenderer {
    TileRenderer::new(Theme::Dark)
}

fn golden_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR, not the current directory, so this works regardless of where `cargo
    // test` happens to be invoked from.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Compare `actual` against the fixture named `name` (no extension), or write it as the new
/// fixture when `UPDATE_GOLDEN` is set.
fn assert_matches_golden(name: &str, actual: &[u8]) {
    let path = golden_dir().join(format!("{name}.png"));

    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual)
            .unwrap_or_else(|e| panic!("failed to write golden fixture {}: {e}", path.display()));
        return;
    }

    let expected = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "golden fixture `{name}` is missing at {path}: {e}\n\
             \n\
             Regenerate it (and inspect the result before committing) with:\n\
             \n    UPDATE_GOLDEN=1 cargo test --offline -p herdr-deck-core --test golden\n",
            path = path.display(),
        )
    });

    assert!(
        actual == expected.as_slice(),
        "rendered tile no longer matches the golden fixture `{name}` \
         ({actual_len} bytes rendered vs {expected_len} bytes in tests/golden/{name}.png).\n\
         \n\
         If this is an intentional visual change (theme, layout, font), regenerate the fixture \
         and eyeball the new PNG before committing it:\n\
         \n    UPDATE_GOLDEN=1 cargo test --offline -p herdr-deck-core --test golden\n\
         \n\
         If it is not intentional, this is the regression the golden tests exist to catch.",
        actual_len = actual.len(),
        expected_len = expected.len(),
    );
}

fn assert_key_matches_golden(name: &str, tile: &Tile, size: u32) {
    let png = renderer()
        .render_key(tile, size)
        .unwrap_or_else(|e| panic!("`{name}` failed to render: {e}"));
    assert_matches_golden(name, &png);
}

// --- AgentStatus coverage, one representative tile each -------------------------------------
//
// Blocked and Working also double up as the focused/unfocused-ring cases below, so they are not
// repeated here.

#[test]
fn an_idle_agent_tile_pins_the_dim_calm_palette() {
    let tile = Tile::Agent {
        label: "idle-agent".into(),
        sublabel: None,
        workspace: None,
        status: AgentStatus::Idle,
        focused: false,
        acknowledged: false,
        waiting: None,
    };
    assert_key_matches_golden("agent_idle", &tile, 120);
}

#[test]
fn a_done_agent_tile_pins_the_amber_completion_palette() {
    let tile = Tile::Agent {
        label: "ship-release".into(),
        sublabel: None,
        workspace: None,
        status: AgentStatus::Done,
        focused: false,
        acknowledged: false,
        waiting: None,
    };
    assert_key_matches_golden("agent_done", &tile, 120);
}

#[test]
fn an_unknown_status_agent_tile_pins_the_fallback_palette() {
    // Unknown is the landing spot for any status string a future herdr adds that we do not
    // recognise yet, so it has to stay visually distinct from every state we do know about.
    let tile = Tile::Agent {
        label: "mystery-agent".into(),
        sublabel: None,
        workspace: None,
        status: AgentStatus::Unknown,
        focused: false,
        acknowledged: false,
        waiting: None,
    };
    assert_key_matches_golden("agent_unknown", &tile, 120);
}

#[test]
fn an_acknowledged_agent_tile_pins_the_calm_palette_and_its_dismissed_glyph() {
    // A blocked agent the user has dismissed with a long press. It must read as calm — that is
    // the whole point of dismissing it — while still carrying a glyph of its own, so it can never
    // be mistaken for an agent that simply has nothing to say.
    let tile = Tile::Agent {
        label: "refactor-auth".into(),
        sublabel: Some("claude".into()),
        workspace: Some("api".into()),
        status: AgentStatus::Blocked,
        focused: false,
        acknowledged: true,
        waiting: None,
    };
    assert_key_matches_golden("agent_acknowledged", &tile, 120);
}

// --- Focus ring, key size, sublabel/workspace footer ---------------------------------------

#[test]
fn a_blocked_focused_agent_tile_pins_the_alert_palette_footer_and_focus_ring() {
    // Blocked is the state the whole product exists for, so this fixture carries the most
    // surface area: alert colours, the sublabel line, the workspace footer, and the focus ring
    // all at once, at the largest native key size (the Stream Deck +'s 120px).
    let tile = Tile::Agent {
        label: "refactor-auth".into(),
        sublabel: Some("claude".into()),
        workspace: Some("api".into()),
        status: AgentStatus::Blocked,
        focused: true,
        acknowledged: false,
        waiting: None,
    };
    assert_key_matches_golden("agent_blocked_focused_120", &tile, 120);
}

#[test]
fn a_working_unfocused_agent_tile_at_the_smallest_key_size_has_no_ring() {
    // The Stream Deck (Original/MK.2) renders at 72px, the smallest native size — small enough
    // that a rendering regression is most likely to bite here first.
    let tile = Tile::Agent {
        label: "api".into(),
        sublabel: None,
        workspace: None,
        status: AgentStatus::Working,
        focused: false,
        acknowledged: false,
        waiting: None,
    };
    assert_key_matches_golden("agent_working_unfocused_72", &tile, 72);
}

#[test]
fn a_real_project_name_in_the_footer_stays_legible_on_the_smallest_key() {
    // The footer used to hold a two-character workspace id; it now carries a real project name,
    // which is the longest text that row has ever had to fit. 72px with a sublabel present is
    // where it is tightest — all three rows competing for the same key at the smallest size.
    let tile = Tile::Agent {
        label: "refactor-auth".into(),
        sublabel: Some("claude".into()),
        workspace: Some("payments-api".into()),
        status: AgentStatus::Blocked,
        focused: true,
        acknowledged: false,
        waiting: None,
    };
    assert_key_matches_golden("agent_project_footer_72", &tile, 72);
}

#[test]
fn a_focused_workspace_tile_pins_its_layout_without_a_sublabel_or_footer() {
    // Workspace reuses the agent layout but never carries a sublabel or workspace footer, so it
    // needs its own fixture to pin that the label alone still centres correctly.
    let tile = Tile::Workspace {
        label: "api".into(),
        status: AgentStatus::Working,
        focused: true,
    };
    assert_key_matches_golden("workspace_focused_96", &tile, 96);
}

// --- Other Tile variants --------------------------------------------------------------------

#[test]
fn attention_with_agents_waiting_pins_the_alert_palette() {
    let tile = Tile::Attention { count: 3 };
    assert_key_matches_golden("attention_waiting", &tile, 120);
}

#[test]
fn attention_with_nothing_waiting_pins_the_all_clear_palette() {
    let tile = Tile::Attention { count: 0 };
    assert_key_matches_golden("attention_all_clear", &tile, 120);
}

#[test]
fn an_active_mode_tile_pins_its_underline_indicator() {
    let tile = Tile::Mode {
        label: "agents".into(),
        active: true,
    };
    assert_key_matches_golden("mode_active", &tile, 120);
}

#[test]
fn an_inactive_mode_tile_pins_the_dimmed_look_with_no_indicator() {
    let tile = Tile::Mode {
        label: "agents".into(),
        active: false,
    };
    assert_key_matches_golden("mode_inactive", &tile, 120);
}

#[test]
fn an_empty_slot_pins_the_neutral_background() {
    assert_key_matches_golden("empty", &Tile::Empty, 120);
}

#[test]
fn an_offline_tile_pins_the_warning_palette_and_its_wrapped_message() {
    let tile = Tile::Offline {
        message: "herdr offline, retrying in 5s".into(),
    };
    assert_key_matches_golden("offline", &tile, 120);
}

#[test]
fn a_touchstrip_segment_pins_its_title_and_value_layout() {
    // 200x100 is a Stream Deck +'s touchstrip divided across its 4 dials — the only touchstrip
    // geometry that exists today.
    let png = renderer()
        .render_strip(
            "agent",
            "refactor-auth",
            Some(AgentStatus::Blocked),
            200,
            100,
        )
        .expect("strip renders");
    assert_matches_golden("touchstrip_segment", &png);
}

// --- Wait escalation ------------------------------------------------------------------------
//
// The escalation has to survive on a tile that is already carrying a label, a sublabel, a
// workspace footer and a status glyph, so every fixture below uses exactly that tile and varies
// only the bucket. Rendered at both ends of the size range: 120px is where it should look
// deliberate, 72px is where a marker one size too big turns the tile into soup. Look at all six
// after regenerating — this is the change most likely to be a mistake, and the fixtures are the
// only place that is visible.

/// The busiest agent tile there is, waiting for `bucket`.
fn waiting_tile(bucket: WaitBucket) -> Tile {
    Tile::Agent {
        label: "refactor-auth".into(),
        sublabel: Some("claude".into()),
        workspace: Some("payments-api".into()),
        status: AgentStatus::Blocked,
        focused: false,
        acknowledged: false,
        waiting: Some(bucket),
    }
}

#[test]
fn a_freshly_blocked_agent_pins_the_quietest_rung_of_the_escalation() {
    assert_key_matches_golden(
        "agent_wait_fresh_120",
        &waiting_tile(WaitBucket::Fresh),
        120,
    );
}

#[test]
fn an_agent_blocked_for_a_few_minutes_pins_the_middle_rung() {
    assert_key_matches_golden(
        "agent_wait_waiting_120",
        &waiting_tile(WaitBucket::Waiting),
        120,
    );
}

#[test]
fn an_agent_blocked_for_over_five_minutes_pins_the_loudest_rung() {
    // Thickest bar, boldest marker. It still must not out-shout the label: the point is to know
    // *which* agent to go to, and the duration only ranks them.
    assert_key_matches_golden(
        "agent_wait_overdue_120",
        &waiting_tile(WaitBucket::Overdue),
        120,
    );
}

#[test]
fn the_quietest_rung_stays_legible_on_the_smallest_key() {
    assert_key_matches_golden("agent_wait_fresh_72", &waiting_tile(WaitBucket::Fresh), 72);
}

#[test]
fn the_middle_rung_stays_legible_on_the_smallest_key() {
    assert_key_matches_golden(
        "agent_wait_waiting_72",
        &waiting_tile(WaitBucket::Waiting),
        72,
    );
}

#[test]
fn the_loudest_rung_stays_legible_on_the_smallest_key() {
    // 72px with all four text rows and both escalation channels present is the worst case the
    // tile will ever be asked to draw. If anything is going to collide, it collides here.
    assert_key_matches_golden(
        "agent_wait_overdue_72",
        &waiting_tile(WaitBucket::Overdue),
        72,
    );
}

// --- The awkward wrapping cases wrap_text exists for ----------------------------------------

#[test]
fn a_long_kebab_case_agent_name_pins_its_hyphen_aware_wrap() {
    // Agent names are nearly always kebab-case; wrap_text prefers to break after a `-` so this
    // reads as "implement-oauth2-" / "token-refresh" rather than a mid-syllable hard cut.
    let tile = Tile::Agent {
        label: "implement-oauth2-token-refresh-flow".into(),
        sublabel: None,
        workspace: Some("api".into()),
        status: AgentStatus::Working,
        focused: false,
        acknowledged: false,
        waiting: None,
    };
    assert_key_matches_golden("agent_long_kebab_case_name", &tile, 96);
}

#[test]
fn an_agent_name_with_no_separators_pins_its_hard_split_wrap() {
    // No `-` or `_` anywhere, so wrap_text has no good break point and must hard-split within
    // the character budget rather than overflow the tile.
    let tile = Tile::Agent {
        label: "xkcdthisisonereallylongwordwithnohyphens".into(),
        sublabel: None,
        workspace: None,
        status: AgentStatus::Working,
        focused: false,
        acknowledged: false,
        waiting: None,
    };
    assert_key_matches_golden("agent_name_with_no_separators", &tile, 96);
}

#[test]
fn an_agent_name_with_xml_metacharacters_pins_correctly_escaped_output() {
    // Agent titles come from herdr, which gets them from arbitrary terminal output — this must
    // render as literal text, not corrupt the SVG or vanish as unescaped markup.
    let tile = Tile::Agent {
        label: "fix <script> & \"quotes\"".into(),
        sublabel: Some("a > b".into()),
        workspace: Some("it's".into()),
        status: AgentStatus::Working,
        focused: false,
        acknowledged: false,
        waiting: None,
    };
    assert_key_matches_golden("agent_xml_metacharacters", &tile, 120);
}
