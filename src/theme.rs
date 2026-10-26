//! The Neo-Noir Cyberpunk look: palette, fonts, egui styling, and drawing
//! helpers for the clipped corners, neon glows, icons and controls the
//! design uses.

use std::sync::Arc;

use eframe::egui::epaint::text::{FontTweak, VariationCoords};
use eframe::egui::epaint::{RectShape, Shadow};
use eframe::egui::text::LayoutJob;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId,
    Galley, InnerResponse, Margin, Painter, Pos2, Rect, Response, Role, Sense, Shape, Stroke, TextFormat,
    Ui, Vec2, WidgetInfo, pos2, vec2,
};

use crate::scanner::Kind;

// ------------------------------------------------------------- palette --

pub const BG: Color32 = Color32::from_rgb(0x07, 0x08, 0x0C);
pub const PANEL: Color32 = Color32::from_rgb(0x0B, 0x0D, 0x13);
pub const SURFACE: Color32 = Color32::from_rgb(0x0E, 0x10, 0x17);
pub const RAISED: Color32 = Color32::from_rgb(0x15, 0x18, 0x22);
pub const BORDER: Color32 = Color32::from_rgb(0x26, 0x2A, 0x38);
pub const TRACK: Color32 = Color32::from_rgb(0x1B, 0x1E, 0x29);
pub const TEXT: Color32 = Color32::from_rgb(0xE6, 0xE8, 0xF0);
pub const MUTED: Color32 = Color32::from_rgb(0x8A, 0x90, 0xA6);
pub const DIM: Color32 = Color32::from_rgb(0x4A, 0x50, 0x68);
pub const GREY: Color32 = Color32::from_rgb(0x6B, 0x70, 0x88);
pub const CYAN: Color32 = Color32::from_rgb(0x19, 0xE3, 0xFF);
pub const CYAN_DEEP: Color32 = Color32::from_rgb(0x07, 0x22, 0x29);
pub const MAGENTA: Color32 = Color32::from_rgb(0xFF, 0x2E, 0x88);
pub const MAGENTA_SOFT: Color32 = Color32::from_rgb(0xFF, 0x9C, 0xC6);
pub const MAGENTA_DEEP: Color32 = Color32::from_rgb(0x1A, 0x0A, 0x13);
pub const YELLOW: Color32 = Color32::from_rgb(0xF5, 0xE6, 0x63);
pub const YELLOW_DEEP: Color32 = Color32::from_rgb(0x17, 0x16, 0x0A);
pub const GREEN: Color32 = Color32::from_rgb(0x7C, 0xFF, 0x6B);

/// Treemap colours for one kind of item: a dark tinted fill, a neon edge,
/// and a light tint for its text.
#[derive(Clone, Copy)]
pub struct KindStyle {
    pub fill: Color32,
    pub edge: Color32,
    pub text: Color32,
}

const fn ks(fill: [u8; 3], edge: [u8; 3], text: [u8; 3]) -> KindStyle {
    KindStyle {
        fill: Color32::from_rgb(fill[0], fill[1], fill[2]),
        edge: Color32::from_rgb(edge[0], edge[1], edge[2]),
        text: Color32::from_rgb(text[0], text[1], text[2]),
    }
}

pub fn kind_style(kind: Kind) -> KindStyle {
    match kind {
        Kind::Folder => KindStyle { fill: SURFACE, edge: BORDER, text: TEXT },
        Kind::Video => ks([0x2A, 0x0D, 0x1C], [0xFF, 0x2E, 0x88], [0xFF, 0x9C, 0xC6]),
        Kind::Audio => ks([0x1E, 0x14, 0x33], [0xA9, 0x74, 0xFF], [0xD2, 0xB8, 0xFF]),
        Kind::Image => ks([0x12, 0x26, 0x0F], [0x7C, 0xFF, 0x6B], [0xB5, 0xFF, 0xAC]),
        Kind::Archive => ks([0x2B, 0x1F, 0x08], [0xFF, 0xB0, 0x20], [0xFF, 0xD2, 0x7A]),
        Kind::Document => ks([0x1C, 0x1E, 0x26], [0xD8, 0xDC, 0xE8], [0xE6, 0xE8, 0xF0]),
        Kind::Code => ks([0x07, 0x22, 0x29], [0x19, 0xE3, 0xFF], [0x8C, 0xF1, 0xFF]),
        Kind::App => ks([0x0F, 0x18, 0x34], [0x4D, 0x7C, 0xFF], [0xA9, 0xBE, 0xFF]),
        Kind::System | Kind::Other => ks([0x17, 0x18, 0x1F], [0x6B, 0x70, 0x88], [0xB4, 0xB8, 0xC8]),
    }
}

