use pecs_cache::{CacheExt, LocalMemoryStore};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::sleep;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DemoItem {
    id: u64,
    name: String,
}

#[tokio::test]
async fn test_memory_basic_and_json() {
    let store = LocalMemoryStore::new();

    // 基础字符串读写
    store.set("test:k1", "val1", None).await.unwrap();
    assert_eq!(store.get("test:k1").await.unwrap(), Some("val1".to_string()));
    assert!(store.exists("test:k1").await.unwrap());

    // 删除
    store.del("test:k1").await.unwrap();
    assert_eq!(store.get("test:k1").await.unwrap(), None);
    assert!(!store.exists("test:k1").await.unwrap());

    // JSON 读写
    let item = DemoItem {
        id: 42,
        name: "Alice".to_string(),
    };
    store.set_json("test:item:42", &item, None).await.unwrap();
    let loaded: Option<DemoItem> = store.get_json("test:item:42").await.unwrap();
    assert_eq!(loaded, Some(item));
}

#[tokio::test]
async fn test_memory_ttl_expiration() {
    let store = LocalMemoryStore::new();

    // 设置 1 秒过期
    store.set("temp_key", "temporary", Some(1)).await.unwrap();
    assert_eq!(
        store.get("temp_key").await.unwrap(),
        Some("temporary".to_string())
    );

    // 等待 1.1 秒
    sleep(Duration::from_millis(1100)).await;
    assert_eq!(store.get("temp_key").await.unwrap(), None);
    assert!(!store.exists("temp_key").await.unwrap());
}

#[tokio::test]
async fn test_memory_del_prefix() {
    let store = LocalMemoryStore::new();

    store.set("order:page:1", "p1", None).await.unwrap();
    store.set("order:page:2", "p2", None).await.unwrap();
    store.set("order:detail:1", "d1", None).await.unwrap();
    store.set("user:profile:1", "u1", None).await.unwrap();

    // 批量删除前缀 "order:page:"
    let deleted = store.del_prefix("order:page:").await.unwrap();
    assert_eq!(deleted, 2);

    assert_eq!(store.get("order:page:1").await.unwrap(), None);
    assert_eq!(store.get("order:page:2").await.unwrap(), None);
    assert_eq!(
        store.get("order:detail:1").await.unwrap(),
        Some("d1".to_string())
    );
    assert_eq!(
        store.get("user:profile:1").await.unwrap(),
        Some("u1".to_string())
    );
}

#[tokio::test]
async fn test_memory_incr_by() {
    let store = LocalMemoryStore::new();

    assert_eq!(store.incr_by("counter", 1).await.unwrap(), 1);
    assert_eq!(store.incr_by("counter", 5).await.unwrap(), 6);
    assert_eq!(store.incr_by("counter", -2).await.unwrap(), 4);
}
