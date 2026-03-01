//! Semantic terminal theme system.
//!
//! All terminal colors used by Buddy resolve through this module so runtime
//! theme switching can update the entire UI consistently.

use crossterm::style::Color;
use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

/// Semantic color token used by terminal rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ThemeToken {
    PromptHost,
    PromptSymbol,
    PromptApprovalQuery,
    PromptApprovalCommand,
    PromptApprovalPrivileged,
    PromptApprovalMutation,
    StatusLine,
    ContinuationPrompt,
    AgentLabel,
    ModelName,
    ToolCallGlyph,
    ToolCallName,
    ToolCallArgs,
    ToolResultGlyph,
    ToolResultText,
    TokenLabel,
    TokenValue,
    TokenSession,
    ReasoningLabel,
    ReasoningMeta,
    ActivityText,
    Warning,
    Error,
    SectionBullet,
    SectionTitle,
    FieldKey,
    FieldValue,
    ProgressFrame,
    ProgressLabel,
    ProgressElapsed,
    AutocompleteSelected,
    AutocompleteUnselected,
    AutocompleteCommand,
    AutocompleteDescription,
    BlockToolBg,
    BlockToolText,
    BlockReasoningBg,
    BlockReasoningText,
    BlockApprovalBg,
    BlockApprovalText,
    BlockAssistantBg,
    BlockAssistantText,
    BlockTruncated,
    MarkdownHeading,
    MarkdownMarker,
    MarkdownQuote,
    MarkdownCode,
    StartupBuddy,
    StartupTarget,
    StartupModel,
    StartupAttach,
    RiskLow,
    RiskMedium,
    RiskHigh,
}

impl ThemeToken {
    /// Stable config key for this token (used by `[themes.<name>]` overrides).
    pub fn key(self) -> &'static str {
        match self {
            Self::PromptHost => "prompt_host",
            Self::PromptSymbol => "prompt_symbol",
            Self::PromptApprovalQuery => "prompt_approval_query",
            Self::PromptApprovalCommand => "prompt_approval_command",
            Self::PromptApprovalPrivileged => "prompt_approval_privileged",
            Self::PromptApprovalMutation => "prompt_approval_mutation",
            Self::StatusLine => "status_line",
            Self::ContinuationPrompt => "continuation_prompt",
            Self::AgentLabel => "agent_label",
            Self::ModelName => "model_name",
            Self::ToolCallGlyph => "tool_call_glyph",
            Self::ToolCallName => "tool_call_name",
            Self::ToolCallArgs => "tool_call_args",
            Self::ToolResultGlyph => "tool_result_glyph",
            Self::ToolResultText => "tool_result_text",
            Self::TokenLabel => "token_label",
            Self::TokenValue => "token_value",
            Self::TokenSession => "token_session",
            Self::ReasoningLabel => "reasoning_label",
            Self::ReasoningMeta => "reasoning_meta",
            Self::ActivityText => "activity_text",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::SectionBullet => "section_bullet",
            Self::SectionTitle => "section_title",
            Self::FieldKey => "field_key",
            Self::FieldValue => "field_value",
            Self::ProgressFrame => "progress_frame",
            Self::ProgressLabel => "progress_label",
            Self::ProgressElapsed => "progress_elapsed",
            Self::AutocompleteSelected => "autocomplete_selected",
            Self::AutocompleteUnselected => "autocomplete_unselected",
            Self::AutocompleteCommand => "autocomplete_command",
            Self::AutocompleteDescription => "autocomplete_description",
            Self::BlockToolBg => "block_tool_bg",
            Self::BlockToolText => "block_tool_text",
            Self::BlockReasoningBg => "block_reasoning_bg",
            Self::BlockReasoningText => "block_reasoning_text",
            Self::BlockApprovalBg => "block_approval_bg",
            Self::BlockApprovalText => "block_approval_text",
            Self::BlockAssistantBg => "block_assistant_bg",
            Self::BlockAssistantText => "block_assistant_text",
            Self::BlockTruncated => "block_truncated",
            Self::MarkdownHeading => "markdown_heading",
            Self::MarkdownMarker => "markdown_marker",
            Self::MarkdownQuote => "markdown_quote",
            Self::MarkdownCode => "markdown_code",
            Self::StartupBuddy => "startup_buddy",
            Self::StartupTarget => "startup_target",
            Self::StartupModel => "startup_model",
            Self::StartupAttach => "startup_attach",
            Self::RiskLow => "risk_low",
            Self::RiskMedium => "risk_medium",
            Self::RiskHigh => "risk_high",
        }
    }

    fn all() -> &'static [ThemeToken] {
        &[
            Self::PromptHost,
            Self::PromptSymbol,
            Self::PromptApprovalQuery,
            Self::PromptApprovalCommand,
            Self::PromptApprovalPrivileged,
            Self::PromptApprovalMutation,
            Self::StatusLine,
            Self::ContinuationPrompt,
            Self::AgentLabel,
            Self::ModelName,
            Self::ToolCallGlyph,
            Self::ToolCallName,
            Self::ToolCallArgs,
            Self::ToolResultGlyph,
            Self::ToolResultText,
            Self::TokenLabel,
            Self::TokenValue,
            Self::TokenSession,
            Self::ReasoningLabel,
            Self::ReasoningMeta,
            Self::ActivityText,
            Self::Warning,
            Self::Error,
            Self::SectionBullet,
            Self::SectionTitle,
            Self::FieldKey,
            Self::FieldValue,
            Self::ProgressFrame,
            Self::ProgressLabel,
            Self::ProgressElapsed,
            Self::AutocompleteSelected,
            Self::AutocompleteUnselected,
            Self::AutocompleteCommand,
            Self::AutocompleteDescription,
            Self::BlockToolBg,
            Self::BlockToolText,
            Self::BlockReasoningBg,
            Self::BlockReasoningText,
            Self::BlockApprovalBg,
            Self::BlockApprovalText,
            Self::BlockAssistantBg,
            Self::BlockAssistantText,
            Self::BlockTruncated,
            Self::MarkdownHeading,
            Self::MarkdownMarker,
            Self::MarkdownQuote,
            Self::MarkdownCode,
            Self::StartupBuddy,
            Self::StartupTarget,
            Self::StartupModel,
            Self::StartupAttach,
            Self::RiskLow,
            Self::RiskMedium,
            Self::RiskHigh,
        ]
    }
}

