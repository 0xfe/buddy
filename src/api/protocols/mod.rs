//! Wire-protocol specific API modules.
//!
//! Each submodule owns request/response translation for one provider protocol:
//! - `completions`: OpenAI-compatible `/chat/completions`
//! - `responses`: OpenAI `/responses`
//! - `messages`: Anthropic `/messages`

pub(crate) mod completions;
pub(crate) mod messages;
pub(crate) mod responses;
