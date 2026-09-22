# 企业级与微服务工程接入实战指南 (Enterprise & Microservice Integration Guide)

本文档面向企业级后端服务与微服务架构，提供标准、解耦且具备生产级完备性的 4 步接入示例。

---

## 架构定位与解耦设计

- **传统硬编码方案**：各个微服务独立实现一套 KV 读写、序列化、防穿透与失效逻辑，造成跨服务样板代码冗余，且策略标准不一；
- **组件化方案**：通过引入通用的 `pecs-cache` 框架，底层存储与策略路由被统一封装，业务服务仅需关注领域实体与声明式切面注解，维护成本极低。

---

## 4 步接入实战

### 第一步：在 `Cargo.toml` 中引入依赖

在微服务项目的 `[dependencies]` 中添加：

```toml
[dependencies]
# 生产环境引入（开启 redis 后端与切面过程宏）
pecs-cache = { git = "https://github.com/pecs-labs/pecs-cache.git", features = ["redis"] }

# 本地工作区开发时亦可通过 path 路径引入：
# pecs-cache = { path = "../pecs-cache", features = ["redis"] }
```

---

### 第二步：上下文解耦——两种任选的接入方式（零侵入）

开源基础设施组件不应强行侵入或约束业务核心的上下文模型。框架提供了两种极简的对接方式：

#### 方式 A（极简配置驱动：纯参数声明，零 Trait 实现）
无需为上下文结构体实现任何 Trait，直接在切面注解中指定主体参数：

```rust
// 通过 sub = ... (或兼容别名 uid = ...) 直接绑定主体隔离键，完全配置驱动
#[cacheable(OrderBo, id = param.payload.id, sub = param.context.user_id)]
pub async fn get(&self, param: &ServiceRequest<OrderBo>) -> AppResult<OrderBo> { ... }
```
宏会自动将参数包装为 `CacheOpts::subject(...)`，既保证严格隔离，又保持上下文模型纯粹。

---

#### 方式 B（推荐：单方法轻量小对象适配器模式）
若希望保持自动推导、避免在每个宏调用处重复声明参数，只需在基础设施层（如 `src/cache/mod.rs`）就近为项目的请求上下文实现 `CacheContext` 的单一方法：

```rust
// 位于缓存基础设施模块内部，将业务请求上下文映射为通用的 CacheOpts 小对象：
impl pecs_cache::CacheContext for crate::common::context::ReqCtx {
    fn to_cache_opts(&self) -> pecs_cache::CacheOpts {
        pecs_cache::CacheOpts {
            // 映射身份主体标识 (Subject / User ID)
            subject: self.user_id.clone(),
            // 映射客户端接入端点 (Origin / Client IP)
            origin: self.client_ip.clone(),
            // 映射特权访问视图 (Privileged / Supervisor 模式)
            is_privileged: self.is_admin,
            // 映射 SaaS 多租户空间（若无则设为 None）
            tenant: self.tenant_id.clone(),
            // 显式自定义作用域覆盖（若无则设为 None）
            explicit_scope: None,
        }
    }
}
```

**设计优势**：
1. **单一小对象传递**：将离散属性一次性打包收敛，无多余虚函数调用开销；
2. **抽象彻底解耦**：核心库只依赖 `subject`、`origin`、`tenant`、`is_privileged` 等通用计算机安全概念；
3. **架构边界清晰**：适配逻辑仅局限于基础设施层，核心领域层零污染。

---

### 第三步：在基础设施层组装 `DynamicCache` 实例

在项目的基础设施初始化模块中组装 `DynamicCache`，并复用已有的连接池（例如 Redis `ConnectionManager` 或连接池）：

```rust
use pecs_cache::{CacheConfig, DynamicCache, LocalMemoryStore, RedisStore};
use std::sync::Arc;

// 重新导出常用宏与类型，供领域层统一使用
pub use pecs_cache::{
    cache_policy, cacheable, cache_evict,
    CachePolicy, CacheStrategy, CacheContext, CacheOpts, SimpleContext,
};

/// 初始化并构建全局 DynamicCache 实例
pub async fn init_dynamic_cache(config: &AppConfig) -> Arc<DynamicCache> {
    let store: Arc<dyn pecs_cache::CacheStore> = if config.cache.backend == "redis" {
        // 复用已有的 Redis 连接管理器（零额外物理连接开销）
        if let Ok(store) = RedisStore::connect(&config.cache.redis_url).await {
            Arc::new(store)
        } else {
            tracing::warn!("连接 Redis 失败，自动安全降级为 LocalMemoryStore 内存缓存");
            Arc::new(LocalMemoryStore::new())
        }
    } else {
        Arc::new(LocalMemoryStore::new())
    };

    let cache_config = CacheConfig {
        enable: config.cache.enable,
        prefix: config.cache.prefix.clone(),
        backend: config.cache.backend.clone(),
        ttl_seconds: config.cache.ttl_seconds,
        redis_url: None,
        entries: Default::default(),
    };

    Arc::new(DynamicCache::new(store, cache_config))
}
```

---

### 第四步：领域服务声明式接入

在领域业务服务中，通过 `cache_policy!` 声明业务实体的安全隔离策略，并通过切面宏实现透明读写拦截与主动淘汰：

```rust
use std::sync::Arc;
use pecs_cache::{cache_evict, cacheable, cache_policy, CacheStrategy, DynamicCache};

// 1. 声明缓存策略：主体隔离 (Subject)，TTL 为 300 秒
cache_policy!(OrderBo, biz = "order", strategy = Subject, ttl = 300);

#[derive(Clone)]
pub struct OrderService {
    pub order_repo: Arc<dyn OrderRepository>,
    // 依赖注入或构造注入 DynamicCache 实例
    pub cache: Arc<DynamicCache>,
}

impl OrderService {
    // 2. 读操作：透明 Cache-Aside，命中直接返回，未命中查库并写回
    #[cacheable(OrderBo, id = param.payload.id)]
    pub async fn get(&self, param: &ServiceRequest<OrderBo>) -> AppResult<OrderBo> {
        let entity = self.order_repo.get(param.payload.id).await?;
        Ok(entity.into())
    }

    // 3. 更新操作：成功后自动淘汰详情与分页，特权代操作自动精准淘汰目标主体私有域
    #[cache_evict(OrderBo, id = param.payload.id, all)]
    pub async fn update(&self, param: &ServiceRequest<OrderBo>) -> AppResult<OrderBo> {
        let updated = self.order_repo.update(&param.payload).await?;
        Ok(updated.into())
    }

    // 4. 新增操作：自动失效该业务的分页缓存列表
    #[cache_evict(OrderBo, page)]
    pub async fn create(&self, param: &ServiceRequest<OrderBo>) -> AppResult<OrderBo> {
        let created = self.order_repo.create(&param.payload).await?;
        Ok(created.into())
    }
}
```

---

## 样板代码清理建议

接入标准化的 `pecs-cache` 后，微服务内部无需再维护冗余的底层缓存实现文件：
- 自行实现的动态缓存编排代码（可由 `DynamicCache` 统一替代）；
- 重复定义的缓存策略枚举与 Trait（可由 `CachePolicy` / `CacheStrategy` 统一替代）；
- 冗余的 Key 拼装与前缀模糊扫描辅助函数（可由 `DynamicCache` 统一管理）。

由此各服务只需保留极简的配置与依赖组装代码，核心逻辑更加纯粹聚焦。
