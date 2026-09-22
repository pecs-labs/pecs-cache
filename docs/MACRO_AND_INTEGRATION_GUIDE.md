# 宏原理、实例定位与架构集成指南 (Macro & Integration Guide)

本文档专门解答开发者在初次使用 `pecs-cache` 时最关心的核心机制问题：
- `#[cacheable]` 宏到底是如何定位到具体的 `cache` 实例的？
- 宏在编译阶段到底展开生成了什么代码？
- 在新项目（无 DI 容器、Axum Web、Actix-web、或自研架构）中，如何将 `DynamicCache` 注入到业务服务中？

---

## 一、 宏的核心约定：它如何知道 cache 实例是什么？

在 Spring / Java 等运行时动态语言生态中，框架通常依赖全局反射容器（ApplicationContext）在运行期动态定位 Bean。

但在 Rust 中，**没有任何隐式全局反射**。`#[cacheable]` 和 `#[cache_evict]` 是纯粹的**编译期抽象语法树（AST）代码重写**。

### 1. 默认约定：`self.cache`（约定优于配置）
宏在修饰一个 `&self` 成员方法时，**默认约定当前结构体内部包含一个名为 `cache` 的字段**（类型为 `Arc<DynamicCache>` 或任何实现了相应方法的类型）。

#### 开发者编写的代码：
```rust
pub struct UserService {
    pub cache: Arc<DynamicCache>, // 👈 必须包含此字段（默认约定名）
}

impl UserService {
    #[cacheable(UserBo, id = param.payload.id)]
    pub async fn get(&self, param: &ServiceRequest<UserBo>) -> AppResult<UserBo> {
        let entity = self.user_repo.get(...).await?;
        Ok(entity.into())
    }
}
```

#### 编译器在编译期实际展开后的代码：
```rust
impl UserService {
    pub async fn get(&self, param: &ServiceRequest<UserBo>) -> AppResult<UserBo> {
        let id_str = param.payload.id.to_string();

        // 1. 宏自动生成的代码：直接访问 self.cache！
        if let Some(cached) = self.cache.get_with_ctx::<UserBo, _>(&id_str, &param.context).await {
            return Ok(cached); // 命中缓存，直接返回，底层数据库查询根本不会执行！
        }

        // 2. 原函数体被包装在异步块中执行
        let res = async move {
            let entity = self.user_repo.get(...).await?;
            Ok(entity.into())
        }.await;

        // 3. 执行成功，宏自动调用 self.cache 写回
        if let Ok(ref val) = res {
            let _ = self.cache.set_with_ctx::<UserBo, _>(&id_str, val, &param.context).await;
        }

        res
    }
}
```

> **编译期安全保障**：
> 如果你的结构体中没有 `cache` 这个字段，Rust 编译器在**编译期就会立刻报错**：
> ```text
> error[E0609]: no field `cache` on type `&UserService`
> ```
> 绝不会等到运行期才报空指针或找不到 Bean。

---

### 2. 自定义字段名与外部实例传参 (`cache = ...`)

如果你的结构体字段不叫 `cache`，或者你是在一个独立函数中调用，可以通过参数显式指定：

#### 场景 A：结构体字段名不同
```rust
pub struct OrderService {
    pub my_custom_cache: Arc<DynamicCache>,
}

impl OrderService {
    // 显式指定缓存字段为 self.my_custom_cache
    #[cacheable(OrderBo, id = order_id, cache = self.my_custom_cache)]
    pub async fn get_order(&self, order_id: u64, ctx: &SimpleContext) -> Result<OrderBo, String> {
        // ...
    }
}
```

#### 场景 B：非成员函数（普通异步函数）
```rust
// 独立函数，cache 作为入参传入
#[cacheable(ArticleBo, id = id, cache = cache_mgr)]
pub async fn fetch_article(
    id: u64,
    ctx: &SimpleContext,
    cache_mgr: &DynamicCache,
) -> Result<ArticleBo, String> {
    // ...
}
```

---

## 二、 上下文 `ctx` 是如何被推导的？

