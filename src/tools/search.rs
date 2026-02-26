//! Web search tool.
//!
//! Searches DuckDuckGo's HTML endpoint (no API key required) and extracts
//! result titles, URLs, and snippets from the response.

use async_trait::async_trait;
use serde::Deserialize;

use super::Tool;
use crate::error::ToolError;
use crate::types::{FunctionDefinition, ToolDefinition};

/// Maximum number of results to extract.
const MAX_RESULTS: usize = 8;

/// Tool that searches the web via DuckDuckGo.
pub struct WebSearchTool;

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

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: Args = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoded(&args.query)
        );

        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (compatible; buddy/0.1)")
            .build()
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let html = client
            .get(&url)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
            .text()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let results = parse_ddg_results(&html);
        if results.is_empty() {
            return Ok("No results found.".into());
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

/// Minimal HTML scraping of DuckDuckGo's results page.
///
/// Looks for result links (class="result__a") and snippets (class="result__snippet").
/// This is intentionally crude — no HTML parser dependency.
fn parse_ddg_results(html: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();

    // Each result is in a div with class "result results_links results_links_deep web-result"
    // The title link has class="result__a" and the snippet has class="result__snippet"
    for chunk in html.split("class=\"result__a\"") {
        if results.len() >= MAX_RESULTS {
            break;
        }
        // Skip the first chunk (before any result).
        if !chunk.contains("result__snippet") {
            continue;
        }

        let title = extract_tag_content(chunk);
        let url = extract_href(chunk);
        let snippet = extract_snippet(chunk);

        if !title.is_empty() {
            results.push(SearchResult {
                title: decode_html_entities(&title),
                url,
                snippet: decode_html_entities(&snippet),
            });
        }
    }

    results
}

/// Extract text content between > and < (first tag content after the split point).
fn extract_tag_content(s: &str) -> String {
    if let Some(start) = s.find('>') {
        if let Some(end) = s[start + 1..].find('<') {
            return s[start + 1..start + 1 + end].trim().to_string();
        }
    }
    String::new()
}

/// Extract href value from the first href="..." in the chunk.
fn extract_href(s: &str) -> String {
    if let Some(pos) = s.find("href=\"") {
        let rest = &s[pos + 6..];
        if let Some(end) = rest.find('"') {
            return rest[..end].to_string();
        }
    }
    String::new()
}

/// Extract snippet text from a result__snippet span.
fn extract_snippet(s: &str) -> String {
    if let Some(pos) = s.find("class=\"result__snippet\"") {
        let rest = &s[pos..];
        if let Some(start) = rest.find('>') {
            let inner = &rest[start + 1..];
            // Collect text, stripping inner tags.
            let mut text = String::new();
            let mut in_tag = false;
            for ch in inner.chars() {
                match ch {
                    '<' => in_tag = true,
                    '>' => in_tag = false,
                    _ if !in_tag => text.push(ch),
                    _ => {}
                }
                // Stop at the closing tag for the snippet.
                if text.len() > 500 {
                    break;
                }
            }
            return text.trim().to_string();
        }
    }
    String::new()
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

    #[test]
    fn decode_entities_plain_text_unchanged() {
        let s = "no entities here";
        assert_eq!(decode_html_entities(s), s);
    }

    #[test]
    fn decode_entities_multiple_in_string() {
        assert_eq!(
            decode_html_entities("a &amp; b &lt; c &gt; d"),
            "a & b < c > d"
        );
    }

    // --- extract_tag_content ---

    #[test]
    fn extract_tag_content_basic() {
        assert_eq!(extract_tag_content(">hello<rest"), "hello");
    }

    #[test]
    fn extract_tag_content_trims_whitespace() {
        assert_eq!(extract_tag_content(">  hello  <rest"), "hello");
    }

    #[test]
    fn extract_tag_content_missing_brackets() {
        assert_eq!(extract_tag_content("no brackets"), "");
    }

    // --- extract_href ---

    #[test]
    fn extract_href_basic() {
        assert_eq!(
            extract_href(r#"href="https://example.com" other"#),
            "https://example.com"
        );
    }

    #[test]
    fn extract_href_missing() {
        assert_eq!(extract_href("no href here"), "");
    }

    // --- extract_snippet ---

    #[test]
    fn extract_snippet_plain_text() {
        let html = r#"class="result__snippet">Some text here</span>"#;
        assert_eq!(extract_snippet(html), "Some text here");
    }

    #[test]
    fn extract_snippet_strips_inner_tags() {
        let html = r#"class="result__snippet">Some <b>bold</b> text</span>"#;
        assert_eq!(extract_snippet(html), "Some bold text");
    }

    #[test]
    fn extract_snippet_missing_class() {
        assert_eq!(extract_snippet("no snippet here"), "");
    }

    // --- parse_ddg_results ---

    #[test]
    fn parse_ddg_results_empty_html() {
        assert!(parse_ddg_results("").is_empty());
    }

    #[test]
    fn parse_ddg_results_no_results() {
        assert!(parse_ddg_results("<html><body>No results</body></html>").is_empty());
    }

    #[test]
    fn parse_ddg_results_single_result() {
        // Simulate the structure parse_ddg_results expects: split on class="result__a",
        // then the chunk after it contains href, title content, and class="result__snippet".
        let html = concat!(
            r#"preamble class="result__a" href="https://example.com">Example Title<span "#,
            r#"class="result__snippet">A short description</span>"#
        );
        let results = parse_ddg_results(html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example Title");
        assert_eq!(results[0].url, "https://example.com");
        assert!(
            results[0].snippet.contains("description"),
            "snippet: {}",
            results[0].snippet
        );
    }

    #[test]
    fn parse_ddg_results_respects_max_limit() {
        // Each chunk must contain result__snippet to be counted.
        let chunk = concat!(
            r#" href="https://example.com">Title<span "#,
            r#"class="result__snippet">Snippet</span> preamble class="result__a""#,
        );
        // Build HTML that starts with the split marker so each chunk is a valid result.
        let html = format!(
            r#"preamble class="result__a"{}"#,
            chunk.repeat(MAX_RESULTS + 2)
        );
        let results = parse_ddg_results(&html);
        assert!(
            results.len() <= MAX_RESULTS,
            "got {} results, expected <= {MAX_RESULTS}",
            results.len()
        );
    }

    // --- tool metadata ---

    #[test]
    fn web_search_tool_name() {
        assert_eq!(WebSearchTool.name(), "web_search");
    }
}