/// The colour key under the treemap.
pub const LEGEND: [(&str, Kind); 8] = [
    ("VIDEO", Kind::Video),
    ("AUDIO", Kind::Audio),
    ("IMAGE", Kind::Image),
    ("ARCHIVE", Kind::Archive),
    ("DOCUMENT", Kind::Document),
    ("CODE", Kind::Code),
    ("APPS", Kind::App),
    ("SYSTEM / OTHER", Kind::System),
];

pub fn kind_tag(kind: Kind) -> &'static str {
    match kind {
        Kind::Folder => "FOLDER",
        Kind::App => "APP",
        Kind::Video => "VIDEO",
        Kind::Audio => "AUDIO",
        Kind::Image => "IMAGE",
        Kind::Archive => "ARCHIVE",
        Kind::Document => "DOCUMENT",
        Kind::Code => "CODE",
        Kind::System => "SYSTEM",
        Kind::Other => "OTHER",
    }
}

// --------------------------------------------------------------- fonts --

const CHAKRA_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/ChakraPetch-SemiBold.ttf");
const CHAKRA_BOLD: &[u8] = include_bytes!("../assets/fonts/ChakraPetch-Bold.ttf");
const INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Variable.ttf");
const JETBRAINS_MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf");

const DISPLAY: &str = "display";
const DISPLAY_BOLD: &str = "display-bold";
const BODY_MEDIUM: &str = "body-medium";
const BODY_SEMIBOLD: &str = "body-semibold";
const MONO_MEDIUM: &str = "mono-medium";

/// Chakra Petch SemiBold: headings, buttons, labels.
pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(DISPLAY.into()))
}

/// Chakra Petch Bold: the logo, primary buttons, big headlines.
pub fn display_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(DISPLAY_BOLD.into()))
}

/// Inter Regular: body copy.
pub fn body(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn body_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BODY_MEDIUM.into()))
}

pub fn body_semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BODY_SEMIBOLD.into()))
}

/// JetBrains Mono Regular: sizes, paths, readouts.
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub fn mono_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MONO_MEDIUM.into()))
}

fn weighted(bytes: &'static [u8], weight: f32) -> Arc<FontData> {
    let tweak = FontTweak { coords: VariationCoords::new([(b"wght", weight)]), ..Default::default() };
    Arc::new(FontData::from_static(bytes).tweak(tweak))
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    // egui's own fonts stay as fallbacks for symbols like ⌘ and ✔.
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mono_fallback = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();

    let data = [
        ("chakra-600", Arc::new(FontData::from_static(CHAKRA_SEMIBOLD))),
        ("chakra-700", Arc::new(FontData::from_static(CHAKRA_BOLD))),
        ("inter-400", weighted(INTER, 400.0)),
        ("inter-500", weighted(INTER, 500.0)),
        ("inter-600", weighted(INTER, 600.0)),
        ("jbm-400", weighted(JETBRAINS_MONO, 400.0)),
        ("jbm-500", weighted(JETBRAINS_MONO, 500.0)),
    ];
    for (name, font) in data {
        fonts.font_data.insert(name.to_owned(), font);
    }
    let family = |first: &str, rest: &[String]| -> Vec<String> {
        std::iter::once(first.to_owned()).chain(rest.iter().cloned()).collect()
    };
    fonts.families.insert(FontFamily::Proportional, family("inter-400", &fallback));
    fonts.families.insert(FontFamily::Monospace, family("jbm-400", &mono_fallback));
    fonts.families.insert(FontFamily::Name(DISPLAY.into()), family("chakra-600", &fallback));
    fonts.families.insert(FontFamily::Name(DISPLAY_BOLD.into()), family("chakra-700", &fallback));
    fonts.families.insert(FontFamily::Name(BODY_MEDIUM.into()), family("inter-500", &fallback));
    fonts.families.insert(FontFamily::Name(BODY_SEMIBOLD.into()), family("inter-600", &fallback));
    fonts.families.insert(FontFamily::Name(MONO_MEDIUM.into()), family("jbm-500", &mono_fallback));
    ctx.set_fonts(fonts);
}

// --------------------------------------------------------------- style --