为了严格实现主体/租户隔离（如 `CacheStrategy::Subject` -> `sub:{sub}`, `CacheStrategy::Tenant` -> `tenant:{tenant}`），宏需要读取当前的请求上下文。宏按照以下优先级智能推导：

| 规则优先级 | 匹配条件 | 宏展开后的代码 |
| :--- | :--- | :--- |
| **1. 显式选项对象** | 宏入参中显式声明了 `opt = 表达式` 或 `opts = 表达式` | 直接作为 `&#opt` 传入 |
| **2. 显式主体标识** | 宏入参中显式声明了 `sub = 表达式` 或 `uid = 表达式` | 自动包装为 `&CacheOpts::subject(&(#sub))` |
| **3. 显式租户标识** | 宏入参中显式声明了 `tenant = 表达式` | 自动包装为 `&CacheOpts::tenant(&(#tenant))` |
| **4. 显式上下文对象** | 宏入参中显式声明了 `ctx = 表达式` | 直接使用指定表达式，如 `#ctx` |
| **5. 标准服务请求** | 方法入参中有名为 `param` 的参数 | 自动解析为 `&param.context` |
| **6. 选项参数命名** | 方法入参中有名为 `opt` 或 `opts` 的参数 | 自动解析为 `&#pat_ident` |
| **7. 主体参数命名** | 方法入参中有名为 `sub` / `subject` / `uid` / `user_id` 的参数 | 自动包装为 `&CacheOpts::subject(&#pat_ident)` |
| **8. 上下文参数命名** | 方法入参中有名为 `ctx` 的参数 | 自动解析为 `&ctx` |
| **9. 缺省兜底** | 方法没有任何相关上下文参数 | 自动兜底为 `&()`（由 `CacheContext for ()` 提供默认空上下文） |

这意味着：**你可以自由选择传递完整的业务上下文对象，或是仅在方法入参中传递 `sub`/`uid`，甚至完全不接收任何上下文参数，宏均能无感推导并安全运行！**

---

## 三、 四大典型工程集成范式

### 范式一：纯手写依赖组装（适合所有 Rust 项目，零 DI 依赖）

在没有引入任何依赖注入框架的新项目中，直接通过普通 Rust 结构体字段组装：

```rust
use pecs_cache::{CacheConfig, DynamicCache, LocalMemoryStore, SimpleContext, cache_policy, cacheable};
use std::sync::Arc;

// 1. 声明数据模型
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct GoodsBo { pub id: u64, pub name: String }
cache_policy!(GoodsBo, biz = "goods", strategy = Shared, ttl = 300);

// 2. 业务服务持有 Arc<DynamicCache>
pub struct GoodsService {
    pub cache: Arc<DynamicCache>,
}

impl GoodsService {
    #[cacheable(GoodsBo, id = id)]
    pub async fn get(&self, id: u64) -> Result<GoodsBo, String> {
        println!("查询数据库: id={id}");
        Ok(GoodsBo { id, name: "商品名称".into() })
    }
}

// 3. 在 main.rs 或容器初始化时赋值
#[tokio::main]
async fn main() {
    let store = Arc::new(LocalMemoryStore::new()); // 或 RedisStore
    let cache = Arc::new(DynamicCache::new(store, CacheConfig::default()));

    // 组装业务 Service
    let goods_service = GoodsService {
        cache: cache.clone(), // 👈 实例在此处直接传入结构体
    };

    // 运行
    let goods = goods_service.get(1001).await.unwrap();
}
```

---

### 范式二：配合依赖注入（DI）框架

在大型微服务架构中，通常使用 DI 容器实现自动装配：

