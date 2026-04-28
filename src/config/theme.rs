use anyhow::{Context, Result};
use egui::Color32;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const THEME_FILE: &str = "theme.toml";

/// A complete UI theme that can be serialized to a TOML file for easy sharing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    #[serde(default)]
    pub builtin: bool,
    pub colors: ThemeColors,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeColors {
    pub bg_primary: [u8; 3],
    pub bg_secondary: [u8; 3],
    pub bg_tertiary: [u8; 3],
    pub text_primary: [u8; 3],
    pub text_secondary: [u8; 3],
    pub accent: [u8; 3],
    pub accent_hover: [u8; 3],
    pub border: [u8; 3],
    pub success: [u8; 3],
    pub warning: [u8; 3],
    pub error: [u8; 3],
    pub selection_bg: [u8; 3],
    pub selection_text: [u8; 3],
    pub tab_bar_bg: [u8; 3],
    pub sidebar_bg: [u8; 3],
    pub status_bar_bg: [u8; 3],
}

impl ThemeColors {
    pub fn color32(&self, field: &str) -> Color32 {
        let c = match field {
            "bg_primary" => self.bg_primary,
            "bg_secondary" => self.bg_secondary,
            "bg_tertiary" => self.bg_tertiary,
            "text_primary" => self.text_primary,
            "text_secondary" => self.text_secondary,
            "accent" => self.accent,
            "accent_hover" => self.accent_hover,
            "border" => self.border,
            "success" => self.success,
            "warning" => self.warning,
            "error" => self.error,
            "selection_bg" => self.selection_bg,
            "selection_text" => self.selection_text,
            "tab_bar_bg" => self.tab_bar_bg,
            "sidebar_bg" => self.sidebar_bg,
            "status_bar_bg" => self.status_bar_bg,
            _ => [128, 128, 128],
        };
        Color32::from_rgb(c[0], c[1], c[2])
    }
}

impl Default for Theme {
    fn default() -> Self {
        dark_theme()
    }
}

/// Dark theme — the default.
pub fn dark_theme() -> Theme {
    Theme {
        name: "Dark".to_string(),
        builtin: true,
        colors: ThemeColors {
            bg_primary: [30, 30, 30],
            bg_secondary: [40, 40, 40],
            bg_tertiary: [50, 50, 50],
            text_primary: [220, 220, 220],
            text_secondary: [150, 150, 150],
            accent: [80, 140, 220],
            accent_hover: [100, 160, 240],
            border: [65, 65, 65],
            success: [80, 180, 100],
            warning: [220, 180, 60],
            error: [210, 90, 90],
            selection_bg: [60, 100, 160],
            selection_text: [230, 230, 230],
            tab_bar_bg: [35, 35, 35],
            sidebar_bg: [28, 28, 28],
            status_bar_bg: [25, 25, 25],
        },
    }
}

/// Light theme.
pub fn light_theme() -> Theme {
    Theme {
        name: "Light".to_string(),
        builtin: true,
        colors: ThemeColors {
            bg_primary: [245, 245, 245],
            bg_secondary: [235, 235, 235],
            bg_tertiary: [225, 225, 225],
            text_primary: [30, 30, 30],
            text_secondary: [100, 100, 100],
            accent: [50, 110, 200],
            accent_hover: [70, 130, 220],
            border: [200, 200, 200],
            success: [50, 150, 70],
            warning: [200, 150, 30],
            error: [200, 60, 60],
            selection_bg: [180, 210, 250],
            selection_text: [20, 20, 20],
            tab_bar_bg: [240, 240, 240],
            sidebar_bg: [248, 248, 248],
            status_bar_bg: [230, 230, 230],
        },
    }
}

/// Nord-inspired dark theme.
pub fn nord_theme() -> Theme {
    Theme {
        name: "Nord".to_string(),
        builtin: true,
        colors: ThemeColors {
            bg_primary: [46, 52, 64],
            bg_secondary: [59, 66, 82],
            bg_tertiary: [67, 76, 94],
            text_primary: [216, 222, 233],
            text_secondary: [143, 152, 168],
            accent: [136, 192, 208],
            accent_hover: [129, 161, 193],
            border: [76, 86, 106],
            success: [163, 190, 140],
            warning: [235, 203, 139],
            error: [191, 97, 106],
            selection_bg: [76, 86, 106],
            selection_text: [229, 233, 240],
            tab_bar_bg: [46, 52, 64],
            sidebar_bg: [41, 46, 56],
            status_bar_bg: [36, 40, 50],
        },
    }
}

/// Solarized Dark theme.
pub fn solarized_theme() -> Theme {
    Theme {
        name: "Solarized".to_string(),
        builtin: true,
        colors: ThemeColors {
            bg_primary: [0, 43, 54],
            bg_secondary: [7, 54, 66],
            bg_tertiary: [20, 65, 75],
            text_primary: [131, 148, 150],
            text_secondary: [88, 110, 117],
            accent: [38, 139, 210],
            accent_hover: [42, 161, 152],
            border: [30, 70, 80],
            success: [133, 153, 0],
            warning: [181, 137, 0],
            error: [220, 50, 47],
            selection_bg: [7, 54, 66],
            selection_text: [147, 161, 161],
            tab_bar_bg: [0, 43, 54],
            sidebar_bg: [0, 38, 48],
            status_bar_bg: [0, 33, 43],
        },
    }
}

