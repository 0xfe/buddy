//! Web search tool.
//!
//! Searches DuckDuckGo's HTML endpoint (no API key required) and extracts
//! result titles, URLs, and snippets from the response.

use async_trait::async_trait;
use scraper::{Html, Selector};
use serde::Deserialize;
use std::time::Duration;

use super::{Tool, ToolContext};
use crate::error::ToolError;
use crate::types::{FunctionDefinition, ToolDefinition};

/// Maximum number of results to extract.
const MAX_RESULTS: usize = 8;

/// Tool that searches the web via DuckDuckGo.
pub struct WebSearchTool {
    http: reqwest::Client,
}

impl WebSearchTool {
    /// Build a search tool with a reusable HTTP client.
    pub fn new(timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent("Mozilla/5.0 (compatible; buddy/0.1)")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { http }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new(Duration::from_secs(15))
    }
}

#[derive(Deserialize)]
struct Args {
    query: String,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description: "Search the web for a query and return the top results with titles, URLs, and snippets.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query"
                        }
                    },
                    "required": ["query"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str, _context: &ToolContext) -> Result<String, ToolError> {
        let args: Args = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoded(&args.query)
        );

        let html = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
            .text()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let results = parse_ddg_results(&html);
        if results.is_empty() {
            return Ok(empty_results_message(&args.query, &html));
        }

        let mut output = String::new();
        for (i, r) in results.iter().enumerate().take(MAX_RESULTS) {
            output.push_str(&format!(
                "{}. {}\n   {}\n   {}\n\n",
                i + 1,
                r.title,
                r.url,
                r.snippet
            ));
        }
        Ok(output)
    }
}

struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// Parse DuckDuckGo HTML results using CSS selectors.
fn parse_ddg_results(html: &str) -> Vec<SearchResult> {
    let document = Html::parse_document(html);
    let result_selector = Selector::parse(".result").expect("valid result selector");
    let link_selector = Selector::parse("a.result__a").expect("valid link selector");
    let snippet_selector = Selector::parse(".result__snippet").expect("valid snippet selector");

    let mut results = Vec::new();

    // Preferred path: parse from stable result containers.
    for result in document.select(&result_selector) {
        if results.len() >= MAX_RESULTS {
            break;
        }
        let Some(link) = result.select(&link_selector).next() else {
            continue;
        };

        let title = extract_element_text(&link);
        let url = link
            .value()
            .attr("href")
            .map(decode_html_entities)
            .unwrap_or_default();
        if title.is_empty() || url.is_empty() {
            continue;
        }

        let snippet = result
            .select(&snippet_selector)
            .next()
            .map(|elem| decode_html_entities(&extract_element_text(&elem)))
            .unwrap_or_default();

        results.push(SearchResult {
            title: decode_html_entities(&title),
            url,
            snippet,
        });
    }

    if !results.is_empty() {
        return results;
    }

    // Fallback path: if container classes move, still try link extraction.
    for link in document.select(&link_selector) {
        if results.len() >= MAX_RESULTS {
            break;
        }
        let title = extract_element_text(&link);
        let url = link
            .value()
            .attr("href")
            .map(decode_html_entities)
            .unwrap_or_default();
        if title.is_empty() || url.is_empty() {
            continue;
        }
        results.push(SearchResult {
            title: decode_html_entities(&title),
            url,
            snippet: String::new(),
        });
    }

    results
}

