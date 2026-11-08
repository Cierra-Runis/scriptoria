//! Colors: Catppuccin (<https://catppuccin.com/palette>), Latte when light and Mocha when dark.
//! The window follows the system's appearance unless one is picked (视图 → 外观); the pick is
//! remembered. The component library's theme is made from the palette here, and the views take
//! the colors of their own drawings from [`palette`], so both always match.

use gpui::component::{ActiveTheme as _, Theme, ThemeMode, ThemeRegistry};
use gpui::{App, Global, Hsla, Window};
use serde_json::{Value, json};

/// A palette color (`0xRRGGBB`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub fn hsla(self) -> Hsla {
        gpui::rgb(self.0).into()
    }

    pub fn opacity(self, a: f32) -> Hsla {
        self.hsla().opacity(a)
    }

    /// As a theme file writes it, with an alpha (0–255) if not opaque.
    fn hex(self, alpha: u8) -> String {
        if alpha == 255 { format!("#{:06x}", self.0) } else { format!("#{:06x}{alpha:02x}", self.0) }
    }
}

impl From<Color> for Hsla {
    fn from(c: Color) -> Hsla {
        c.hsla()
    }
}

/// One flavor of the palette: its accents, then its text and surfaces from the most to the least
/// contrast with the background (`base`).
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub rosewater: Color,
    pub flamingo: Color,
    pub pink: Color,
    pub mauve: Color,
    pub red: Color,
    pub maroon: Color,
    pub peach: Color,
    pub yellow: Color,
    pub green: Color,
    pub teal: Color,
    pub sky: Color,
    pub sapphire: Color,
    pub blue: Color,
    pub lavender: Color,
    pub text: Color,
    pub subtext1: Color,
    pub subtext0: Color,
    pub overlay2: Color,
    pub overlay1: Color,
    pub overlay0: Color,
    pub surface2: Color,
    pub surface1: Color,
    pub surface0: Color,
    pub base: Color,
    pub mantle: Color,
    pub crust: Color,
}

pub const LATTE: Palette = Palette {
    rosewater: Color(0xdc8a78),
    flamingo: Color(0xdd7878),
    pink: Color(0xea76cb),
    mauve: Color(0x8839ef),
    red: Color(0xd20f39),
    maroon: Color(0xe64553),
    peach: Color(0xfe640b),
    yellow: Color(0xdf8e1d),
    green: Color(0x40a02b),
    teal: Color(0x179299),
    sky: Color(0x04a5e5),
    sapphire: Color(0x209fb5),
    blue: Color(0x1e66f5),
    lavender: Color(0x7287fd),
    text: Color(0x4c4f69),
    subtext1: Color(0x5c5f77),
    subtext0: Color(0x6c6f85),
    overlay2: Color(0x7c7f93),
    overlay1: Color(0x8c8fa1),
    overlay0: Color(0x9ca0b0),
    surface2: Color(0xacb0be),
    surface1: Color(0xbcc0cc),
    surface0: Color(0xccd0da),
    base: Color(0xeff1f5),
    mantle: Color(0xe6e9ef),
    crust: Color(0xdce0e8),
};

pub const MOCHA: Palette = Palette {
    rosewater: Color(0xf5e0dc),
    flamingo: Color(0xf2cdcd),
    pink: Color(0xf5c2e7),
    mauve: Color(0xcba6f7),
    red: Color(0xf38ba8),
    maroon: Color(0xeba0ac),
    peach: Color(0xfab387),
    yellow: Color(0xf9e2af),
    green: Color(0xa6e3a1),
    teal: Color(0x94e2d5),
    sky: Color(0x89dceb),
    sapphire: Color(0x74c7ec),
    blue: Color(0x89b4fa),
    lavender: Color(0xb4befe),
    text: Color(0xcdd6f4),
    subtext1: Color(0xbac2de),
    subtext0: Color(0xa6adc8),
    overlay2: Color(0x9399b2),
    overlay1: Color(0x7f849c),
    overlay0: Color(0x6c7086),
    surface2: Color(0x585b70),
    surface1: Color(0x45475a),
    surface0: Color(0x313244),
    base: Color(0x1e1e2e),
    mantle: Color(0x181825),
    crust: Color(0x11111b),
};

/// The palette in use.
pub fn palette(cx: &App) -> &'static Palette {
    if cx.theme().is_dark() { &MOCHA } else { &LATTE }
}

const LIGHT_NAME: &str = "Catppuccin Latte";
const DARK_NAME: &str = "Catppuccin Mocha";

