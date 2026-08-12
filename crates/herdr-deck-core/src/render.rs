//! Tile rendering.
//!
//! # Why the daemon renders, not the frontends
//!
//! The macOS frontend runs inside Elgato's app and the Linux frontend writes to raw USB HID.
//! If each drew its own tiles they would drift — different fonts, different metrics, different
//! rounding. Instead both are handed finished pixels from here, so "it looks the same on both
//! machines" is true by construction rather than by discipline.
//!
//! The font is **vendored** (`assets/fonts`) rather than taken from the system for the same
//! reason: a CI runner and a laptop must produce byte-identical output, which is what makes the
//! golden-image tests meaningful.

use herdr_deck_herdr::wire::AgentStatus;

use crate::theme::{StatusStyle, Theme};

const FONT_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans-Bold.ttf");
const FONT_FAMILY: &str = "DejaVu Sans";

/// Average glyph advance for DejaVu Sans, as a fraction of font size.
///
/// Used to decide where to truncate. Measuring properly would mean shaping the text twice —
/// once to measure and once to draw — and being a few pixels conservative costs nothing on a
/// tile this small.
const AVG_ADVANCE: f32 = 0.60;

/// What a tile should say.
///
/// `Hash` matters: the daemon hashes tiles to send only the keys that actually changed,
/// rather than repainting the whole deck on every reconcile.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Tile {
    /// An agent: the main event.
    Agent {
        label: String,
        sublabel: Option<String>,
        workspace: Option<String>,
        status: AgentStatus,
        focused: bool,
        /// The user dismissed this agent's attention state without focusing it, so the tile is
        /// drawn calm. `status` still says what herdr reports, because that is what an
        /// acknowledgement is *about* and the deck must not start inventing states.
        acknowledged: bool,
    },
    /// A workspace, for the navigation page.
    Workspace {
        label: String,
        status: AgentStatus,
        focused: bool,
    },
    /// "N agents want you" — dark when nothing does.
    Attention { count: usize },
    /// A mode/page toggle.
    Mode { label: String, active: bool },
    /// A slot with nothing bound to it.
    Empty,
    /// herdr is unreachable.
    Offline { message: String },
}

/// Renders tiles to PNG.
pub struct TileRenderer {
    options: usvg::Options<'static>,
    theme: Theme,
}

impl std::fmt::Debug for TileRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileRenderer")
            .field("theme", &self.theme)
            .finish_non_exhaustive()
    }
}

impl TileRenderer {
    pub fn new(theme: Theme) -> Self {
        let mut options = usvg::Options::default();
        {
            let db = options.fontdb_mut();
            db.load_font_data(FONT_REGULAR.to_vec());
            db.load_font_data(FONT_BOLD.to_vec());
            db.set_sans_serif_family(FONT_FAMILY);
        }
        options.font_family = FONT_FAMILY.to_string();
        Self { options, theme }
    }

    pub fn theme(&self) -> Theme {
        self.theme
    }

    /// Render a tile to a PNG at `size` x `size` pixels.
    pub fn render_key(&self, tile: &Tile, size: u32) -> anyhow::Result<Vec<u8>> {
        let svg = self.key_svg(tile, size);
        self.rasterise(&svg, size, size)
    }

    /// Render one touchstrip segment (a wide, short strip above a dial).
    pub fn render_strip(
        &self,
        title: &str,
        value: &str,
        status: Option<AgentStatus>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Vec<u8>> {
        let svg = self.strip_svg(title, value, status, width, height);
        self.rasterise(&svg, width, height)
    }

    /// Rasterise arbitrary SVG at a given size.
    ///
    /// Public so build tooling can generate the plugin's icon set from the same renderer the
    /// deck uses — the icons are then reproducible from source rather than committed binaries
    /// nobody can regenerate.
    pub fn render_svg(&self, svg: &str, width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
        self.rasterise(svg, width, height)
    }

    fn rasterise(&self, svg: &str, width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
        let tree = usvg::Tree::from_str(svg, &self.options)
            .map_err(|e| anyhow::anyhow!("tile SVG failed to parse: {e}"))?;
        let mut pixmap = tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| anyhow::anyhow!("invalid tile size {width}x{height}"))?;
        resvg::render(
            &tree,
            tiny_skia::Transform::identity(),
            &mut pixmap.as_mut(),
        );
        pixmap
            .encode_png()
            .map_err(|e| anyhow::anyhow!("PNG encode failed: {e}"))
    }

