//! OKF bundle-level artifacts: the reserved `index.md` (§6) and `log.md`
//! (§7) files, plus body-link extraction (§5) used to derive the link graph
//! from markdown bodies rather than a frontmatter array.
//!
//! Everything here is a pure function of its inputs so it can be unit-tested
//! without a store or an LLM; `MarkdownStore` wires these to disk.

use pk_core::types::{ArticleId, WikiEntry};
use pulldown_cmark::{Event, Parser, Tag};

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
}
