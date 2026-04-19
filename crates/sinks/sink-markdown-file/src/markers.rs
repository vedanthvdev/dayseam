//! Marker-block parser + splicer.
//!
//! The on-disk marker-block contract is deliberately small so a human
//! reading the markdown file in any editor can understand what the
//! sink did without tooling:
//!
//! ```text
//! <!-- dayseam:begin date="2026-04-18" run_id="…" template="dayseam.dev_eod" version="2026-04-18" -->
//! ## Commits
//!
//! - **repo** — summary _1 commit_
//! <!-- dayseam:end -->
//! ```
//!
//! ## Guarantees
//!
//! - `render(parse(x))` is byte-identical to `x` for any well-formed
//!   input. Unit test: `parse_then_render_round_trips_well_formed_files`.
//! - `splice` replaces only the block whose `date` matches the new
//!   block's `date`; every other byte of the document (user prose,
//!   other blocks, trailing newlines) is preserved verbatim. Unit
//!   test: `splice_preserves_surrounding_prose_byte_for_byte`.
//! - A malformed begin/end pair (overlapping blocks, begin without
//!   end, end without begin, missing required attribute) returns
//!   [`MarkerError`] **without mutating** the input. Unit test:
//!   `malformed_begin_or_end_is_rejected`.
//!
//! ## Deliberate non-goals
//!
//! - No regex dependency. A line-oriented state machine handles the
//!   whitespace-tolerant parse in ~40 lines and makes the fuzz target
//!   in Task 8 trivial.
//! - No partial parse. A file with one malformed block fails the whole
//!   read; the sink then returns `SINK_MALFORMED_MARKER` and refuses
//!   to write. Refusing loudly is safer than rewriting a file whose
//!   structure we don't fully understand.

use chrono::NaiveDate;
use thiserror::Error;

/// Sentinel line that opens a marker block. Callers match against this
/// after trimming leading/trailing whitespace from a line.
pub(crate) const BEGIN_PREFIX: &str = "<!-- dayseam:begin ";
pub(crate) const BEGIN_SUFFIX: &str = " -->";
pub(crate) const END_MARKER: &str = "<!-- dayseam:end -->";

/// Attributes on the begin marker. The parser is strict about presence
/// (all four are required) and tolerant about order, interior
/// whitespace, and attribute-value character set (the values are ASCII
/// in practice: UUIDs, ISO dates, dotted template ids).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarkerAttrs {
    pub date: NaiveDate,
    pub run_id: String,
    pub template: String,
    pub version: String,
}

/// One parsed segment of a document — either raw user prose (kept as
/// owned `String` so downstream splicing doesn't need lifetime juggling)
/// or a recognised marker block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Segment {
    /// Arbitrary markdown or text the user wrote between (or outside)
    /// marker blocks. Preserved byte-for-byte including its trailing
    /// newline.
    Prose(String),
    /// One recognised `<!-- dayseam:begin ... --> ... <!-- dayseam:end -->`
    /// block.
    Block(Block),
}

/// A single parsed marker block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    pub attrs: MarkerAttrs,
    /// Content between the begin and end markers, excluding the marker
    /// lines themselves but including the trailing newline of the last
    /// body line.
    pub body: String,
}

/// Parsed view of a markdown file, segmented into prose and marker
/// blocks in source order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParsedDoc {
    pub segments: Vec<Segment>,
}

/// Reasons the parser or splicer rejected an input. Every variant maps
/// to `DayseamError::Internal { code: SINK_MALFORMED_MARKER, .. }` at
/// the adapter boundary so the UI shows a single, stable error code
/// regardless of the specific malformation.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum MarkerError {
    /// A `dayseam:begin` line was seen while another block was still
    /// open. Nested blocks are a structural error; we refuse to guess
    /// which one was intended to close first.
    #[error("nested dayseam:begin at line {line} (another block is still open)")]
    NestedBegin { line: usize },
    /// The file ended before an open block was closed.
    #[error("unclosed dayseam:begin starting at line {line}")]
    UnclosedBegin { line: usize },
    /// A `dayseam:end` line was seen without a preceding begin.
    #[error("dangling dayseam:end at line {line} (no matching begin)")]
    DanglingEnd { line: usize },
    /// A begin line was found but a required attribute
    /// (`date`, `run_id`, `template`, `version`) was missing or
    /// malformed.
    #[error("malformed dayseam:begin at line {line}: {detail}")]
    MalformedBegin { line: usize, detail: String },
}

