//! Optional YAML frontmatter block at the top of the rendered file.
//!
//! Enabled per-sink via `SinkConfig::MarkdownFile { frontmatter: true }`,
//! the frontmatter is a short, Obsidian-Dataview-indexable header:
//!
//! ```yaml
//! ---
//! date: 2026-04-18
//! template: dayseam.dev_eod
//! template_version: 2026-04-18
//! generated_at: 2026-04-18T22:15:09Z
//! ---
//! ```
//!
//! ## Merge semantics
//!
//! If the target file already carries a frontmatter block, this module
//! merges rather than replaces:
//!
//! - Every existing key is preserved verbatim (including unknown keys
//!   the user may have added by hand for personal workflows).
//! - Only `generated_at` is rewritten on every save.
//! - Our four required keys (`date`, `template`, `template_version`,
//!   `generated_at`) are inserted if absent.
//!
//! That combination means the sink can be turned on for an existing
//! Obsidian file and the user's hand-maintained metadata (tags,
//! aliases, custom view flags) stays intact.
//!
//! ## Deliberate non-goals
//!
//! - No full YAML parser. Frontmatter in practice is a flat map of
//!   `key: scalar` pairs, which is all the spec above uses. Anything
//!   more exotic (nested maps, multi-line scalars) is preserved
//!   byte-for-byte as a single `Other` line — the merge only rewrites
//!   the keys we own.
//! - No `serde_yaml` dependency. Same cost/benefit argument as
//!   `markers`: a small hand-rolled reader is easier to reason about
//!   and keeps the crate's dependency surface minimal.

use chrono::{DateTime, NaiveDate, Utc};

/// Delimiter line opening and closing a YAML frontmatter block.
const DELIM: &str = "---";

/// Fields the markdown-file sink writes on every save.
#[derive(Debug, Clone)]
pub(crate) struct FrontmatterFields {
    pub date: NaiveDate,
    pub template: String,
    pub template_version: String,
    pub generated_at: DateTime<Utc>,
}

/// Split a full-file string into `(frontmatter_block, body)`.
/// Returns `(None, original)` if the file does not start with a
/// `---` delimiter. The returned `frontmatter_block` includes the two
/// delimiter lines and every line in between; the body is everything
/// after the closing delimiter's trailing newline.
pub(crate) fn split(text: &str) -> (Option<&str>, &str) {
    // A well-formed opening is `---\n` at offset 0; anything else
    // (leading prose, BOM, a bare `---` at EOF) means "no frontmatter".
    if !text.starts_with(DELIM) {
        return (None, text);
    }
    let search_start = DELIM.len() + 1;
    if text.len() < search_start || !text[DELIM.len()..].starts_with('\n') {
        return (None, text);
    }
    let remaining = &text[search_start..];
    let Some(close_rel) = find_delim_line(remaining) else {
        return (None, text);
    };
    let close_abs = search_start + close_rel;
    let mut end_of_block = close_abs + DELIM.len();
    if text[end_of_block..].starts_with('\n') {
        end_of_block += 1;
    }
    (Some(&text[..end_of_block]), &text[end_of_block..])
}

/// Return the byte offset of a line that contains only `---` (ignoring
/// trailing whitespace), or `None` if no such line exists.
fn find_delim_line(text: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']).trim();
        if trimmed == DELIM {
            return Some(offset);
        }
        offset += line.len();
    }
    // Handle final line without trailing newline.
    if offset < text.len() {
        let tail = &text[offset..];
        if tail.trim_end_matches(['\r', '\n']).trim() == DELIM {
            return Some(offset);
        }
    }
    None
}

