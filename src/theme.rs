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
//! - **Text has three tiers and they are measured, not eyeballed** — by a test, on every
//!   surface a tier is drawn on (`PANE`, `RAISED`, `CANVAS`), in both palettes, so none of
//!   the three can fall under WCAG AA's 4.5:1 for body text. (`TEXT_FAINT` used to be
//!   measured on `PANE` alone and read 4.39:1 on a node, where counts and hints sit.)
//!   Only `TEXT_DISABLED` is below it, and it is only for things that are disabled, which
//!   WCAG exempts. The tiers alone are too close to carry hierarchy; size and weight (see
//!   [`Typography`]) do the rest.
//! - **Each hue means one thing.** Blue is interaction, green is "true / happening now",
//!   red is "false / broken", amber is a warning, and each node kind that draws a glyph has
//!   its own. The aliases (`TRUE` = `SUCCESS`, `PLAYING` = `SUCCESS`) are deliberate: same
//!   hue, same idea, but the call site names the reason.
//!
//! **There are two palettes, `DARK` and `LIGHT`, and one is live at a time.** A token is a
//! function that reads the live one, so the whole view repaints in the other palette on the
//! next frame with no call site knowing the difference, and no colour is threaded through a
//! function signature. The editor is single-threaded, so the mode is one atomic flag, set by
//! `set_mode` from the window's appearance or the author's override.
//!
//! Not here: `CAST_COLORS` in `main.rs`. Those values are written into `.dln` files as
//! character colours, so they are document data, not theme — changing one would orphan
//! every saved character that uses it. For the same reason those colours cannot be swapped
//! per theme: [`ink`] re-tones one for legibility on the live surface instead, and the
//! document never learns which theme drew it.

use gpui::{FontWeight, Hsla, Rgba, Styled, px};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// `rgb()` is not a `const fn` in gpui 0.2.2, and palettes want to be constants.
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

/// Which palette is live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

static LIGHT_LIVE: AtomicBool = AtomicBool::new(false);

/// Makes `mode` the live palette. Cheap and idempotent, so the view may call it every frame.
pub fn set_mode(mode: Mode) {
    LIGHT_LIVE.store(mode == Mode::Light, Ordering::Relaxed);
}

pub fn mode() -> Mode {
    if LIGHT_LIVE.load(Ordering::Relaxed) {
        Mode::Light
    } else {
        Mode::Dark
    }
}

/// Every colour the editor draws with, for one mode. The fields are documented on the
/// token functions below, which are what call sites use.
struct Palette {
    canvas: Rgba,
    pane: Rgba,
    raised: Rgba,
    hover: Rgba,
    pressed: Rgba,
    scrim: Rgba,
    divider: Rgba,
    outline: Rgba,
    edge: Rgba,
    text: Rgba,
    text_muted: Rgba,
    text_faint: Rgba,
    text_disabled: Rgba,
    accent: Rgba,
    accent_wash: Rgba,
    accent_selection: Rgba,
    success: Rgba,
    warning: Rgba,
    danger: Rgba,
    trail: Rgba,
    kind_speech: Rgba,
    kind_hub: Rgba,
    kind_jump: Rgba,
    kind_instruction: Rgba,
    kind_condition: Rgba,
    kind_note: Rgba,
    ending: Rgba,
    field: Rgba,
    note_fill: Rgba,
    note_edge: Rgba,
}

const DARK: Palette = Palette {
    canvas: hex(0x1e1e22),
    pane: hex(0x26262c),
    raised: hex(0x2b2b33),
    hover: hex(0x33333c),
    pressed: hex(0x3a3a44),
    scrim: hexa(0x0000008c),
    divider: hex(0x3a3a44),
    outline: hex(0x4a4a57),
    edge: hex(0x7d7d8c),
    text: hex(0xe2e2e8),
    text_muted: hex(0xb0b0bc),
    text_faint: hex(0x9494a0),
    text_disabled: hex(0x6c6c78),
    accent: hex(0x7aa2f7),
    accent_wash: hexa(0x7aa2f722),
    accent_selection: hexa(0x7aa2f755),
    success: hex(0x8ce99a),
    warning: hex(0xf7c96a),
    danger: hex(0xff7a7a),
    trail: hexa(0x8ce99a99),
    kind_speech: hex(SPEAKER_DEFAULT),
    kind_hub: hex(0xb0b0bc),
    kind_jump: hex(0xc3a6f7),
    kind_instruction: hex(0x6fd6c3),
    kind_condition: hex(0xf29fc5),
    kind_note: hex(0xc8b88a),
    ending: hex(0xf5a97f),
    field: hex(0x1e1e22),
    note_fill: hex(0x2b2b33),
    note_edge: hex(0x4a4a57),
};