    fn key_svg(&self, tile: &Tile, size: u32) -> String {
        let s = size as f32;
        match tile {
            Tile::Agent {
                label,
                sublabel,
                workspace,
                status,
                focused,
                acknowledged,
            } => {
                let style = if *acknowledged {
                    self.theme.acknowledged()
                } else {
                    self.theme.status(*status)
                };
                self.agent_svg(
                    s,
                    label,
                    sublabel.as_deref(),
                    workspace.as_deref(),
                    style,
                    *focused,
                )
            }
            Tile::Workspace {
                label,
                status,
                focused,
            } => {
                let style = self.theme.status(*status);
                self.agent_svg(s, label, None, None, style, *focused)
            }
            Tile::Attention { count } => self.attention_svg(s, *count),
            Tile::Mode { label, active } => self.mode_svg(s, label, *active),
            Tile::Empty => format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}"><rect width="{s}" height="{s}" fill="{bg}"/></svg>"##,
                bg = self.theme.neutral_background()
            ),
            Tile::Offline { message } => {
                let style = self.theme.offline();
                self.offline_svg(s, message, style)
            }
        }
    }

    fn agent_svg(
        &self,
        s: f32,
        label: &str,
        sublabel: Option<&str>,
        workspace: Option<&str>,
        style: StatusStyle,
        focused: bool,
    ) -> String {
        let pad = s * 0.08;
        let label_px = s * 0.17;
        let small_px = s * 0.115;
        let glyph_px = s * 0.20;
        let inner_w = s - pad * 2.0;

        let lines = wrap_text(label, inner_w, label_px, 2);
        let line_height = label_px * 1.12;
        // Centre the label block, nudged up to leave room for the workspace footer.
        let block_top = s * 0.5 - (lines.len() as f32 * line_height) / 2.0 + label_px * 0.35
            - if workspace.is_some() { s * 0.05 } else { 0.0 };

        let mut body = String::new();
        for (i, line) in lines.iter().enumerate() {
            body.push_str(&format!(
                r##"<text x="{x:.2}" y="{y:.2}" font-family="{font}" font-size="{fs:.2}" font-weight="bold" fill="{fg}" text-anchor="middle">{text}</text>"##,
                x = s / 2.0,
                y = block_top + i as f32 * line_height,
                font = FONT_FAMILY,
                fs = label_px,
                fg = style.foreground,
                text = escape(line),
            ));
        }

        if let Some(sub) = sublabel.filter(|s| !s.is_empty()) {
            let sub = truncate_to_width(sub, inner_w, small_px);
            body.push_str(&format!(
                r##"<text x="{x:.2}" y="{y:.2}" font-family="{font}" font-size="{fs:.2}" fill="{fg}" text-anchor="middle" opacity="0.85">{text}</text>"##,
                x = s / 2.0,
                y = block_top + lines.len() as f32 * line_height,
                font = FONT_FAMILY,
                fs = small_px,
                fg = style.foreground,
                text = escape(&sub),
            ));
        }

        if let Some(ws) = workspace.filter(|w| !w.is_empty()) {
            let ws = truncate_to_width(ws, inner_w, small_px);
            body.push_str(&format!(
                r##"<text x="{x:.2}" y="{y:.2}" font-family="{font}" font-size="{fs:.2}" fill="{fg}" text-anchor="middle" opacity="0.72">{text}</text>"##,
                x = s / 2.0,
                y = s - pad * 0.7,
                font = FONT_FAMILY,
                fs = small_px,
                fg = style.foreground,
                text = escape(&ws),
            ));
        }

        // A focus ring, so you can see which agent herdr is currently on.
        let focus_ring = if focused {
            format!(
                r##"<rect x="1.5" y="1.5" width="{w:.2}" height="{w:.2}" rx="{r:.2}" fill="none" stroke="{c}" stroke-width="3"/>"##,
                w = s - 3.0,
                r = s * 0.10,
                c = style.accent,
            )
        } else {
            String::new()
        };

        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}">