/// The component library's colors for a flavor (the theme file's keys).
fn theme_colors(p: &Palette, dark: bool) -> Value {
    let c = |c: Color| c.hex(255);
    let a = |c: Color, alpha: u8| c.hex(alpha);
    // The text on an accent: the darkest surface when dark (accents are light), else the base.
    let on_accent = if dark { p.crust } else { p.base };
    let colors = [
        ("background", c(p.base)),
        ("foreground", c(p.text)),
        ("border", c(p.surface0)),
        ("window.border", c(p.surface0)),
        ("input.border", c(p.surface1)),
        ("ring", c(p.lavender)),
        ("caret", c(p.rosewater)),
        ("selection.background", a(p.overlay2, 0x4d)),
        ("overlay", a(p.crust, if dark { 0x99 } else { 0x40 })),
        ("accent.background", c(p.surface0)),
        ("accent.foreground", c(p.text)),
        ("accordion.background", c(p.base)),
        ("muted.background", c(p.mantle)),
        ("muted.foreground", c(p.subtext0)),
        ("primary.background", c(p.mauve)),
        ("primary.hover.background", a(p.mauve, 0xe0)),
        ("primary.active.background", c(p.lavender)),
        ("primary.foreground", c(on_accent)),
        ("secondary.background", c(p.surface0)),
        ("secondary.hover.background", c(p.surface1)),
        ("secondary.active.background", c(p.surface2)),
        ("secondary.foreground", c(p.text)),
        ("danger.background", c(p.red)),
        ("danger.foreground", c(on_accent)),
        ("success.background", c(p.green)),
        ("success.foreground", c(on_accent)),
        ("warning.background", c(p.yellow)),
        ("warning.foreground", c(on_accent)),
        ("info.background", c(p.sky)),
        ("info.foreground", c(on_accent)),
        ("link.foreground", c(p.blue)),
        ("link.hover.foreground", c(p.sapphire)),
        ("link.active.foreground", c(p.blue)),
        ("drag_border", c(p.blue)),
        ("drop_target.background", a(p.blue, 0x40)),
        ("popover.background", c(p.base)),
        ("popover.foreground", c(p.text)),
        ("group_box.background", c(p.mantle)),
        ("group_box.foreground", c(p.text)),
        ("description_list_label.foreground", c(p.subtext1)),
        ("list.background", c(p.base)),
        ("list.hover.background", a(p.surface0, 0x80)),
        ("list.active.background", a(p.mauve, 0x26)),
        ("list.active.border", c(p.mauve)),
        ("list.even.background", c(p.mantle)),
        ("list.head.background", c(p.mantle)),
        ("table.background", c(p.base)),
        ("table.hover.background", a(p.surface0, 0x80)),
        ("table.active.background", a(p.mauve, 0x26)),
        ("table.active.border", c(p.mauve)),
        ("table.even.background", c(p.mantle)),
        ("table.head.background", c(p.mantle)),
        ("table.head.foreground", c(p.subtext0)),
        ("table.row.border", c(p.surface0)),
        ("scrollbar.background", a(p.base, 0)),
        ("scrollbar.thumb.background", c(p.surface2)),
        ("scrollbar.thumb.hover.background", c(p.overlay0)),
        ("sidebar.background", c(p.mantle)),
        ("sidebar.foreground", c(p.text)),
        ("sidebar.border", c(p.surface0)),
        ("sidebar.accent.background", c(p.surface0)),
        ("sidebar.accent.foreground", c(p.text)),
        ("sidebar.primary.background", c(p.mauve)),
        ("sidebar.primary.foreground", c(on_accent)),
        ("skeleton.background", c(p.surface0)),
        ("slider.bar.background", c(p.mauve)),
        ("slider.thumb.background", c(p.base)),
        ("switch.background", c(p.surface1)),
        ("progress_bar.background", c(p.mauve)),
        ("tab.background", a(p.mantle, 0)),
        ("tab.foreground", c(p.subtext0)),
        ("tab.active.background", c(p.base)),
        ("tab.active.foreground", c(p.text)),
        ("tab_bar.background", c(p.mantle)),
        ("tab_bar.segmented.background", c(p.mantle)),
        ("title_bar.background", c(p.crust)),
        ("title_bar.border", c(p.surface0)),
        ("status_bar.background", c(p.crust)),
        ("status_bar.border", c(p.surface0)),
        ("chart.1", c(p.blue)),
        ("chart.2", c(p.mauve)),
        ("chart.3", c(p.teal)),
        ("chart.4", c(p.peach)),
        ("chart.5", c(p.pink)),
        ("chart.grid", a(p.surface1, 0x99)),
        ("chart.bullish", c(p.green)),
        ("chart.bearish", c(p.red)),
        ("base.red", c(p.red)),
        ("base.red.light", a(p.red, 0x99)),
        ("base.green", c(p.green)),
        ("base.green.light", a(p.green, 0x99)),
        ("base.blue", c(p.blue)),
        ("base.blue.light", a(p.blue, 0x99)),
        ("base.yellow", c(p.yellow)),
        ("base.yellow.light", a(p.yellow, 0x99)),
        ("base.magenta", c(p.mauve)),
        ("base.magenta.light", a(p.mauve, 0x99)),
        ("base.cyan", c(p.teal)),
        ("base.cyan.light", a(p.teal, 0x99)),
    ];
    Value::Object(colors.into_iter().map(|(k, v)| (k.to_string(), Value::String(v))).collect())
}