/// Installs the fonts and the always-dark style.
pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        use egui::TextStyle;
        style.text_styles = [
            (TextStyle::Small, body(11.0)),
            (TextStyle::Body, body(14.0)),
            (TextStyle::Monospace, mono(13.0)),
            (TextStyle::Button, display(14.0)),
            (TextStyle::Heading, display_bold(20.0)),
        ]
        .into();

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.weak_text_color = Some(MUTED);
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.window_shadow = Shadow { offset: [0, 0], blur: 32, spread: 0, color: glow_color(CYAN, 0.12) };
        v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(170) };
        v.extreme_bg_color = BG;
        v.text_edit_bg_color = Some(BG);
        v.faint_bg_color = PANEL;
        v.code_bg_color = PANEL;
        v.hyperlink_color = CYAN;
        v.warn_fg_color = YELLOW;
        v.error_fg_color = MAGENTA;
        v.selection.bg_fill = CYAN;
        v.selection.stroke = Stroke::new(1.0, BG);
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.55 };

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = SURFACE;
        w.noninteractive.weak_bg_fill = SURFACE;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.bg_fill = TRACK;
        w.inactive.weak_bg_fill = SURFACE;
        w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.bg_fill = TRACK;
        w.hovered.weak_bg_fill = RAISED;
        w.hovered.bg_stroke = Stroke::new(1.0, CYAN);
        w.hovered.fg_stroke = Stroke::new(1.5, CYAN);
        w.active.bg_fill = TRACK;
        w.active.weak_bg_fill = MAGENTA_DEEP;
        w.active.bg_stroke = Stroke::new(1.0, MAGENTA);
        w.active.fg_stroke = Stroke::new(1.5, MAGENTA);
        w.open = w.hovered;
        for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            wv.corner_radius = CornerRadius::ZERO;
            wv.expansion = 0.0;
        }

        let s = &mut style.spacing;
        s.item_spacing = vec2(8.0, 8.0);
        s.button_padding = vec2(12.0, 6.0);
        s.interact_size.y = 28.0;
        s.slider_width = 130.0;
        s.slider_rail_height = 4.0;
        s.menu_margin = Margin::same(8);
    });
}

/// A neon colour as a translucent glow.
pub fn glow_color(color: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (alpha * 255.0) as u8)
}

// ------------------------------------------------------------- drawing --

/// The design's cut corners: top-right and bottom-left.
pub fn chamfer_points(r: Rect, cut: f32) -> Vec<Pos2> {
    let c = cut.min(r.width() / 2.0).min(r.height() / 2.0);
    vec![
        r.left_top(),
        pos2(r.right() - c, r.top()),
        pos2(r.right(), r.top() + c),
        r.right_bottom(),
        pos2(r.left() + c, r.bottom()),
        pos2(r.left(), r.bottom() - c),
    ]
}

pub fn chamfer_shape(r: Rect, cut: f32, fill: Color32, stroke: Stroke) -> Shape {
    // Inset the outline by half the stroke so it stays inside `r`.
    Shape::convex_polygon(chamfer_points(r.shrink(stroke.width / 2.0), cut), fill, stroke)
}

/// A soft neon haze behind `r`.
pub fn glow_shape(r: Rect, color: Color32, alpha: f32, blur: f32) -> Shape {
    RectShape::filled(r, 0.0, glow_color(color, alpha)).with_blur_width(blur).into()
}

pub fn glow(painter: &Painter, r: Rect, color: Color32, alpha: f32, blur: f32) {
    painter.add(glow_shape(r, color, alpha, blur));
}

/// A neon glow hugging the edges of `r`, inside and out, without washing
/// over the middle (for large boxes).
pub fn edge_glow(painter: &Painter, r: Rect, color: Color32) {
    for (width, alpha) in [(10.0, 0.08), (6.0, 0.14), (3.0, 0.25)] {
        painter.rect_stroke(r, 0.0, Stroke::new(width, glow_color(color, alpha)), egui::StrokeKind::Middle);
    }
}

pub fn dashed_rect(painter: &Painter, r: Rect, stroke: Stroke) {
    let r = r.shrink(stroke.width / 2.0);
    let points = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    painter.extend(Shape::dashed_line(&points, stroke, 5.0, 4.0));
}

/// Lays out text with letter spacing and an optional wrap width. The text
/// is laid out in the placeholder colour, so painting chooses its colour.
pub fn galley(painter: &Painter, text: impl Into<String>, font: FontId, spacing: f32, wrap: f32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(
        text.into(),
        TextFormat { font_id: font, color: Color32::PLACEHOLDER, extra_letter_spacing: spacing, ..Default::default() },
    );
    job.wrap.max_width = wrap;
    painter.layout_job(job)
}

