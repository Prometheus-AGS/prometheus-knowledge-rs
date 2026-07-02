//! OKF bundle-level artifacts: the reserved `index.md` (§6) and `log.md`
//! (§7) files, plus body-link extraction (§5) used to derive the link graph
//! from markdown bodies rather than a frontmatter array.
//!
//! Everything here is a pure function of its inputs so it can be unit-tested
//! without a store or an LLM; `MarkdownStore` wires these to disk.

use pk_core::types::{ArticleId, LintReport, LintSeverity, WikiEntry};
use pulldown_cmark::{Event, Parser, Tag};
use std::collections::HashSet;

/// OKF §5.1: a bundle-relative link begins with `/` (interpreted from the
/// bundle root) and, for a concept link, ends in `.md`. Converts such a link
/// destination to its concept ID (path minus leading `/` and `.md` suffix).
/// Returns `None` for external URLs, anchors, and non-`.md` targets.
fn bundle_link_to_concept_id(dest: &str) -> Option<ArticleId> {
    let dest = dest.trim();
    if !dest.starts_with('/') || !dest.ends_with(".md") {
        return None;
    }
    let id = dest.trim_start_matches('/').trim_end_matches(".md");
    if id.is_empty() {
        return None;
    }
    Some(ArticleId::from(id))
}

/// Extract the concept IDs a markdown body links to via bundle-relative
/// links (OKF §5). This is the source of truth for the link graph; the
/// frontmatter `links` array is retained only for back-compat on read.
/// Order-preserving and deduplicated.
pub fn extract_body_links(content: &str) -> Vec<ArticleId> {
    let mut links: Vec<ArticleId> = Vec::new();
    for event in Parser::new(content) {
        if let Event::Start(Tag::Link { dest_url, .. }) = event {
            if let Some(id) = bundle_link_to_concept_id(&dest_url) {
                if !links.contains(&id) {
                    links.push(id);
                }
            }
        }
    }
    links
}

const INDEX_TITLE: &str = "# Wiki Index";
const LOG_TITLE: &str = "# Update Log";

