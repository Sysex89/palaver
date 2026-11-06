//! Design tokens: every colour and text style the editor draws with, named for what it
//! *means* rather than what it looks like. A call site says `theme::DANGER`, never a hex
//! value, so one meaning cannot quietly drift into two shades, and one shade cannot
//! quietly carry two meanings — which is how amber once meant an ending, a condition, a
//! note and a number all at once.
//!
//! Three rules hold the set together:
//!
//! - **Surfaces are a ladder, back to front**: `CANVAS` → `PANE` → `RAISED` → `HOVER` →
//!   `PRESSED`. Something that sits on a surface takes the next rung up for its hover, which
//!   is why a menu item (on `RAISED`) hovers to `PRESSED` rather than `HOVER`.
//! - **Text has three tiers and they are measured, not eyeballed.** On `PANE`: `TEXT`
//!   11.7:1, `TEXT_MUTED` ~7:1, `TEXT_FAINT` 4.7:1 — all three pass WCAG AA for body text.
//!   Only `TEXT_DISABLED` is below it, and it is only for things that are disabled, which
//!   WCAG exempts. The tiers alone are too close to carry hierarchy; size and weight (see
//!   [`Typography`]) do the rest.
//! - **Each hue means one thing.** Blue is interaction, green is "true / happening now",
//!   red is "false / broken", amber is a warning, and each node kind that draws a glyph has
//!   its own. The aliases (`TRUE` = `SUCCESS`, `PLAYING` = `SUCCESS`) are deliberate: same
//!   hue, same idea, but the call site names the reason.
//!
//! Not here: `CAST_COLORS` in `main.rs`. Those values are written into `.dln` files as
//! character colours, so they are document data, not theme — changing one would orphan
//! every saved character that uses it.

use gpui::{FontWeight, Rgba, Styled, px};