<rect width="{s}" height="{s}" fill="{bg}"/>
<rect x="0" y="0" width="{s}" height="{bar:.2}" fill="{accent}"/>
<text x="{gx:.2}" y="{gy:.2}" font-family="{font}" font-size="{gfs:.2}" font-weight="bold" fill="{accent}" text-anchor="end">{glyph}</text>
{body}
{focus_ring}
</svg>"##,
            s = s,
            bg = style.background,
            bar = s * 0.045,
            accent = style.accent,
            gx = s - s * 0.07,
            gy = s * 0.235,
            font = FONT_FAMILY,
            gfs = glyph_px,
            glyph = escape(style.glyph),
            body = body,
            focus_ring = focus_ring,
        )
    }

    fn attention_svg(&self, s: f32, count: usize) -> String {
        let (bg, fg, accent) = if count == 0 {
            (
                self.theme.neutral_background(),
                self.theme.dim_foreground(),
                self.theme.dim_foreground(),
            )
        } else {
            let style = self.theme.status(AgentStatus::Blocked);
            (style.background, style.foreground, style.accent)
        };
        let caption = if count == 0 { "all clear" } else { "need you" };
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}">
<rect width="{s}" height="{s}" fill="{bg}"/>
<text x="{cx:.2}" y="{ny:.2}" font-family="{font}" font-size="{nfs:.2}" font-weight="bold" fill="{fg}" text-anchor="middle">{count}</text>
<text x="{cx:.2}" y="{cy:.2}" font-family="{font}" font-size="{cfs:.2}" fill="{accent}" text-anchor="middle">{caption}</text>
</svg>"##,
            s = s,
            bg = bg,
            cx = s / 2.0,
            ny = s * 0.52,
            font = FONT_FAMILY,
            nfs = s * 0.42,
            fg = fg,
            count = count,
            cy = s * 0.76,
            cfs = s * 0.125,
            accent = accent,
            caption = caption,
        )
    }

    fn mode_svg(&self, s: f32, label: &str, active: bool) -> String {
        let fg = if active {
            self.theme.neutral_foreground()
        } else {
            self.theme.dim_foreground()
        };
        let label_px = s * 0.15;
        let lines = wrap_text(label, s * 0.84, label_px, 2);
        let line_height = label_px * 1.15;
        let top = s * 0.5 - (lines.len() as f32 * line_height) / 2.0 + label_px * 0.35;
        let mut body = String::new();
        for (i, line) in lines.iter().enumerate() {
            body.push_str(&format!(
                r##"<text x="{x:.2}" y="{y:.2}" font-family="{font}" font-size="{fs:.2}" fill="{fg}" text-anchor="middle">{text}</text>"##,
                x = s / 2.0,
                y = top + i as f32 * line_height,
                font = FONT_FAMILY,
                fs = label_px,
                fg = fg,
                text = escape(line),
            ));
        }
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}">
<rect width="{s}" height="{s}" fill="{bg}"/>
{indicator}
{body}
</svg>"##,
            s = s,
            bg = self.theme.neutral_background(),
            indicator = if active {
                format!(
                    r##"<rect x="0" y="{y:.2}" width="{s}" height="{h:.2}" fill="{c}"/>"##,
                    y = s - s * 0.05,
                    s = s,
                    h = s * 0.05,
                    c = self.theme.neutral_foreground()
                )
            } else {
                String::new()
            },
            body = body,
        )
    }

    fn offline_svg(&self, s: f32, message: &str, style: StatusStyle) -> String {
        let label_px = s * 0.125;
        let lines = wrap_text(message, s * 0.86, label_px, 3);
        let line_height = label_px * 1.18;
        let top = s * 0.62 - (lines.len() as f32 * line_height) / 2.0;
        let mut body = String::new();
        for (i, line) in lines.iter().enumerate() {
            body.push_str(&format!(
                r##"<text x="{x:.2}" y="{y:.2}" font-family="{font}" font-size="{fs:.2}" fill="{fg}" text-anchor="middle">{text}</text>"##,
                x = s / 2.0,
                y = top + i as f32 * line_height,
                font = FONT_FAMILY,
                fs = label_px,
                fg = style.foreground,
                text = escape(line),
            ));
        }
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}">
<rect width="{s}" height="{s}" fill="{bg}"/>
<text x="{x:.2}" y="{gy:.2}" font-family="{font}" font-size="{gfs:.2}" fill="{accent}" text-anchor="middle">{glyph}</text>
{body}
</svg>"##,
            s = s,
            bg = style.background,
            x = s / 2.0,
            gy = s * 0.34,
            font = FONT_FAMILY,
            gfs = s * 0.26,
            accent = style.accent,
            glyph = escape(style.glyph),
            body = body,
        )
    }

    fn strip_svg(
        &self,
        title: &str,
        value: &str,
        status: Option<AgentStatus>,
        width: u32,
        height: u32,
    ) -> String {
        let w = width as f32;
        let h = height as f32;
        let style = status.map(|s| self.theme.status(s));
        let bg = style.map_or(self.theme.neutral_background(), |s| s.background);
        let fg = style.map_or(self.theme.neutral_foreground(), |s| s.foreground);
        let accent = style.map_or(self.theme.dim_foreground(), |s| s.accent);
        let title_px = h * 0.20;
        let value_px = h * 0.30;
        let inner = w * 0.90;

        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
<rect width="{w}" height="{h}" fill="{bg}"/>
<text x="{cx:.2}" y="{ty:.2}" font-family="{font}" font-size="{tfs:.2}" fill="{accent}" text-anchor="middle">{title}</text>
<text x="{cx:.2}" y="{vy:.2}" font-family="{font}" font-size="{vfs:.2}" font-weight="bold" fill="{fg}" text-anchor="middle">{value}</text>
</svg>"##,
            w = w,
            h = h,
            bg = bg,
            cx = w / 2.0,
            ty = h * 0.34,
            font = FONT_FAMILY,
            tfs = title_px,
            accent = accent,
            title = escape(&truncate_to_width(title, inner, title_px)),
            vy = h * 0.78,
            vfs = value_px,
            fg = fg,
            value = escape(&truncate_to_width(value, inner, value_px)),
        )
    }
}

