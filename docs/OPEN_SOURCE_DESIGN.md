# pecs-cache: 通用开源缓存架构与设计规范

本文档详述 `pecs-cache` 作为通用开源 Rust 缓存中间件的核心架构设计、生产级三防机制（防击穿、防穿透、防雪崩）、多维策略路由与安全隔离机制。

---

## 目录

1. [核心架构全景图](#1-核心架构全景图)
2. [生产级三防机制 (Production Hardening)](#2-生产级三防机制-production-hardening)
   - [2.1 防缓存击穿 (Single-flight 并发合并)](#21-防缓存击穿-single-flight-并发合并)
   - [2.2 防缓存穿透 (Null Caching 空值占位缓存)](#22-防缓存穿透-null-caching-空值占位缓存)
   - [2.3 防缓存雪崩 (TTL 随机抖动 Jitter)](#23-防缓存雪崩-ttl-随机抖动-jitter)
3. [多维隔离策略与防数据泄漏](#3-多维隔离策略与防数据泄漏)
   - [3.1 策略矩阵与命名空间](#31-策略矩阵与命名空间)
   - [3.2 分页查询共享与行级隔离平衡 (`ISOLATE_PAGE`)](#32-分页查询共享与行级隔离平衡-isolate_page)
   - [3.3 特权模式与真实数据属主淘汰 (`owner` 机制)](#33-特权模式与真实数据属主淘汰-owner-机制)
4. [存储引擎与并发优化](#4-存储引擎与并发优化)
   - [4.1 `LocalMemoryStore` 16 分段锁与防 OOM 容量限制](#41-localmemorystore-16-分段锁与防-oom-容量限制)
   - [4.2 `RedisStore` 生产级连接管理与批量失效](#42-redisstore-生产级连接管理与批量失效)
5. [声明式 AOP 切面过程宏](#5-声明式-aop-切面过程宏)
   - [5.1 `#[cacheable]` 对 `Result<Option<T>, E>` 的透明适配](#51-cacheable-对-resultoptiont-e-的透明适配)
   - [5.2 `#[cache_evict]` 属主定向失效与异常追踪](#52-cache_evict-属主定向失效与异常追踪)
6. [可观测性与统计指标 (Observability)](#6-可观测性与统计指标-observability)

---

## 1. 核心架构全景图

```mermaid
flowchart TD
    App["业务函数 (Biz / Service)"] -->|#[cacheable] 或 DynamicCache API| DynamicCache["DynamicCache 动态编排器"]
    
    subgraph Engine ["编排与防护核心"]
        DynamicCache --> PolicyRouter["策略路由与作用域推导 (Policy Router)"]
        PolicyRouter --> SF["Single-flight 并发合并器 (防击穿)"]
        SF --> NullGuard["Null Cache 哨兵守卫 (防穿透)"]
        NullGuard --> JitterGen["TTL 随机抖动器 (防雪崩)"]
    end

    subgraph StoreLayer ["存储驱动层 (CacheStore Trait)"]
        JitterGen --> ShardedMem["LocalMemoryStore (16 分段锁 + 容量上限)"]
        JitterGen --> RedisMgr["RedisStore (基于 redis-rs ConnectionManager)"]
    end

    subgraph DB ["回源查库 (Fallback)"]
        SF -.->|仅 Leader 协程回源| Database[(PostgreSQL / MySQL / Mongo)]
    end
```

---

## 2. 生产级三防机制 (Production Hardening)

### 2.1 防缓存击穿 (Single-flight 并发合并)
- **痛点**：在热点 Key（如爆款商品、热点新闻）失效瞬间，若同时有 5000 个并发请求到达，若无并发合并，5000 个请求会同时穿透至底层数据库，造成 DB 连接池瞬间耗尽。
- **设计**：
  - 基于 Tokio 的轻量无死锁 `Singleflight` 机制；
  - 并发请求同一个 Missing Key 时，首个到达的请求成为 **Leader** 并执行底层 `loader().await`；
  - 其余并发请求自动挂起等待 Leader 结果，直接复用回填数据，不再重复击穿；
  - 经测试，50 个并发请求同时访问冷 Key 时，数据库 Loader **严格仅被调用 1 次**，49 次调用被透明合并。

### 2.2 防缓存穿透 (Null Caching 空值占位缓存)
- **痛点**：恶意请求或爬虫利用数据库中根本不存在的 ID（如 `-1` 或随机 UUID）持续发起查询，若未命中则不缓存，导致每次请求都直接压到底层数据库。
- **设计**：
  - 引入全局哨兵 `NULL_CACHE_SENTINEL = "__PECS_CACHE_NULL__"`；
  - 当底层 `loader().await` 返回 `Ok(None)` 时，若配置开启了 `cache_null`（默认开启），自动对该 Key 写入哨兵值并赋予较短的保护 TTL（默认 30 秒）；
  - 下次查询命中哨兵时，直接返回 `Ok(None)`，阻断回源查库；
  - 同时在监控指标中记录 `null_hits`，方便安全审计。

### 2.3 防缓存雪崩 (TTL 随机抖动 Jitter)
- **痛点**：大量同类业务数据同时写入缓存时使用完全固定的 TTL（如 300 秒），导致几分钟后大批 Key 在同一时刻集中过期，引发雪崩。
- **设计**：
  - 支持配置 `ttl_jitter_ratio`（默认 0.05 即 ±5%）；
  - 写入缓存时，实际有效 TTL 会根据当前时间纳秒级哈希值进行随机抖动：
    $$\text{Final\_TTL} = \text{Base\_TTL} \pm (\text{Base\_TTL} \times \text{Ratio} \times \text{RandomFactor})$$
  - 将失效时间打散，保证数据库访问平滑。

---

## 3. 多维隔离策略与防数据泄漏

### 3.1 策略矩阵与命名空间

| 策略 (`CacheStrategy`) | 隔离维度 | Key 命名空间 (`scope`) | 典型应用 |
| :--- | :--- | :--- | :--- |
| **`None`** (默认) | 关闭缓存 | 不产生 Key | 密码、金融流水、动态变更极频繁数据 |
| **`Shared`** | 全局共享 | `shared` | 字典、公开文章、全站配置、商品目录 |
| **`Subject`** | 身份主体 | `sub:{subject}` | 用户资料、购物车、个人设置、设备详情 |
| **`Tenant`** | 组织空间 | `tenant:{tenant}` | 企业通讯录、部门配置、SaaS 空间数据 |
| **`Origin`** | 来源端点 | `origin:{origin}` | 客户端 IP 防刷、无状态访客频控 |
| **`Cascading`** | 智能级联 | `sub:{sub}` → `origin:{origin}` → 不缓存 | 兼容游客与正式会员的临时数据 |

### 3.2 分页查询共享与行级隔离平衡 (`ISOLATE_PAGE`)
- **历史隐患**：若为了防越权，把所有登录用户的 `Shared` 分页一律改成 `sub:{uid}`，会导致全站公共文章列表对 10,000 个登录用户生成 10,000 个重复缓存，命中率趋近于 0。
- **解决方案**：
  - 纯公共数据（如商品、文章）：默认 `ISOLATE_PAGE = false`，所有人（匿名或已登录）共享 `shared` 分页缓存，发挥最大吞吐；
  - 含有个人行级权限或草稿的实体（如 Wiki、个人工单）：在策略中声明 `isolate_page = true`：
    ```rust
    cache_policy!(MyWiki, biz = "wiki", strategy = Shared, ttl = 300, isolate_page = true);
    ```
    已登录用户访问时自动隔离至 `sub:{uid}`，未登录访客访问时归入 `shared`；
  - 动态请求控制：调用方可在单个请求中通过 `ctx.with_isolate_page(true)` 动态提升当前查询的隔离级别。

### 3.3 特权模式与真实数据属主淘汰 (`owner` 机制)
- **历史隐患**：管理员修改订单 9999（买家是 10026），若以 `format!("sub:{id}")` 淘汰，会导致淘汰了 `sub:9999`，而真正的属主 `sub:10026` 发生脏读！
- **解决方案**：
  - 淘汰时支持显式传入真实数据属主 (`target_subject` / `owner`)：
    ```rust
    // 代码调用
    cache.evict_with_owner::<OrderRecord>("buyer_10026", Some("9999")).await?;
    
    // 或在宏中声明
    #[cache_evict(OrderRecord, id = order.id, all, owner = order.buyer_id)]
    pub async fn admin_update_order(&self, order: &OrderRecord) -> Result<(), String>
    ```

---

## 4. 存储引擎与并发优化

### 4.1 `LocalMemoryStore` 16 分段锁与防 OOM 容量限制
- **分段读写锁 (16 Shards)**：
  - 将整块大 Hash 表拆分为 16 个独立分片，每个分片持有独立的 `tokio::sync::RwLock`；
  - 按 Key 的 64 位 FNV 哈希分散至各分片，并发锁争用降低 16 倍。
- **容量上限与主动驱逐**：
  - 支持 `LocalMemoryStore::with_capacity(capacity)`（默认 100,000）；
  - 当单个分片超过容量配额时：
    1. 优先剔除已过期的键；
    2. 若仍超配额，驱逐最先过期的键（近似 LRU 策略），彻底杜绝内存无界泄漏。

### 4.2 `RedisStore` 生产级连接管理与批量失效
- 基于 `redis-rs` 异步 `ConnectionManager`，自动断线重连；
- 支持微服务复用已有连接池 (`RedisStore::from_manager`)；
- 前缀扫描采用非阻塞的游标 `SCAN` 替代危险的阻塞性 `KEYS`，单批次处理 100 个，安全可靠。

---

## 5. 声明式 AOP 切面过程宏

### 5.1 `#[cacheable]` 对 `Result<Option<T>, E>` 的透明适配
无论方法是返回确定的实体还是可能不存在的可选值，宏均能透明支持并自动融入三防机制：

```rust
// 场景 1: 返回实体对象
#[cacheable(User, id = id)]
pub async fn get_user(&self, id: u64) -> Result<User, AppError> { ... }

// 场景 2: 数据库查询标准返回值 (支持 Option 包装与空值防穿透)
#[cacheable(User, id = id, optional)] // 或根据签名自动识别 Option
pub async fn find_user(&self, id: u64) -> Result<Option<User>, AppError> { ... }
```

### 5.2 `#[cache_evict]` 属主定向失效与异常追踪
```rust
// 写操作成功后自动淘汰：支持指定真实属主、全量清空或单独清空分页
#[cache_evict(Order, id = id, all, owner = order.buyer_id)]
pub async fn update_order(&self, id: u64, order: &Order) -> Result<(), AppError> { ... }
```
- 若写操作返回 `Err`，自动跳过缓存淘汰，维护最终一致性；
- 若底层淘汰网络超时，自动输出 `tracing::warn!` 警告日志，方便排查潜在不一致。

---

## 6. 可观测性与统计指标 (Observability)

通过 `cache.stats()` 随时获取当前运行期统计快照：

```rust
let summary = cache.stats();
println!("缓存命中数: {}", summary.hits);
println!("穿透查库数: {}", summary.misses);
println!("防穿透空值命中: {}", summary.null_hits);
println!("失效淘汰数: {}", summary.evictions);
println!("综合命中率: {:.2}%", summary.hit_rate * 100.0);
```

也可通过 `cache.coalesced_calls()` 查看 Single-flight 为底层数据库省去的并发洪峰调用次数。