/// Named theme resolved by token.
#[derive(Debug, Clone)]
pub struct Theme {
    /// User-facing theme name (`dark`, `light`, or custom key).
    pub name: String,
    colors: BTreeMap<ThemeToken, Color>,
}

impl Theme {
    /// Resolve a color for this theme.
    pub fn color(&self, token: ThemeToken) -> Color {
        self.colors
            .get(&token)
            .copied()
            .unwrap_or_else(|| dark_theme().color(token))
    }
}

/// Theme registry with built-ins and optional custom overrides.
#[derive(Debug, Clone, Default)]
pub struct ThemeRegistry {
    themes: BTreeMap<String, Theme>,
}

impl ThemeRegistry {
    /// Build registry from built-ins plus custom `[themes.<name>]` overrides.
    pub fn from_overrides(overrides: &BTreeMap<String, BTreeMap<String, String>>) -> Self {
        let mut themes = BTreeMap::new();
        let dark = dark_theme();
        let light = light_theme();
        themes.insert(dark.name.clone(), dark);
        themes.insert(light.name.clone(), light);

        for (name, values) in overrides {
            let normalized_name = normalize_theme_name(name);
            if normalized_name.is_empty() {
                continue;
            }
            let base = themes
                .get(&normalized_name)
                .cloned()
                .unwrap_or_else(|| dark_theme_named(&normalized_name));
            if let Ok(custom) = apply_theme_overrides(base, values) {
                themes.insert(normalized_name, custom);
            }
        }

        Self { themes }
    }

    /// Stable ordered names.
    pub fn names(&self) -> Vec<String> {
        self.themes.keys().cloned().collect()
    }

    /// Resolve one theme by name.
    pub fn get(&self, name: &str) -> Option<&Theme> {
        self.themes.get(&normalize_theme_name(name))
    }
}

#[derive(Debug)]
struct ThemeState {
    registry: ThemeRegistry,
    active: String,
}