/// Roughly how wide `text` will draw at `font_px`.
fn estimated_width(text: &str, font_px: f32) -> f32 {
    text.chars().count() as f32 * font_px * AVG_ADVANCE
}

/// How many characters fit in `max_px`.
fn max_chars(max_px: f32, font_px: f32) -> usize {
    if font_px <= 0.0 {
        return 0;
    }
    ((max_px / (font_px * AVG_ADVANCE)).floor() as isize).max(0) as usize
}

/// Truncate with an ellipsis so it fits on one line.
pub fn truncate_to_width(text: &str, max_px: f32, font_px: f32) -> String {
    let limit = max_chars(max_px, font_px);
    if limit == 0 {
        return String::new();
    }
    if text.chars().count() <= limit {
        return text.to_string();
    }
    if limit == 1 {
        return "…".to_string();
    }
    let kept: String = text.chars().take(limit - 1).collect();
    format!("{}…", kept.trim_end())
}

/// Wrap `text` into at most `max_lines`, truncating the last line if it still overflows.
///
/// Words longer than a line (a long agent id, say) are hard-split rather than allowed to
/// overflow the tile.
pub fn wrap_text(text: &str, max_px: f32, font_px: f32, max_lines: usize) -> Vec<String> {
    let limit = max_chars(max_px, font_px);
    if limit == 0 || max_lines == 0 {
        return vec![];
    }
    let text = text.trim();
    if text.is_empty() {
        return vec![String::new()];
    }
    if estimated_width(text, font_px) <= max_px {
        return vec![text.to_string()];
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if candidate.chars().count() <= limit {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            if lines.len() == max_lines {
                break;
            }
        }
        // A single word too long for a line gets split. Agent names are nearly always
        // kebab- or snake-case, so breaking after a separator ("refactor-" / "auth") reads far
        // better than a blind cut mid-syllable ("refactor-au" / "th").
        let mut rest: String = word.to_string();
        while rest.chars().count() > limit && lines.len() < max_lines {
            let cut = split_point(&rest, limit);
            lines.push(rest.chars().take(cut).collect());
            rest = rest.chars().skip(cut).collect();
        }
        if lines.len() == max_lines {
            current.clear();
            break;
        }
        current = rest;
    }

    if !current.is_empty() && lines.len() < max_lines {
        lines.push(current);
    }

    // Anything that did not fit gets folded into an ellipsis on the final line.
    if lines.len() == max_lines {
        let consumed: usize =
            lines.iter().map(|l| l.chars().count()).sum::<usize>() + lines.len().saturating_sub(1);
        if consumed < text.chars().count() {
            if let Some(last) = lines.last_mut() {
                *last = truncate_to_width(&format!("{last}…"), max_px, font_px);
            }
        }
    }

    lines
}

