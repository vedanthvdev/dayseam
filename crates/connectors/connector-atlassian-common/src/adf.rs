//! Atlassian Document Format (ADF) → plain-text walker.
//!
//! Every Atlassian body field (Jira comments, Confluence page bodies
//! when requested with `body-format=atlas_doc_format`) arrives as a
//! rich JSON tree of `{type, content, attrs}` nodes. The EOD report
//! template renders markdown, so we flatten ADF to a plain string at
//! ingestion time — the report layer sees a `String`, not a JSON
//! blob.
//!
//! Supported nodes (verbatim from the DAY-73 spike fixtures
//! `docs/spikes/2026-04-20-atlassian-connectors-data-shape.md` §5):
//!
//! | Node type | Rendered as |
//! |---|---|
//! | `doc` | concatenation of children (top-level container) |
//! | `paragraph` | children joined, trailing `\n` |
//! | `text` | the `text` field verbatim |
//! | `mention` | `attrs.text` verbatim (keeps the `@` prefix — see note below) |
//! | `hardBreak` | `\n` |
//! | `bulletList` | children as `- <child>\n` |
//! | `orderedList` | children as `<idx>. <child>\n` starting at 1 |
//! | `listItem` | children concatenated, **no** trailing newline (parent list adds it) |
//! | `codeBlock` | ```` ```\n<text>\n``` ```` (fenced markdown block) |
//! | `heading` | children prefixed with `#` × `attrs.level` + space |
//! | `blockquote` | children prefixed with `> ` |
//! | `rule` | `\n---\n` |
//! | `emoji` | `attrs.text` if present, else empty (see note) |
//! | `inlineCard` / `blockCard` | `attrs.url` if present, else empty |
//! | `media*` / `mediaGroup` / `mediaSingle` | empty (not renderable in markdown) |
//! | anything else | `[unsupported content]` + optional warn log |
//!
//! **Privacy note on `mention`.** Spike §12 (risk: "comment mentions
//! include PII-ish text") mandates emitting only the `attrs.text`
//! (display-name) portion of a mention, never `attrs.id` (accountId)
//! or `attrs.email`. The walker defends that by reading only
//! `attrs.text` — if upstream ever adds an `email` field the walker
//! still ignores it.
//!
//! The walker is deliberately tolerant: malformed nodes degrade
//! rather than panic. A `mention` with no `attrs.text` emits nothing;
//! an `orderedList` with non-array `content` emits the `[unsupported
//! content]` marker.

use dayseam_core::LogLevel;
use dayseam_events::LogSender;
use serde_json::Value;

/// Marker a renderer inserts in place of an unrecognised ADF node
/// type. Visible in rendered bullets so the operator can spot the
/// shape change in the daily report; the `LogSender`-driven warn
/// log (when the caller wired one in) is the observability surface
/// for programmatic alerting.
pub const UNSUPPORTED_MARKER: &str = "[unsupported content]";