```rust
// 1. 注册 DynamicCache 为全局 Bean
#[inject_container::component]
pub async fn create_dynamic_cache() -> Arc<DynamicCache> {
    let config = load_config();
    let redis_cm = init_redis(&config).await;
    let store = Arc::new(RedisStore::from_manager(redis_cm));
    Arc::new(DynamicCache::new(store, config.cache))
}

// 2. 领域服务通过 #[inject] 自动装配
#[derive(Clone)]
pub struct OrderService {
    #[inject]
    pub order_repo: Arc<dyn OrderRepository>,

    // 👇 DI 容器启动时自动将注册的 DynamicCache 注入到此字段
    #[inject]
    pub cache: Arc<DynamicCache>,
}

impl OrderService {
    #[cacheable(OrderBo, id = param.payload.id)]
    pub async fn get(&self, param: &ServiceRequest<OrderBo>) -> AppResult<OrderBo> {
        // ...
    }
}
```

---

### 范式三：Web 框架状态共享（以 Axum 为例）

在 Web API 开发中，通常将缓存放在全局 `State` 中供所有 Handler 共享：

```rust
use axum::{extract::{Path, State}, routing::get, Json, Router};
use pecs_cache::{CacheConfig, DynamicCache, LocalMemoryStore, SimpleContext};
use std::sync::Arc;

#[derive(Clone)]
struct AppState {
    pub cache: Arc<DynamicCache>,
}

async fn get_user_handler(
    State(state): State<AppState>,
    Path(user_id): Path<u64>,
) -> Json<UserBo> {
    let ctx = SimpleContext::new().with_user_id(user_id.to_string());

    // 在 Handler 中直接调用 cache 的函数式 API
    let user = state.cache.get_or_load_with_ctx::<UserBo, _, _, _, String>(
        user_id.to_string(),
        &ctx,
        || async move {
            // 回源查库逻辑
            Ok(Some(query_db_user(user_id).await))
        },
    )
    .await
    .unwrap()
    .unwrap();

    Json(user)
}
```

---

### 范式四：不用宏的“纯函数式调用”（最透明、零学习成本）

如果你不想在结构体上使用属性宏，或者业务逻辑非常动态（如条件性跳过缓存、根据运行期变量动态计算 Key），**直接调用 `DynamicCache` 的内置方法即可**：

| 业务需求 | 原生 API 方法 | 核心功能 |
| :--- | :--- | :--- |
| **单条详情读取** | `cache.get_or_load_with_ctx(id, ctx, loader)` | 命中返回，未命中查库并回写 |
| **分页列表读取** | `cache.get_or_load_page(query, ctx, loader)` | 基于 FNV-1a 查询指纹缓存分页 |
| **更新后单条失效** | `cache.evict_with_ctx(id, ctx)` | 淘汰当前主体私有详情缓存 |
| **更新后全量失效** | `cache.evict_all_smart(id, ctx)` | 同时淘汰详情缓存与关联的分页前缀 |
| **特权代客操作** | `cache.evict_subject(target_sub, id)` | 精准淘汰目标归属主体的私有详情与分页 |

```rust
// 纯函数式调用示例：
let user = cache
    .get_or_load_with_ctx::<UserBo, _, _, _, AppError>(id, &ctx, || async move {
        repo.find_by_id(id).await
    })
    .await?;
```

---

## 四、 常见问题与排错手册 (FAQ)

### Q1：编译报错 `error[E0609]: no field cache on type &MyService`？
- **原因**：你的结构体上没有定义名为 `cache` 的字段；
- **解决**：在结构体中添加 `pub cache: Arc<DynamicCache>`；或者在宏中显式指明你的字段名：`#[cacheable(UserBo, id = ..., cache = self.my_other_cache)]`。

### Q2：加了 `#[cacheable]`，为什么每次请求依然穿透查库？
请依次排查以下 3 点：
1. **策略是否为 `None`**：检查实体声明是否写成了 `strategy = None`，或者在 `config.yaml` 的 `entries` 中被配置为了 `enable: false`；
2. **上下文未提供身份主体**：如果策略是 `Subject`（或兼容别名 `User`），而当前请求没有主体身份（`ctx.subject()` 为 `None`），框架根据**默认安全法则**会主动放弃写入全局缓存，直接穿透查库，以防止未认证访客之间串线；
3. **方法是否返回了 `Err`**：若数据库查询结果为 `Err`，框架绝不会把错误结果写进缓存。
