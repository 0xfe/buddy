//! URL fetch tool.
//!
//! Performs an async HTTP GET and returns the response body as text.

use async_trait::async_trait;
use serde::Deserialize;
use std::io::{IsTerminal, Write};
use std::net::IpAddr;
use std::time::Duration;
use tokio::net::lookup_host;

use super::result_envelope::wrap_result;
use super::shell::ShellApprovalBroker;
use super::{Tool, ToolContext};
use crate::error::ToolError;
use crate::textutil::truncate_with_suffix_by_bytes;
use crate::types::{FunctionDefinition, ToolDefinition};

/// Maximum characters of response body to return.
const MAX_BODY_LEN: usize = 8000;
/// Default request timeout for network fetches.
const DEFAULT_FETCH_TIMEOUT_SECS: u64 = 20;

/// Tool that fetches a URL and returns its text content.
pub struct FetchTool {
    /// Reused HTTP client with configured timeout.
    http: reqwest::Client,
    /// Whether each fetch must be explicitly approved by the user.
    confirm: bool,
    /// Optional domain allowlist (empty means unrestricted by allowlist).
    allowed_domains: Vec<String>,
    /// Explicit domain denylist checked before network calls.
    blocked_domains: Vec<String>,
    /// Optional interactive approval broker for UI-driven confirmations.
    approval: Option<ShellApprovalBroker>,
}

impl FetchTool {
    /// Build a fetch tool with policy and timeout settings.
    pub fn new(
        timeout: Duration,
        confirm: bool,
        allowed_domains: Vec<String>,
        blocked_domains: Vec<String>,
        approval: Option<ShellApprovalBroker>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            http,
            confirm,
            allowed_domains: normalize_domain_rules(allowed_domains),
            blocked_domains: normalize_domain_rules(blocked_domains),
            approval,
        }
    }
}

impl Default for FetchTool {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(DEFAULT_FETCH_TIMEOUT_SECS),
            false,
            Vec::new(),
            Vec::new(),
            None,
        )
    }
}

#[derive(Deserialize)]
struct Args {
    /// URL to request.
    url: String,
}

#[async_trait]
impl Tool for FetchTool {
    fn name(&self) -> &'static str {
        "fetch_url"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description: "Fetch the contents of a URL and return the response body as text."
                    .into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to fetch"
                        }
                    },
                    "required": ["url"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str, _context: &ToolContext) -> Result<String, ToolError> {
        // Parse and validate policy before any outbound HTTP request.
        let args: Args = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        let url =
            validate_url_policy(&args.url, &self.allowed_domains, &self.blocked_domains).await?;

        // Optional operator confirmation for fetches in higher-control environments.
        if self.confirm {
            let approved = if let Some(approval) = &self.approval {
                approval
                    .request(format!("fetch {}", url.as_str()), None)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
            } else {
                if !std::io::stdin().is_terminal() {
                    return Err(ToolError::ExecutionFailed(
                        "fetch_url confirmation required, but stdin is not interactive. Disable tools.fetch_confirm or run in interactive mode.".to_string(),
                    ));
                }
                eprint!("  Fetch: {} [y/N] ", url.as_str());
                let _ = std::io::stderr().flush();
                let mut input = String::new();
                std::io::stdin()
                    .read_line(&mut input)
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
                matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes")
            };
            if !approved {
                return wrap_result("Fetch request denied by user.");
            }
        }

        // Keep response handling intentionally simple: GET + body text extraction.
        let body = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
            .text()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        wrap_result(truncate_body(&body))
    }
}

fn truncate_body(body: &str) -> String {
    // Bound response size so one fetch cannot consume the full model context.
    truncate_with_suffix_by_bytes(body, MAX_BODY_LEN, "...[truncated]")
}

fn normalize_domain_rules(rules: Vec<String>) -> Vec<String> {
    // Normalize once so comparisons are case-insensitive and suffix-safe.
    rules
        .into_iter()
        .map(|value| value.trim().trim_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

fn domain_matches_rule(host: &str, rule: &str) -> bool {
    // Rule applies to exact host or any subdomain.
    host == rule || host.ends_with(&format!(".{rule}"))
}

fn is_forbidden_ip(ip: IpAddr) -> bool {
    // Reject local/private/multicast ranges to reduce SSRF surface area.
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_multicast()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_multicast()
        }
    }
}