/// "Paper": warm greys with white cards. The ladder runs the other way — a higher surface
/// is *lighter* in the dark palette and *whiter* here, but the sunk and hover rungs go
/// darker, so `CANVAS` is the deepest grey and `RAISED` is white.
const LIGHT: Palette = Palette {
    canvas: hex(0xebe9e3),
    pane: hex(0xf7f6f2),
    raised: hex(0xffffff),
    hover: hex(0xeeece5),
    pressed: hex(0xe3e0d6),
    scrim: hexa(0x1e1f2466),
    divider: hex(0xdcd9d0),
    outline: hex(0xd2cec2),
    edge: hex(0x857f72),
    text: hex(0x1e1f24),
    text_muted: hex(0x55575f),
    text_faint: hex(0x666870),
    text_disabled: hex(0x9a9ca3),
    accent: hex(0x2f6fed),
    accent_wash: hexa(0x2f6fed22),
    accent_selection: hexa(0x2f6fed40),
    success: hex(0x217a45),
    warning: hex(0x946010),
    danger: hex(0xc23a3a),
    trail: hexa(0x217a4599),
    kind_speech: hex(0x4b8df8),
    kind_hub: hex(0x666870),
    kind_jump: hex(0x8250df),
    kind_instruction: hex(0x0f8a76),
    kind_condition: hex(0xc2255c),
    kind_note: hex(0xa37f12),
    ending: hex(0xc2622d),
    field: hex(0xffffff),
    note_fill: hex(0xfff7d6),
    note_edge: hex(0xe6d9a3),
};

fn palette() -> &'static Palette {
    match mode() {
        Mode::Dark => &DARK,
        Mode::Light => &LIGHT,
    }
}

// ── Surfaces, back to front ─────────────────────────────────────────────────────────────

/// The graph's background, and anything sunk into a pane: text fields, edge-label backs.
pub fn canvas() -> Rgba {
    palette().canvas
}
/// The menu bar and the panes either side of the canvas.
pub fn pane() -> Rgba {
    palette().pane
}
/// Things that sit on a surface: nodes, chips, buttons, the open menu.
pub fn raised() -> Rgba {
    palette().raised
}
/// Hover over a pane.
pub fn hover() -> Rgba {
    palette().hover
}
/// Hover over something raised, an open menu title, the chip that is on.
pub fn pressed() -> Rgba {
    palette().pressed
}
/// What a modal overlay dims the window with. The only translucent surface: everything
/// else is opaque, so a rung on the ladder always reads as the same colour.
pub fn scrim() -> Rgba {
    palette().scrim
}

// ── Lines ───────────────────────────────────────────────────────────────────────────────

/// Between panes.
pub fn divider() -> Rgba {
    palette().divider
}
/// Around raised things: nodes, menus, text fields.
pub fn outline() -> Rgba {
    palette().outline
}
/// Edges, arrowheads and port rings.
pub fn edge() -> Rgba {
    palette().edge
}

// ── Text ────────────────────────────────────────────────────────────────────────────────

/// Content: dialogue, names, values — what the author wrote.
pub fn text() -> Rgba {
    palette().text
}
/// Labels and secondary text — what the editor says *about* the content.
pub fn text_muted() -> Rgba {
    palette().text_muted
}
/// Hints, counts, idle icon buttons, placeholders. The floor for anything readable.
pub fn text_faint() -> Rgba {
    palette().text_faint
}
/// Disabled only — a spent choice. Below AA on purpose, and only where WCAG allows it.
pub fn text_disabled() -> Rgba {
    palette().text_disabled
}

// ── Interaction ─────────────────────────────────────────────────────────────────────────