/// Walk the ADF document rooted at `value` and return the flattened
/// plain-text rendering.
///
/// Pass `Some(sender)` to receive one [`LogLevel::Warn`] per unknown
/// node-type encounter — the observability counterpart of the
/// `[unsupported content]` marker in the returned string. Tests use
/// `None` for the happy path and `Some(...)` for the degraded path.
pub fn adf_to_plain(value: &Value, logs: Option<&LogSender>) -> String {
    let mut out = String::new();
    walk(value, &mut out, logs, 0);
    // Trim a single trailing newline — every `paragraph` / list-item
    // emits one, and the top-level `doc` is almost always a sequence
    // of paragraphs. The rendered bullet looks cleaner without the
    // dangling `\n`.
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

fn walk(value: &Value, out: &mut String, logs: Option<&LogSender>, depth: usize) {
    let node_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match node_type {
        // Containers — just recurse into `content`.
        "doc" => walk_content(value, out, logs, depth),

        "paragraph" => {
            walk_content(value, out, logs, depth);
            out.push('\n');
        }

        "text" => {
            if let Some(text) = value.get("text").and_then(Value::as_str) {
                out.push_str(text);
            }
        }

        "mention" => {
            // Spike §12 privacy rule: only `attrs.text`. Never `attrs.id` / `attrs.email`.
            if let Some(text) = value
                .get("attrs")
                .and_then(|a| a.get("text"))
                .and_then(Value::as_str)
            {
                out.push_str(text);
            }
        }

        "hardBreak" => out.push('\n'),

        "bulletList" => walk_list(value, out, logs, depth, ListKind::Bullet),
        "orderedList" => walk_list(value, out, logs, depth, ListKind::Ordered),

        "listItem" => {
            // `listItem` is only reached as a direct child of a list;
            // we delegate the prefix to the list walker. Here we just
            // concat children without a trailing newline.
            walk_content_trim_trailing_newline(value, out, logs, depth);
        }

        "codeBlock" => {
            // ADF `codeBlock.content` is a flat array of `text` nodes.
            out.push_str("```");
            if let Some(lang) = value
                .get("attrs")
                .and_then(|a| a.get("language"))
                .and_then(Value::as_str)
            {
                out.push_str(lang);
            }
            out.push('\n');
            walk_content(value, out, logs, depth);
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```\n");
        }

        "heading" => {
            let level = value
                .get("attrs")
                .and_then(|a| a.get("level"))
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 6) as usize;
            for _ in 0..level {
                out.push('#');
            }
            out.push(' ');
            walk_content(value, out, logs, depth);
            out.push('\n');
        }

        "blockquote" => {
            out.push_str("> ");
            walk_content(value, out, logs, depth);
            // One trailing newline so a quote followed by a paragraph
            // stays visually separated.
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }

        "rule" => out.push_str("\n---\n"),

        "emoji" => {
            if let Some(text) = value
                .get("attrs")
                .and_then(|a| a.get("text"))
                .and_then(Value::as_str)
            {
                out.push_str(text);
            }
        }

        "inlineCard" | "blockCard" => {
            if let Some(url) = value
                .get("attrs")
                .and_then(|a| a.get("url"))
                .and_then(Value::as_str)
            {
                out.push_str(url);
            }
        }

        // Media-ish nodes — not renderable in markdown, emit nothing.
        "media" | "mediaGroup" | "mediaSingle" | "mediaInline" => {}

        // Unknown node — degrade, log.
        other => {
            out.push_str(UNSUPPORTED_MARKER);
            if let Some(sender) = logs {
                sender.send(
                    LogLevel::Warn,
                    None,
                    format!("adf: unrenderable node type {other:?}"),
                    serde_json::json!({
                        "node_type": other,
                        "depth": depth,
                    }),
                );
            }
        }
    }
}

fn walk_content(value: &Value, out: &mut String, logs: Option<&LogSender>, depth: usize) {
    if let Some(arr) = value.get("content").and_then(Value::as_array) {
        for child in arr {
            walk(child, out, logs, depth + 1);
        }
    }
}

/// Same as `walk_content` but strips a single trailing `\n` off the
/// accumulator after the children are walked. Used by `listItem` so a
/// list like `[p, p]` renders as `- a\n- b` rather than `- a\n\n- b`.
fn walk_content_trim_trailing_newline(
    value: &Value,
    out: &mut String,
    logs: Option<&LogSender>,
    depth: usize,
) {
    let before = out.len();
    walk_content(value, out, logs, depth);
    if out.len() > before && out.ends_with('\n') {
        out.pop();
    }
}

enum ListKind {
    Bullet,
    Ordered,
}

fn walk_list(
    value: &Value,
    out: &mut String,
    logs: Option<&LogSender>,
    depth: usize,
    kind: ListKind,
) {
    let Some(items) = value.get("content").and_then(Value::as_array) else {
        // `content` is not an array — shape violation, degrade.
        out.push_str(UNSUPPORTED_MARKER);
        if let Some(sender) = logs {
            sender.send(
                LogLevel::Warn,
                None,
                "adf: list node without array content".to_string(),
                serde_json::json!({ "depth": depth }),
            );
        }
        return;
    };
    for (idx, item) in items.iter().enumerate() {
        match kind {
            ListKind::Bullet => out.push_str("- "),
            ListKind::Ordered => {
                out.push_str(&format!("{}. ", idx + 1));
            }
        }
        walk(item, out, logs, depth + 1);
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dayseam_events::{LogReceiver, RunId, RunStreams};

    fn plain(json: &str) -> String {
        let v: Value = serde_json::from_str(json).expect("valid json");
        adf_to_plain(&v, None)
    }

    fn plain_with_logs(json: &str) -> (String, Vec<String>) {
        let v: Value = serde_json::from_str(json).expect("valid json");
        let streams = RunStreams::new(RunId::new());
        let ((_ptx, ltx), (_, mut lrx)) = streams.split();
        let out = adf_to_plain(&v, Some(&ltx));
        // Drop the sender so the receiver sees end-of-stream.
        drop(ltx);
        let mut logs = Vec::new();
        while let Ok(evt) = lrx.try_recv() {
            logs.push(evt.message);
        }
        drop::<LogReceiver>(lrx);
        (out, logs)
    }

    #[test]
    fn text_node_round_trips_verbatim() {
        let json = r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"hello"}]}]}"#;
        assert_eq!(plain(json), "hello");
    }

    /// The today's-real fixture from the spike: KTON-4550 comment
    /// body. Drives the plan's `adf_walker_reproduces_live_comment_text`
    /// invariant (paragraph + mention + text).
    #[test]
    fn paragraph_with_mention_reproduces_live_comment_text() {
        let json = r#"{
          "type":"doc","version":1,
          "content":[{
            "type":"paragraph",
            "content":[
              {"type":"mention","attrs":{"id":"712020:abc","text":"@Saravanan Ramanathan"}},
              {"type":"text","text":" while reproducing this bug, it takes us to octa page and not directly to authy approved. Can you update the replication steps please"}
            ]
          }]
        }"#;
        let rendered = plain(json);
        assert_eq!(
            rendered,
            "@Saravanan Ramanathan while reproducing this bug, it takes us to octa page and not directly to authy approved. Can you update the replication steps please"
        );
    }

    #[test]
    fn mention_never_emits_account_id_or_email_even_if_present() {
        // Spike §12 privacy invariant: `attrs.id` and `attrs.email`
        // must never surface in the rendered string.
        let json = r#"{"type":"doc","content":[{"type":"paragraph","content":[
          {"type":"mention","attrs":{
             "id":"712020:super-secret-account-id",
             "email":"someone@modulrfinance.com",
             "text":"@Someone"
          }}
        ]}]}"#;
        let rendered = plain(json);
        assert!(rendered.contains("@Someone"));
        assert!(!rendered.contains("712020"));
        assert!(!rendered.contains("super-secret"));
        assert!(!rendered.contains("modulrfinance.com"));
    }

    #[test]
    fn bullet_list_renders_as_dash_prefixed_lines() {
        let json = r#"{"type":"doc","content":[
          {"type":"bulletList","content":[
            {"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"one"}]}]},
            {"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"two"}]}]}
          ]}
        ]}"#;
        assert_eq!(plain(json), "- one\n- two");
    }

    #[test]
    fn ordered_list_renders_as_numbered_lines_starting_at_one() {
        let json = r#"{"type":"doc","content":[
          {"type":"orderedList","content":[
            {"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"alpha"}]}]},
            {"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"beta"}]}]}
          ]}
        ]}"#;
        assert_eq!(plain(json), "1. alpha\n2. beta");
    }

    #[test]
    fn code_block_renders_as_fenced_markdown_with_language() {
        let json = r#"{"type":"doc","content":[
          {"type":"codeBlock","attrs":{"language":"rust"},"content":[
            {"type":"text","text":"fn main() {}"}
          ]}
        ]}"#;
        assert_eq!(plain(json), "```rust\nfn main() {}\n```");
    }

    #[test]
    fn code_block_without_language_attr_still_fences() {
        let json = r#"{"type":"doc","content":[
          {"type":"codeBlock","content":[{"type":"text","text":"foo"}]}
        ]}"#;
        assert_eq!(plain(json), "```\nfoo\n```");
    }

    #[test]
    fn hard_break_emits_newline_inside_paragraph() {
        let json = r#"{"type":"doc","content":[{"type":"paragraph","content":[
          {"type":"text","text":"line1"},
          {"type":"hardBreak"},
          {"type":"text","text":"line2"}
        ]}]}"#;
        assert_eq!(plain(json), "line1\nline2");
    }

    #[test]
    fn heading_prefixes_with_hash_for_level() {
        let json = r#"{"type":"doc","content":[
          {"type":"heading","attrs":{"level":3},"content":[{"type":"text","text":"Heading"}]}
        ]}"#;
        assert_eq!(plain(json), "### Heading");
    }

    #[test]
    fn blockquote_prefixes_with_angle_bracket() {
        let json = r#"{"type":"doc","content":[
          {"type":"blockquote","content":[{"type":"paragraph","content":[{"type":"text","text":"quoted"}]}]}
        ]}"#;
        assert_eq!(plain(json), "> quoted");
    }

    #[test]
    fn image_and_media_are_stripped() {
        let json = r#"{"type":"doc","content":[
          {"type":"paragraph","content":[
            {"type":"text","text":"before "},
            {"type":"mediaSingle","content":[{"type":"media","attrs":{"id":"abc"}}]},
            {"type":"text","text":" after"}
          ]}
        ]}"#;
        assert_eq!(plain(json), "before  after");
    }

    #[test]
    fn inline_card_renders_url() {
        let json = r#"{"type":"doc","content":[{"type":"paragraph","content":[
          {"type":"inlineCard","attrs":{"url":"https://example.com/x"}}
        ]}]}"#;
        assert_eq!(plain(json), "https://example.com/x");
    }

    #[test]
    fn rule_inserts_horizontal_divider() {
        let json = r#"{"type":"doc","content":[
          {"type":"paragraph","content":[{"type":"text","text":"above"}]},
          {"type":"rule"},
          {"type":"paragraph","content":[{"type":"text","text":"below"}]}
        ]}"#;
        assert_eq!(plain(json), "above\n\n---\nbelow");
    }

    /// Drives the plan's `adf_walker_unknown_node_degrades` invariant.
    #[test]
    fn unknown_node_degrades_to_marker_and_emits_one_warn_log() {
        let json = r#"{"type":"doc","content":[
          {"type":"paragraph","content":[
            {"type":"text","text":"before "},
            {"type":"nonsense","attrs":{"whatever":"x"}},
            {"type":"text","text":" after"}
          ]}
        ]}"#;
        let (rendered, logs) = plain_with_logs(json);
        assert!(rendered.contains(UNSUPPORTED_MARKER), "got: {rendered}");
        assert!(rendered.contains("before "));
        assert!(rendered.contains(" after"));
        assert_eq!(logs.len(), 1, "expected exactly one warn log, got {logs:?}");
        assert!(logs[0].contains("nonsense"));
    }

    #[test]
    fn list_node_with_non_array_content_degrades() {
        let json = r#"{"type":"doc","content":[
          {"type":"bulletList","content":"not an array"}
        ]}"#;
        let (rendered, logs) = plain_with_logs(json);
        assert!(rendered.contains(UNSUPPORTED_MARKER));
        assert_eq!(logs.len(), 1);
    }

    #[test]
    fn empty_doc_round_trips_to_empty_string() {
        let json = r#"{"type":"doc","content":[]}"#;
        assert_eq!(plain(json), "");
    }

    #[test]
    fn mention_without_text_attr_emits_nothing_rather_than_panicking() {
        let json = r#"{"type":"doc","content":[{"type":"paragraph","content":[
          {"type":"mention","attrs":{"id":"712020:abc"}}
        ]}]}"#;
        assert_eq!(plain(json), "");
    }

    #[test]
    fn deeply_nested_paragraph_in_blockquote_in_list_does_not_overflow() {
        // Defensive against stack blow-up; the walker is recursive so
        // a pathological tree should stay shallow in practice, but we
        // still test a 4-level nest works.
        let json = r#"{"type":"doc","content":[
          {"type":"bulletList","content":[
            {"type":"listItem","content":[
              {"type":"blockquote","content":[
                {"type":"paragraph","content":[{"type":"text","text":"deep"}]}
              ]}
            ]}
          ]}
        ]}"#;
        assert_eq!(plain(json), "- > deep");
    }
}
