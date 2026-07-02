use pk_core::types::{ArticleId, RawDoc, WikiEntry};
use pk_store::MarkdownStore;
use std::sync::Arc;

async fn temp_store() -> (Arc<MarkdownStore>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        MarkdownStore::open(dir.path())
            .await
            .expect("store open"),
    );
    (store, dir)
}

/// OKF §6/§7: after ingests, the wiki root carries an index.md cataloging
/// every entry and a log.md with dated, newest-first entries. Reserved files
/// must survive a store reopen (they are skipped as concept documents).
#[tokio::test]
async fn index_and_log_are_written_and_survive_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wiki = dir.path().join("wiki");

    {
        let store = MarkdownStore::open(dir.path()).await.unwrap();

        let mut a = WikiEntry::new("Axum", "Async web framework.");
        a.description = Some("Async Rust web framework.".into());
        store.upsert(a.clone()).await.unwrap();
        store.regenerate_index().await.unwrap();
        store.append_log("Creation", &a.title, &a.id).await.unwrap();

        let mut b = WikiEntry::new("Tower", "Middleware layers.");
        b.description = Some("Composable middleware.".into());
        store.upsert(b.clone()).await.unwrap();
        store.regenerate_index().await.unwrap();
        store.append_log("Creation", &b.title, &b.id).await.unwrap();
    }

    let index = tokio::fs::read_to_string(wiki.join("index.md")).await.unwrap();
    assert!(index.contains("[Axum](/axum.md)"), "index missing Axum: {index}");
    assert!(index.contains("[Tower](/tower.md)"), "index missing Tower: {index}");
    assert!(index.contains("Async Rust web framework."));

    let log = tokio::fs::read_to_string(wiki.join("log.md")).await.unwrap();
    assert!(log.contains("## "), "log missing a date heading: {log}");
    // Newest entry (Tower) appended most recently → leads within the day.
    let tower = log.find("[Tower]").unwrap();
    let axum = log.find("[Axum]").unwrap();
    assert!(tower < axum, "newest log entry must lead: {log}");

    // Reserved files are not loaded as concept documents.
    let store2 = MarkdownStore::open(dir.path()).await.unwrap();
    assert_eq!(store2.entry_count().await, 2);
}

#[tokio::test]
async fn upsert_and_get_roundtrip() {
    let (store, _dir) = temp_store().await;

    let entry = WikiEntry::new("Universal Agent Runtime", "Core Prometheus execution substrate.")
        .with_tags(["rust", "uar"])
        .with_sources(["test"]);

    let saved = store.upsert(entry.clone()).await.unwrap();
    assert_eq!(saved.id, entry.id);
    assert_eq!(saved.revision, 0);

    let retrieved = store.get(&saved.id).await.unwrap();
    assert_eq!(retrieved.title, "Universal Agent Runtime");
    assert_eq!(retrieved.content, "Core Prometheus execution substrate.");
    assert_eq!(retrieved.tags, vec!["rust", "uar"]);
}

#[tokio::test]
async fn upsert_same_id_bumps_revision() {
    let (store, _dir) = temp_store().await;

    let entry = WikiEntry::new("Kaia", "Agent certification platform.");
    let v1 = store.upsert(entry.clone()).await.unwrap();
    assert_eq!(v1.revision, 0);

    let mut entry2 = entry.clone();
    entry2.content = "Kaia issues W3C Verifiable Credentials.".into();
    let v2 = store.upsert(entry2).await.unwrap();
    assert_eq!(v2.revision, 1);

    let on_disk = store.get(&v1.id).await.unwrap();
    assert_eq!(on_disk.content, "Kaia issues W3C Verifiable Credentials.");
    assert_eq!(on_disk.revision, 1);
}

#[tokio::test]
async fn delete_removes_entry() {
    let (store, _dir) = temp_store().await;

    let entry = WikiEntry::new("Ephemeral Article", "Will be deleted.");
    let saved = store.upsert(entry).await.unwrap();

    store.delete(&saved.id).await.unwrap();
    assert_eq!(store.entry_count().await, 0);
    assert!(store.get(&saved.id).await.is_err());
}