/// Render OKF §6 `index.md` from the current entries, grouped by concept
/// `type`. Each entry is listed as `* [Title](/id.md) - description`, with
/// the description taken from frontmatter when present. Groups and entries
/// are sorted for deterministic output (stable diffs). Contains no
/// frontmatter, per §6.
pub fn render_index(entries: &[WikiEntry]) -> String {
    use std::collections::BTreeMap;

    let mut groups: BTreeMap<String, Vec<&WikiEntry>> = BTreeMap::new();
    for e in entries {
        let group = e
            .entry_type
            .clone()
            .unwrap_or_else(|| "Uncategorized".to_string());
        groups.entry(group).or_default().push(e);
    }

    let mut out = String::new();
    out.push_str(INDEX_TITLE);
    out.push_str("\n\n");

    if entries.is_empty() {
        out.push_str("_No entries yet._\n");
        return out;
    }

    for (group, mut items) in groups {
        items.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        out.push_str("## ");
        out.push_str(&group);
        out.push_str("\n\n");
        for e in items {
            out.push_str("* [");
            out.push_str(&e.title);
            out.push_str("](/");
            out.push_str(e.id.as_str());
            out.push_str(".md)");
            if let Some(desc) = e.description.as_deref().filter(|d| !d.trim().is_empty()) {
                out.push_str(" - ");
                out.push_str(desc.trim());
            }
            out.push('\n');
        }
        out.push('\n');
    }

    // Trim the trailing blank line for a stable single-newline ending.
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Insert `line` under the `date` group in an existing OKF §7 `log.md` body,
/// newest date first. Creates the log title and/or the date group when
/// absent; prepends within an existing date group so the most recent entry
/// leads. `date` must be ISO 8601 `YYYY-MM-DD`.
pub fn append_log_line(existing: &str, date: &str, line: &str) -> String {
    let date_heading = format!("## {date}");

    // Fresh log: title + first date group.
    if existing.trim().is_empty() {
        return format!("{LOG_TITLE}\n\n{date_heading}\n{line}\n");
    }

    let mut lines: Vec<String> = existing.lines().map(str::to_owned).collect();

    // Find the existing group for this date, if any.
    if let Some(pos) = lines.iter().position(|l| l.trim() == date_heading) {
        // Insert the new line immediately after the date heading so the
        // newest entry within the day leads.
        lines.insert(pos + 1, line.to_owned());
        return finish_log(lines);
    }

    // No group for this date yet: insert a new group ahead of the first
    // existing `## ` date heading (newest-first), or after the title.
    let insert_at = lines
        .iter()
        .position(|l| l.trim_start().starts_with("## "))
        .unwrap_or_else(|| {
            // No date groups yet; place after the title line (+ following
            // blank line if present).
            let title_pos = lines
                .iter()
                .position(|l| l.trim() == LOG_TITLE)
                .map(|p| p + 1)
                .unwrap_or(0);
            title_pos
        });

    let block = vec![date_heading.clone(), line.to_owned(), String::new()];
    for (i, b) in block.into_iter().enumerate() {
        lines.insert(insert_at + i, b);
    }
    finish_log(lines)
}

fn finish_log(lines: Vec<String>) -> String {
    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// OKF v0.1 §9 conformance lint (deterministic; no LLM).
//
// Permissive consumption (§9) shapes the severity split: the ONLY hard
// requirements are a parseable frontmatter block and a non-empty `type` —
// those are errors. Everything else (missing recommended fields, unknown
// types, broken cross-links, orphans, reserved-file shape) is advisory and
// is reported as a warning, never a rejection.
// ---------------------------------------------------------------------------

fn report(
    id: Option<&str>,
    severity: LintSeverity,
    issue: impl Into<String>,
    suggestion: impl Into<String>,
    auto_fixable: bool,
) -> LintReport {
    LintReport {
        entry_id: id.map(ArticleId::from),
        severity,
        issue: issue.into(),
        suggestion: suggestion.into(),
        auto_fixable,
    }
}

/// OKF §9 conformance checks for one concept document. `raw` is the file's
/// full content; `concept_id` is its wiki-relative id (used as the parse
/// fallback and the report subject); `known_ids` is every concept ID in the
/// bundle, for broken-link detection.
pub fn okf_document_reports(
    concept_id: &str,
    raw: &str,
    known_ids: &HashSet<String>,
) -> Vec<LintReport> {
    let mut reports = Vec::new();

    // §9.1: parseable frontmatter is a hard requirement.
    let entry = match crate::markdown::markdown_to_entry(raw, Some(concept_id)) {
        Ok(entry) => entry,
        Err(e) => {
            reports.push(report(
                Some(concept_id),
                LintSeverity::Error,
                format!("frontmatter does not parse: {e}"),
                "fix the YAML frontmatter block (delimited by --- fences)",
                false,
            ));
            return reports;
        }
    };

    // §9.2: a non-empty `type` is the format's one required field.
    if entry.entry_type.as_deref().map(str::trim).unwrap_or("").is_empty() {
        reports.push(report(
            Some(concept_id),
            LintSeverity::Error,
            "missing or empty required OKF `type` frontmatter key",
            "add a non-empty `type:` line (e.g. `type: Reference`)",
            true,
        ));
    }

    // Recommended field (§4.1): advisory only.
    if entry.description.as_deref().map(str::trim).unwrap_or("").is_empty() {
        reports.push(report(
            Some(concept_id),
            LintSeverity::Warning,
            "missing recommended OKF `description`",
            "add a one-sentence `description:` for index and search snippets",
            false,
        ));
    }

    // Broken cross-links (§5): tolerated, so warn — never error.
    for link in extract_body_links(&entry.content) {
        if !known_ids.contains(link.as_str()) {
            reports.push(report(
                Some(concept_id),
                LintSeverity::Warning,
                format!("body links to /{}.md, which is not in the bundle", link.as_str()),
                "create the target page or fix the link (OKF §5 tolerates broken links)",
                false,
            ));
        }
    }

    reports
}

/// OKF orphan detection: concepts with no inbound body link from any other
/// concept. Advisory (§: healthy-wiki guidance, not conformance). Skipped
/// for a single-entry bundle where "orphan" is meaningless.
pub fn okf_orphan_reports(entries: &[WikiEntry]) -> Vec<LintReport> {
    let mut reports = Vec::new();
    if entries.len() <= 1 {
        return reports;
    }
    let mut linked_to: HashSet<String> = HashSet::new();
    for e in entries {
        for l in extract_body_links(&e.content) {
            linked_to.insert(l.0);
        }
    }
    for e in entries {
        if !linked_to.contains(e.id.as_str()) {
            reports.push(report(
                Some(e.id.as_str()),
                LintSeverity::Warning,
                "orphan page: no inbound links from any other page",
                "link to this page from a related page or from index.md",
                false,
            ));
        }
    }
    reports
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..].iter().all(u8::is_ascii_digit)
}

/// OKF §6 structure check for a root `index.md`: it carries no frontmatter,
/// except that the bundle-root index MAY carry a block declaring only
/// `okf_version` (§11). Any other leading `---` block is a warning.
pub fn okf_index_reports(raw: &str) -> Vec<LintReport> {
    let mut reports = Vec::new();
    let trimmed = raw.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let fm = &rest[..end];
            let only_okf_version = fm
                .lines()
                .filter(|l| !l.trim().is_empty())
                .all(|l| l.trim_start().starts_with("okf_version"));
            if !only_okf_version {
                reports.push(report(
                    Some("index.md"),
                    LintSeverity::Warning,
                    "index.md carries frontmatter beyond an okf_version declaration",
                    "remove the frontmatter (OKF §6: index.md has none; only the root MAY declare okf_version)",
                    false,
                ));
            }
        }
    }
    reports
}