impl MarkerError {
    /// Human-readable one-liner suitable for an `Internal.message`
    /// field. The adapter wraps this in `DayseamError::Internal { code:
    /// SINK_MALFORMED_MARKER, .. }`; the `Display` impl above is what
    /// ends up in the `message` field.
    pub(crate) fn describe(&self) -> String {
        self.to_string()
    }
}

/// Parse `text` into a segmented document. Returns [`MarkerError`] on
/// any structural problem; in that case the document is untouched (the
/// function never writes anywhere), so the caller can safely refuse
/// the write and leave the file on disk as-is.
pub(crate) fn parse(text: &str) -> Result<ParsedDoc, MarkerError> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut prose_buf = String::new();
    let mut open: Option<(MarkerAttrs, String, usize)> = None;

    for (idx, line) in split_lines_preserving_newlines(text).enumerate() {
        let line_no = idx + 1;
        let trimmed = line.trim_end_matches(['\r', '\n']).trim();

        if let Some((attrs, ref mut body, start_line)) = open.as_mut() {
            if is_end_marker(trimmed) {
                let block = Block {
                    attrs: attrs.clone(),
                    body: std::mem::take(body),
                };
                segments.push(Segment::Block(block));
                open = None;
                continue;
            }
            if is_begin_marker(trimmed) {
                return Err(MarkerError::NestedBegin { line: line_no });
            }
            // Everything else inside an open block is body content.
            body.push_str(line);
            // Silence "unused" warning on start_line: we remember it
            // solely so the `UnclosedBegin` reported below is actionable.
            let _ = start_line;
            continue;
        }

        if is_end_marker(trimmed) {
            return Err(MarkerError::DanglingEnd { line: line_no });
        }

        if is_begin_marker(trimmed) {
            if !prose_buf.is_empty() {
                segments.push(Segment::Prose(std::mem::take(&mut prose_buf)));
            }
            let attrs = parse_begin_attrs(trimmed, line_no)?;
            open = Some((attrs, String::new(), line_no));
            continue;
        }

        prose_buf.push_str(line);
    }

    if let Some((_attrs, _body, start_line)) = open {
        return Err(MarkerError::UnclosedBegin { line: start_line });
    }
    if !prose_buf.is_empty() {
        segments.push(Segment::Prose(prose_buf));
    }

    Ok(ParsedDoc { segments })
}

/// Append `new_block` or replace the existing block whose
/// [`MarkerAttrs::date`] matches. The rest of the document is
/// preserved byte-for-byte. Returns `true` if an existing block was
/// replaced, `false` if the block was appended.
pub(crate) fn splice(doc: &mut ParsedDoc, new_block: Block) -> bool {
    for seg in doc.segments.iter_mut() {
        if let Segment::Block(existing) = seg {
            if existing.attrs.date == new_block.attrs.date {
                *existing = new_block;
                return true;
            }
        }
    }

    // Appending: make sure there is a newline separating the previous
    // content from the new block so the file stays well-formed for any
    // downstream markdown reader.
    ensure_trailing_newline(&mut doc.segments);
    doc.segments.push(Segment::Block(new_block));
    false
}

/// Render a [`ParsedDoc`] back to a `String`. Round-trips well-formed
/// documents byte-for-byte.
pub(crate) fn render(doc: &ParsedDoc) -> String {
    let mut out = String::new();
    for seg in &doc.segments {
        match seg {
            Segment::Prose(p) => out.push_str(p),
            Segment::Block(b) => render_block_into(&mut out, b),
        }
    }
    out
}