/// Paints one line of text anchored at `pos`; returns where it went.
pub fn text(painter: &Painter, pos: Pos2, anchor: Align2, text: impl Into<String>, font: FontId, color: Color32, spacing: f32) -> Rect {
    let g = galley(painter, text, font, spacing, f32::INFINITY);
    let rect = anchor.anchor_size(pos, g.size());
    painter.galley(rect.min, g, color);
    rect
}

/// Text with a neon halo, for the big readouts.
pub fn glow_text(painter: &Painter, pos: Pos2, anchor: Align2, txt: impl Into<String>, font: FontId, color: Color32, spacing: f32) -> Rect {
    let g = galley(painter, txt, font, spacing, f32::INFINITY);
    let rect = anchor.anchor_size(pos, g.size());
    painter.add(glow_shape(rect.shrink2(vec2(rect.width() * 0.05, rect.height() * 0.2)), color, 0.22, rect.height() * 0.9));
    let halo = glow_color(color, 0.18);
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
        painter.galley(rect.min + vec2(dx, dy), g.clone(), halo);
    }
    painter.galley(rect.min, g, color);
    rect
}

/// A small glowing square status light.
pub fn status_light(painter: &Painter, center: Pos2, color: Color32) {
    let r = Rect::from_center_size(center, vec2(8.0, 8.0));
    glow(painter, r.expand(2.0), color, 0.5, 8.0);
    painter.rect_filled(r, 0.0, color);
}

// --------------------------------------------------------------- icons --

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Logo,
    Folder,
    FolderPlus,
    Rescan,
    Up,
    Gear,
    Home,
    Disk,
    Download,
    Stop,
}

/// Draws a square-capped stroke icon on a 24-unit grid inside `r`.
pub fn icon(painter: &Painter, r: Rect, icon: Icon, color: Color32) {
    let grid = if icon == Icon::Logo { 32.0 } else { 24.0 };
    let s = r.width().min(r.height()) / grid;
    let o = r.center() - vec2(grid, grid) * s / 2.0;
    let p = |x: f32, y: f32| o + vec2(x, y) * s;
    let w = (2.0 * s).max(1.2);
    let stroke = Stroke::new(w, color);
    let line = |pts: &[Pos2]| painter.add(Shape::line(pts.to_vec(), stroke));
    let closed = |pts: &[Pos2]| painter.add(Shape::closed_line(pts.to_vec(), stroke));
    match icon {
        Icon::Logo => {
            let hex = [p(16.0, 2.0), p(28.0, 9.0), p(28.0, 23.0), p(16.0, 30.0), p(4.0, 23.0), p(4.0, 9.0)];
            painter.add(Shape::closed_line(hex.to_vec(), Stroke::new(1.5 * s, CYAN)));
            let bar = Stroke::new(2.0 * s, MAGENTA);
            for (y, x2) in [(13.0, 22.0), (17.0, 18.0), (21.0, 15.0)] {
                painter.line_segment([p(10.0, y), p(x2, y)], bar);
            }
        }
        Icon::Folder | Icon::FolderPlus => {
            closed(&[p(3.0, 6.0), p(10.0, 6.0), p(12.0, 8.0), p(21.0, 8.0), p(21.0, 19.0), p(3.0, 19.0)]);
            if icon == Icon::FolderPlus {
                line(&[p(12.0, 11.0), p(12.0, 16.0)]);
                line(&[p(9.5, 13.5), p(14.5, 13.5)]);
            }
        }
        Icon::Rescan => {
            let arc: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let a = (-40.0 + 290.0 * i as f32 / 24.0).to_radians();
                    p(12.0 + 8.5 * a.cos(), 12.0 + 8.5 * a.sin())
                })
                .collect();
            line(&arc);
            line(&[p(20.0, 3.0), p(20.0, 7.5), p(15.5, 7.5)]);
        }
        Icon::Up => {
            line(&[p(12.0, 19.0), p(12.0, 5.0)]);
            line(&[p(5.0, 12.0), p(12.0, 5.0), p(19.0, 12.0)]);
        }
        Icon::Gear => {
            painter.circle_stroke(p(12.0, 12.0), 3.0 * s, stroke);
            for (a, b) in [
                ((12.0, 2.0), (12.0, 5.0)),
                ((12.0, 19.0), (12.0, 22.0)),
                ((4.2, 4.2), (6.3, 6.3)),
                ((17.7, 17.7), (19.8, 19.8)),
                ((2.0, 12.0), (5.0, 12.0)),
                ((19.0, 12.0), (22.0, 12.0)),
                ((4.2, 19.8), (6.3, 17.7)),
                ((17.7, 6.3), (19.8, 4.2)),
            ] {
                line(&[p(a.0, a.1), p(b.0, b.1)]);
            }
        }
        Icon::Home => {
            closed(&[p(3.0, 11.0), p(12.0, 4.0), p(21.0, 11.0), p(21.0, 21.0), p(15.0, 21.0), p(15.0, 15.0), p(9.0, 15.0), p(9.0, 21.0), p(3.0, 21.0)]);
        }
        Icon::Disk => {
            closed(&[p(3.0, 6.0), p(21.0, 6.0), p(21.0, 18.0), p(3.0, 18.0)]);
            line(&[p(16.0, 12.0), p(18.0, 12.0)]);
        }
        Icon::Download => {
            line(&[p(12.0, 4.0), p(12.0, 15.0)]);
            line(&[p(7.0, 10.0), p(12.0, 15.0), p(17.0, 10.0)]);
            line(&[p(5.0, 20.0), p(19.0, 20.0)]);
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(r.center(), vec2(12.0, 12.0) * s * 2.0 / 2.0), 0.0, color);
        }
    }
}

