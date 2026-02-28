//! Crude token tracking and context window management.
//!
//! Tracks exact counts from the API's `usage` field when available,
//! and provides a rough estimation heuristic (~1 token per 4 chars)
//! for pre-flight context limit checks.

use crate::types::Message;
use serde::Deserialize;
use serde_json::Value;
use std::sync::OnceLock;

/// Tracks token usage across a conversation session.
#[derive(Debug, Clone)]
pub struct TokenTracker {
    /// Model's context window size in tokens.
    pub context_limit: usize,
    /// Running total of prompt tokens sent.
    pub total_prompt_tokens: u64,
    /// Running total of completion tokens received.
    pub total_completion_tokens: u64,
    /// Prompt tokens in the most recent request.
    pub last_prompt_tokens: u64,
    /// Completion tokens in the most recent response.
    pub last_completion_tokens: u64,
}

impl TokenTracker {
    /// Create a fresh tracker for a model with the provided context limit.
    pub fn new(context_limit: usize) -> Self {
        Self {
            context_limit,
            total_prompt_tokens: 0,
            total_completion_tokens: 0,
            last_prompt_tokens: 0,
            last_completion_tokens: 0,
        }
    }

    /// Record token counts from an API response's `usage` field.
    pub fn record(&mut self, prompt_tokens: u64, completion_tokens: u64) {
        self.last_prompt_tokens = prompt_tokens;
        self.last_completion_tokens = completion_tokens;
        self.total_prompt_tokens = self.total_prompt_tokens.saturating_add(prompt_tokens);
        self.total_completion_tokens = self
            .total_completion_tokens
            .saturating_add(completion_tokens);
    }

    /// Estimate how many tokens a set of messages would consume.
    ///
    /// Crude heuristic: ~1 token per 4 characters, plus overhead per message.
    pub fn estimate_messages(messages: &[Message]) -> usize {
        let mut chars = 0usize;
        for msg in messages {
            // Per-message overhead (~4 tokens for role + framing).
            chars += 16;
            if let Some(content) = &msg.content {
                chars += content.len();
            }
            if let Some(tool_calls) = &msg.tool_calls {
                for tc in tool_calls {
                    chars += tc.function.name.len();
                    chars += tc.function.arguments.len();
                }
            }
            for value in msg.extra.values() {
                chars += json_value_char_count(value);
            }
        }
        chars / 4
    }

    /// Fraction of context window estimated to be used by these messages.
    pub fn usage_fraction(&self, messages: &[Message]) -> f64 {
        if self.context_limit == 0 {
            return 0.0;
        }
        Self::estimate_messages(messages) as f64 / self.context_limit as f64
    }

    /// True if estimated usage exceeds 80% of the context window.
    pub fn is_approaching_limit(&self, messages: &[Message]) -> bool {
        self.usage_fraction(messages) > 0.8
    }

    /// Total tokens consumed across the entire session.
    pub fn session_total(&self) -> u64 {
        self.total_prompt_tokens
            .saturating_add(self.total_completion_tokens)
    }
}

/// Estimate string-equivalent character footprint of arbitrary JSON values.
fn json_value_char_count(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(_) => 5,
        Value::Number(n) => n.to_string().len(),
        Value::String(s) => s.len(),
        Value::Array(items) => items.iter().map(json_value_char_count).sum(),
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| k.len() + json_value_char_count(v))
            .sum(),
    }
}

// ---------------------------------------------------------------------------
// Context limit defaults
// ---------------------------------------------------------------------------

/// Catalog entry matching strategy.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ModelMatchKind {
    /// Rule matches when candidate model name equals pattern.
    Exact,
    /// Rule matches when candidate model name starts with pattern.
    Prefix,
    /// Rule matches when candidate model name contains pattern.
    Contains,
}

/// A single context-window match rule from `models.toml`.
#[derive(Debug, Clone, Deserialize)]
struct ModelContextRule {
    /// Matching strategy used for this rule.
    #[serde(rename = "match")]
    kind: ModelMatchKind,
    /// Pattern compared against normalized model IDs.
    pattern: String,
    /// Context window applied when the rule matches.
    context_window: usize,
}

