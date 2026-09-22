# pecs-cache: 通用声明式多级策略缓存框架

[![Rust Version](https://img.shields.io/badge/rustc-1.75+-blue.svg)](https://blog.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

专为 Rust 高性能后端服务与微服务量身打造的通用缓存编排与存储框架。

---

## 🌟 核心特性

- 🛡️ **默认安全与防数据泄漏**：提供 5 大缓存策略（`None`, `Shared`, `User`, `Ip`, `Private`），基于调用者身份严格计算 Key 命名空间，彻底杜绝多租户/多用户数据串线；
- 🚀 **AOP 切面属性宏**：通过 `#[cacheable]` 与 `#[cache_evict]` 实现零侵入透明缓存拦截回填与失效，业务逻辑保持纯粹；
- 🔌 **双存储驱动支持**：
  - **`LocalMemoryStore`**：纯 Rust 并发安全内存实现，零外部依赖，纳秒/微秒级响应，本地开发与单元测试秒级就绪；
  - **`RedisStore`**：基于 `redis-rs` 官方异步 `ConnectionManager`，**支持直接复用微服务已有的连接池**，避免连接翻倍；
- 👥 **操作人 vs 数据属主精准淘汰**：彻底解决管理员后台代客操作、异步任务处理时“缓存未按目标用户失效”的隐蔽脏读问题；
- 🔍 **确定性哈希查询指纹**：对分页与多条件复杂查询自动生成 64 位 FNV-1a 确定性指纹，支持按前缀批量淘汰；
- ⚙️ **配置驱动动态插拔**：支持在 `config.yaml` / `CacheConfig` 中按业务名动态关闭缓存、覆盖生效策略或调整 TTL，零重启生效。

---

## 📦 安装与特性 (Features)

在你的 `Cargo.toml` 中引入：

```toml
[dependencies]
# 默认开启内存缓存与过程宏支持 (零外部 C/Redis 依赖)
pecs-cache = { git = "https://github.com/pecs-labs/pecs-cache.git" }

# 如果需要使用 Redis 支持：
# pecs-cache = { git = "https://github.com/pecs-labs/pecs-cache.git", features = ["redis"] }
```

### Feature Flags

| Feature | 描述 | 默认开启 |
| :--- | :--- | :---: |
| `memory` | 本地并发内存缓存后端 (`LocalMemoryStore`) | ✅ |
| `macros` | 声明式切面过程宏 (`#[cacheable]`, `#[cache_evict]`) | ✅ |
| `redis` | 高性能 Redis 异步后端 (`RedisStore`) | ❌ |
| `serde_yaml`| 支持从 YAML 字符串反序列化 `CacheConfig` | ❌ |

---

## ⚡ 3 分钟快速上手

### 场景一：零依赖本地内存模式（推荐新项目/测试/CLI）

无需安装 Redis，开箱即用：

```rust
use pecs_cache::{
    cache_policy, cacheable, cache_evict, CacheConfig, CacheStrategy,
    DynamicCache, LocalMemoryStore, SimpleContext,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// 1. 定义数据对象并声明缓存策略（User 策略：严格用户隔离，默认 300 秒）
#[derive(Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: u64,
    pub nickname: String,
}
cache_policy!(UserProfile, biz = "user_profile", strategy = User, ttl = 300);

// 2. 业务服务结构体
pub struct UserService {
    pub cache: Arc<DynamicCache>,
}

impl UserService {
    // 读操作：透明 Cache-Aside，命中直接返回，未命中查库并回填
    #[cacheable(UserProfile, id = id)]
    pub async fn get_profile(&self, id: u64, ctx: &SimpleContext) -> Result<UserProfile, String> {
        println!("==> 穿透查库: id={id}");
        Ok(UserProfile { id, nickname: format!("User_{id}") })
    }

    // 写操作：执行成功后，自动淘汰当前用户详情与分页缓存
    #[cache_evict(UserProfile, id = profile.id, all)]
    pub async fn update_profile(&self, profile: &UserProfile, ctx: &SimpleContext) -> Result<(), String> {
        println!("==> 执行数据库更新: id={}", profile.id);
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    // 3. 初始化内存缓存后端
    let store = Arc::new(LocalMemoryStore::new());
    let cache = Arc::new(DynamicCache::new(store, CacheConfig::default()));
    let svc = UserService { cache };

    // 4. 模拟登录用户上下文
    let ctx = SimpleContext::new().with_user_id("100");

    // 第一次调用：穿透查库
    let p1 = svc.get_profile(100, &ctx).await.unwrap();
    // 第二次调用：命中缓存，不再打印 "穿透查库"
    let p2 = svc.get_profile(100, &ctx).await.unwrap();

    // 更新资料：自动失效缓存
    svc.update_profile(&p1, &ctx).await.unwrap();
    // 再次查询：重新穿透查库
    let _ = svc.get_profile(100, &ctx).await.unwrap();
}
```

---

### 场景二：生产级 Redis 模式（复用微服务已有连接池）

在微服务中，通常已有现成的 Redis 连接管理器（`ConnectionManager`），可以直接无缝包装复用：

```rust
use pecs_cache::{CacheConfig, DynamicCache, RedisStore};
use std::sync::Arc;

// 方式 A：微服务已有 ConnectionManager 时，直接注入共享底层物理连接池！
pub async fn init_cache_from_existing_redis(
    redis_conn: redis::aio::ConnectionManager,
) -> Arc<DynamicCache> {
    let store = Arc::new(RedisStore::from_manager(redis_conn));
    Arc::new(DynamicCache::new(store, CacheConfig::redis("shop_svc_", "")))
}

// 方式 B：直接指定 Redis URL 连接（新独立服务极简使用）
pub async fn init_cache_from_url(redis_url: &str) -> Result<Arc<DynamicCache>, pecs_cache::CacheError> {
    let store = Arc::new(RedisStore::connect(redis_url).await?);
    Ok(Arc::new(DynamicCache::new(store, CacheConfig::redis("shop_svc_", redis_url))))
}
```

---

## 🛡️ 多维策略与安全隔离 (`CacheStrategy`)

| 策略 (`CacheStrategy`) | 适用业务场景 | 命名空间 (`scope`) | 隔离机制与安全目的 |
| :--- | :--- | :--- | :--- |
| **`None`** (默认安全) | **未显式评估的业务（默认值）**、日志、密码凭证、金融流水 | 不产生缓存 | **默认安全法则**。直接穿透底层存储，绝不产生因配置失误导致的数据泄漏。 |
| **`Subject`** / `User` | 强身份私有数据：用户资料、设备状态、账号凭据、私人配置 | `sub:{subject}` | 严格按身份主体隔离。未认证请求直接查库，绝不写入共享域，杜绝主体间串线。 |
| **`Tenant`** | 企业/组织多租户 SaaS 数据：部门架构、工作区配置、企业通讯录 | `tenant:{tenant}` | 严格按租户空间物理隔离，不同企业客户之间天然空间阻断。 |
| **`Origin`** / `Ip` | 客户端来源频控、防刷限制、访客点赞投票 | `origin:{origin}` | 基于客户端请求端点（如 IP/网关源）隔离缓存，用于无主体身份时的频控与防刷。 |
| **`Cascading`** / `Private` | 兼容游客与已登录会员的混合型数据（如临时购物车） | `sub:{sub}` → `origin:{origin}` → 不缓存 | 三级智能级联：优先绑定主体；无主体降级为来源端点；无凭证安全穿透。 |
| **`Shared`** | 公共公开数据：系统字典、基础枚举、公开文章、全局站点配置 | `shared` | 所有主体共享相同 Key。写操作时级联失效全局详情与特权视图分页。 |

---

## 🧩 过程宏属性语法详解

### 1. `#[cacheable]` (读操作 Cache-Aside)
```rust
#[cacheable(TargetBo, id = 表达式 [, sub = 主体] [, opt = 选项] [, ctx = 上下文] [, cache = 引用])]
```
- **工作机制**：
  1. 先查缓存：Key 为 `{prefix}{biz}:detail:{scope}:id:{id}`；
  2. 命中缓存：直接返回 `Ok(缓存反序列化对象)`，原函数体不会被执行；
  3. 未命中缓存：执行原函数体，若返回 `Ok(val)` 则自动序列化写回缓存，并返回结果。

### 2. `#[cache_evict]` (写操作主动失效)
```rust
// 场景 1: 仅失效该业务的分页前缀列表
#[cache_evict(TargetBo, page)]

// 场景 2: 仅失效指定单条详情
#[cache_evict(TargetBo, id = param.id)]

// 场景 3: 全量失效（同时淘汰单条详情与关联的分页列表）
#[cache_evict(TargetBo, id = param.id, all)]
```
- **工作机制**：
  1. 宏首先执行原更新/删除函数体；
  2. **仅当函数返回 `Ok` 时**才触发淘汰动作；若数据库更新报错返回 `Err`，自动跳过淘汰，保证一致性；
  3. **特权代操作防护**：若当前操作者处于特权模式（`is_privileged=true`），且策略为 `Subject`，宏将自动把 `id` 作为目标主体的 `sub` 进行跨域定向清理！

---

## 🌐 上下文集成 (`CacheContext`)

如果你的项目已有自定义的请求上下文（如 Axum Extension、Actix Data、gRPC Context 或微服务 `ReqCtx`），**仅需实现一个方法**返回轻量小对象 `CacheOpts`：

```rust
use pecs_cache::{CacheContext, CacheOpts};

pub struct MyRequestContext {
    pub current_uid: Option<u64>,
    pub remote_ip: Option<String>,
    pub tenant_id: Option<String>,
    pub is_admin: bool,
}

// 仅需实现一个方法！将自身上下文映射为通用的 CacheOpts 小对象：
impl CacheContext for MyRequestContext {
    fn to_cache_opts(&self) -> CacheOpts {
        CacheOpts {
            subject: self.current_uid.map(|id| id.to_string()),
            origin: self.remote_ip.clone(),
            tenant: self.tenant_id.clone(),
            is_privileged: self.is_admin,
            explicit_scope: None,
        }
    }
}
```

> **小技巧**：
> - 如果是脚本或异步任务没有上下文，直接传入 `&()` 即可（内置全默认空实现）；
> - 如果方法只需按 UID 隔离，甚至可以直接在入参中传入 `uid` 基础类型（如 `1001u64`、`"1001"`），无需构造任何上下文结构体！

---

## ⚙️ 配置文件驱动与动态覆盖 (`config.yaml`)

运行时可通过配置动态开启/关闭或调优业务缓存，无需重新编译发版：

```yaml
cache:
  enable: true                  # 全局总开关
  prefix: "pecs_shop_"          # Key 前缀
  backend: "redis"              # 可选: redis / memory
  ttl_seconds: 300              # 全局默认 TTL (秒)
  redis_url: "redis://127.0.0.1:6379"
  entries:
    # 示例 1: 针对 user_profile 覆盖策略和延长过期时间至 600 秒
    user_profile:
      enable: true
      strategy: "user"
      ttl: 600
    # 示例 2: 针对敏感业务一键降级（瞬间停用缓存，直接查库）
    order_payment:
      enable: false
```

---

## ❓ 核心机制问答：宏是如何定位到具体的 `cache` 实例的？

很多开发者常有疑问：`#[cacheable(OrderBo, id = param.payload.id)]` **里并没有显式传递 cache 实例，它怎么知道该调用哪个缓存？**

### 1. 编译期约定：宏直接展开为调用 `self.cache`
Rust 中没有运行期全局反射容器。宏在编译时会**直接展开为访问当前结构体的 `self.cache` 字段**：
```rust
// 宏展开后本质上调用了：
self.cache.get_with_ctx::<OrderBo, _>(&id_str, &param.context).await
```
因此，业务服务结构体只需持有一个名为 `cache` 的字段（类型为 `Arc<DynamicCache>`）。如果未定义该字段，Rust 编译器在**编译期就会立刻给出精准提示**：`no field 'cache' on type '&YourService'`。

### 2. `self.cache` 的实例从何而来？
- **使用依赖注入 (DI) 的微服务**：
  在结构体字段上标记 `#[inject] pub cache: Arc<DynamicCache>`，容器在启动时会自动装配；
- **普通工程 / 独立服务**：
  直接在初始化业务服务时进行结构体赋值：`let svc = UserService { cache: my_cache.clone(), repo: ... };`。

### 3. 如果结构体字段不叫 `cache` 怎么办？
可以在宏参数中显式指定：`#[cacheable(OrderBo, id = id, cache = self.my_redis_cache)]`。

---

## 📚 进阶专题指南

- [企业级与微服务工程接入实战指南 (`PECS_PROJECT_INTEGRATION_EXAMPLE.md`)](docs/PECS_PROJECT_INTEGRATION_EXAMPLE.md)
- [宏原理、实例定位与四大工程集成指南 (`MACRO_AND_INTEGRATION_GUIDE.md`)](docs/MACRO_AND_INTEGRATION_GUIDE.md)
- [本地内存缓存深度指南 (`LOCALMEMORY_GUIDE.md`)](docs/LOCALMEMORY_GUIDE.md)
- [Redis 生产级连接池与安全淘汰指南 (`REDIS_GUIDE.md`)](docs/REDIS_GUIDE.md)

---

## 📄 开源许可证

本项目基于 [MIT](LICENSE) 或 [Apache-2.0](LICENSE) 协议开源。
