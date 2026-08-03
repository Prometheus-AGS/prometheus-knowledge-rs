use pk_watcher::{spawn_wiki_watcher, WikiWatchEvent};
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::test]
async fn wiki_watcher_reports_start_and_direct_changes() {
    let fixture = tempfile::tempdir().unwrap();
    let wiki = fixture.path().join("wiki");
    tokio::fs::create_dir_all(&wiki).await.unwrap();
    let (tx, mut rx) = mpsc::channel(8);
    let _handle = spawn_wiki_watcher(wiki.clone(), tx).unwrap();

    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap(),
        Some(WikiWatchEvent::Started)
    ));
    tokio::fs::write(wiki.join("direct.md"), "fixture")
        .await
        .unwrap();
    let changed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(rx.recv().await, Some(WikiWatchEvent::Changed)) {
                break true;
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(changed);
}