fn theme_state() -> &'static RwLock<ThemeState> {
    static STATE: OnceLock<RwLock<ThemeState>> = OnceLock::new();
    STATE.get_or_init(|| {
        let mut themes = BTreeMap::new();
        let dark = dark_theme();
        themes.insert(dark.name.clone(), dark);
        RwLock::new(ThemeState {
            registry: ThemeRegistry { themes },
            active: "dark".to_string(),
        })
    })
}

/// Initialize global theme registry and active theme.
pub fn initialize(
    active: &str,
    custom_overrides: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<(), String> {
    let mut state = theme_state()
        .write()
        .map_err(|_| "theme state lock poisoned".to_string())?;
    state.registry = ThemeRegistry::from_overrides(custom_overrides);
    state.active = normalize_theme_name(active);
    if !state.registry.themes.contains_key(&state.active) {
        state.active = "dark".to_string();
    }
    Ok(())
}

/// Active theme name.
pub fn active_theme_name() -> String {
    theme_state()
        .read()
        .ok()
        .map(|state| state.active.clone())
        .unwrap_or_else(|| "dark".to_string())
}

/// Available theme names.
pub fn available_theme_names() -> Vec<String> {
    theme_state()
        .read()
        .ok()
        .map(|state| state.registry.names())
        .unwrap_or_else(|| vec!["dark".to_string()])
}

/// Switch active theme.
pub fn set_active_theme(name: &str) -> Result<(), String> {
    let mut state = theme_state()
        .write()
        .map_err(|_| "theme state lock poisoned".to_string())?;
    let normalized = normalize_theme_name(name);
    if !state.registry.themes.contains_key(&normalized) {
        let available = state.registry.names().join(", ");
        return Err(format!(
            "unknown theme `{name}`. Available themes: {available}"
        ));
    }
    state.active = normalized;
    Ok(())
}

/// Resolve one token from active theme.
pub fn color(token: ThemeToken) -> Color {
    theme_state()
        .read()
        .ok()
        .and_then(|state| {
            state
                .registry
                .get(&state.active)
                .map(|theme| theme.color(token))
        })
        .unwrap_or_else(|| dark_theme().color(token))
}

/// Resolve one token as RGB tuple.
pub fn rgb(token: ThemeToken) -> (u8, u8, u8) {
    to_rgb(color(token))
}

fn apply_theme_overrides(
    base: Theme,
    overrides: &BTreeMap<String, String>,
) -> Result<Theme, String> {
    let mut colors = base.colors;
    for (key, value) in overrides {
        let Some(token) = token_from_key(key) else {
            continue;
        };
        colors.insert(token, parse_color(value)?);
    }
    Ok(Theme {
        name: base.name,
        colors,
    })
}

fn token_from_key(key: &str) -> Option<ThemeToken> {
    let normalized = key.trim().to_ascii_lowercase();
    ThemeToken::all()
        .iter()
        .copied()
        .find(|token| token.key() == normalized)
}

fn normalize_theme_name(name: &str) -> String {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        "dark".to_string()
    } else {
        normalized
    }
}

fn dark_theme_named(name: &str) -> Theme {
    Theme {
        name: name.to_string(),
        colors: dark_colors(),
    }
}

fn dark_theme() -> Theme {
    dark_theme_named("dark")
}

fn light_theme() -> Theme {
    Theme {
        name: "light".to_string(),
        colors: light_colors(),
    }
}