// ------------------------------------------------------------ controls --

#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    /// Filled neon with dark text and a cut corner.
    Primary(Color32),
    /// Neon outline and text.
    Outline(Color32),
    /// Quiet grey outline.
    Ghost,
}

/// The design's uppercase Chakra Petch button.
pub struct Button<'a> {
    label: &'a str,
    tone: Tone,
    icon: Option<Icon>,
    shortcut: Option<&'a str>,
    height: f32,
    size: f32,
    width: Option<f32>,
    enabled: bool,
    mono: bool,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str, tone: Tone) -> Self {
        Button { label, tone, icon: None, shortcut: None, height: 40.0, size: 14.0, width: None, enabled: true, mono: false }
    }

    /// Monospace label, kept as written (for readouts like counts).
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn shortcut(mut self, shortcut: &'a str) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    pub fn small(mut self) -> Self {
        self.height = 32.0;
        self.size = 12.0;
        self
    }

    /// Label size in points.
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let painter = ui.painter().clone();
        let label = if self.mono {
            galley(&painter, self.label, mono(self.size), 0.0, f32::INFINITY)
        } else {
            let font = match self.tone {
                Tone::Primary(_) => display_bold(self.size),
                _ => display(self.size),
            };
            galley(&painter, self.label.to_uppercase(), font, self.size * 0.06, f32::INFINITY)
        };
        let shortcut = self.shortcut.map(|s| galley(&painter, s, mono(11.0), 0.0, f32::INFINITY));
        let pad = if self.height < 36.0 { 12.0 } else { 16.0 };
        let icon_w = if self.icon.is_some() { 16.0 + 8.0 } else { 0.0 };
        let shortcut_w = shortcut.as_ref().map_or(0.0, |g| g.size().x + 8.0);
        let content_w = icon_w + label.size().x + shortcut_w;
        let width = self.width.unwrap_or(content_w + pad * 2.0);
        let sense = if self.enabled { Sense::click() } else { Sense::hover() };
        let (rect, response) = ui.allocate_exact_size(vec2(width, self.height), sense);
        let (name, enabled) = (self.label, self.enabled);
        response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, name));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let hovered = self.enabled && response.hovered();
        let fade = |c: Color32| if self.enabled { c } else { c.gamma_multiply(0.35) };

        let (fg, sub) = match self.tone {
            Tone::Primary(c) => {
                if hovered {
                    glow(&painter, rect, c, 0.45, 16.0);
                }
                let fill = if hovered { c.lerp_to_gamma(Color32::WHITE, 0.15) } else { c };
                painter.add(chamfer_shape(rect, 10.0, fade(fill), Stroke::NONE));
                (fade(BG), fade(BG.gamma_multiply(0.7)))
            }
            Tone::Outline(c) => {
                let fill = if hovered { glow_color(c, 0.12) } else { Color32::TRANSPARENT };
                if hovered {
                    glow(&painter, rect, c, 0.25, 12.0);
                }
                painter.rect(rect, 0.0, fill, Stroke::new(1.0, fade(c)), egui::StrokeKind::Inside);
                (fade(c), fade(MUTED))
            }
            Tone::Ghost => {
                let edge = if hovered { CYAN } else { BORDER };
                painter.rect(rect, 0.0, if hovered { RAISED } else { SURFACE }, Stroke::new(1.0, fade(edge)), egui::StrokeKind::Inside);
                (fade(if hovered { CYAN } else { TEXT }), fade(MUTED))
            }
        };

        let mut x = rect.center().x - content_w / 2.0;
        if let Some(i) = self.icon {
            icon(&painter, Rect::from_min_size(pos2(x, rect.center().y - 8.0), vec2(16.0, 16.0)), i, fg);
            x += icon_w;
        }
        painter.galley(pos2(x, rect.center().y - label.size().y / 2.0), label.clone(), fg);
        x += label.size().x + 8.0;
        if let Some(g) = shortcut {
            painter.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, sub);
        }
        if hovered {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        response
    }
}

