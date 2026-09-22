# 本地并发内存缓存深度指南 (LocalMemoryStore Guide)

`LocalMemoryStore` 是 `pecs-cache` 内置的纯 Rust 高并发进程内缓存实现。它不依赖任何外部 C 库或中间件（零网络 IO），具备纳秒至微秒级响应性能。

---

## 核心设计与特性

1. **Tokio 异步友好与高并发读写**：
   - 内部基于 `Arc<tokio::sync::RwLock<HashMap<String, MemoryEntry>>>` 实现；
   - 读操作获取共享只读锁（`read()`），多 Task 之间完全并发无阻塞；
   - 写操作获取独占写锁（`write()`），写入后立即释放。
2. **惰性过期 + 主动清理混合机制 (Lazy Expiration & Purge)**：
   - **惰性检查**：当调用 `get` / `exists` 时，基于 `std::time::Instant` 检查当前时间是否已超过 `expires_at`。若过期则立即视作不存在；
   - **主动清理**：提供 `purge_expired()` 方法，可在后台定期 Task 中主动剔除已过期的死键，避免长时间运行导致的内存碎片堆积。
3. **开箱即用的原子操作**：
   - 原生支持 `incr_by` 原子计数器增减；
   - 原生支持基于前缀的快速批量删除 (`del_prefix`)。

---

## 使用场景

- 🧪 **单元测试与 CI 管道**：在 GitHub Actions 或本地运行 `cargo test` 时，无需通过 Docker 启动 Redis 容器，测试用例秒级完成；
- 💻 **单机工具与轻量级后台**：CLI 工具、桌面应用、轻量微服务，单机部署无需运维 Redis 服务；
- 📴 **离线开发与平滑降级**：当开发者在没有内网 Redis 环境的高铁、飞机上开发时，可一键切换为 `backend: memory` 继续调试。

---

## 代码使用示例

### 1. 独立使用 `LocalMemoryStore`

```rust
use pecs_cache::{CacheExt, CacheStore, LocalMemoryStore};
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    let store = LocalMemoryStore::new();

    // 1. 设置带 2 秒 TTL 的字符串
    store.set("session:1001", "token_xyz", Some(2)).await.unwrap();

    // 2. 立即读取
    let val = store.get("session:1001").await.unwrap();
    println!("读取值: {:?}", val); // Some("token_xyz")

    // 3. 查询剩余秒数
    let ttl = store.ttl("session:1001").await.unwrap();
    println!("剩余 TTL: {:?} 秒", ttl);

    // 4. 等待过期
    sleep(Duration::from_millis(2100)).await;
    let expired = store.get("session:1001").await.unwrap();
    println!("过期后读取: {:?}", expired); // None

    // 5. 原子自增计数
    let count = store.incr_by("api_calls", 1).await.unwrap();
    println!("当前计数: {}", count); // 1
}
```

### 2. 结合 `DynamicCache` 注入为应用缓存

```rust
use pecs_cache::{CacheConfig, DynamicCache, LocalMemoryStore};
use std::sync::Arc;

let store = Arc::new(LocalMemoryStore::new());
let config = CacheConfig::memory("my_app_");

let cache = Arc::new(DynamicCache::new(store.clone(), config));

// 后台定时清理过期 Key（可选，推荐在长时间运行的常驻服务中使用）
tokio::spawn(async move {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        interval.tick().await;
        let purged = store.purge_expired().await;
        if purged > 0 {
            tracing::info!("清理了 {purged} 个已过期的本地内存缓存条目");
        }
    }
});
```