/// Selection, focus, the edge being dragged, things you can click to edit.
pub fn accent() -> Rgba {
    palette().accent
}
/// The marquee's fill.
pub fn accent_wash() -> Rgba {
    palette().accent_wash
}
/// Selected text in a field.
pub fn accent_selection() -> Rgba {
    palette().accent_selection
}

// ── Status ──────────────────────────────────────────────────────────────────────────────

pub fn success() -> Rgba {
    palette().success
}
pub fn warning() -> Rgba {
    palette().warning
}
pub fn danger() -> Rgba {
    palette().danger
}

/// Where a play-through is standing.
pub fn playing() -> Rgba {
    success()
}
/// A node or edge the play-through has already passed. Fainter than [`playing`], so the
/// current node is still the one bright thing.
pub fn trail() -> Rgba {
    palette().trail
}
/// A once-only choice the play-through has used up.
pub fn spent() -> Rgba {
    outline()
}
/// A condition's true pin, and a flag set or checked true.
pub fn true_pin() -> Rgba {
    success()
}
/// A condition's false pin, and a flag set or checked false.
pub fn false_pin() -> Rgba {
    danger()
}

// ── Node kinds ──────────────────────────────────────────────────────────────────────────

/// A line of dialogue. Only the chip and the inspector header use it: on the canvas a
/// speech node is tinted by its speaker, which is the whole point of declaring a cast.
pub fn kind_speech() -> Rgba {
    palette().kind_speech
}
/// A hub's glyph and label. Neutral: a hub is plumbing with a name, not a statement.
pub fn kind_hub() -> Rgba {
    palette().kind_hub
}
pub fn kind_jump() -> Rgba {
    palette().kind_jump
}
pub fn kind_instruction() -> Rgba {
    palette().kind_instruction
}
pub fn kind_condition() -> Rgba {
    palette().kind_condition
}
/// Desaturated on purpose: a note is for the author, and must never read as content.
pub fn kind_note() -> Rgba {
    palette().kind_note
}
/// A node marked as a deliberate ending. Peach rather than amber, so it never reads as
/// the `DeadEnd` warning it exists to silence.
pub fn ending() -> Rgba {
    palette().ending
}

/// A text field's fill. Sunk into the pane in the dark palette, where the ladder runs
/// darker; in the light one a grey field reads as disabled, so it is the white of a card
/// with an outline, which is how a paper form draws a place to write.
pub fn field() -> Rgba {
    palette().field
}
/// A note's fill and edge: its own surface, so the author's marginalia never reads as a
/// line of dialogue. Plain card colours in the dark palette; a sticky note in the light.
pub fn note_fill() -> Rgba {
    palette().note_fill
}
pub fn note_edge() -> Rgba {
    palette().note_edge
}

/// The tint for a speaker nobody declared. A `u32`, not an `Rgba`, because it stands in
/// for a `Character::color`, which is stored as one.
pub const SPEAKER_DEFAULT: u32 = 0x9ecbff;

/// A colour the *document* chose — a character's tint — made legible as text on the live
/// surface. In the dark palette it is returned as written. In the light one a pastel that
/// reads well on near-black (amber, mint, sky) would nearly vanish on white, so its
/// lightness is lowered, hue and saturation kept, until it clears 4.5:1 against a node.
/// Applied where the colour is drawn, never stored: the `.dln` holds what the author picked.
pub fn ink(color: Rgba) -> Rgba {
    if mode() == Mode::Dark {
        return color;
    }
    let against = raised();
    let mut hsla: Hsla = color.into();
    while contrast(Rgba::from(hsla), against) < 4.5 && hsla.l > 0.0 {
        hsla.l = (hsla.l - 0.02).max(0.0);
    }
    hsla.into()
}