/// A square icon-only button (the Settings gear).
pub fn icon_button(ui: &mut Ui, ic: Icon, size: f32, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, tooltip));
    let hovered = response.hovered();
    let color = if hovered { CYAN } else { TEXT };
    ui.painter().rect(rect, 0.0, if hovered { RAISED } else { Color32::TRANSPARENT }, Stroke::new(1.0, if hovered { CYAN } else { BORDER }), egui::StrokeKind::Inside);
    icon(ui.painter(), Rect::from_center_size(rect.center(), vec2(18.0, 18.0)), ic, color);
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response.on_hover_text(tooltip)
}

/// The design's square neon on/off switch.
pub fn switch(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(60.0, 30.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let state = *on;
    response.widget_info(|| WidgetInfo::selected(Role::Switch, true, state, label));
    let painter = ui.painter();
    let knob_x = if *on { rect.right() - 4.0 - 22.0 } else { rect.left() + 4.0 };
    let knob = Rect::from_min_size(pos2(knob_x, rect.top() + 4.0), vec2(22.0, 22.0));
    if *on {
        glow(painter, rect, MAGENTA, 0.3, 14.0);
        painter.rect(rect, 0.0, Color32::from_rgb(0x2A, 0x0D, 0x1C), Stroke::new(1.0, MAGENTA), egui::StrokeKind::Inside);
        painter.rect_filled(knob, 0.0, MAGENTA);
    } else {
        let edge = if response.hovered() { GREY } else { DIM };
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
        painter.rect_filled(knob, 0.0, edge);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response
}

/// A tab with a magenta underline when selected.
pub fn tab(ui: &mut Ui, label: &str, selected: bool, width: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::Tab, true, selected, label));
    let painter = ui.painter();
    let color = if selected { TEXT } else if response.hovered() { CYAN } else { MUTED };
    text(painter, rect.center(), Align2::CENTER_CENTER, label, display(13.0), color, 1.0);
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, BORDER));
    if selected {
        let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 2.0), rect.right_bottom());
        glow(painter, bar, MAGENTA, 0.5, 6.0);
        painter.rect_filled(bar, 0.0, MAGENTA);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response
}

/// A `// LABEL` caption in muted, spaced monospace.
pub fn caption(ui: &mut Ui, label: &str, color: Color32) -> Response {
    ui.label(egui::RichText::new(label).font(mono(11.0)).color(color).extra_letter_spacing(1.5))
}

/// Paints a frame behind whatever `add` puts in it: a filled, outlined box
/// with an optional cut corner and neon glow.
pub fn framed<R>(
    ui: &mut Ui,
    fill: Color32,
    stroke: Stroke,
    cut: f32,
    glow_with: Option<Color32>,
    margin: impl Into<Margin>,
    add: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let glow_idx = ui.painter().add(Shape::Noop);
    let bg_idx = ui.painter().add(Shape::Noop);
    let inner = egui::Frame::NONE.inner_margin(margin).show(ui, add);
    let rect = inner.response.rect;
    if let Some(c) = glow_with {
        ui.painter().set(glow_idx, glow_shape(rect, c, 0.22, 28.0));
    }
    ui.painter().set(bg_idx, chamfer_shape(rect, cut, fill, stroke));
    inner
}

/// Points along a circle from `start` to `end` (radians, clockwise from
/// the top).
pub fn arc(center: Pos2, radius: f32, start: f32, end: f32) -> Vec<Pos2> {
    let steps = ((end - start).abs() * radius / 4.0).ceil().max(2.0) as usize;
    (0..=steps)
        .map(|i| {
            let a = start + (end - start) * i as f32 / steps as f32 - std::f32::consts::FRAC_PI_2;
            center + Vec2::angled(a) * radius
        })
        .collect()
}
