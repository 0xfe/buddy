//! Model reasoning-effort capability helpers.
//!
//! These helpers provide a single source of truth for:
//! - when reasoning-effort controls are applicable
//! - which effort levels should be exposed in interactive pickers
//! - basic model/version-specific compatibility behavior

use super::{ApiProtocol, ModelProvider, ReasoningEffort};

const EMPTY_REASONING_EFFORTS: &[ReasoningEffort] = &[];
const REASONING_EFFORTS_GPT5_PRO: &[ReasoningEffort] = &[ReasoningEffort::High];
const REASONING_EFFORTS_GPT51: &[ReasoningEffort] = &[
    ReasoningEffort::None,
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
];
const REASONING_EFFORTS_GPT5_PLUS: &[ReasoningEffort] = &[
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
    ReasoningEffort::Xhigh,
];
const REASONING_EFFORTS_GPT5_BASE: &[ReasoningEffort] = &[
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
];
const REASONING_EFFORTS_O_SERIES: &[ReasoningEffort] = &[
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
];

/// Return supported reasoning effort levels for the provider/protocol/model tuple.
///
/// Capability scope is currently limited to OpenAI `/responses` reasoning models.
pub fn supported_reasoning_efforts(
    provider: ModelProvider,
    protocol: ApiProtocol,
    model: &str,
) -> &'static [ReasoningEffort] {
    if provider != ModelProvider::Openai || protocol != ApiProtocol::Responses {
        return EMPTY_REASONING_EFFORTS;
    }
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return EMPTY_REASONING_EFFORTS;
    }

    if normalized.contains("gpt-5-pro") {
        return REASONING_EFFORTS_GPT5_PRO;
    }
    if normalized.starts_with("gpt-5.1") {
        return REASONING_EFFORTS_GPT51;
    }
    if normalized.starts_with("gpt-5.2")
        || normalized.starts_with("gpt-5.3")
        || normalized.starts_with("gpt-5.4")
        || normalized.contains("codex-spark")
    {
        return REASONING_EFFORTS_GPT5_PLUS;
    }
    if normalized.starts_with("gpt-5") || normalized.contains("codex") {
        return REASONING_EFFORTS_GPT5_BASE;
    }
    if normalized.starts_with("o1") || normalized.starts_with("o3") || normalized.starts_with("o4")
    {
        return REASONING_EFFORTS_O_SERIES;
    }

    EMPTY_REASONING_EFFORTS
}

/// Return true when model supports configurable reasoning effort.
pub fn supports_reasoning_effort(
    provider: ModelProvider,
    protocol: ApiProtocol,
    model: &str,
) -> bool {
    !supported_reasoning_efforts(provider, protocol, model).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_gpt53_codex_supports_low_medium_high_xhigh() {
        let values = supported_reasoning_efforts(
            ModelProvider::Openai,
            ApiProtocol::Responses,
            "gpt-5.3-codex",
        );
        assert_eq!(values, REASONING_EFFORTS_GPT5_PLUS);
    }

    #[test]
    fn openai_gpt51_supports_none_low_medium_high() {
        let values =
            supported_reasoning_efforts(ModelProvider::Openai, ApiProtocol::Responses, "gpt-5.1");
        assert_eq!(values, REASONING_EFFORTS_GPT51);
    }

    #[test]
    fn openrouter_models_do_not_advertise_openai_reasoning_effort_picker() {
        let values = supported_reasoning_efforts(
            ModelProvider::Openrouter,
            ApiProtocol::Completions,
            "deepseek/deepseek-v3.2",
        );
        assert!(values.is_empty());
    }
}