/// Produce a `---\n…\n---\n` block whose keys are the union of
/// `existing` frontmatter (if any) and the four `fields` keys. The
/// only key this function rewrites on every call is `generated_at`;
/// every other key is preserved verbatim, including unknown ones that
/// the user may have added by hand.
pub(crate) fn merge(existing: Option<&str>, fields: &FrontmatterFields) -> String {
    let mut entries: Vec<Entry> = existing.map(parse_flat).unwrap_or_default();

    upsert(&mut entries, "date", &fields.date.to_string());
    upsert(&mut entries, "template", &fields.template);
    upsert(&mut entries, "template_version", &fields.template_version);
    // RFC 3339 with a `Z` suffix reads cleanly both to humans and to
    // `chrono::DateTime::parse_from_rfc3339`.
    let ts = fields
        .generated_at
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    upsert(&mut entries, "generated_at", &ts);

    let mut out = String::new();
    out.push_str(DELIM);
    out.push('\n');
    for entry in &entries {
        match entry {
            Entry::Pair { key, value } => {
                out.push_str(key);
                out.push_str(": ");
                out.push_str(value);
                out.push('\n');
            }
            Entry::Other(raw) => {
                out.push_str(raw);
                if !raw.ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }
    out.push_str(DELIM);
    out.push('\n');
    out
}

#[derive(Debug, Clone)]
enum Entry {
    Pair {
        key: String,
        value: String,
    },
    /// Line we did not recognise as `key: value`. Preserved verbatim
    /// so multi-line scalars (`description: |`), comments, and blank
    /// spacing all round-trip.
    Other(String),
}

fn parse_flat(block: &str) -> Vec<Entry> {
    // Strip the surrounding delimiters so only body lines remain.
    let inner = block
        .strip_prefix(DELIM)
        .and_then(|s| s.strip_prefix('\n'))
        .unwrap_or(block);
    let mut body = inner;
    if let Some(close) = find_delim_line(body) {
        body = &body[..close];
    }

    body.lines().map(parse_line).collect()
}

fn parse_line(line: &str) -> Entry {
    // Match `key: value` with a non-empty bareword key. Anything else
    // — comments, blank lines, multi-line scalar markers — is
    // preserved as `Other` so we never silently mangle user YAML.
    if let Some((key, value)) = split_key_value(line) {
        Entry::Pair {
            key: key.to_string(),
            value: value.to_string(),
        }
    } else {
        Entry::Other(line.to_string())
    }
}

fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.is_empty() {
        return None;
    }
    let (key, rest) = trimmed.split_once(':')?;
    let key = key.trim();
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    let value = rest.trim_start().trim_end_matches(['\r', '\n']);
    Some((key, value))
}

fn upsert(entries: &mut Vec<Entry>, key: &str, value: &str) {
    for entry in entries.iter_mut() {
        if let Entry::Pair {
            key: k,
            value: existing,
        } = entry
        {
            if k == key {
                *existing = value.to_string();
                return;
            }
        }
    }
    entries.push(Entry::Pair {
        key: key.to_string(),
        value: value.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_fields() -> FrontmatterFields {
        FrontmatterFields {
            date: NaiveDate::from_ymd_opt(2026, 4, 18).unwrap(),
            template: "dayseam.dev_eod".to_string(),
            template_version: "2026-04-18".to_string(),
            generated_at: Utc.with_ymd_and_hms(2026, 4, 18, 22, 15, 9).unwrap(),
        }
    }

    #[test]
    fn split_returns_none_when_file_has_no_frontmatter() {
        let text = "# Journal\n\nbody line\n";
        let (fm, body) = split(text);
        assert!(fm.is_none());
        assert_eq!(body, text);
    }

    #[test]
    fn split_separates_frontmatter_and_body() {
        let text = "---\nkey: val\n---\n# Journal\n";
        let (fm, body) = split(text);
        assert_eq!(fm.unwrap(), "---\nkey: val\n---\n");
        assert_eq!(body, "# Journal\n");
    }

    #[test]
    fn merge_creates_fresh_frontmatter_when_none_exists() {
        let out = merge(None, &sample_fields());
        assert!(out.starts_with("---\n"));
        assert!(out.ends_with("---\n"));
        assert!(out.contains("date: 2026-04-18"));
        assert!(out.contains("template: dayseam.dev_eod"));
        assert!(out.contains("template_version: 2026-04-18"));
        assert!(out.contains("generated_at: 2026-04-18T22:15:09Z"));
    }

    #[test]
    fn merge_preserves_unknown_user_keys() {
        let existing = "---\ntags: [daily, work]\naliases:\n  - eod\ndate: 2026-04-18\n---\n";
        let out = merge(Some(existing), &sample_fields());
        assert!(out.contains("tags: [daily, work]"));
        assert!(
            out.contains("aliases:") && out.contains("  - eod"),
            "multi-line scalars must be preserved verbatim: {out}"
        );
        // date stays the same — our value matches the existing one.
        assert!(out.contains("date: 2026-04-18"));
    }

    #[test]
    fn merge_rewrites_only_generated_at_when_other_keys_match() {
        let existing = concat!(
            "---\n",
            "date: 2026-04-18\n",
            "template: dayseam.dev_eod\n",
            "template_version: 2026-04-18\n",
            "generated_at: 2020-01-01T00:00:00Z\n",
            "---\n",
        );
        let out = merge(Some(existing), &sample_fields());
        assert!(
            !out.contains("2020-01-01"),
            "generated_at must be rewritten on save"
        );
        assert!(out.contains("generated_at: 2026-04-18T22:15:09Z"));
        // Other required keys are not duplicated even though we called upsert.
        let date_hits = out.matches("date: ").count();
        assert_eq!(date_hits, 1, "date must not be duplicated: {out}");
    }

    #[test]
    fn merge_adds_missing_required_keys() {
        let existing = "---\ntags: [daily]\n---\n";
        let out = merge(Some(existing), &sample_fields());
        assert!(out.contains("tags: [daily]"));
        assert!(out.contains("date: 2026-04-18"));
        assert!(out.contains("template: dayseam.dev_eod"));
        assert!(out.contains("template_version: 2026-04-18"));
        assert!(out.contains("generated_at: 2026-04-18T22:15:09Z"));
    }
}
