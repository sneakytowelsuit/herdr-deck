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

use herdr_deck_herdr::wire::{AgentStatus, PaneDirection};

use crate::state::WaitBucket;
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
        /// How long this agent has been asking for you, when something has been timing it.
        ///
        /// A bucket and not a duration on purpose: this is part of the tile's hash, and a value
        /// that moved every second would defeat the diffing that keeps the deck quiet.
        waiting: Option<WaitBucket>,
    },
    /// A workspace, for the navigation page.
    Workspace {
        label: String,
        status: AgentStatus,
        focused: bool,
    },
    /// A git worktree, for the worktree page.
    ///
    /// No status: a checkout cannot be blocked or done, and giving it one would mean inventing a
    /// state herdr never reported. What it has instead is `open` — whether herdr already holds it
    /// as a workspace — which is the one thing worth knowing before pressing it.
    Worktree {
        label: String,
        open: bool,
        focused: bool,
    },
    /// "N agents want you" — dark when nothing does.
    ///
    /// `away` says the deck is showing something other than the top of the agents page, which is
    /// the only time this key's second job — bringing you back — is worth spelling out. On the
    /// agents page with nothing waiting, "all clear" is the whole truth and an offer to go
    /// somewhere you already are would be noise.
    Attention { count: usize, away: bool },
    /// Which page the deck is showing, and which one the next press goes to.
    ///
    /// `next` is `None` when there is nowhere else to go — a session with one workspace, no
    /// worktrees and every command page already on keys of its own. The key then draws dim and
    /// says only where you are, rather than promising a journey it cannot make.
    Page {
        current: String,
        next: Option<String>,
        /// Which screen of a page too long for the keys, when there is more than one.
        screen: Option<(usize, usize)>,
    },
    /// A key with a word on it and nothing else: paging, scrubbing, and the commands whose target
    /// no shape could name.
    Label { label: String, active: bool },
    /// A key bound to one herdr command: a shape, and a word under it.
    Command {
        /// What the key does, said as a shape. This is the part meant to be read.
        glyph: KeyGlyph,
        /// The caption under the shape, for the times the shape is not quite enough.
        label: String,
        /// This command only fires on a hold, and the tile has to say so before anyone presses it
        /// — a guard nobody can see is a key that appears broken the first time it is tapped.
        hold: bool,
        /// Whether the key can act right now. A command with nothing to act on draws dimmed
        /// rather than disappearing, so the layout does not rearrange itself under the user.
        enabled: bool,
    },
    /// A slot with nothing bound to it.
    Empty,
    /// herdr is unreachable.
    Offline { message: String },
}

/// The shape a command key wears.
///
/// Drawn as geometry rather than typeset as a character, for two reasons. The vendored font is
/// only guaranteed to carry the glyphs already in use, and adding a key would otherwise mean
/// hoping DejaVu has a square-with-its-bottom-half-filled. And a shape built from rectangles is
/// still crisp at 72px, where a typeset arrow at the same size is a smudge.
///
/// Every one of these has a distinct silhouette, not merely a distinct fill: these keys are meant
/// to be understood at a glance and from an angle, and a direction key you had to read the caption
/// of would be a slow key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyGlyph {
    /// A solid triangle pointing that way.
    Arrow(PaneDirection),
    /// A tab with a new pane appearing beside or below the one you are in.
    SplitRight,
    SplitDown,
    /// One pane swallowing its tab, and the panes back in their tiles.
    ZoomIn,
    ZoomOut,
    /// A pane with a line drawn through it.
    Close,
    /// A tab, drawn as a pane with a strip along its top — and the same shape crossed out.
    NewTab,
    CloseTab,
    /// Two panes offset behind one another, which is what a workspace looks like from outside it.
    NewWorkspace,
    /// A pane divided the way a saved layout divides one.
    Layout,
    /// A fork: a trunk with a branch leaving it, and the same shape crossed out.
    NewWorktree,
    RemoveWorktree,
}