fn dark_colors() -> BTreeMap<ThemeToken, Color> {
    let base00 = rgb_color(0x65, 0x7b, 0x83);
    let base0 = rgb_color(0x83, 0x94, 0x96);
    let base1 = rgb_color(0x93, 0xa1, 0xa1);
    let base2 = rgb_color(0xee, 0xe8, 0xd5);
    let yellow = rgb_color(0xb5, 0x89, 0x00);
    let orange = rgb_color(0xcb, 0x4b, 0x16);
    let red = rgb_color(0xdc, 0x32, 0x2f);
    let magenta = rgb_color(0xd3, 0x36, 0x82);
    let blue = rgb_color(0x26, 0x8b, 0xd2);
    let cyan = rgb_color(0x2a, 0xa1, 0x98);
    let green = rgb_color(0x85, 0x99, 0x00);

    let mut map = BTreeMap::new();
    map.insert(ThemeToken::PromptHost, base1);
    map.insert(ThemeToken::PromptSymbol, base2);
    map.insert(ThemeToken::PromptApprovalQuery, yellow);
    map.insert(ThemeToken::PromptApprovalCommand, base2);
    map.insert(ThemeToken::PromptApprovalPrivileged, red);
    map.insert(ThemeToken::PromptApprovalMutation, yellow);
    map.insert(ThemeToken::StatusLine, base00);
    map.insert(ThemeToken::ContinuationPrompt, base00);
    map.insert(ThemeToken::AgentLabel, green);
    map.insert(ThemeToken::ModelName, yellow);
    map.insert(ThemeToken::ToolCallGlyph, orange);
    map.insert(ThemeToken::ToolCallName, yellow);
    map.insert(ThemeToken::ToolCallArgs, base00);
    map.insert(ThemeToken::ToolResultGlyph, base00);
    map.insert(ThemeToken::ToolResultText, base1);
    map.insert(ThemeToken::TokenLabel, base00);
    map.insert(ThemeToken::TokenValue, cyan);
    map.insert(ThemeToken::TokenSession, blue);
    map.insert(ThemeToken::ReasoningLabel, magenta);
    map.insert(ThemeToken::ReasoningMeta, base00);
    map.insert(ThemeToken::ActivityText, base1);
    map.insert(ThemeToken::Warning, yellow);
    map.insert(ThemeToken::Error, red);
    map.insert(ThemeToken::SectionBullet, base00);
    map.insert(ThemeToken::SectionTitle, cyan);
    map.insert(ThemeToken::FieldKey, base00);
    map.insert(ThemeToken::FieldValue, base2);
    map.insert(ThemeToken::ProgressFrame, cyan);
    map.insert(ThemeToken::ProgressLabel, base1);
    map.insert(ThemeToken::ProgressElapsed, base00);
    map.insert(ThemeToken::AutocompleteSelected, orange);
    map.insert(ThemeToken::AutocompleteUnselected, base00);
    map.insert(ThemeToken::AutocompleteCommand, yellow);
    map.insert(ThemeToken::AutocompleteDescription, base00);
    map.insert(ThemeToken::BlockToolBg, rgb_color(0x12, 0x35, 0x2d));
    map.insert(ThemeToken::BlockToolText, base2);
    map.insert(ThemeToken::BlockReasoningBg, rgb_color(0x10, 0x30, 0x2a));
    map.insert(ThemeToken::BlockReasoningText, base1);
    map.insert(ThemeToken::BlockApprovalBg, rgb_color(0x3a, 0x1a, 0x1a));
    map.insert(ThemeToken::BlockApprovalText, rgb_color(0xf4, 0xd0, 0xd0));
    map.insert(ThemeToken::BlockAssistantBg, rgb_color(0x12, 0x35, 0x2d));
    map.insert(ThemeToken::BlockAssistantText, base2);
    map.insert(ThemeToken::BlockTruncated, base0);
    map.insert(ThemeToken::MarkdownHeading, rgb_color(0xc8, 0xe4, 0xbc));
    map.insert(ThemeToken::MarkdownMarker, rgb_color(0xa1, 0xc5, 0xa8));
    map.insert(ThemeToken::MarkdownQuote, rgb_color(0xb8, 0xd2, 0xc4));
    map.insert(ThemeToken::MarkdownCode, rgb_color(0xee, 0xe0, 0xbc));
    map.insert(ThemeToken::StartupBuddy, green);
    map.insert(ThemeToken::StartupTarget, base2);
    map.insert(ThemeToken::StartupModel, yellow);
    map.insert(ThemeToken::StartupAttach, base2);
    map.insert(ThemeToken::RiskLow, green);
    map.insert(ThemeToken::RiskMedium, yellow);
    map.insert(ThemeToken::RiskHigh, red);
    map
}