/// `rgb()` is not a `const fn` in gpui 0.2.2, and tokens want to be constants.
const fn hex(c: u32) -> Rgba {
    Rgba {
        r: ((c >> 16) & 0xff) as f32 / 255.0,
        g: ((c >> 8) & 0xff) as f32 / 255.0,
        b: (c & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

/// `hex` with an alpha byte on the end, as gpui's `rgba()` reads it.
const fn hexa(c: u32) -> Rgba {
    let mut color = hex(c >> 8);
    color.a = (c & 0xff) as f32 / 255.0;
    color
}

// ── Surfaces, back to front ─────────────────────────────────────────────────────────────

/// The graph's background, and anything sunk into a pane: text fields, edge-label backs.
pub const CANVAS: Rgba = hex(0x1e1e22);
/// The menu bar and the panes either side of the canvas.
pub const PANE: Rgba = hex(0x26262c);
/// Things that sit on a surface: nodes, chips, buttons, the open menu.
pub const RAISED: Rgba = hex(0x2b2b33);
/// Hover over a pane.
pub const HOVER: Rgba = hex(0x33333c);
/// Hover over something raised, an open menu title, the chip that is on.
pub const PRESSED: Rgba = hex(0x3a3a44);
/// What a modal overlay dims the window with. The only translucent surface: everything
/// else is opaque, so a rung on the ladder always reads as the same colour.
pub const SCRIM: Rgba = hexa(0x0000008c);

// ── Lines ───────────────────────────────────────────────────────────────────────────────

/// Between panes.
pub const DIVIDER: Rgba = hex(0x3a3a44);
/// Around raised things: nodes, menus, text fields.
pub const OUTLINE: Rgba = hex(0x4a4a57);
/// Edges, arrowheads and port rings.
pub const EDGE: Rgba = hex(0x7d7d8c);

// ── Text ────────────────────────────────────────────────────────────────────────────────

/// Content: dialogue, names, values — what the author wrote.
pub const TEXT: Rgba = hex(0xe2e2e8);
/// Labels and secondary text — what the editor says *about* the content.
pub const TEXT_MUTED: Rgba = hex(0xb0b0bc);
/// Hints, counts, idle icon buttons, placeholders. The floor for anything readable.
pub const TEXT_FAINT: Rgba = hex(0x8f8f9b);
/// Disabled only — a spent choice. Below AA on purpose, and only where WCAG allows it.
pub const TEXT_DISABLED: Rgba = hex(0x6c6c78);

// ── Interaction ─────────────────────────────────────────────────────────────────────────

/// Selection, focus, the edge being dragged, things you can click to edit.
pub const ACCENT: Rgba = hex(0x7aa2f7);
/// The marquee's fill.
pub const ACCENT_WASH: Rgba = hexa(0x7aa2f722);
/// Selected text in a field.
pub const ACCENT_SELECTION: Rgba = hexa(0x7aa2f755);

// ── Status ──────────────────────────────────────────────────────────────────────────────

pub const SUCCESS: Rgba = hex(0x8ce99a);
pub const WARNING: Rgba = hex(0xf7c96a);
pub const DANGER: Rgba = hex(0xff7a7a);

/// Where a play-through is standing.
pub const PLAYING: Rgba = SUCCESS;
/// A node or edge the play-through has already passed. Fainter than [`PLAYING`], so the
/// current node is still the one bright thing.
pub const TRAIL: Rgba = hexa(0x8ce99a99);
/// A once-only choice the play-through has used up.
pub const SPENT: Rgba = OUTLINE;
/// A condition's true pin, and a flag set or checked true.
pub const TRUE: Rgba = SUCCESS;
/// A condition's false pin, and a flag set or checked false.
pub const FALSE: Rgba = DANGER;

// ── Node kinds ──────────────────────────────────────────────────────────────────────────

/// A line of dialogue. Only the chip and the inspector header use it: on the canvas a
/// speech node is tinted by its speaker, which is the whole point of declaring a cast.
pub const KIND_SPEECH: Rgba = hex(SPEAKER_DEFAULT);
/// A hub's glyph and label. Neutral: a hub is plumbing with a name, not a statement.
pub const KIND_HUB: Rgba = TEXT_MUTED;
pub const KIND_JUMP: Rgba = hex(0xc3a6f7);
pub const KIND_INSTRUCTION: Rgba = hex(0x6fd6c3);
pub const KIND_CONDITION: Rgba = hex(0xf29fc5);
/// Desaturated on purpose: a note is for the author, and must never read as content.
pub const KIND_NOTE: Rgba = hex(0xc8b88a);
/// A node marked as a deliberate ending. Peach rather than amber, so it never reads as
/// the `DeadEnd` warning it exists to silence.
pub const ENDING: Rgba = hex(0xf5a97f);

/// The tint for a speaker nobody declared. A `u32`, not an `Rgba`, because it stands in
/// for a `Character::color`, which is stored as one.
pub const SPEAKER_DEFAULT: u32 = 0x9ecbff;

// ── Type ────────────────────────────────────────────────────────────────────────────────

/// Set once at the root and inherited. Named rather than left to `.SystemUIFont`, which
/// on Linux asks for IBM Plex Sans and falls back to whatever fontconfig has — so a weight
/// could render as a different face on every machine.
// ponytail: not bundled. Where Inter is missing gpui falls back to the system face, which
// is readable but loses the weights. Embed it with `text_system().add_fonts` if that bites.
pub const UI_FONT: &str = "Inter";
/// A note's face: the bundled UI font has no italic, and a note must not read as speech.
pub const NOTE_FONT: &str = "Noto Serif";

/// Section labels, which are deliberately the smallest thing on screen: they name a group,
/// they are not content, and weight plus capitals carry them at this size.
pub const TINY: f32 = 11.0;
/// Inspector headings: what is selected, what is playing.
pub const TITLE: f32 = 15.0;
/// The chrome's default size.
pub const BODY: f32 = 13.0;
/// Field labels, section labels, hints and shortcuts.
pub const SMALL: f32 = 12.0;
/// Dialogue on the canvas at 1:1. Bigger than the chrome on purpose: what the author
/// wrote outranks the editor's own UI.
pub const CANVAS_TEXT: f32 = 14.0;

/// Text roles, as methods on anything styled, so a call site names the role and the role
/// owns size, weight and colour together. Three roles is the whole scale; anything else is
/// `BODY` in one of the text colours.
pub trait Typography: Styled + Sized {
    /// What the inspector is showing. The one heavy, large thing in a pane.
    fn type_title(self) -> Self {
        self.text_size(px(TITLE))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(TEXT)
    }

    /// Names a field or a section. Weight, not colour, is what lifts it off its content.
    fn type_label(self) -> Self {
        self.text_size(px(SMALL))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(TEXT_MUTED)
    }

    /// Guidance and empty states: readable, never competing.
    fn type_hint(self) -> Self {
        self.text_size(px(SMALL)).text_color(TEXT_FAINT)
    }

    /// Names a *group* of fields — the one level above [`Typography::type_label`]. Written
    /// in capitals by its caller; gpui 0.2.2 has no letter-spacing, so the capitals are
    /// what keep it from reading as just a smaller label.
    fn type_section(self) -> Self {
        self.text_size(px(TINY))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(TEXT_FAINT)
    }

    /// One line that ends in `…` when it does not fit. Not `truncate()`: that sets
    /// no-wrap, and gpui only truncates text it is first measured at a definite width — a
    /// flex item is first probed at its full width, the answer is cached, and the later,
    /// correctly sized pass returns early without ever truncating. The text is then merely
    /// clipped by the box, mid-letter, with no ellipsis. A one-line *clamp* keeps wrapping
    /// on, so it is re-measured whenever the width changes.
    fn one_line(self) -> Self {
        self.text_ellipsis().line_clamp(1)
    }
}

impl<T: Styled> Typography for T {}