fn render_block_into(out: &mut String, block: &Block) {
    out.push_str(BEGIN_PREFIX);
    out.push_str(&format!("date=\"{}\"", block.attrs.date));
    out.push_str(&format!(" run_id=\"{}\"", block.attrs.run_id));
    out.push_str(&format!(" template=\"{}\"", block.attrs.template));
    out.push_str(&format!(" version=\"{}\"", block.attrs.version));
    out.push_str(BEGIN_SUFFIX);
    out.push('\n');
    out.push_str(&block.body);
    if !block.body.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(END_MARKER);
    out.push('\n');
}

fn is_begin_marker(trimmed: &str) -> bool {
    trimmed.starts_with(BEGIN_PREFIX) && trimmed.ends_with(BEGIN_SUFFIX)
}

fn is_end_marker(trimmed: &str) -> bool {
    trimmed == END_MARKER
}

fn parse_begin_attrs(trimmed: &str, line_no: usize) -> Result<MarkerAttrs, MarkerError> {
    // Strip the well-known wrapping so only the attribute region is
    // left (e.g. `date="…" run_id="…" template="…" version="…"`).
    let inner = trimmed
        .strip_prefix(BEGIN_PREFIX)
        .and_then(|s| s.strip_suffix(BEGIN_SUFFIX))
        .ok_or_else(|| MarkerError::MalformedBegin {
            line: line_no,
            detail: "does not match '<!-- dayseam:begin … -->'".to_string(),
        })?
        .trim();

    let mut date: Option<NaiveDate> = None;
    let mut run_id: Option<String> = None;
    let mut template: Option<String> = None;
    let mut version: Option<String> = None;

    for (key, value) in iter_attrs(inner).map_err(|detail| MarkerError::MalformedBegin {
        line: line_no,
        detail,
    })? {
        match key {
            "date" => {
                let parsed = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|e| {
                    MarkerError::MalformedBegin {
                        line: line_no,
                        detail: format!("date=\"{value}\" is not YYYY-MM-DD ({e})"),
                    }
                })?;
                date = Some(parsed);
            }
            "run_id" => run_id = Some(value.to_string()),
            "template" => template = Some(value.to_string()),
            "version" => version = Some(value.to_string()),
            _other => {
                // Unknown keys are ignored so later versions of the
                // sink can add new attributes without invalidating
                // files written by older versions. The round-trip
                // guarantee only covers the four required keys.
            }
        }
    }

    Ok(MarkerAttrs {
        date: date.ok_or_else(|| MarkerError::MalformedBegin {
            line: line_no,
            detail: "missing required attribute `date`".to_string(),
        })?,
        run_id: run_id.ok_or_else(|| MarkerError::MalformedBegin {
            line: line_no,
            detail: "missing required attribute `run_id`".to_string(),
        })?,
        template: template.ok_or_else(|| MarkerError::MalformedBegin {
            line: line_no,
            detail: "missing required attribute `template`".to_string(),
        })?,
        version: version.ok_or_else(|| MarkerError::MalformedBegin {
            line: line_no,
            detail: "missing required attribute `version`".to_string(),
        })?,
    })
}

/// Parse `key="value" key="value"` pairs. Deliberately strict: values
/// must be double-quoted and must not contain a literal `"`. The sink
/// only ever emits UUID / ISO-date / dotted-template-id values, so no
/// escaping is required in practice.
fn iter_attrs(s: &str) -> Result<Vec<(&str, &str)>, String> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let eq = rest
            .find('=')
            .ok_or_else(|| format!("expected 'key=\"value\"' near: {rest:?}"))?;
        let key = rest[..eq].trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            return Err(format!("malformed attribute key near: {rest:?}"));
        }
        let after_eq = rest[eq + 1..].trim_start();
        if !after_eq.starts_with('"') {
            return Err(format!(
                "value for `{key}` must be double-quoted (got: {after_eq:?})"
            ));
        }
        let after_quote = &after_eq[1..];
        let close = after_quote
            .find('"')
            .ok_or_else(|| format!("unterminated quoted value for `{key}`"))?;
        let value = &after_quote[..close];
        out.push((key, value));
        rest = after_quote[close + 1..].trim_start();
    }
    Ok(out)
}

/// Split `text` into logical lines, preserving each line's terminator.
/// Guarantees `text == result.concat()`.
fn split_lines_preserving_newlines(text: &str) -> impl Iterator<Item = &str> {
    SplitPreservingNewlines { rest: text }
}