/// Embedded model context catalog loaded from `templates/models.toml`.
#[derive(Debug, Clone, Deserialize)]
struct ModelCatalog {
    /// Fallback context window when no explicit rule matches.
    #[serde(default = "default_unknown_context_limit")]
    default_context_window: usize,
    /// Ordered rule list (first match wins).
    #[serde(default)]
    rule: Vec<ModelContextRule>,
}

impl ModelCatalog {
    /// Find the first matching rule for the given model id.
    fn lookup(&self, model: &str) -> Option<usize> {
        let normalized = normalize_model_name(model);
        if normalized.is_empty() {
            return None;
        }

        // Match both full provider/model IDs and provider-stripped tails so
        // catalog rules can stay concise.
        let mut candidates = vec![normalized.clone()];
        if let Some((_, tail)) = normalized.rsplit_once('/') {
            if !tail.is_empty() && tail != normalized {
                candidates.push(tail.to_string());
            }
        }

        self.rule.iter().find_map(|rule| {
            let pattern = normalize_model_name(&rule.pattern);
            if pattern.is_empty() {
                return None;
            }
            let matched = match rule.kind {
                ModelMatchKind::Exact => candidates.iter().any(|c| c == &pattern),
                ModelMatchKind::Prefix => candidates.iter().any(|c| c.starts_with(&pattern)),
                ModelMatchKind::Contains => candidates.iter().any(|c| c.contains(&pattern)),
            };
            matched.then_some(rule.context_window)
        })
    }
}

/// Parsed once at runtime from the embedded `templates/models.toml`.
static MODEL_CATALOG: OnceLock<Option<ModelCatalog>> = OnceLock::new();

/// Parse and cache the embedded model catalog once per process.
fn model_catalog() -> Option<&'static ModelCatalog> {
    MODEL_CATALOG
        .get_or_init(|| toml::from_str(include_str!("templates/models.toml")).ok())
        .as_ref()
}

/// Normalize a model id so matching is case-insensitive and robust to
/// OpenRouter variants like `:free` or `:exacto`.
fn normalize_model_name(model: &str) -> String {
    let m = model.trim().to_lowercase();
    match m.split_once(':') {
        // Drop provider-specific suffixes like `:free` so rules do not need
        // duplicate entries for every suffix variant.
        Some((base, _)) => base.trim().to_string(),
        None => m,
    }
}

/// Default context window when no catalog signal exists.
fn default_unknown_context_limit() -> usize {
    8_192
}

/// Legacy fallback heuristics used if `models.toml` fails to parse.
fn legacy_default_context_limit(model: &str) -> usize {
    let m = normalize_model_name(model);
    match () {
        _ if m.starts_with("gpt-5") => 400_000,
        _ if m.starts_with("gpt-4.1") => 1_047_576,
        _ if m.starts_with("gpt-4o") => 128_000,
        _ if m.starts_with("gpt-4-turbo") => 128_000,
        _ if m.starts_with("gpt-4") => 8_192,
        _ if m.starts_with("gpt-3.5") => 16_385,
        _ if m.starts_with("o1")
            || m.starts_with("o3")
            || m.starts_with("o4")
            || m.starts_with("openai/o1")
            || m.starts_with("openai/o3")
            || m.starts_with("openai/o4") =>
        {
            200_000
        }
        _ if m.contains("claude") => 200_000,
        _ if m.contains("gemini") => 1_048_576,
        _ if m.contains("kimi-k2.5") || m.contains("kimi-k2-thinking") => 262_144,
        _ if m.contains("kimi") => 131_072,
        _ if m.contains("llama") => 8_192,
        _ if m.contains("mistral") => 32_768,
        _ if m.contains("qwen") => 32_768,
        _ if m.contains("gemma") => 8_192,
        _ if m.contains("deepseek") => 64_000,
        _ => default_unknown_context_limit(),
    }
}