/// All built-in themes.
pub fn builtin_themes() -> Vec<Theme> {
    vec![dark_theme(), light_theme(), nord_theme(), solarized_theme()]
}

/// Theme file path inside the config directory.
pub fn theme_path(config_dir: &Path) -> PathBuf {
    config_dir.join(THEME_FILE)
}

/// Load theme from disk, falling back to default.
pub fn load_theme(config_dir: &Path) -> Theme {
    let path = theme_path(config_dir);
    if !path.exists() {
        return Theme::default();
    }
    match load_theme_file(&path) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load theme, using default");
            Theme::default()
        }
    }
}

fn load_theme_file(path: &Path) -> Result<Theme> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let theme: Theme = toml::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(theme)
}

/// Save theme to disk.
pub fn save_theme(config_dir: &Path, theme: &Theme) -> Result<()> {
    std::fs::create_dir_all(config_dir)
        .with_context(|| format!("creating {}", config_dir.display()))?;
    let text = toml::to_string_pretty(theme).context("serializing theme")?;
    std::fs::write(theme_path(config_dir), text)
        .with_context(|| format!("writing theme to {}", config_dir.display()))?;
    Ok(())
}

/// Import a theme from an arbitrary path.
pub fn import_theme(path: &Path) -> Result<Theme> {
    load_theme_file(path)
}

/// Export the current theme to an arbitrary path.
pub fn export_theme(path: &Path, theme: &Theme) -> Result<()> {
    let text = toml::to_string_pretty(theme).context("serializing theme")?;
    std::fs::write(path, text)
        .with_context(|| format!("writing theme to {}", path.display()))?;
    Ok(())
}

/// Apply a Theme to the egui context visuals.
pub fn apply_theme(ctx: &egui::Context, theme: &Theme) {
    let c = &theme.colors;
    let is_dark = c.bg_primary[0] < 128;

    let mut visuals = if is_dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    let bg = Color32::from_rgb(c.bg_primary[0], c.bg_primary[1], c.bg_primary[2]);
    let bg2 = Color32::from_rgb(c.bg_secondary[0], c.bg_secondary[1], c.bg_secondary[2]);
    let bg3 = Color32::from_rgb(c.bg_tertiary[0], c.bg_tertiary[1], c.bg_tertiary[2]);
    let text = Color32::from_rgb(c.text_primary[0], c.text_primary[1], c.text_primary[2]);
    let _text_weak = Color32::from_rgb(c.text_secondary[0], c.text_secondary[1], c.text_secondary[2]);
    let accent = Color32::from_rgb(c.accent[0], c.accent[1], c.accent[2]);
    let accent_hover = Color32::from_rgb(c.accent_hover[0], c.accent_hover[1], c.accent_hover[2]);
    let border = Color32::from_rgb(c.border[0], c.border[1], c.border[2]);
    let sel_bg = Color32::from_rgb(c.selection_bg[0], c.selection_bg[1], c.selection_bg[2]);
    let sel_text = Color32::from_rgb(c.selection_text[0], c.selection_text[1], c.selection_text[2]);

    visuals.override_text_color = Some(text);
    visuals.panel_fill = bg;
    visuals.window_fill = bg2;
    visuals.extreme_bg_color = bg;
    visuals.faint_bg_color = bg3;
    visuals.code_bg_color = bg2;

    visuals.selection.bg_fill = sel_bg;
    visuals.selection.stroke.color = sel_text;

    visuals.widgets.noninteractive.bg_fill = bg2;
    visuals.widgets.noninteractive.weak_bg_fill = bg;
    visuals.widgets.noninteractive.fg_stroke.color = text;
    visuals.widgets.noninteractive.bg_stroke.color = border;

    visuals.widgets.inactive.bg_fill = bg2;
    visuals.widgets.inactive.weak_bg_fill = bg2;
    visuals.widgets.inactive.fg_stroke.color = text;
    visuals.widgets.inactive.bg_stroke.color = border;

    visuals.widgets.hovered.bg_fill = bg3;
    visuals.widgets.hovered.weak_bg_fill = bg3;
    visuals.widgets.hovered.fg_stroke.color = accent_hover;
    visuals.widgets.hovered.bg_stroke.color = accent;

    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.weak_bg_fill = bg3;
    visuals.widgets.active.fg_stroke.color = text;
    visuals.widgets.active.bg_stroke.color = accent;

    visuals.widgets.open.bg_fill = bg3;
    visuals.widgets.open.weak_bg_fill = bg3;
    visuals.widgets.open.fg_stroke.color = text;
    visuals.widgets.open.bg_stroke.color = accent;

    visuals.hyperlink_color = accent;
    visuals.warn_fg_color = Color32::from_rgb(c.warning[0], c.warning[1], c.warning[2]);
    visuals.error_fg_color = Color32::from_rgb(c.error[0], c.error[1], c.error[2]);

    visuals.window_stroke.color = border;

    ctx.set_visuals(visuals);
}