/// Everything the shared agent/workspace layout draws.
///
/// Gathered into one piece because the two tiles that share that layout were already handing it
/// six positional arguments, and a seventh would have made the call sites unreadable in exactly
/// the place where mixing two of them up is silent.
struct AgentFace<'a> {
    label: &'a str,
    sublabel: Option<&'a str>,
    workspace: Option<&'a str>,
    style: StatusStyle,
    focused: bool,
    waiting: Option<WaitBucket>,
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
                waiting,
            } => {
                let style = if *acknowledged {
                    self.theme.acknowledged()
                } else {
                    self.theme.status(*status)
                };
                self.agent_svg(
                    s,
                    AgentFace {
                        label,
                        sublabel: sublabel.as_deref(),
                        workspace: workspace.as_deref(),
                        style,
                        focused: *focused,
                        waiting: *waiting,
                    },
                )
            }
            Tile::Workspace {
                label,
                status,
                focused,
            } => {
                let style = self.theme.status(*status);
                self.agent_svg(
                    s,
                    AgentFace {
                        label,
                        sublabel: None,
                        workspace: None,
                        style,
                        focused: *focused,
                        // A workspace is a rollup of many agents, so there is no single wait to
                        // report and inventing one would be worse than silence.
                        waiting: None,
                    },
                )
            }
            Tile::Worktree {
                label,
                open,
                focused,
            } => self.agent_svg(
                s,
                AgentFace {
                    label,
                    // The caption says what pressing it does, which for a checkout that is already
                    // open is "take me there" rather than "open it" — the same key either way,
                    // because `worktree.open` is idempotent, but not the same sentence.
                    sublabel: Some(if *open { "open" } else { "not open" }),
                    workspace: None,
                    style: self.theme.worktree(*open),
                    focused: *focused,
                    waiting: None,
                },
            ),
            Tile::Attention { count, away } => self.attention_svg(s, *count, *away),
            Tile::Page {
                current,
                next,
                screen,
            } => self.page_svg(s, current, next.as_deref(), *screen),
            Tile::Label { label, active } => self.label_svg(s, label, *active),
            Tile::Command {
                glyph,
                label,
                hold,
                enabled,
            } => self.command_svg(s, *glyph, label, *hold, *enabled),
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

    fn agent_svg(&self, s: f32, face: AgentFace<'_>) -> String {
        let AgentFace {
            label,
            sublabel,
            workspace,
            style,
            focused,
            waiting,
        } = face;
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

        // The escalation. Two quiet channels rather than one loud one: a marker saying how long,
        // and a top bar that thickens with the wait so a tile reads as more urgent from across
        // the desk without being read at all. Neither channel is colour — the tile's colour
        // already means the status, and giving it a second meaning would make both less
        // trustworthy.
        //
        // The marker sits in the band between the bar and the top of the label, mirroring the
        // status glyph across the tile. That band is only about a tenth of the key high, which
        // is what sets every constant below: this is the largest text that clears the thickest
        // bar above it and a two-line label beneath it, and the buckets are three characters
        // wide precisely because that is what fits there on a 72px key. Move any of it and look
        // at the golden fixtures — the collision it causes is a two-pixel one.
        let marker_px = s * 0.10;
        let marker = waiting.map_or(String::new(), |bucket| {
            format!(
                r##"<text x="{x:.2}" y="{y:.2}" font-family="{font}" font-size="{fs:.2}" font-weight="{weight}" fill="{c}" text-anchor="start" opacity="{opacity}">{text}</text>"##,
                x = pad * 0.75,
                y = s * 0.175,
                font = FONT_FAMILY,
                fs = marker_px,
                weight = if bucket == WaitBucket::Overdue {
                    "bold"
                } else {
                    "normal"
                },
                c = style.accent,
                // The quietest rung sits at the workspace footer's opacity, which is this tile's
                // "incidental information" tier and the faintest thing still legible on a 72px
                // key. Anything dimmer stops being readable and starts being visual noise.
                opacity = match bucket.level() {
                    0 => "0.72",
                    1 => "0.86",
                    _ => "1",
                },
                text = escape(bucket.marker()),
            )
        });
        let bar = s * 0.045 * (1.0 + waiting.map_or(0.0, |b| b.level() as f32) / 3.0);

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
{marker}
{body}
{focus_ring}
</svg>"##,
            s = s,
            bg = style.background,
            bar = bar,
            accent = style.accent,
            gx = s - s * 0.07,
            gy = s * 0.235,
            font = FONT_FAMILY,
            gfs = glyph_px,
            glyph = escape(style.glyph),
            marker = marker,
            body = body,
            focus_ring = focus_ring,
        )
    }

    fn attention_svg(&self, s: f32, count: usize, away: bool) -> String {
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
        // Three captions for one key, because it has one meaning with two halves and which half
        // matters depends on where you are. Something waiting always wins: that is the sentence
        // this deck exists to say.
        let caption = match (count, away) {
            (0, false) => "all clear",
            (0, true) => "◀ agents",
            _ => "need you",
        };
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

    /// Where you are, in bold; where the next press goes, underneath and quieter.
    ///
    /// Two sizes rather than two keys. The name of the current page is the larger because it is
    /// the one glanced at — "which page am I on" is asked constantly and answered in a fraction of
    /// a second, while "where does this go" is asked once a week. The screen counter sits in the
    /// same corner and the same size as an agent tile's wait marker and a command tile's hold
    /// marker: one place the eye goes for "there is more to this key than its face".
    fn page_svg(
        &self,
        s: f32,
        current: &str,
        next: Option<&str>,
        screen: Option<(usize, usize)>,
    ) -> String {
        let inner = s * 0.86;
        let current_px = s * 0.19;
        let next_px = s * 0.115;
        let marker = match screen {
            Some((at, of)) => format!(
                r##"<text x="{x:.2}" y="{y:.2}" font-family="{font}" font-size="{fs:.2}" fill="{c}" text-anchor="start" opacity="0.85">{at}/{of}</text>"##,
                x = s * 0.06,
                y = s * 0.175,
                font = FONT_FAMILY,
                fs = s * 0.10,
                c = self.theme.dim_foreground(),
            ),
            None => String::new(),
        };
        let onward = match next {
            Some(next) => format!(
                r##"<text x="{x:.2}" y="{y:.2}" font-family="{font}" font-size="{fs:.2}" fill="{c}" text-anchor="middle">▶ {text}</text>"##,
                x = s / 2.0,
                y = s * 0.76,
                font = FONT_FAMILY,
                fs = next_px,
                c = self.theme.dim_foreground(),
                text = escape(&truncate_to_width(next, inner * 0.8, next_px)),
            ),
            None => String::new(),
        };
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}">
<rect width="{s}" height="{s}" fill="{bg}"/>
{marker}
<text x="{cx:.2}" y="{cy:.2}" font-family="{font}" font-size="{fs:.2}" font-weight="bold" fill="{fg}" text-anchor="middle">{current}</text>
{onward}
</svg>"##,
            s = s,
            bg = self.theme.neutral_background(),
            marker = marker,
            cx = s / 2.0,
            cy = s * 0.55,
            font = FONT_FAMILY,
            fs = current_px,
            fg = self.theme.neutral_foreground(),
            current = escape(&truncate_to_width(current, inner, current_px)),
            onward = onward,
        )
    }

    fn label_svg(&self, s: f32, label: &str, active: bool) -> String {
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

    /// A shape, a caption, and — when the command is guarded — the word that unlocks it.
    ///
    /// The shape gets the top two-thirds of the key and the caption a single line at the bottom,
    /// which is the opposite weighting to an agent tile. That is deliberate: an agent tile is read
    /// for *which* agent, so its label dominates, while a pane key is always the same key and is
    /// found by feel and by silhouette. The caption is there for the first week, not the second.
    fn command_svg(
        &self,
        s: f32,
        glyph: KeyGlyph,
        label: &str,
        hold: bool,
        enabled: bool,
    ) -> String {
        let (fg, accent) = if enabled {
            (
                self.theme.neutral_foreground(),
                self.theme.neutral_foreground(),
            )
        } else {
            (self.theme.dim_foreground(), self.theme.dim_foreground())
        };
        let pad = s * 0.08;
        let label_px = s * 0.135;
        let shape = glyph_svg(glyph, s, fg);

        // The hold marker sits where an agent tile's wait marker sits, in the same size and the
        // same corner. There is one confirmation idiom on this hardware; there should be one place
        // the eye goes to find out whether a key has one.
        let marker = if hold {
            format!(
                r##"<text x="{x:.2}" y="{y:.2}" font-family="{font}" font-size="{fs:.2}" font-weight="bold" fill="{c}" text-anchor="start">hold</text>"##,
                x = pad * 0.75,
                y = s * 0.175,
                font = FONT_FAMILY,
                fs = s * 0.10,
                c = accent,
            )
        } else {
            String::new()
        };

        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}">
<rect width="{s}" height="{s}" fill="{bg}"/>
{marker}
{shape}
<text x="{cx:.2}" y="{ly:.2}" font-family="{font}" font-size="{fs:.2}" fill="{fg}" text-anchor="middle">{label}</text>
</svg>"##,
            s = s,
            bg = self.theme.neutral_background(),
            marker = marker,
            shape = shape,
            cx = s / 2.0,
            ly = s - pad * 0.9,
            font = FONT_FAMILY,
            fs = label_px,
            fg = fg,
            label = escape(&truncate_to_width(label, s - pad * 2.0, label_px)),
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

/// Draw one [`KeyGlyph`] into the upper part of a key of side `s`.
///
/// Everything is expressed as a fraction of `s` so the same shape holds together at 72px and at
/// 120px. The box is square and centred horizontally, sitting above the caption line: 34% of the
/// key is as large as the shape can be without crowding a caption underneath it, and as small as
/// it can be while a triangle still reads as a triangle on the smallest deck we support.
fn glyph_svg(glyph: KeyGlyph, s: f32, colour: &str) -> String {
    let d = s * 0.34;
    let x0 = (s - d) / 2.0;
    let y0 = s * 0.20;
    let stroke = (s * 0.035).max(1.5);

    // A pane, as this deck draws one: an outline the fills and cuts below sit inside.
    let outline = format!(
        r##"<rect x="{x0:.2}" y="{y0:.2}" width="{d:.2}" height="{d:.2}" rx="{r:.2}" fill="none" stroke="{colour}" stroke-width="{stroke:.2}"/>"##,
        r = d * 0.10,
    );
    let triangle = |points: String| format!(r##"<polygon points="{points}" fill="{colour}"/>"##);
    let fill = |x: f32, y: f32, w: f32, h: f32| {
        format!(r##"<rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" fill="{colour}"/>"##)
    };
    let line = |x1: f32, y1: f32, x2: f32, y2: f32| {
        format!(
            r##"<line x1="{x1:.2}" y1="{y1:.2}" x2="{x2:.2}" y2="{y2:.2}" stroke="{colour}" stroke-width="{stroke:.2}" stroke-linecap="round"/>"##
        )
    };
    // A cross inside a `d`-sided box: what "this one goes away" looks like on every key that says
    // it. One shape for one meaning, so a crossed-out anything reads without being learned.
    let cross = |x: f32, y: f32, d: f32| {
        format!(
            "{}{}",
            line(x + d * 0.26, y + d * 0.26, x + d * 0.74, y + d * 0.74),
            line(x + d * 0.74, y + d * 0.26, x + d * 0.26, y + d * 0.74)
        )
    };
    // ...and a plus for "one more of these", by the same rule.
    let plus = |cx: f32, cy: f32, size: f32| {
        let h = size / 2.0;
        format!(
            "{}{}",
            line(cx - h, cy, cx + h, cy),
            line(cx, cy - h, cx, cy + h)
        )
    };
    // A trunk with one branch leaving it — git's own picture of a worktree, and the only shape
    // here that is not a rectangle, which is what keeps it findable by feel in a row of panes.
    let fork = || {
        let trunk = x0 + d * 0.26;
        let branch = x0 + d * 0.74;
        let dot = |cx: f32, cy: f32| {
            format!(
                r##"<circle cx="{cx:.2}" cy="{cy:.2}" r="{r:.2}" fill="{colour}"/>"##,
                r = d * 0.13
            )
        };
        format!(
            "{}{}{}{}",
            line(trunk, y0 + d * 0.10, trunk, y0 + d * 0.90),
            line(trunk, y0 + d * 0.34, branch, y0 + d * 0.34),
            dot(trunk, y0 + d * 0.10),
            dot(branch, y0 + d * 0.34),
        )
    };

    match glyph {
        KeyGlyph::Arrow(direction) => triangle(match direction {
            PaneDirection::Left => format!(
                "{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
                x0 + d,
                y0,
                x0 + d,
                y0 + d,
                x0,
                y0 + d / 2.0
            ),
            PaneDirection::Right => format!(
                "{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
                x0,
                y0,
                x0,
                y0 + d,
                x0 + d,
                y0 + d / 2.0
            ),
            PaneDirection::Up => format!(
                "{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
                x0,
                y0 + d,
                x0 + d,
                y0 + d,
                x0 + d / 2.0,
                y0
            ),
            PaneDirection::Down => format!(
                "{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
                x0,
                y0,
                x0 + d,
                y0,
                x0 + d / 2.0,
                y0 + d
            ),
        }),
        // The new pane is the filled half, so the shape says which side of the divider you end up
        // on as well as where the divider goes. The fill is inset by the stroke on its outer
        // edges and flush on the divider, which keeps the pane's rounded outline intact — a fill
        // running past it squares the corners off and the whole thing stops reading as a pane.
        KeyGlyph::SplitRight => format!(
            "{}{}",
            outline,
            fill(
                x0 + d / 2.0,
                y0 + stroke,
                d / 2.0 - stroke,
                d - stroke * 2.0
            )
        ),
        KeyGlyph::SplitDown => format!(
            "{}{}",
            outline,
            fill(
                x0 + stroke,
                y0 + d / 2.0,
                d - stroke * 2.0,
                d / 2.0 - stroke
            )
        ),
        // One pane swallowing the tab...
        KeyGlyph::ZoomIn => format!(
            "{}{}",
            outline,
            fill(x0 + d * 0.22, y0 + d * 0.22, d * 0.56, d * 0.56)
        ),
        // ...and the tab handed back to all of them.
        KeyGlyph::ZoomOut => format!(
            "{}{}{}",
            outline,
            line(x0 + d / 2.0, y0, x0 + d / 2.0, y0 + d),
            line(x0, y0 + d / 2.0, x0 + d, y0 + d / 2.0)
        ),
        KeyGlyph::Close => format!("{}{}", outline, cross(x0, y0, d)),

        // A tab is a pane wearing a strip: the same body as every pane shape above, with the
        // header the tab bar draws. That makes "tab" and "pane" tell each other apart at a glance
        // while keeping the family resemblance that says they are the same kind of thing.
        KeyGlyph::NewTab => format!(
            "{}{}{}",
            outline,
            fill(x0 + stroke, y0 + stroke, d * 0.42, d * 0.16),
            plus(x0 + d / 2.0, y0 + d * 0.60, d * 0.34)
        ),
        KeyGlyph::CloseTab => format!(
            "{}{}{}",
            outline,
            fill(x0 + stroke, y0 + stroke, d * 0.42, d * 0.16),
            cross(x0 + d * 0.08, y0 + d * 0.18, d * 0.84)
        ),
        // A workspace holds tabs the way a stack of paper holds sheets: what you see from outside
        // is the one on top and the corner of the one behind it.
        KeyGlyph::NewWorkspace => format!(
            r##"<rect x="{bx:.2}" y="{by:.2}" width="{bd:.2}" height="{bd:.2}" rx="{r:.2}" fill="none" stroke="{colour}" stroke-width="{stroke:.2}" opacity="0.55"/>{front}{plus}"##,
            bx = x0 + d * 0.18,
            by = y0,
            bd = d * 0.82,
            r = d * 0.10,
            front = format_args!(
                r##"<rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{w:.2}" rx="{r:.2}" fill="{bgish}" stroke="{colour}" stroke-width="{stroke:.2}"/>"##,
                x = x0,
                y = y0 + d * 0.18,
                w = d * 0.82,
                r = d * 0.10,
                // Painted, not transparent, so the sheet behind it genuinely goes behind rather
                // than showing through and turning two squares into a lattice.
                bgish = "#0E0F12",
            ),
            plus = plus(x0 + d * 0.41, y0 + d * 0.59, d * 0.36),
        ),
        // A saved layout is not one pane and not four equal ones — it is a deliberate, uneven
        // arrangement, and the shape says so by being uneven.
        KeyGlyph::Layout => format!(
            "{}{}{}",
            outline,
            line(x0 + d * 0.42, y0, x0 + d * 0.42, y0 + d),
            line(x0 + d * 0.42, y0 + d * 0.55, x0 + d, y0 + d * 0.55)
        ),
        // A worktree is made, or given back: the fork with a plus on the branch, or with the
        // branch struck out.
        KeyGlyph::NewWorktree => {
            format!("{}{}", fork(), plus(x0 + d * 0.74, y0 + d * 0.78, d * 0.36))
        }
        KeyGlyph::RemoveWorktree => {
            format!(
                "{}{}",
                fork(),
                cross(x0 + d * 0.56, y0 + d * 0.60, d * 0.36)
            )
        }
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
                waiting: None,
            },
            Tile::Workspace {
                label: "api".into(),
                status: AgentStatus::Working,
                focused: false,
            },
            Tile::Attention {
                count: 3,
                away: false,
            },
            Tile::Attention {
                count: 0,
                away: false,
            },
            Tile::Label {
                label: "agents".into(),
                active: true,
            },
            Tile::Command {
                glyph: KeyGlyph::Arrow(PaneDirection::Down),
                label: "down".into(),
                hold: false,
                enabled: true,
            },
            Tile::Command {
                glyph: KeyGlyph::Close,
                label: "close".into(),
                hold: true,
                enabled: false,
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
            waiting: None,
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
            waiting: None,
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
            waiting: None,
        };
        let blocked = r.render_key(&make(AgentStatus::Blocked), 120).unwrap();
        let idle = r.render_key(&make(AgentStatus::Idle), 120).unwrap();
        assert_ne!(blocked, idle);
    }

    #[test]
    fn each_wait_bucket_draws_a_visibly_different_tile() {
        // Status is never colour alone (see `theme.rs`), and neither is a wait: each rung has to
        // differ in something a photocopier would keep. Byte inequality is a weak check, so the
        // golden fixtures carry the real judgement — this just stops a bucket being accepted
        // and then silently dropped on the floor.
        let r = renderer();
        let make = |waiting| Tile::Agent {
            label: "refactor-auth".into(),
            sublabel: Some("claude".into()),
            workspace: Some("api".into()),
            status: AgentStatus::Blocked,
            focused: false,
            acknowledged: false,
            waiting,
        };
        let rendered: Vec<_> = [
            None,
            Some(WaitBucket::Fresh),
            Some(WaitBucket::Waiting),
            Some(WaitBucket::Overdue),
        ]
        .into_iter()
        .map(|bucket| r.render_key(&make(bucket), 120).unwrap())
        .collect();
        for (i, a) in rendered.iter().enumerate() {
            for b in rendered.iter().skip(i + 1) {
                assert_ne!(a, b, "two wait treatments render identically");
            }
        }
    }

    #[test]
    fn the_status_bar_thickens_with_every_rung_of_the_wait() {
        // The half of the escalation that works without being read. It has to be monotonic:
        // a treatment that grew and then shrank would say the agent had calmed down.
        let r = renderer();
        let bar_height = |waiting| {
            let svg = r.key_svg(
                &Tile::Agent {
                    label: "refactor-auth".into(),
                    sublabel: None,
                    workspace: None,
                    status: AgentStatus::Blocked,
                    focused: false,
                    acknowledged: false,
                    waiting,
                },
                120,
            );
            // The second rect is the accent bar; the first is the tile background.
            let rect = svg.match_indices("<rect").nth(1).expect("tile has a bar").0;
            let height = svg[rect..].split(r#"height=""#).nth(1).unwrap();
            height[..height.find('"').unwrap()].parse::<f32>().unwrap()
        };
        let none = bar_height(None);
        let fresh = bar_height(Some(WaitBucket::Fresh));
        let waiting = bar_height(Some(WaitBucket::Waiting));
        let overdue = bar_height(Some(WaitBucket::Overdue));
        assert_eq!(
            none, fresh,
            "a wait nobody has been timing must look exactly like one that just started"
        );
        assert!(
            fresh < waiting && waiting < overdue,
            "bar heights must escalate, got {fresh} / {waiting} / {overdue}"
        );
        assert!(
            overdue < 120.0 * 0.1,
            "the bar has grown into a band and is eating the tile"
        );
    }

    #[test]
    fn every_command_shape_draws_something_no_other_one_draws() {
        // These keys sit in a block and are pressed by feel, so a shape that came out identical to
        // its neighbour would make two keys indistinguishable without reading them — which is the
        // one thing they exist to avoid. Byte inequality is a weak check; the golden fixtures carry
        // the real judgement about whether each is legible. This just stops one being added and
        // silently drawing nothing.
        let r = renderer();
        let glyphs = [
            KeyGlyph::Arrow(PaneDirection::Left),
            KeyGlyph::Arrow(PaneDirection::Right),
            KeyGlyph::Arrow(PaneDirection::Up),
            KeyGlyph::Arrow(PaneDirection::Down),
            KeyGlyph::SplitRight,
            KeyGlyph::SplitDown,
            KeyGlyph::ZoomIn,
            KeyGlyph::ZoomOut,
            KeyGlyph::Close,
        ];
        // Rendered with the same caption throughout, so any difference is the shape and not the
        // word underneath it — and at 72px, where a shape one notch too large turns into a smudge.
        let rendered: Vec<_> = glyphs
            .iter()
            .map(|glyph| {
                r.render_key(
                    &Tile::Command {
                        glyph: *glyph,
                        label: "key".into(),
                        hold: false,
                        enabled: true,
                    },
                    72,
                )
                .unwrap()
            })
            .collect();
        for (i, a) in rendered.iter().enumerate() {
            for (j, b) in rendered.iter().enumerate().skip(i + 1) {
                assert_ne!(a, b, "{:?} and {:?} draw identically", glyphs[i], glyphs[j]);
            }
        }
    }

    #[test]
    fn a_guarded_key_and_an_unusable_one_each_look_different_from_a_plain_one() {
        // Both are things the key has to say before it is pressed: one that it needs a hold, one
        // that it cannot act at all. A treatment that rendered the same as an ordinary key would
        // be a guard nobody can see and a dead key that looks alive.
        let r = renderer();
        let tile = |hold, enabled| Tile::Command {
            glyph: KeyGlyph::Close,
            label: "close".into(),
            hold,
            enabled,
        };
        let plain = r.render_key(&tile(false, true), 120).unwrap();
        let guarded = r.render_key(&tile(true, true), 120).unwrap();
        let unusable = r.render_key(&tile(true, false), 120).unwrap();
        assert_ne!(plain, guarded);
        assert_ne!(guarded, unusable);
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
            waiting: None,
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