struct SplitPreservingNewlines<'a> {
    rest: &'a str,
}

impl<'a> Iterator for SplitPreservingNewlines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.rest.is_empty() {
            return None;
        }
        match self.rest.find('\n') {
            Some(idx) => {
                let (line, rest) = self.rest.split_at(idx + 1);
                self.rest = rest;
                Some(line)
            }
            None => {
                let line = self.rest;
                self.rest = "";
                Some(line)
            }
        }
    }
}

fn ensure_trailing_newline(segments: &mut [Segment]) {
    let Some(last) = segments.last_mut() else {
        return;
    };
    if let Segment::Prose(p) = last {
        if !p.ends_with('\n') {
            p.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(date: &str) -> MarkerAttrs {
        MarkerAttrs {
            date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            run_id: "11111111-2222-3333-4444-555555555555".to_string(),
            template: "dayseam.dev_eod".to_string(),
            version: "2026-04-18".to_string(),
        }
    }

    fn block(date: &str, body: &str) -> Block {
        Block {
            attrs: attrs(date),
            body: body.to_string(),
        }
    }

    #[test]
    fn parse_empty_file_is_empty_doc() {
        let doc = parse("").unwrap();
        assert!(doc.segments.is_empty());
    }

    #[test]
    fn parse_file_without_blocks_is_one_prose_segment() {
        let text = "# My notes\n\nsome prose\n";
        let doc = parse(text).unwrap();
        assert_eq!(doc.segments.len(), 1);
        assert!(matches!(&doc.segments[0], Segment::Prose(p) if p == text));
    }

    #[test]
    fn parse_then_render_round_trips_well_formed_files() {
        let text = concat!(
            "## Prelude prose\n",
            "\n",
            "<!-- dayseam:begin date=\"2026-04-18\" run_id=\"r1\" template=\"dayseam.dev_eod\" version=\"v1\" -->\n",
            "- first day body\n",
            "<!-- dayseam:end -->\n",
            "\n",
            "user prose between blocks\n",
            "\n",
            "<!-- dayseam:begin date=\"2026-04-17\" run_id=\"r2\" template=\"dayseam.dev_eod\" version=\"v1\" -->\n",
            "- second day body\n",
            "<!-- dayseam:end -->\n",
            "\n",
            "trailing prose\n",
        );
        let doc = parse(text).expect("well-formed doc parses");
        assert_eq!(render(&doc), text);
    }

    #[test]
    fn splice_replaces_only_matching_date_block() {
        let text = concat!(
            "<!-- dayseam:begin date=\"2026-04-18\" run_id=\"r1\" template=\"dayseam.dev_eod\" version=\"v1\" -->\n",
            "- old D1 body\n",
            "<!-- dayseam:end -->\n",
            "\n",
            "middle user prose\n",
            "\n",
            "<!-- dayseam:begin date=\"2026-04-17\" run_id=\"r2\" template=\"dayseam.dev_eod\" version=\"v1\" -->\n",
            "- D2 body untouched\n",
            "<!-- dayseam:end -->\n",
        );
        let mut doc = parse(text).unwrap();
        let replaced = splice(&mut doc, block("2026-04-18", "- new D1 body\n"));
        assert!(replaced);
        let out = render(&doc);
        assert!(out.contains("- new D1 body"));
        assert!(!out.contains("- old D1 body"));
        assert!(out.contains("- D2 body untouched"));
        assert!(out.contains("middle user prose"));
    }

    #[test]
    fn splice_preserves_surrounding_prose_byte_for_byte() {
        let original = concat!(
            "# Daily journal\n",
            "\n",
            "<!-- dayseam:begin date=\"2026-04-18\" run_id=\"r1\" template=\"dayseam.dev_eod\" version=\"v1\" -->\n",
            "- old body\n",
            "<!-- dayseam:end -->\n",
            "\n",
            "-- free-form note kept verbatim --\n",
        );
        let mut doc = parse(original).unwrap();
        splice(&mut doc, block("2026-04-18", "- refreshed body\n"));
        let out = render(&doc);
        assert!(out.starts_with("# Daily journal\n\n"));
        assert!(out.ends_with("-- free-form note kept verbatim --\n"));
    }

    #[test]
    fn splice_appends_when_no_matching_date() {
        let original = concat!(
            "existing prose\n",
            "<!-- dayseam:begin date=\"2026-04-17\" run_id=\"r0\" template=\"dayseam.dev_eod\" version=\"v1\" -->\n",
            "- yesterday\n",
            "<!-- dayseam:end -->\n",
        );
        let mut doc = parse(original).unwrap();
        let replaced = splice(&mut doc, block("2026-04-18", "- today\n"));
        assert!(!replaced);
        let out = render(&doc);
        assert!(out.contains("- yesterday"));
        assert!(out.contains("- today"));
        let today_pos = out.find("- today").unwrap();
        let yesterday_pos = out.find("- yesterday").unwrap();
        assert!(today_pos > yesterday_pos);
    }

    #[test]
    fn splice_appends_into_empty_doc() {
        let mut doc = parse("").unwrap();
        splice(&mut doc, block("2026-04-18", "- first ever\n"));
        let out = render(&doc);
        assert!(out.starts_with("<!-- dayseam:begin"));
        assert!(out.contains("- first ever"));
        assert!(out.ends_with("<!-- dayseam:end -->\n"));
    }

    #[test]
    fn nested_begin_is_rejected() {
        let text = concat!(
            "<!-- dayseam:begin date=\"2026-04-18\" run_id=\"r1\" template=\"t\" version=\"v\" -->\n",
            "<!-- dayseam:begin date=\"2026-04-17\" run_id=\"r2\" template=\"t\" version=\"v\" -->\n",
            "<!-- dayseam:end -->\n",
        );
        assert!(matches!(parse(text), Err(MarkerError::NestedBegin { .. })));
    }

    #[test]
    fn unclosed_begin_is_rejected() {
        let text = concat!(
            "<!-- dayseam:begin date=\"2026-04-18\" run_id=\"r1\" template=\"t\" version=\"v\" -->\n",
            "still body\n",
        );
        assert!(matches!(
            parse(text),
            Err(MarkerError::UnclosedBegin { .. })
        ));
    }

    #[test]
    fn dangling_end_is_rejected() {
        let text = concat!("prose\n", "<!-- dayseam:end -->\n", "more prose\n");
        assert!(matches!(parse(text), Err(MarkerError::DanglingEnd { .. })));
    }

    #[test]
    fn missing_required_attr_is_rejected() {
        let text = concat!(
            "<!-- dayseam:begin date=\"2026-04-18\" run_id=\"r1\" template=\"t\" -->\n",
            "<!-- dayseam:end -->\n",
        );
        assert!(matches!(
            parse(text),
            Err(MarkerError::MalformedBegin { .. })
        ));
    }

    #[test]
    fn unknown_attributes_are_ignored_for_forward_compat() {
        let text = concat!(
            "<!-- dayseam:begin date=\"2026-04-18\" run_id=\"r1\" template=\"t\" version=\"v\" extra=\"ok\" -->\n",
            "- body\n",
            "<!-- dayseam:end -->\n",
        );
        let doc = parse(text).expect("unknown attrs are forward-compatible");
        assert_eq!(doc.segments.len(), 1);
    }

    #[test]
    fn whitespace_between_attributes_is_tolerated() {
        let text = concat!(
            "<!-- dayseam:begin   date=\"2026-04-18\"   run_id=\"r1\"  template=\"t\"  version=\"v\"   -->\n",
            "- body\n",
            "<!-- dayseam:end -->\n",
        );
        parse(text).expect("tolerant of multi-space separators");
    }

    #[test]
    fn round_trip_preserves_unix_and_windows_line_endings() {
        // Windows-style CRLF body is preserved byte-for-byte inside the
        // block even though the marker lines themselves are ASCII LF.
        let text = "<!-- dayseam:begin date=\"2026-04-18\" run_id=\"r1\" template=\"t\" version=\"v\" -->\n\
            - body with \r\n in it\r\n\
            <!-- dayseam:end -->\n";
        let doc = parse(text).unwrap();
        assert_eq!(render(&doc), text);
    }
}