/// Best-effort context limit lookup.
///
/// Prefers explicit rules from `models.toml` and falls back to conservative
/// built-in heuristics if the catalog is unavailable.
///
/// Can always be overridden via `config.api.context_limit`.
pub fn default_context_limit(model: &str) -> usize {
    if let Some(catalog) = model_catalog() {
        if let Some(context_window) = catalog.lookup(model) {
            return context_window;
        }
        return catalog.default_context_window;
    }
    legacy_default_context_limit(model)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Ensures built-in catalog parsing works and key model mappings stay stable.
    #[test]
    fn context_limit_lookup_uses_catalog_rules() {
        assert!(model_catalog().is_some());
        assert_eq!(default_context_limit("gpt-4o"), 128_000);
        assert_eq!(default_context_limit("openai/gpt-4o"), 128_000);
        assert_eq!(default_context_limit("openai/gpt-4o:extended"), 128_000);
        assert_eq!(default_context_limit("gpt-4.1-mini"), 1_047_576);
        assert_eq!(default_context_limit("moonshotai/kimi-k2.5"), 262_144);
        assert_eq!(
            default_context_limit("anthropic/claude-opus-4.6"),
            1_000_000
        );
        assert_eq!(default_context_limit("gpt-4o-mini"), 128_000);
        assert_eq!(default_context_limit("gpt-4"), 8_191);
        assert_eq!(default_context_limit("claude-3-sonnet"), 200_000);
        assert_eq!(default_context_limit("llama3.2:1b"), 8_192);
        assert_eq!(default_context_limit("unknown-model"), 8_192);
    }

    // Ensures exact/prefix/contains match modes all resolve as expected.
    #[test]
    fn catalog_rule_matching_variants() {
        let catalog: ModelCatalog = toml::from_str(
            r#"
            default_context_window = 42

            [[rule]]
            match = "exact"
            pattern = "foo-model"
            context_window = 100

            [[rule]]
            match = "prefix"
            pattern = "bar-"
            context_window = 200

            [[rule]]
            match = "contains"
            pattern = "baz"
            context_window = 300
            "#,
        )
        .unwrap();

        assert_eq!(catalog.lookup("foo-model"), Some(100));
        assert_eq!(catalog.lookup("provider/foo-model"), Some(100));
        assert_eq!(catalog.lookup("bar-v2"), Some(200));
        assert_eq!(catalog.lookup("provider/bar-v2"), Some(200));
        assert_eq!(catalog.lookup("something-baz-here"), Some(300));
        assert_eq!(catalog.lookup("no-match"), None);
    }

    // Ensures heuristic estimation produces plausible non-zero token counts.
    #[test]
    fn estimate_messages_basic() {
        let msgs = vec![
            Message::system("You are helpful."),
            Message::user("Hello world"),
        ];
        let est = TokenTracker::estimate_messages(&msgs);
        // (16 + 16) overhead + (16 + 11) content = 59 chars / 4 = ~14
        assert!(est > 0);
        assert!(est < 100);
    }

    // Ensures per-call and running totals are both updated by `record`.
    #[test]
    fn tracker_record() {
        let mut t = TokenTracker::new(1000);
        t.record(50, 20);
        assert_eq!(t.session_total(), 70);
        t.record(100, 30);
        assert_eq!(t.session_total(), 200);
        assert_eq!(t.last_prompt_tokens, 100);
        assert_eq!(t.last_completion_tokens, 30);
    }

    // Ensures arithmetic uses saturation to avoid u64 overflow panics/wrap.
    #[test]
    fn tracker_record_saturates_totals() {
        let mut t = TokenTracker::new(1000);
        t.total_prompt_tokens = u64::MAX - 3;
        t.total_completion_tokens = u64::MAX - 2;
        t.record(10, 10);
        assert_eq!(t.total_prompt_tokens, u64::MAX);
        assert_eq!(t.total_completion_tokens, u64::MAX);
        assert_eq!(t.session_total(), u64::MAX);
    }

    // Ensures usage threshold warning fires once estimates exceed 80%.
    #[test]
    fn approaching_limit() {
        let t = TokenTracker::new(100);
        // Create messages that estimate to > 80 tokens worth of chars.
        let long = "x".repeat(400); // 400 chars / 4 = 100 tokens + overhead
        let msgs = vec![Message::user(&long)];
        assert!(t.is_approaching_limit(&msgs));
    }

    // Ensures model-visible JSON extras are included in rough size estimates.
    #[test]
    fn estimate_includes_extra_message_fields() {
        let mut msg = Message::user("hello");
        msg.extra
            .insert("reasoning_content".into(), json!("x".repeat(200)));
        let base = TokenTracker::estimate_messages(&[Message::user("hello")]);
        let with_extra = TokenTracker::estimate_messages(&[msg]);
        assert!(with_extra > base);
    }
}