/// Both flavors as a theme file.
fn theme_set() -> String {
    json!({
        "name": "Catppuccin",
        "author": "Catppuccin",
        "url": "https://catppuccin.com",
        "themes": [
            { "name": LIGHT_NAME, "mode": "light", "colors": theme_colors(&LATTE, false) },
            { "name": DARK_NAME, "mode": "dark", "colors": theme_colors(&MOCHA, true) },
        ],
    })
    .to_string()
}

/// Light or dark, or as the system is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    fn name(self) -> &'static str {
        match self {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        [Appearance::System, Appearance::Light, Appearance::Dark].into_iter().find(|a| a.name() == name)
    }
}

/// The appearance picked.
struct Chosen(Appearance);

impl Global for Chosen {}

/// Where the pick is remembered: the user's configuration folder.
fn settings_file() -> Option<std::path::PathBuf> {
    let var = |name| std::env::var_os(name).filter(|v| !v.is_empty()).map(std::path::PathBuf::from);
    let dir = if cfg!(windows) {
        var("APPDATA")?
    } else if cfg!(target_os = "macos") {
        var("HOME")?.join("Library/Application Support")
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))?
    };
    Some(dir.join("Scriptoria").join("appearance"))
}

/// Registers the flavors and makes them the light and dark themes; the appearance remembered is
/// applied with the first window ([`apply`]).
pub fn init(cx: &mut App) {
    if let Err(e) = ThemeRegistry::global_mut(cx).load_themes_from_str(&theme_set()) {
        eprintln!("the Catppuccin theme does not load: {e}");
        return;
    }
    let registry = ThemeRegistry::global(cx);
    let (light, dark) = (registry.themes().get(LIGHT_NAME).cloned(), registry.themes().get(DARK_NAME).cloned());
    if let (Some(light), Some(dark)) = (light, dark) {
        let theme = Theme::global_mut(cx);
        theme.light_theme = light;
        theme.dark_theme = dark;
    }
    let remembered = settings_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|s| Appearance::from_name(s.trim()))
        .unwrap_or_default();
    cx.set_global(Chosen(remembered));
    let mode = Theme::global(cx).mode;
    Theme::change(mode, None, cx);
}

/// The appearance picked.
pub fn appearance(cx: &App) -> Appearance {
    cx.try_global::<Chosen>().map(|c| c.0).unwrap_or_default()
}

/// Shows the window in the appearance picked (the system's, unless one was picked).
pub fn apply(window: &mut Window, cx: &mut App) {
    match appearance(cx) {
        Appearance::System => Theme::sync_system_appearance(Some(window), cx),
        Appearance::Light => Theme::change(ThemeMode::Light, Some(window), cx),
        Appearance::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
    }
}

/// Picks an appearance, remembers it and shows it.
pub fn choose(appearance: Appearance, window: &mut Window, cx: &mut App) {
    cx.set_global(Chosen(appearance));
    if let Some(file) = settings_file() {
        let saved = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, appearance.name()));
        if let Err(e) = saved {
            eprintln!("cannot remember the appearance in {}: {e}", file.display());
        }
    }
    apply(window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_file_reads() {
        let set: gpui::component::ThemeSet = serde_json::from_str(&theme_set()).unwrap();
        let names: Vec<&str> = set.themes.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(names, [LIGHT_NAME, DARK_NAME]);
        assert!(set.themes[1].mode.is_dark());
    }

    #[test]
    fn colors_keep_their_alpha() {
        assert_eq!(MOCHA.base.hex(255), "#1e1e2e");
        assert_eq!(LATTE.mauve.hex(0x26), "#8839ef26");
    }
}
