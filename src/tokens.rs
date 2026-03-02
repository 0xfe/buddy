//! Crude token tracking and context window management.
//!
//! Tracks exact counts from the API's `usage` field when available,
//! and provides a rough estimation heuristic (~1 token per 4 chars)
//! for pre-flight context limit checks.

use crate::types::Message;
use serde::Deserialize;
use serde_json::Value;
use std::sync::OnceLock;

/// Per-model runtime calibration state for token estimation.
#[derive(Debug, Clone, Copy)]
pub struct ModelTokenCalibration {
    /// Smoothed multiplier applied to heuristic character-based estimates.
    ratio: f64,
    /// Number of observations used to build this calibration state.
    samples: u32,
}

impl Default for ModelTokenCalibration {
    fn default() -> Self {
        Self {
            ratio: 1.0,
            samples: 0,
        }
    }
}

impl ModelTokenCalibration {
    /// Apply current calibration ratio to a raw heuristic estimate.
    pub fn calibrated_estimate(&self, raw_estimate: usize) -> usize {
        if raw_estimate == 0 {
            return 0;
        }
        let adjusted = (raw_estimate as f64 * self.ratio).round();
        adjusted.max(1.0) as usize
    }

    /// Update calibration from one observed request.
    pub fn observe_prompt_usage(&mut self, raw_estimate: u64, observed_prompt_tokens: u64) {
        if raw_estimate == 0 || observed_prompt_tokens == 0 {
            return;
        }

        // Keep the multiplier bounded so one outlier does not destabilize
        // context-limit decisions.
        let observed_ratio = (observed_prompt_tokens as f64 / raw_estimate as f64).clamp(0.5, 2.5);
        let alpha = if self.samples < 4 { 0.35 } else { 0.15 };
        self.ratio = (self.ratio * (1.0 - alpha)) + (observed_ratio * alpha);
        self.samples = self.samples.saturating_add(1);
    }
}

/// Apply optional model calibration to a raw estimated token count.
pub fn calibrated_estimate(
    raw_estimate: usize,
    calibration: Option<&ModelTokenCalibration>,
) -> usize {
    calibration
        .map(|state| state.calibrated_estimate(raw_estimate))
        .unwrap_or(raw_estimate)
}

/// Per-model pricing rates used for request/session cost estimation.
///
/// Values are expressed in USD per 1M tokens (`per_mtok`) so downstream code
/// can estimate request cost from provider usage telemetry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    /// Input-token price in USD per 1M tokens.
    pub input_price_per_mtok: f64,
    /// Output-token price in USD per 1M tokens.
    pub output_price_per_mtok: f64,
    /// Optional cache-read input price in USD per 1M tokens.
    pub cache_read_price_per_mtok: Option<f64>,
}

/// One request-level cost estimate in USD.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageCostEstimate {
    /// Input-token cost in USD.
    pub input_usd: f64,
    /// Output-token cost in USD.
    pub output_usd: f64,
    /// Cache-read-token cost in USD.
    pub cache_read_usd: f64,
    /// Total request cost in USD.
    pub total_usd: f64,
}

/// Estimate request cost from usage totals and model pricing.
///
/// `cached_prompt_tokens` is optional because many providers do not report it.
/// When unavailable, all prompt tokens are priced using `input_price_per_mtok`.
pub fn estimate_usage_cost(
    pricing: &ModelPricing,
    prompt_tokens: u64,
    completion_tokens: u64,
    cached_prompt_tokens: Option<u64>,
) -> UsageCostEstimate {
    let cached_tokens = cached_prompt_tokens.unwrap_or(0).min(prompt_tokens);
    let billable_input_tokens = prompt_tokens.saturating_sub(cached_tokens);

    let input_usd = tokens_to_mtok(billable_input_tokens) * pricing.input_price_per_mtok;
    let output_usd = tokens_to_mtok(completion_tokens) * pricing.output_price_per_mtok;
    let cache_read_usd = pricing
        .cache_read_price_per_mtok
        .map(|rate| tokens_to_mtok(cached_tokens) * rate)
        .unwrap_or(0.0);
    let total_usd = input_usd + output_usd + cache_read_usd;

    UsageCostEstimate {
        input_usd,
        output_usd,
        cache_read_usd,
        total_usd,
    }
}

/// Convert raw token count into "millions of tokens" units.
fn tokens_to_mtok(tokens: u64) -> f64 {
    tokens as f64 / 1_000_000.0
}

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
    /// Optional input-token price in USD per 1M tokens.
    #[serde(default)]
    input_price_per_mtok: Option<f64>,
    /// Optional output-token price in USD per 1M tokens.
    #[serde(default)]
    output_price_per_mtok: Option<f64>,
    /// Optional cache-read input price in USD per 1M tokens.
    #[serde(default)]
    cache_read_price_per_mtok: Option<f64>,
}