/// WCAG relative luminance of an sRGB colour.
fn luminance(c: Rgba) -> f32 {
    let lin = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// WCAG contrast ratio between two opaque colours, 1.0 to 21.0.
pub fn contrast(a: Rgba, b: Rgba) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

// ── Type ────────────────────────────────────────────────────────────────────────────────

/// The faces in use, set by the Preferences card. Named, never left to `.SystemUIFont`, which
/// on Linux asks for IBM Plex Sans and falls back to whatever fontconfig has — so a weight
/// could render as a different face on every machine. All of them are bundled (see
/// `assets/fonts`), so a choice looks the same on a machine that has none of them installed.
static FONTS: Mutex<(&str, Option<&str>)> = Mutex::new(("Inter", None));

/// Chooses the UI face and the text face (`None` for "same as the UI"). Cheap and idempotent,
/// so the view may call it every frame, as it does `set_mode`.
pub fn set_fonts(ui: &'static str, text: Option<&'static str>) {
    if let Ok(mut fonts) = FONTS.lock() {
        *fonts = (ui, text);
    }
}

/// The face of the chrome: menus, labels, the inspector, identifiers. Set once at the root
/// and inherited.
pub fn ui_font() -> &'static str {
    FONTS.lock().map_or("Inter", |f| f.0)
}

/// The face of what a character says: dialogue, speaker names, the choices a player picks.
/// The UI face unless the author chose another, so identifiers never take it.
pub fn text_font() -> &'static str {
    FONTS.lock().map_or("Inter", |f| f.1.unwrap_or(f.0))
}

/// A note's face: bundled, and italic, because the UI font has no italic face and a note must
/// not read as something a character says.
pub const NOTE_FONT: &str = "Newsreader 16pt";

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
            .text_color(text())
    }

    /// Names a field or a section. Weight, not colour, is what lifts it off its content.
    fn type_label(self) -> Self {
        self.text_size(px(SMALL))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(text_muted())
    }

    /// Guidance and empty states: readable, never competing.
    fn type_hint(self) -> Self {
        self.text_size(px(SMALL)).text_color(text_faint())
    }

    /// Names a *group* of fields — the one level above [`Typography::type_label`]. Written
    /// in capitals by its caller; gpui 0.2.2 has no letter-spacing, so the capitals are
    /// what keep it from reading as just a smaller label.
    fn type_section(self) -> Self {
        self.text_size(px(TINY))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(text_faint())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The header promises the three text tiers are measured, so measure them — in both
    /// palettes, on the surface each is actually drawn on.
    #[test]
    fn text_tiers_clear_aa_in_both_palettes() {
        for (name, p) in [("dark", &DARK), ("light", &LIGHT)] {
            for (tier, colour) in [
                ("text", p.text),
                ("muted", p.text_muted),
                ("faint", p.text_faint),
            ] {
                for (surface_name, surface) in
                    [("pane", p.pane), ("raised", p.raised), ("canvas", p.canvas)]
                {
                    let ratio = contrast(colour, surface);
                    assert!(
                        ratio >= 4.5,
                        "{name} {tier} on {surface_name} is {ratio:.2}:1"
                    );
                }
            }
        }
    }

    /// A status colour or a kind colour is read as text (a count, a glyph, a label), so it
    /// has to clear the same bar on a node.
    #[test]
    fn status_and_kind_colours_are_legible_on_a_node() {
        for (name, p) in [("dark", &DARK), ("light", &LIGHT)] {
            for (what, colour) in [
                ("accent", p.accent),
                ("success", p.success),
                ("warning", p.warning),
                ("danger", p.danger),
                ("jump", p.kind_jump),
                ("condition", p.kind_condition),
            ] {
                let ratio = contrast(colour, p.raised);
                assert!(ratio >= 4.0, "{name} {what} on a node is {ratio:.2}:1");
            }
        }
    }

    /// A character's pastel, chosen for a dark canvas, must still be readable on white.
    #[test]
    fn ink_makes_a_document_colour_legible_in_the_light_palette() {
        set_mode(Mode::Light);
        for pastel in [0xf7c96a, 0x8ce99a, 0x9ecbff, 0xc3a6f7, 0xff7a7a] {
            let drawn = ink(hex(pastel));
            assert!(
                contrast(drawn, LIGHT.raised) >= 4.5,
                "{pastel:06x} drawn as {drawn:?}"
            );
        }
        set_mode(Mode::Dark);
        assert_eq!(
            ink(hex(0xf7c96a)),
            hex(0xf7c96a),
            "dark draws it as written"
        );
    }
}