/// Where to break an over-long word, in characters.
///
/// Prefers just after the last `-` or `_` inside the limit, so `refactor-auth` becomes
/// `refactor-` / `auth`. Falls back to a hard cut when there is no separator, or when the
/// separator is so early that breaking there would waste most of the line.
fn split_point(word: &str, limit: usize) -> usize {
    let chars: Vec<char> = word.chars().collect();
    let window = chars.len().min(limit);
    // Requiring the break past the halfway mark stops `a-verylongword` from becoming a lonely
    // "a-" on its own line.
    let earliest = limit / 2;
    for i in (earliest..window).rev() {
        if chars[i] == '-' || chars[i] == '_' {
            return i + 1;
        }
    }
    limit
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renderer() -> TileRenderer {
        TileRenderer::new(Theme::Dark)
    }

    fn is_png(bytes: &[u8]) -> bool {
        bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
    }

    #[test]
    fn renders_every_tile_variant_to_a_png() {
        let r = renderer();
        let tiles = vec![
            Tile::Agent {
                label: "refactor-auth".into(),
                sublabel: Some("claude".into()),
                workspace: Some("api".into()),
                status: AgentStatus::Blocked,
                focused: true,
                acknowledged: false,
            },
            Tile::Workspace {
                label: "api".into(),
                status: AgentStatus::Working,
                focused: false,
            },
            Tile::Attention { count: 3 },
            Tile::Attention { count: 0 },
            Tile::Mode {
                label: "agents".into(),
                active: true,
            },
            Tile::Empty,
            Tile::Offline {
                message: "herdr offline".into(),
            },
        ];
        for tile in tiles {
            let png = r.render_key(&tile, 120).expect("tile renders");
            assert!(is_png(&png), "{tile:?} did not produce a PNG");
            assert!(png.len() > 100, "{tile:?} produced a suspiciously tiny PNG");
        }
    }

    #[test]
    fn renders_at_every_models_native_key_size() {
        let r = renderer();
        let tile = Tile::Agent {
            label: "api".into(),
            sublabel: None,
            workspace: None,
            status: AgentStatus::Working,
            focused: false,
            acknowledged: false,
        };
        for size in [72, 80, 96, 120] {
            let png = r.render_key(&tile, size).expect("renders");
            assert!(is_png(&png), "failed at {size}px");
        }
    }

    #[test]
    fn renders_a_touchstrip_segment() {
        let r = renderer();
        let png = r
            .render_strip(
                "agent",
                "refactor-auth",
                Some(AgentStatus::Blocked),
                200,
                100,
            )
            .expect("strip renders");
        assert!(is_png(&png));
    }

    #[test]
    fn rendering_is_deterministic_which_is_what_makes_golden_tests_meaningful() {
        let r = renderer();
        let tile = Tile::Agent {
            label: "refactor-auth".into(),
            sublabel: Some("claude".into()),
            workspace: Some("api".into()),
            status: AgentStatus::Blocked,
            focused: false,
            acknowledged: false,
        };
        let a = r.render_key(&tile, 120).unwrap();
        let b = r.render_key(&tile, 120).unwrap();
        assert_eq!(a, b);
        // And across renderer instances — proving we are not picking up ambient system fonts.
        let c = renderer().render_key(&tile, 120).unwrap();
        assert_eq!(a, c);
    }

    #[test]
    fn different_statuses_produce_visibly_different_tiles() {
        let r = renderer();
        let make = |status| Tile::Agent {
            label: "same-label".into(),
            sublabel: None,
            workspace: None,
            status,
            focused: false,
            acknowledged: false,
        };
        let blocked = r.render_key(&make(AgentStatus::Blocked), 120).unwrap();
        let idle = r.render_key(&make(AgentStatus::Idle), 120).unwrap();
        assert_ne!(blocked, idle);
    }

    #[test]
    fn xml_special_characters_in_agent_names_do_not_corrupt_the_svg() {
        // Agent titles come from herdr, which gets them from arbitrary terminal output.
        let r = renderer();
        let tile = Tile::Agent {
            label: "fix <script> & \"quotes\"".into(),
            sublabel: Some("a > b".into()),
            workspace: Some("it's".into()),
            status: AgentStatus::Working,
            focused: false,
            acknowledged: false,
        };
        let png = r
            .render_key(&tile, 120)
            .expect("renders despite metacharacters");
        assert!(is_png(&png));
    }

    #[test]
    fn truncation_adds_an_ellipsis_and_respects_the_width() {
        let out = truncate_to_width("a-very-long-agent-name-indeed", 60.0, 14.0);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= max_chars(60.0, 14.0));
    }

    #[test]
    fn short_text_is_left_alone() {
        assert_eq!(truncate_to_width("api", 200.0, 14.0), "api");
    }

    #[test]
    fn wrapping_splits_on_words_and_respects_the_line_budget() {
        let lines = wrap_text("refactor the auth middleware now", 120.0, 16.0, 2);
        assert!(lines.len() <= 2);
        assert!(!lines.is_empty());
    }

    #[test]
    fn an_unbroken_word_longer_than_a_line_is_hard_split_rather_than_overflowing() {
        let lines = wrap_text("term_abcdefghijklmnopqrstuvwxyz0123456789", 100.0, 16.0, 2);
        assert!(lines.len() <= 2);
        let limit = max_chars(100.0, 16.0);
        for line in &lines {
            assert!(
                line.chars().count() <= limit,
                "line {line:?} exceeds {limit} chars"
            );
        }
    }

    #[test]
    fn a_kebab_case_agent_name_breaks_at_the_hyphen() {
        // Agent names are nearly always kebab- or snake-case; "write-do/cs" reads badly.
        let limit_px = 11.0 * 16.0 * AVG_ADVANCE; // room for 11 characters
        let lines = wrap_text("write-docs", limit_px, 16.0, 2);
        assert_eq!(lines, vec!["write-docs"], "it fits on one line here");

        let narrow = 8.0 * 16.0 * AVG_ADVANCE;
        let lines = wrap_text("write-docs", narrow, 16.0, 2);
        assert_eq!(lines[0], "write-", "should break after the hyphen");
        assert_eq!(lines[1], "docs");
    }

    #[test]
    fn snake_case_breaks_at_the_underscore_too() {
        let narrow = 9.0 * 16.0 * AVG_ADVANCE;
        let lines = wrap_text("migrate_database", narrow, 16.0, 2);
        assert_eq!(lines[0], "migrate_");
    }

    #[test]
    fn a_separator_too_early_in_the_word_is_ignored_to_avoid_wasting_a_line() {
        let narrow = 10.0 * 16.0 * AVG_ADVANCE;
        let lines = wrap_text("a-verylongwordhere", narrow, 16.0, 2);
        assert_ne!(
            lines[0], "a-",
            "breaking there would waste most of the line"
        );
    }

    #[test]
    fn a_word_with_no_separator_still_hard_splits_within_the_limit() {
        let narrow = 8.0 * 16.0 * AVG_ADVANCE;
        let limit = max_chars(narrow, 16.0);
        let lines = wrap_text("abcdefghijklmnopqrstuvwxyz", narrow, 16.0, 2);
        for line in &lines {
            assert!(line.chars().count() <= limit);
        }
    }

    #[test]
    fn empty_text_yields_one_empty_line_rather_than_panicking() {
        assert_eq!(wrap_text("", 100.0, 16.0, 2), vec![String::new()]);
    }

    #[test]
    fn a_zero_width_budget_yields_no_lines_instead_of_dividing_by_zero() {
        assert!(wrap_text("anything", 0.0, 16.0, 2).is_empty());
        assert_eq!(truncate_to_width("anything", 0.0, 16.0), "");
    }

    #[test]
    fn escaping_covers_every_xml_metacharacter() {
        assert_eq!(escape("<&>\"'"), "&lt;&amp;&gt;&quot;&apos;");
    }
}