async fn validate_url_policy(
    raw: &str,
    allowed_domains: &[String],
    blocked_domains: &[String],
) -> Result<reqwest::Url, ToolError> {
    // Parse and validate URL syntax first.
    let parsed = reqwest::Url::parse(raw)
        .map_err(|e| ToolError::InvalidArguments(format!("invalid url `{raw}`: {e}")))?;

    // Only HTTP(S) is supported.
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(ToolError::ExecutionFailed(format!(
                "fetch_url blocked: unsupported scheme `{scheme}` (only http/https allowed)"
            )))
        }
    }

    // Normalize host for policy checks.
    let host = parsed
        .host_str()
        .ok_or_else(|| ToolError::InvalidArguments("url host is required".to_string()))?
        .trim()
        .trim_matches('.')
        .to_ascii_lowercase();

    // Hard-block localhost aliases.
    if host == "localhost" || host.ends_with(".localhost") {
        return Err(ToolError::ExecutionFailed(
            "fetch_url blocked: localhost targets are not allowed".to_string(),
        ));
    }

    // Denylist is checked before allowlist.
    if blocked_domains
        .iter()
        .any(|rule| domain_matches_rule(&host, rule))
    {
        return Err(ToolError::ExecutionFailed(format!(
            "fetch_url blocked: domain `{host}` matches blocked policy"
        )));
    }

    let explicitly_allowed = !allowed_domains.is_empty()
        && allowed_domains
            .iter()
            .any(|rule| domain_matches_rule(&host, rule));

    if !allowed_domains.is_empty() && !explicitly_allowed {
        return Err(ToolError::ExecutionFailed(format!(
            "fetch_url blocked: domain `{host}` is not in tools.fetch_allowed_domains"
        )));
    }

    // If host is a literal IP, validate directly without DNS resolution.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_forbidden_ip(ip) && !explicitly_allowed {
            return Err(ToolError::ExecutionFailed(format!(
                "fetch_url blocked: target IP `{ip}` is private or local"
            )));
        }
        return Ok(parsed);
    }

    // For hostname targets, resolve and reject any private/local results.
    let port = parsed.port_or_known_default().unwrap_or(80);
    let resolved = lookup_host((host.as_str(), port))
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("failed to resolve `{host}`: {e}")))?;
    for addr in resolved {
        if is_forbidden_ip(addr.ip()) && !explicitly_allowed {
            return Err(ToolError::ExecutionFailed(format!(
                "fetch_url blocked: `{host}` resolves to private/local address `{}`",
                addr.ip()
            )));
        }
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn parse_envelope(result: &str) -> serde_json::Value {
        serde_json::from_str(result).expect("tool result envelope")
    }

    #[test]
    fn truncate_body_keeps_short_text() {
        // Short responses should pass through unchanged.
        assert_eq!(truncate_body("hello"), "hello");
    }

    #[test]
    fn truncate_body_is_utf8_safe() {
        // Truncation should keep valid UTF-8 boundaries.
        let body = "🙂".repeat(MAX_BODY_LEN + 5);
        let out = truncate_body(&body);
        assert!(out.ends_with("...[truncated]"), "got: {out}");
    }

    #[test]
    fn domain_rule_matches_subdomains() {
        // Domain rules should match both root domain and subdomains.
        assert!(domain_matches_rule("api.example.com", "example.com"));
        assert!(domain_matches_rule("example.com", "example.com"));
        assert!(!domain_matches_rule("example.net", "example.com"));
    }

    #[tokio::test]
    async fn validate_url_policy_blocks_localhost() {
        // Localhost endpoints should always be blocked.
        let err = validate_url_policy("http://localhost:8080", &[], &[])
            .await
            .expect_err("localhost should be blocked");
        assert!(
            err.to_string().contains("localhost"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn validate_url_policy_blocks_private_ip() {
        // Private IP literals should be blocked unless explicitly allowed.
        let err = validate_url_policy("http://10.0.0.1", &[], &[])
            .await
            .expect_err("private ip should be blocked");
        assert!(
            err.to_string().contains("private"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn validate_url_policy_respects_blocked_domains() {
        // Denylist matches should reject otherwise valid URLs.
        let err = validate_url_policy(
            "https://api.internal.example.com/v1",
            &[],
            &["internal.example.com".to_string()],
        )
        .await
        .expect_err("blocked domain expected");
        assert!(
            err.to_string().contains("blocked policy"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn validate_url_policy_respects_allowlist() {
        // Allowlist mismatches should be rejected when allowlist is configured.
        let err = validate_url_policy(
            "https://api.example.com/v1",
            &["allowed.example.com".to_string()],
            &[],
        )
        .await
        .expect_err("allowlist mismatch expected");
        assert!(
            err.to_string().contains("fetch_allowed_domains"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn validate_url_policy_allows_public_ip_when_explicitly_allowed() {
        // Explicit allowlist should allow direct public IP targets.
        let url = validate_url_policy("https://1.1.1.1/dns-query", &["1.1.1.1".to_string()], &[])
            .await
            .expect("public ip should be allowed");
        assert_eq!(url.host_str(), Some("1.1.1.1"));
    }

    #[tokio::test]
    async fn fetch_tool_respects_configured_timeout() {
        // Client timeout should terminate slow/hanging requests.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Accept one connection and intentionally never send an HTTP response.
        let _accept = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("accept");
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let tool = FetchTool::new(
            Duration::from_millis(50),
            false,
            vec!["127.0.0.1".to_string()],
            Vec::new(),
            None,
        );
        let args = format!(r#"{{"url":"http://{addr}/hang"}}"#);
        let outcome = tokio::time::timeout(
            Duration::from_millis(400),
            tool.execute(&args, &ToolContext::empty()),
        )
        .await
        .expect("fetch should not hang indefinitely");
        assert!(outcome.is_err(), "expected fetch request to fail");
    }

    #[tokio::test]
    async fn fetch_tool_confirmation_denied_returns_message() {
        // Denied approval should return a non-failing explanatory payload.
        let (broker, mut rx) = ShellApprovalBroker::channel();
        let tool = FetchTool::new(
            Duration::from_secs(1),
            true,
            vec!["1.1.1.1".to_string()],
            Vec::new(),
            Some(broker),
        );
        let join = tokio::spawn(async move {
            tool.execute(
                r#"{"url":"https://1.1.1.1/dns-query"}"#,
                &ToolContext::empty(),
            )
            .await
        });

        let req = rx.recv().await.expect("approval request expected");
        assert!(req.command().contains("fetch https://1.1.1.1"));
        req.deny();

        let result = join.await.expect("join should succeed").unwrap();
        assert_eq!(
            parse_envelope(&result)["result"],
            "Fetch request denied by user."
        );
    }
}