/// OKF §7 structure check for `log.md`: every `## ` heading is an ISO
/// `YYYY-MM-DD` date.
pub fn okf_log_reports(raw: &str) -> Vec<LintReport> {
    let mut reports = Vec::new();
    for line in raw.lines() {
        if let Some(heading) = line.trim().strip_prefix("## ") {
            if !is_iso_date(heading.trim()) {
                reports.push(report(
                    Some("log.md"),
                    LintSeverity::Warning,
                    format!("log.md date heading {heading:?} is not ISO YYYY-MM-DD"),
                    "use `## YYYY-MM-DD` date headings (OKF §7)",
                    false,
                ));
            }
        }
    }
    reports
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, title: &str, ty: Option<&str>, desc: Option<&str>) -> WikiEntry {
        let mut e = WikiEntry::new(title, "body");
        e.id = ArticleId::from(id);
        e.entry_type = ty.map(str::to_owned);
        e.description = desc.map(str::to_owned);
        e
    }

    #[test]
    fn extracts_bundle_relative_links_only() {
        let body = "See [Orders](/tables/orders.md) and [Customers](/tables/customers.md). \
                    External [site](https://example.com) and [rel](./other.md) and \
                    [anchor](#section) are ignored.";
        let links = extract_body_links(body);
        assert_eq!(
            links,
            vec![
                ArticleId::from("tables/orders"),
                ArticleId::from("tables/customers")
            ]
        );
    }

    #[test]
    fn body_links_are_deduplicated() {
        let body = "[A](/a.md) then [A again](/a.md) then [B](/b.md).";
        let links = extract_body_links(body);
        assert_eq!(links, vec![ArticleId::from("a"), ArticleId::from("b")]);
    }

    #[test]
    fn index_groups_by_type_with_descriptions() {
        let entries = vec![
            entry("orders", "Orders", Some("Table"), Some("One row per order.")),
            entry("triage", "Triage", Some("Playbook"), None),
            entry("customers", "Customers", Some("Table"), Some("One row per customer.")),
        ];
        let idx = render_index(&entries);
        assert!(idx.starts_with("# Wiki Index"));
        assert!(idx.contains("## Table"));
        assert!(idx.contains("## Playbook"));
        // Within a group, entries are alphabetical by title.
        let customers = idx.find("[Customers]").unwrap();
        let orders = idx.find("[Orders]").unwrap();
        assert!(customers < orders);
        assert!(idx.contains("* [Orders](/orders.md) - One row per order."));
        // No description → no trailing " - ".
        assert!(idx.contains("* [Triage](/triage.md)\n"));
    }

    #[test]
    fn index_handles_empty() {
        assert!(render_index(&[]).contains("_No entries yet._"));
    }

    #[test]
    fn log_creates_fresh_with_title_and_group() {
        let out = append_log_line("", "2026-07-02", "* **Creation**: [Foo](/foo.md)");
        assert_eq!(
            out,
            "# Update Log\n\n## 2026-07-02\n* **Creation**: [Foo](/foo.md)\n"
        );
    }

    #[test]
    fn log_prepends_within_existing_date_group() {
        let existing = "# Update Log\n\n## 2026-07-02\n* **Creation**: [Foo](/foo.md)\n";
        let out = append_log_line(existing, "2026-07-02", "* **Update**: [Bar](/bar.md)");
        let foo = out.find("[Foo]").unwrap();
        let bar = out.find("[Bar]").unwrap();
        // Newest entry within the day leads.
        assert!(bar < foo);
        assert_eq!(out.matches("## 2026-07-02").count(), 1);
    }

    #[test]
    fn log_inserts_new_date_group_newest_first() {
        let existing = "# Update Log\n\n## 2026-07-01\n* **Creation**: [Old](/old.md)\n";
        let out = append_log_line(existing, "2026-07-02", "* **Creation**: [New](/new.md)");
        let new_group = out.find("## 2026-07-02").unwrap();
        let old_group = out.find("## 2026-07-01").unwrap();
        assert!(new_group < old_group, "newest date group must lead");
    }

    fn ids(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn conformant_document_produces_no_errors() {
        let raw = "---\ntype: Reference\ntitle: Foo\ndescription: A thing.\n---\n\nBody with [Bar](/bar.md).";
        let reports = okf_document_reports("foo", raw, &ids(&["foo", "bar"]));
        assert!(reports.iter().all(|r| r.severity != LintSeverity::Error), "{reports:?}");
    }

    #[test]
    fn unparseable_frontmatter_is_an_error() {
        let raw = "no frontmatter fence here";
        let reports = okf_document_reports("foo", raw, &ids(&["foo"]));
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].severity, LintSeverity::Error);
        assert!(reports[0].issue.contains("does not parse"));
    }

    #[test]
    fn missing_type_is_an_autofixable_error() {
        let raw = "---\ntitle: No Type\ndescription: x\n---\n\nBody.";
        let reports = okf_document_reports("foo", raw, &ids(&["foo"]));
        let type_err = reports
            .iter()
            .find(|r| r.issue.contains("`type`"))
            .expect("expected a type error");
        assert_eq!(type_err.severity, LintSeverity::Error);
        assert!(type_err.auto_fixable);
    }

    #[test]
    fn missing_description_is_a_warning_not_an_error() {
        let raw = "---\ntype: Reference\ntitle: Foo\n---\n\nBody.";
        let reports = okf_document_reports("foo", raw, &ids(&["foo"]));
        assert!(reports.iter().all(|r| r.severity != LintSeverity::Error));
        assert!(reports.iter().any(|r| r.issue.contains("description") && r.severity == LintSeverity::Warning));
    }

    #[test]
    fn broken_body_link_is_a_warning() {
        let raw = "---\ntype: Reference\ntitle: Foo\ndescription: x\n---\n\nSee [Gone](/missing.md).";
        let reports = okf_document_reports("foo", raw, &ids(&["foo"]));
        let broken = reports.iter().find(|r| r.issue.contains("missing.md")).expect("broken link");
        assert_eq!(broken.severity, LintSeverity::Warning);
    }

    #[test]
    fn orphan_detection_flags_unlinked_pages() {
        let mut a = entry("a", "A", Some("Reference"), Some("d"));
        a.content = "Links to [B](/b.md).".into();
        let b = entry("b", "B", Some("Reference"), Some("d")); // no inbound? a links to b
        let c = entry("c", "C", Some("Reference"), Some("d")); // orphan
        let reports = okf_orphan_reports(&[a, b, c]);
        let orphans: Vec<_> = reports.iter().filter_map(|r| r.entry_id.as_ref().map(|i| i.as_str())).collect();
        assert!(orphans.contains(&"c"), "c should be an orphan: {orphans:?}");
        assert!(orphans.contains(&"a"), "a has no inbound link either: {orphans:?}");
        assert!(!orphans.contains(&"b"), "b is linked from a: {orphans:?}");
    }

    #[test]
    fn index_with_extra_frontmatter_warns_but_okf_version_ok() {
        let with_version = "---\nokf_version: \"0.1\"\n---\n\n# Wiki Index\n";
        assert!(okf_index_reports(with_version).is_empty());

        let with_extra = "---\nokf_version: \"0.1\"\ntitle: nope\n---\n\n# Wiki Index\n";
        let reports = okf_index_reports(with_extra);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].severity, LintSeverity::Warning);

        let no_fm = "# Wiki Index\n\n* [Foo](/foo.md)\n";
        assert!(okf_index_reports(no_fm).is_empty());
    }

    #[test]
    fn log_non_iso_date_heading_warns() {
        let bad = "# Update Log\n\n## July 2 2026\n* **Creation**: [Foo](/foo.md)\n";
        let reports = okf_log_reports(bad);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].severity, LintSeverity::Warning);

        let good = "# Update Log\n\n## 2026-07-02\n* **Creation**: [Foo](/foo.md)\n";
        assert!(okf_log_reports(good).is_empty());
    }
}