fn light_colors() -> BTreeMap<ThemeToken, Color> {
    let base1 = rgb_color(0x93, 0xa1, 0xa1);
    let base0 = rgb_color(0x83, 0x94, 0x96);
    let base00 = rgb_color(0x65, 0x7b, 0x83);
    let base01 = rgb_color(0x58, 0x6e, 0x75);
    let base02 = rgb_color(0x07, 0x36, 0x42);
    let yellow = rgb_color(0xb5, 0x89, 0x00);
    let orange = rgb_color(0xcb, 0x4b, 0x16);
    let red = rgb_color(0xdc, 0x32, 0x2f);
    let magenta = rgb_color(0xd3, 0x36, 0x82);
    let blue = rgb_color(0x26, 0x8b, 0xd2);
    let cyan = rgb_color(0x2a, 0xa1, 0x98);
    let green = rgb_color(0x85, 0x99, 0x00);

    let mut map = BTreeMap::new();
    map.insert(ThemeToken::PromptHost, base00);
    map.insert(ThemeToken::PromptSymbol, base02);
    map.insert(ThemeToken::PromptApprovalQuery, yellow);
    map.insert(ThemeToken::PromptApprovalCommand, base02);
    map.insert(ThemeToken::PromptApprovalPrivileged, red);
    map.insert(ThemeToken::PromptApprovalMutation, orange);
    map.insert(ThemeToken::StatusLine, base0);
    map.insert(ThemeToken::ContinuationPrompt, base0);
    map.insert(ThemeToken::AgentLabel, green);
    map.insert(ThemeToken::ModelName, yellow);
    map.insert(ThemeToken::ToolCallGlyph, orange);
    map.insert(ThemeToken::ToolCallName, yellow);
    map.insert(ThemeToken::ToolCallArgs, base0);
    map.insert(ThemeToken::ToolResultGlyph, base0);
    map.insert(ThemeToken::ToolResultText, base00);
    map.insert(ThemeToken::TokenLabel, base0);
    map.insert(ThemeToken::TokenValue, cyan);
    map.insert(ThemeToken::TokenSession, blue);
    map.insert(ThemeToken::ReasoningLabel, magenta);
    map.insert(ThemeToken::ReasoningMeta, base0);
    map.insert(ThemeToken::ActivityText, base00);
    map.insert(ThemeToken::Warning, yellow);
    map.insert(ThemeToken::Error, red);
    map.insert(ThemeToken::SectionBullet, base0);
    map.insert(ThemeToken::SectionTitle, cyan);
    map.insert(ThemeToken::FieldKey, base0);
    map.insert(ThemeToken::FieldValue, base02);
    map.insert(ThemeToken::ProgressFrame, cyan);
    map.insert(ThemeToken::ProgressLabel, base00);
    map.insert(ThemeToken::ProgressElapsed, base0);
    map.insert(ThemeToken::AutocompleteSelected, orange);
    map.insert(ThemeToken::AutocompleteUnselected, base0);
    map.insert(ThemeToken::AutocompleteCommand, blue);
    map.insert(ThemeToken::AutocompleteDescription, base0);
    map.insert(ThemeToken::BlockToolBg, rgb_color(0xf2, 0xf4, 0xe9));
    map.insert(ThemeToken::BlockToolText, base02);
    map.insert(ThemeToken::BlockReasoningBg, rgb_color(0xea, 0xef, 0xe7));
    map.insert(ThemeToken::BlockReasoningText, base01);
    map.insert(ThemeToken::BlockApprovalBg, rgb_color(0xfb, 0xea, 0xe8));
    map.insert(ThemeToken::BlockApprovalText, base02);
    map.insert(ThemeToken::BlockAssistantBg, rgb_color(0xf2, 0xf4, 0xe9));
    map.insert(ThemeToken::BlockAssistantText, base02);
    map.insert(ThemeToken::BlockTruncated, base1);
    map.insert(ThemeToken::MarkdownHeading, green);
    map.insert(ThemeToken::MarkdownMarker, cyan);
    map.insert(ThemeToken::MarkdownQuote, blue);
    map.insert(ThemeToken::MarkdownCode, orange);
    map.insert(ThemeToken::StartupBuddy, green);
    map.insert(ThemeToken::StartupTarget, base02);
    map.insert(ThemeToken::StartupModel, yellow);
    map.insert(ThemeToken::StartupAttach, base02);
    map.insert(ThemeToken::RiskLow, green);
    map.insert(ThemeToken::RiskMedium, yellow);
    map.insert(ThemeToken::RiskHigh, red);
    map
}