impl ModelContextRule {
    /// Return pricing when both input/output rates are present.
    fn pricing(&self) -> Option<ModelPricing> {
        Some(ModelPricing {
            input_price_per_mtok: self.input_price_per_mtok?,
            output_price_per_mtok: self.output_price_per_mtok?,
            cache_read_price_per_mtok: self.cache_read_price_per_mtok,
        })
    }
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
    fn lookup_rule(&self, model: &str) -> Option<&ModelContextRule> {
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

        self.rule.iter().find(|rule| {
            let pattern = normalize_model_name(&rule.pattern);
            if pattern.is_empty() {
                return false;
            }
            match rule.kind {
                ModelMatchKind::Exact => candidates.iter().any(|c| c == &pattern),
                ModelMatchKind::Prefix => candidates.iter().any(|c| c.starts_with(&pattern)),
                ModelMatchKind::Contains => candidates.iter().any(|c| c.contains(&pattern)),
            }
        })
    }

    /// Return context-window size for first matching rule.
    fn lookup_context(&self, model: &str) -> Option<usize> {
        self.lookup_rule(model).map(|rule| rule.context_window)
    }

    /// Return pricing for first matching rule when rates are available.
    fn lookup_pricing(&self, model: &str) -> Option<ModelPricing> {
        self.lookup_rule(model).and_then(ModelContextRule::pricing)
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
        if let Some(context_window) = catalog.lookup_context(model) {
            return context_window;
        }
        return catalog.default_context_window;
    }
    legacy_default_context_limit(model)
}

/// Best-effort pricing lookup for one model id.
///
/// Returns `None` when no pricing metadata exists in the embedded catalog.
pub fn model_pricing(model: &str) -> Option<ModelPricing> {
    model_catalog().and_then(|catalog| catalog.lookup_pricing(model))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Ensures calibration smooths toward observed provider usage and stays bounded.
    #[test]
    fn model_token_calibration_tracks_observed_usage() {
        let mut state = ModelTokenCalibration::default();
        assert_eq!(state.calibrated_estimate(100), 100);
        state.observe_prompt_usage(100, 150);
        let adjusted = state.calibrated_estimate(100);
        assert!(adjusted > 100, "adjusted={adjusted}");
        state.observe_prompt_usage(100, 300); // clamped at 2.5x
        let clamped_adjusted = state.calibrated_estimate(100);
        assert!(clamped_adjusted <= 250, "adjusted={clamped_adjusted}");
    }

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

        assert_eq!(catalog.lookup_context("foo-model"), Some(100));
        assert_eq!(catalog.lookup_context("provider/foo-model"), Some(100));
        assert_eq!(catalog.lookup_context("bar-v2"), Some(200));
        assert_eq!(catalog.lookup_context("provider/bar-v2"), Some(200));
        assert_eq!(catalog.lookup_context("something-baz-here"), Some(300));
        assert_eq!(catalog.lookup_context("no-match"), None);
    }

    // Ensures pricing metadata can be resolved from matching catalog rules.
    #[test]
    fn catalog_pricing_lookup() {
        let catalog: ModelCatalog = toml::from_str(
            r#"
            default_context_window = 42

            [[rule]]
            match = "prefix"
            pattern = "gpt-x"
            context_window = 100000
            input_price_per_mtok = 1.5
            output_price_per_mtok = 6.0
            cache_read_price_per_mtok = 0.5
            "#,
        )
        .unwrap();

        let pricing = catalog.lookup_pricing("gpt-x-1").expect("pricing");
        assert_eq!(pricing.input_price_per_mtok, 1.5);
        assert_eq!(pricing.output_price_per_mtok, 6.0);
        assert_eq!(pricing.cache_read_price_per_mtok, Some(0.5));
    }

    // Ensures request-cost estimation computes each price bucket consistently.
    #[test]
    fn usage_cost_estimation_splits_input_output_and_cache() {
        let pricing = ModelPricing {
            input_price_per_mtok: 2.0,
            output_price_per_mtok: 8.0,
            cache_read_price_per_mtok: Some(0.5),
        };
        let estimate = estimate_usage_cost(&pricing, 2_000, 500, Some(400));
        // Input billed tokens = 1600 => 0.0016 * $2.0
        assert!((estimate.input_usd - 0.0032).abs() < 1e-9);
        // Output billed tokens = 500 => 0.0005 * $8.0
        assert!((estimate.output_usd - 0.004).abs() < 1e-9);
        // Cache billed tokens = 400 => 0.0004 * $0.5
        assert!((estimate.cache_read_usd - 0.0002).abs() < 1e-9);
        assert!((estimate.total_usd - 0.0074).abs() < 1e-9);
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