fn extract_element_text(element: &scraper::ElementRef<'_>) -> String {
    element
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn empty_results_message(query: &str, html: &str) -> String {
    if query.trim().is_empty() {
        return "No results found.".to_string();
    }

    let lower = html.to_ascii_lowercase();
    if lower.contains("no results")
        || lower.contains("did not match")
        || lower.contains("sorry, no results")
    {
        return "No results found.".to_string();
    }

    "No results found. (DuckDuckGo returned a page, but buddy could not parse results; the layout may have changed.)".to_string()
}

/// Minimal URL encoding for the query string.
fn urlencoded(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

/// Decode common HTML entities.
fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- urlencoded ---

    #[test]
    fn urlencoded_alphanumeric_passthrough() {
        assert_eq!(urlencoded("hello123"), "hello123");
        assert_eq!(urlencoded("ABC"), "ABC");
    }

    #[test]
    fn urlencoded_unreserved_chars_passthrough() {
        assert_eq!(urlencoded("-_.~"), "-_.~");
    }

    #[test]
    fn urlencoded_space_becomes_plus() {
        assert_eq!(urlencoded("hello world"), "hello+world");
    }

    #[test]
    fn urlencoded_special_chars_percent_encoded() {
        assert_eq!(urlencoded("a&b"), "a%26b");
        assert_eq!(urlencoded("a=b"), "a%3Db");
        assert_eq!(urlencoded("a+b"), "a%2Bb");
        assert_eq!(urlencoded("a/b"), "a%2Fb");
    }

    // --- decode_html_entities ---

    #[test]
    fn decode_entities_all_known() {
        assert_eq!(decode_html_entities("&amp;"), "&");
        assert_eq!(decode_html_entities("&lt;"), "<");
        assert_eq!(decode_html_entities("&gt;"), ">");
        assert_eq!(decode_html_entities("&quot;"), "\"");
        assert_eq!(decode_html_entities("&#x27;"), "'");
        assert_eq!(decode_html_entities("&#39;"), "'");
        assert_eq!(decode_html_entities("&nbsp;"), " ");
    }

    // --- parse_ddg_results ---

    #[test]
    fn parse_ddg_results_empty_html() {
        assert!(parse_ddg_results("").is_empty());
    }

    #[test]
    fn parse_ddg_results_from_result_container() {
        let html = r#"
            <div class="result results_links results_links_deep web-result">
              <a class="result__a" href="https://example.com">Example Title</a>
              <a class="result__snippet">A short description</a>
            </div>
        "#;

        let results = parse_ddg_results(html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example Title");
        assert_eq!(results[0].url, "https://example.com");
        assert_eq!(results[0].snippet, "A short description");
    }

    #[test]
    fn parse_ddg_results_handles_attribute_reordering() {
        let html = r#"
            <div class="result">
              <a href="https://example.com" rel="noopener" class="result__a">Title</a>
              <span class="result__snippet">Snippet text</span>
            </div>
        "#;
        let results = parse_ddg_results(html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Title");
        assert_eq!(results[0].snippet, "Snippet text");
    }

    #[test]
    fn parse_ddg_results_falls_back_to_link_scan() {
        let html = r#"
            <section>
              <a class="result__a" href="https://fallback.example">Fallback title</a>
            </section>
        "#;

        let results = parse_ddg_results(html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Fallback title");
        assert_eq!(results[0].url, "https://fallback.example");
        assert_eq!(results[0].snippet, "");
    }

    #[test]
    fn parse_ddg_results_respects_max_limit() {
        let mut html = String::new();
        for idx in 0..(MAX_RESULTS + 3) {
            html.push_str(&format!(
                r#"<div class="result"><a class="result__a" href="https://example.com/{idx}">T{idx}</a><span class="result__snippet">S{idx}</span></div>"#
            ));
        }

        let results = parse_ddg_results(&html);
        assert_eq!(results.len(), MAX_RESULTS);
    }

    #[test]
    fn empty_results_message_reports_parser_breakage() {
        let msg = empty_results_message("rust", "<html><body>unexpected layout</body></html>");
        assert!(msg.contains("could not parse"), "message: {msg}");
    }

    #[test]
    fn empty_results_message_preserves_true_no_results() {
        let msg = empty_results_message("rust", "<html><body>No results found</body></html>");
        assert_eq!(msg, "No results found.");
    }

    // --- tool metadata ---

    #[test]
    fn web_search_tool_name() {
        assert_eq!(WebSearchTool::default().name(), "web_search");
    }
}