fn parse_color(input: &str) -> Result<Color, String> {
    let normalized = input.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err("theme color value cannot be empty".to_string());
    }
    if let Some(hex) = normalized.strip_prefix('#') {
        if hex.len() != 6 {
            return Err(format!("invalid hex color `{input}` (expected #RRGGBB)"));
        }
        let r = u8::from_str_radix(&hex[0..2], 16)
            .map_err(|_| format!("invalid hex color `{input}`"))?;
        let g = u8::from_str_radix(&hex[2..4], 16)
            .map_err(|_| format!("invalid hex color `{input}`"))?;
        let b = u8::from_str_radix(&hex[4..6], 16)
            .map_err(|_| format!("invalid hex color `{input}`"))?;
        return Ok(Color::Rgb { r, g, b });
    }

    let color = match normalized.as_str() {
        "black" => Color::Black,
        "darkgrey" | "dark-gray" | "dark_grey" => Color::DarkGrey,
        "grey" | "gray" => Color::Grey,
        "white" => Color::White,
        "red" => Color::Red,
        "darkred" | "dark-red" => Color::DarkRed,
        "green" => Color::Green,
        "darkgreen" | "dark-green" => Color::DarkGreen,
        "yellow" => Color::Yellow,
        "darkyellow" | "dark-yellow" => Color::DarkYellow,
        "blue" => Color::Blue,
        "darkblue" | "dark-blue" => Color::DarkBlue,
        "magenta" => Color::Magenta,
        "darkmagenta" | "dark-magenta" => Color::DarkMagenta,
        "cyan" => Color::Cyan,
        "darkcyan" | "dark-cyan" => Color::DarkCyan,
        _ => return Err(format!("unsupported color value `{input}`")),
    };
    Ok(color)
}

fn rgb_color(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb { r, g, b }
}

fn to_rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Black => (0, 0, 0),
        Color::DarkGrey => (85, 85, 85),
        Color::Grey => (170, 170, 170),
        Color::White => (255, 255, 255),
        Color::Red => (255, 0, 0),
        Color::DarkRed => (128, 0, 0),
        Color::Green => (0, 255, 0),
        Color::DarkGreen => (0, 128, 0),
        Color::Yellow => (255, 255, 0),
        Color::DarkYellow => (128, 128, 0),
        Color::Blue => (0, 0, 255),
        Color::DarkBlue => (0, 0, 128),
        Color::Magenta => (255, 0, 255),
        Color::DarkMagenta => (128, 0, 128),
        Color::Cyan => (0, 255, 255),
        Color::DarkCyan => (0, 128, 128),
        Color::Rgb { r, g, b } => (r, g, b),
        Color::AnsiValue(value) => {
            // Simple fallback for 256-color variants.
            (value, value, value)
        }
        Color::Reset => (255, 255, 255),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_builtin_themes() {
        let registry = ThemeRegistry::from_overrides(&BTreeMap::new());
        let names = registry.names();
        assert!(names.contains(&"dark".to_string()));
        assert!(names.contains(&"light".to_string()));
    }

    #[test]
    fn custom_override_applies() {
        let mut overrides = BTreeMap::new();
        let mut custom = BTreeMap::new();
        custom.insert("warning".to_string(), "#aabbcc".to_string());
        overrides.insert("custom".to_string(), custom);
        let registry = ThemeRegistry::from_overrides(&overrides);
        let custom_theme = registry.get("custom").expect("custom theme should exist");
        assert_eq!(
            custom_theme.color(ThemeToken::Warning),
            Color::Rgb {
                r: 0xaa,
                g: 0xbb,
                b: 0xcc
            }
        );
    }

    #[test]
    fn initialize_unknown_theme_falls_back_to_dark() {
        initialize("unknown", &BTreeMap::new()).expect("init");
        assert_eq!(active_theme_name(), "dark");
    }

    #[test]
    fn set_active_theme_rejects_unknown_name() {
        initialize("dark", &BTreeMap::new()).expect("init");
        let err = set_active_theme("missing").expect_err("must reject");
        assert!(err.contains("unknown theme"));
    }

    #[test]
    fn parse_color_supports_hex_and_names() {
        assert_eq!(
            parse_color("#010203").expect("hex"),
            Color::Rgb { r: 1, g: 2, b: 3 }
        );
        assert_eq!(parse_color("yellow").expect("named"), Color::Yellow);
    }
}