#[tokio::test]
async fn delete_nonexistent_returns_error() {
    let (store, _dir) = temp_store().await;
    let result = store.delete(&ArticleId::from("does-not-exist")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn snapshot_returns_all_entries() {
    let (store, _dir) = temp_store().await;

    let titles = ["UAR", "Kaia", "TurboQuant", "mempalace-rs"];
    for title in &titles {
        store.upsert(WikiEntry::new(*title, "body")).await.unwrap();
    }

    let snapshot = store.snapshot().await.unwrap();
    assert_eq!(snapshot.len(), titles.len());
}

#[tokio::test]
async fn search_returns_relevant_results() {
    let (store, _dir) = temp_store().await;

    store.upsert(
        WikiEntry::new("Universal Agent Runtime", "Rust async agent execution engine with liter-llm routing.")
            .with_tags(["rust", "agent", "uar"])
    ).await.unwrap();

    store.upsert(
        WikiEntry::new("TurboQuant KV Cache", "3-bit FWHT compression for KV cache in Rust.")
            .with_tags(["rust", "compression", "kv-cache"])
    ).await.unwrap();

    store.upsert(
        WikiEntry::new("Kaia Agent Certification", "W3C Verifiable Credentials issued for agent behavior.")
            .with_tags(["kaia", "vc", "agent"])
    ).await.unwrap();

    let results = store.search("agent runtime execution", 3).await.unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].id, ArticleId::from_slug("Universal Agent Runtime"));
}

#[tokio::test]
async fn search_empty_store_returns_empty() {
    let (store, _dir) = temp_store().await;
    let results = store.search("anything", 5).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn entries_persist_across_store_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");

    {
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        for title in ["UAR", "Kaia", "SurrealDB"] {
            store.upsert(WikiEntry::new(title, "body")).await.unwrap();
        }
    }

    let store2 = MarkdownStore::open(dir.path()).await.unwrap();
    assert_eq!(store2.entry_count().await, 3);

    let uar = store2.get(&ArticleId::from_slug("UAR")).await.unwrap();
    assert_eq!(uar.title, "UAR");
}

/// OKF v0.1 §2: a Concept ID is the wiki-relative path minus `.md`, so
/// concepts MAY live in subdirectories (§3's bundle tree). A nested entry
/// must write to, and reload correctly from, a subdirectory of wiki/.
#[tokio::test]
async fn nested_concept_paths_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");

    let mut entry = WikiEntry::new("Orders", "One row per completed order.");
    entry.id = ArticleId::from("tables/orders");

    {
        let store = MarkdownStore::open(dir.path()).await.unwrap();
        store.upsert(entry.clone()).await.unwrap();

        let on_disk = dir.path().join("wiki/tables/orders.md");
        assert!(on_disk.exists(), "expected {on_disk:?} to exist");
    }

    let store2 = MarkdownStore::open(dir.path()).await.unwrap();
    assert_eq!(store2.entry_count().await, 1);
    let reloaded = store2.get(&ArticleId::from("tables/orders")).await.unwrap();
    assert_eq!(reloaded.title, "Orders");
}

#[tokio::test]
async fn unsafe_article_id_is_rejected_on_upsert() {
    let (store, _dir) = temp_store().await;

    let mut entry = WikiEntry::new("Evil", "body");
    entry.id = ArticleId::from("../../etc/passwd");

    assert!(store.upsert(entry).await.is_err());
}

/// OKF v0.1 §3.1: index.md and log.md are reserved bundle files, never
/// concept documents. A frontmatter-less index.md must not be treated as a
/// malformed entry or block store load.
#[tokio::test]
async fn reserved_filenames_are_skipped_on_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wiki_dir = dir.path().join("wiki");
    tokio::fs::create_dir_all(&wiki_dir).await.unwrap();

    tokio::fs::write(wiki_dir.join("index.md"), "# Index\n\n* [Foo](foo.md) - a concept\n")
        .await
        .unwrap();
    tokio::fs::write(wiki_dir.join("log.md"), "# Log\n\n## 2026-07-02\n* **Creation**: seeded\n")
        .await
        .unwrap();

    let store = MarkdownStore::open(dir.path()).await.unwrap();
    assert_eq!(store.entry_count().await, 0);

    store
        .upsert(WikiEntry::new("Foo", "A real concept."))
        .await
        .unwrap();

    let store2 = MarkdownStore::open(dir.path()).await.unwrap();
    assert_eq!(store2.entry_count().await, 1);
}

#[tokio::test]
async fn related_entries_finds_overlapping_content() {
    let (store, _dir) = temp_store().await;

    store.upsert(
        WikiEntry::new("Axum Web Framework", "Async Rust web framework built on Tower.")
            .with_tags(["rust", "axum", "web"])
    ).await.unwrap();

    store.upsert(
        WikiEntry::new("Tower Middleware", "Composable middleware layers for async Rust services.")
            .with_tags(["rust", "tower", "middleware"])
    ).await.unwrap();

    let raw = RawDoc::from_path(
        "test.md",
        "Notes on building Axum middleware using Tower service traits",
    );
    let related = store.related_entries(&raw, 5).await.unwrap();
    assert!(!related.is_empty());
}
