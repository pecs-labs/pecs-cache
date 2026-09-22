# Redis 生产级存储后端指南 (RedisStore Guide)

`RedisStore` 是 `pecs-cache` 针对生产级分布式环境提供的高性能存储驱动，基于 `redis-rs` 的异步 `ConnectionManager` 封装。

---

## 核心设计与权责解耦

在分布式微服务体系中，微服务自身往往需要使用 Redis 进行多种操作：
- **分布式锁**（如 Redlock、Redisson 式悲观锁）；
- **延迟队列 / 任务分发**（List、Stream）；
- **在线状态 / 会话管理**（Hash、Bitmap）；
- **接口频控与限流**（RateLimiter）。

如果通用缓存库“包办”了 Redis 的连接初始化与生命周期，会导致微服务内出现两个甚至多个相互割裂的 Redis 连接池，白白浪费网络连接与内存。

因此，`pecs-cache` 采用**依赖倒置（DIP）与适配器（Adapter）设计**：
- **基础设施权责归属微服务**：微服务负责配置、认证与维护自身的 Redis 连接管理器；
- **缓存框架通过适配器复用连接**：通过 `RedisStore::from_manager(manager)` 直接复用该底层连接！

---

## 两种接入方式

### 方式一：复用微服务已有连接池（强烈推荐）

如果你的微服务已经初始化了 `redis::aio::ConnectionManager`：

```rust
use pecs_cache::{CacheConfig, DynamicCache, RedisStore};
use std::sync::Arc;

// 1. 微服务自身初始化 Redis ConnectionManager（可同时供分布式锁等其他组件使用）
let client = redis::Client::open("redis://:password@127.0.0.1:6379/0").unwrap();
let connection_manager = client.get_connection_manager().await.unwrap();

// 2. 将同一个 connection_manager 包装给 RedisStore
let redis_store = Arc::new(RedisStore::from_manager(connection_manager));

// 3. 组装并注入 DynamicCache
let cache_config = CacheConfig::redis("user_service_", "");
let dynamic_cache = Arc::new(DynamicCache::new(redis_store, cache_config));
```

### 方式二：独立单体服务一键连接（极简开箱即用）

对于独立开发的新项目或脚本，也可以直接提供 Redis 连接 URL：

```rust
use pecs_cache::{create_dynamic_cache, CacheConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = CacheConfig::default();
    config.backend = "redis".to_string();
    config.redis_url = Some("redis://:password@127.0.0.1:6379/0".to_string());
    config.prefix = "my_app_".to_string();

    // 一行代码初始化
    let cache = create_dynamic_cache(config).await?;

    // 业务使用 ...
    Ok(())
}
```

---

## 生产安全：基于 SCAN 的非阻塞前缀批量淘汰

在 Redis 中，若使用 `KEYS prefix*` 命令来查找并删除缓存，当 Redis 中有数百万个 Key 时，`KEYS` 命令将导致 Redis 单线程发生**毫秒甚至秒级的严重全局阻塞**，进而引发线上雪崩。

`pecs-cache` 的 `RedisStore::del_prefix` 严格遵循生产级安全规范：
- 采用非阻塞游标遍历 `SCAN cursor MATCH pattern COUNT 100`；
- 分批次递增迭代匹配，并按批次执行 `DEL`；
- 绝不阻塞 Redis 主线程正常处理其他业务请求。

---

## 生产防击穿与防雪崩建议

1. **错峰过期与随机抖动**：
   框架各实体的默认 TTL 可以通过 `config.yaml` 灵活微调。对于超高频业务，建议在 `config.yaml` 中配置合理的 TTL，避免多个热点表在同一秒集中过期；
2. **防击穿（Cache-Aside Loader 记忆化）**：
   在读取单记录或分页时，使用 `get_or_load_with_ctx` 或属性宏 `#[cacheable]`。未命中时回源，成功后立即回填，配合合理的连接池大小可有效抵抗瞬时流量高峰；
3. **安全穿透（默认安全法则）**：
   对于 `User` 策略实体，未登录游客自动穿透底层数据库，绝不在 Redis 中创建形如 `uid:anon` 的无效脏键，极大节约 Redis 内存并防止脏读。
