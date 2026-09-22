//! 动态插拔与策略路由缓存编排层 (DynamicCache)

use crate::config::CacheConfig;
use crate::error::CacheResult;
use crate::policy::{CachePolicy, CacheStrategy};
use crate::traits::{CacheContext, CacheExt, CacheStore};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::future::Future;
use std::sync::Arc;

/// 计算稳定查询指纹（FNV-1a 64-bit 确定性哈希，跨节点与跨重启稳定）
#[inline]
pub fn query_fingerprint<Q: Serialize>(query: &Q) -> String {
    let json_bytes = serde_json::to_vec(query).unwrap_or_default();
    let mut hash = 0xcbf29ce484222325u64;
    for &b in &json_bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

/// 动态插拔式缓存编排器 (DynamicCache)
///
/// 具备能力：
/// 1. **声明式策略路由**：根据 `CachePolicy` 自动推导业务命名空间、安全策略与 TTL；
/// 2. **配置驱动动态插拔**：根据 `CacheConfig.entries` 在运行期动态开关、覆盖策略或调整 TTL，零重启生效；
/// 3. **防数据泄漏作用域隔离**：Subject (sub:{sub}), Tenant (tenant:{tenant}), Origin (origin:{origin}), Privileged, Shared, 杜绝跨主体/跨租户串线；
/// 4. **操作人与数据属主智能推导**：特权身份代操作时自动联动淘汰真实归属主体私有域与分页；
/// 5. **Cache-Aside 自动回填与主动失效**：提供方法与过程宏透明拦截支持。
#[derive(Clone)]
pub struct DynamicCache {
    store: Arc<dyn CacheStore>,
    config: CacheConfig,
}

impl DynamicCache {
    /// 构造新的 DynamicCache 实例（支持 CacheConfig, &CacheConfig, Arc<CacheConfig> 等任意可转换类型）
    pub fn new(store: Arc<dyn CacheStore>, config: impl Into<CacheConfig>) -> Self {
        Self {
            store,
            config: config.into(),
        }
    }

    /// 获取底层存储后端引用
    pub fn store(&self) -> &Arc<dyn CacheStore> {
        &self.store
    }

    /// 获取当前生效的配置引用
    pub fn config(&self) -> &CacheConfig {
        &self.config
    }

    /// 构建标准详情缓存 Key：`{prefix}{biz}:detail:{scope}:id:{id}`
    pub fn build_key(&self, biz: &str, scope: &str, id: &str) -> String {
        format!("{}{}:detail:{}:id:{}", self.config.prefix, biz, scope, id)
    }

    /// 构建标准分页缓存前缀：`{prefix}{biz}:page:{scope}`
    pub fn build_page_prefix(&self, biz: &str, scope: &str) -> String {
        format!("{}{}:page:{}", self.config.prefix, biz, scope)
    }

    /// 构建标准分页缓存 Key：`{prefix}{biz}:page:{scope}:q:{query_fingerprint}`
    pub fn build_page_key(&self, biz: &str, scope: &str, fingerprint: &str) -> String {
        format!("{}:q:{}", self.build_page_prefix(biz, scope), fingerprint)
    }

    /// 解析当前实体绑定的最终有效策略（配置文件配置优先于代码静态默认值）
    pub fn resolve_policy<T: CachePolicy>(&self) -> (bool, CacheStrategy, u64) {
        if !self.config.enable {
            return (false, CacheStrategy::None, 0);
        }

        if let Some(entry) = self.config.entries.get(T::BIZ) {
            if !entry.enable {
                return (false, CacheStrategy::None, 0);
            }
            let strategy = entry.strategy.unwrap_or(T::DEFAULT_STRATEGY);
            let ttl = entry.ttl.unwrap_or(T::DEFAULT_TTL);
            (true, strategy, ttl)
        } else {
            (true, T::DEFAULT_STRATEGY, T::DEFAULT_TTL)
        }
    }

    /// 基于 `CacheContext` 与 `CachePolicy` 动态推导单记录作用域 (Scope)
    pub fn resolve_scope<T: CachePolicy>(&self, ctx: &impl CacheContext) -> String {
        let (_, strategy, _) = self.resolve_policy::<T>();
        let opts = ctx.to_cache_opts();

        // 1. 显式自定义 Scope 优先级最高
        if let Some(ref explicit) = opts.explicit_scope {
            if !explicit.trim().is_empty() {
                return explicit.clone();
            }
        }

        // 2. 特权操作（管理员、特权模式、系统提权）拥有跨隔离域全局视图
        if opts.is_privileged {
            return "privileged".to_string();
        }

        // 3. 依据策略推导
        match strategy {
            CacheStrategy::Shared => "shared".to_string(),
            CacheStrategy::Tenant => {
                if let Some(ref tenant) = opts.tenant {
                    if !tenant.trim().is_empty() {
                        return format!("tenant:{tenant}");
                    }
                }
                "none".to_string()
            }
            CacheStrategy::Subject => {
                if let Some(ref sub) = opts.subject {
                    if !sub.trim().is_empty() {
                        return format!("sub:{sub}");
                    }
                }
                "none".to_string()
            }
            CacheStrategy::Origin => {
                if let Some(ref origin) = opts.origin {
                    if !origin.trim().is_empty() {
                        return format!("origin:{origin}");
                    }
                }
                "none".to_string()
            }
            CacheStrategy::Cascading => {
                if let Some(ref sub) = opts.subject {
                    if !sub.trim().is_empty() {
                        return format!("sub:{sub}");
                    }
                }
                if let Some(ref origin) = opts.origin {
                    if !origin.trim().is_empty() {
                        return format!("origin:{origin}");
                    }
                }
                "none".to_string()
            }
            CacheStrategy::None => "none".to_string(),
        }
    }

    /// 基于 `CacheContext` 与 `CachePolicy` 动态推导分页作用域 (Scope)
    pub fn resolve_page_scope<T: CachePolicy>(&self, ctx: &impl CacheContext) -> String {
        let (_, strategy, _) = self.resolve_policy::<T>();
        let opts = ctx.to_cache_opts();

        if let Some(ref explicit) = opts.explicit_scope {
            if !explicit.trim().is_empty() {
                return explicit.clone();
            }
        }

        if opts.is_privileged {
            return "privileged".to_string();
        }

        match strategy {
            CacheStrategy::Shared => {
                // 如果当前已有主体标识，为防止行级权限（如主体自己的未发布草稿）污染共享缓存，隔离至 sub
                if let Some(ref sub) = opts.subject {
                    if !sub.trim().is_empty() {
                        return format!("sub:{sub}");
                    }
                }
                "shared".to_string()
            }
            CacheStrategy::Tenant => {
                if let Some(ref tenant) = opts.tenant {
                    if !tenant.trim().is_empty() {
                        return format!("tenant:{tenant}");
                    }
                }
                "none".to_string()
            }
            CacheStrategy::Subject => {
                if let Some(ref sub) = opts.subject {
                    if !sub.trim().is_empty() {
                        return format!("sub:{sub}");
                    }
                }
                "none".to_string()
            }
            CacheStrategy::Origin => {
                if let Some(ref origin) = opts.origin {
                    if !origin.trim().is_empty() {
                        return format!("origin:{origin}");
                    }
                }
                "none".to_string()
            }
            CacheStrategy::Cascading => {
                if let Some(ref sub) = opts.subject {
                    if !sub.trim().is_empty() {
                        return format!("sub:{sub}");
                    }
                }
                if let Some(ref origin) = opts.origin {
                    if !origin.trim().is_empty() {
                        return format!("origin:{origin}");
                    }
                }
                "none".to_string()
            }
            CacheStrategy::None => "none".to_string(),
        }
    }

    /// 针对 `#[cacheable]` 宏设计的快速读接口：若命中返回 Some(val)，否则返回 None
    pub async fn get_with_ctx<T>(&self, id: &str, ctx: &impl CacheContext) -> Option<T>
    where
        T: CachePolicy + DeserializeOwned + Send + Sync,
    {
        let (enabled, strategy, _) = self.resolve_policy::<T>();
        if !enabled || strategy == CacheStrategy::None {
            return None;
        }

        let scope = self.resolve_scope::<T>(ctx);
        if scope == "none" || scope == "anon" {
            return None;
        }

        let key = self.build_key(T::BIZ, &scope, id);
        match self.store.get_json::<T>(&key).await {
            Ok(Some(val)) => {
                tracing::debug!(biz = %T::BIZ, key = %key, "⚡ [Cache HIT]");
                Some(val)
            }
            _ => None,
        }
    }

    /// 针对 `#[cacheable]` 宏设计的快速写回接口
    pub async fn set_with_ctx<T>(&self, id: &str, val: &T, ctx: &impl CacheContext) -> CacheResult<()>
    where
        T: CachePolicy + Serialize + Send + Sync,
    {
        let (enabled, strategy, ttl) = self.resolve_policy::<T>();
        if !enabled || strategy == CacheStrategy::None {
            return Ok(());
        }

        let scope = self.resolve_scope::<T>(ctx);
        if scope == "none" || scope == "anon" {
            return Ok(());
        }

        let key = self.build_key(T::BIZ, &scope, id);
        let ttl_opt = if ttl > 0 { Some(ttl) } else { None };
        self.store.set_json(&key, val, ttl_opt).await
    }

    /// 记忆化读取单记录（Cache-Aside，带上下文）
    pub async fn get_or_load_with_ctx<T, F, Fut, E>(
        &self,
        id: impl AsRef<str>,
        ctx: &impl CacheContext,
        loader: F,
    ) -> Result<Option<T>, E>
    where
        T: CachePolicy + Serialize + DeserializeOwned + Send + Sync,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Option<T>, E>>,
    {
        let id_str = id.as_ref();
        let (enabled, strategy, ttl) = self.resolve_policy::<T>();
        if !enabled || strategy == CacheStrategy::None {
            return loader().await;
        }

        let scope = self.resolve_scope::<T>(ctx);
        if scope == "none" || scope == "anon" {
            return loader().await;
        }

        let key = self.build_key(T::BIZ, &scope, id_str);
        if let Ok(Some(cached)) = self.store.get_json::<T>(&key).await {
            tracing::debug!(biz = %T::BIZ, key = %key, "⚡ [Cache HIT]");
            return Ok(Some(cached));
        }

        tracing::debug!(biz = %T::BIZ, key = %key, "💨 [Cache MISS] 执行回源查库");
        let loaded = loader().await?;
        if let Some(ref val) = loaded {
            let ttl_opt = if ttl > 0 { Some(ttl) } else { None };
            let _ = self.store.set_json(&key, val, ttl_opt).await;
        }
        Ok(loaded)
    }

    /// 记忆化读取单记录（无上下文，仅适用于 Shared 策略实体）
    pub async fn get_or_load<T, F, Fut, E>(
        &self,
        id: impl AsRef<str>,
        loader: F,
    ) -> Result<Option<T>, E>
    where
        T: CachePolicy + Serialize + DeserializeOwned + Send + Sync,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Option<T>, E>>,
    {
        let (enabled, strategy, _) = self.resolve_policy::<T>();
        if !enabled || strategy != CacheStrategy::Shared {
            return loader().await;
        }
        self.get_or_load_with_ctx(id, &(), loader).await
    }

    /// 记忆化分页/列表查询（支持泛型 Page 类型，防未登录串线）
    pub async fn get_or_load_page<T, C, Q, P, F, Fut, E>(
        &self,
        query: &Q,
        ctx: &C,
        loader: F,
    ) -> Result<P, E>
    where
        T: CachePolicy,
        C: CacheContext,
        Q: Serialize + Send + Sync,
        P: Serialize + DeserializeOwned + Send + Sync,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<P, E>>,
    {
        let (enabled, strategy, ttl) = self.resolve_policy::<T>();
        if !enabled || strategy == CacheStrategy::None {
            return loader().await;
        }

        let scope = self.resolve_page_scope::<T>(ctx);
        if scope == "none" || scope == "anon" {
            return loader().await;
        }

        let fingerprint = query_fingerprint(query);
        let key = self.build_page_key(T::BIZ, &scope, &fingerprint);

        if let Ok(Some(cached_page)) = self.store.get_json::<P>(&key).await {
            tracing::debug!(biz = %T::BIZ, key = %key, "⚡ [Cache HIT] 命中分页缓存");
            return Ok(cached_page);
        }

        tracing::debug!(biz = %T::BIZ, key = %key, "💨 [Cache MISS] 执行分页查库");
        let page_data = loader().await?;
        let ttl_opt = if ttl > 0 { Some(ttl) } else { None };
        let _ = self.store.set_json(&key, &page_data, ttl_opt).await;
        Ok(page_data)
    }

    /// 针对指定实体类型的单条记录缓存操作代理句柄
    ///
    /// 解决 Rust 在调用泛型方法时需书写冗长类型参数的问题：
    /// ```rust,ignore
    /// cache.item::<UserBo>().load("100", &ctx, || async move { ... }).await
    /// ```
    pub fn item<T: CachePolicy>(&self) -> DynamicCacheItem<'_, T> {
        DynamicCacheItem {
            cache: self,
            _marker: std::marker::PhantomData,
        }
    }

    /// 针对指定实体类型的分页缓存操作代理句柄
    ///
    /// 解决 Rust 在调用泛型方法时需书写冗长类型参数的问题：
    /// ```rust,ignore
    /// cache.page::<UserBo>().load(&query, &ctx, || async move { ... }).await
    /// ```
    pub fn page<T: CachePolicy>(&self) -> DynamicCachePage<'_, T> {
        DynamicCachePage {
            cache: self,
            _marker: std::marker::PhantomData,
        }
    }

    /// 主动失效特定 Scope 下的单条详情缓存
    pub async fn evict_scoped<T: CachePolicy>(&self, scope: &str, id: &str) -> CacheResult<()> {
        let key = self.build_key(T::BIZ, scope, id);
        self.store.del(&key).await
    }

    /// 主动失效单条详情缓存（带上下文推导）
    pub async fn evict_with_ctx<T: CachePolicy>(
        &self,
        id: impl AsRef<str>,
        ctx: &impl CacheContext,
    ) -> CacheResult<()> {
        let id_str = id.as_ref();
        let opts = ctx.to_cache_opts();
        let scope = self.resolve_scope::<T>(ctx);
        self.evict_scoped::<T>(&scope, id_str).await?;

        // 针对特权代操作场景：智能联动淘汰该主体私有域详情与分页
        let (_, strategy, _) = self.resolve_policy::<T>();
        if (strategy == CacheStrategy::Subject || strategy == CacheStrategy::Cascading)
            && opts.is_privileged
        {
            let target_subject_scope = format!("sub:{id_str}");
            if target_subject_scope != scope {
                let _ = self.evict_scoped::<T>(&target_subject_scope, id_str).await;
                let page_prefix = self.build_page_prefix(T::BIZ, &target_subject_scope);
                let _ = self.store.del_prefix(&page_prefix).await;
            }
        }
        Ok(())
    }

    /// 主动失效分页/列表缓存
    pub async fn evict_page<T: CachePolicy>(&self, ctx: &impl CacheContext) -> CacheResult<()> {
        let scope = self.resolve_page_scope::<T>(ctx);
        let prefix = self.build_page_prefix(T::BIZ, &scope);
        self.store.del_prefix(&prefix).await?;

        let (_, strategy, _) = self.resolve_policy::<T>();
        if strategy == CacheStrategy::Shared {
            // Shared 发生变更，级联清理特权视图的分页
            let privileged_prefix = self.build_page_prefix(T::BIZ, "privileged");
            let _ = self.store.del_prefix(&privileged_prefix).await;
        }
        Ok(())
    }

    /// 智能全量淘汰（同时淘汰详情与关联的分页列表）
    pub async fn evict_all_smart<T: CachePolicy>(
        &self,
        id: Option<impl AsRef<str>>,
        ctx: &impl CacheContext,
    ) -> CacheResult<()> {
        if let Some(id) = id {
            self.evict_with_ctx::<T>(id, ctx).await?;
        }
        self.evict_page::<T>(ctx).await
    }

    /// 主体代操作场景：明确根据目标所属主体淘汰私有缓存
    pub async fn evict_subject<T: CachePolicy>(
        &self,
        subject: impl AsRef<str>,
        id: Option<impl AsRef<str>>,
    ) -> CacheResult<()> {
        let sub = subject.as_ref();
        let scope = format!("sub:{sub}");

        if let Some(id_ref) = id {
            let _ = self.evict_scoped::<T>(&scope, id_ref.as_ref()).await;
        }

        let page_prefix = self.build_page_prefix(T::BIZ, &scope);
        self.store.del_prefix(&page_prefix).await?;
        Ok(())
    }

    /// 兼容老命名的别名方法（淘汰用户维度私有缓存）
    #[inline]
    pub async fn evict_user<T: CachePolicy>(
        &self,
        user_id: impl AsRef<str>,
        id: Option<impl AsRef<str>>,
    ) -> CacheResult<()> {
        self.evict_subject::<T>(user_id, id).await
    }

    /// 无上下文淘汰单条详情（仅适用于 Shared 实体）
    pub async fn evict<T: CachePolicy>(&self, id: impl AsRef<str>) -> CacheResult<()> {
        let (_, strategy, _) = self.resolve_policy::<T>();
        if strategy != CacheStrategy::Shared {
            tracing::warn!(
                biz = %T::BIZ,
                strategy = %strategy.as_str(),
                "非 Shared 策略实体请使用 `evict_with_ctx` 或 `evict_subject`"
            );
            return Ok(());
        }
        self.evict_scoped::<T>("shared", id.as_ref()).await
    }

    /// 兼容旧命名的全量淘汰接口（同时淘汰详情与分页）
    #[inline]
    pub async fn evict_all<T: CachePolicy>(
        &self,
        id: impl AsRef<str>,
        ctx: &impl CacheContext,
    ) -> CacheResult<()> {
        self.evict_all_smart::<T>(Some(id), ctx).await
    }

    /// 获取底层存储中的原始字符串值
    #[inline]
    pub async fn get(&self, key: &str) -> CacheResult<Option<String>> {
        self.store.get(key).await
    }

    /// 设置原始字符串键值对与过期时间（秒，None 为不过期）
    #[inline]
    pub async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> CacheResult<()> {
        self.store.set(key, value, ttl_secs).await
    }

    /// 删除指定缓存键
    #[inline]
    pub async fn del(&self, key: &str) -> CacheResult<()> {
        self.store.del(key).await
    }

    /// 判断键是否存在且未过期
    #[inline]
    pub async fn exists(&self, key: &str) -> CacheResult<bool> {
        self.store.exists(key).await
    }

    /// 按前缀批量删除键，返回删除成功的数量
    #[inline]
    pub async fn del_prefix(&self, prefix: &str) -> CacheResult<usize> {
        self.store.del_prefix(prefix).await
    }

    /// 获取并自动反序列化为 JSON 泛型对象
    #[inline]
    pub async fn get_json<V: DeserializeOwned>(&self, key: &str) -> CacheResult<Option<V>> {
        self.store.get_json::<V>(key).await
    }

    /// 序列化泛型对象并写入底层存储
    #[inline]
    pub async fn set_json<V: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &V,
        ttl_secs: Option<u64>,
    ) -> CacheResult<()> {
        self.store.set_json(key, value, ttl_secs).await
    }
}

impl std::ops::Deref for DynamicCache {
    type Target = Arc<dyn CacheStore>;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

/// 分页缓存操作代理结构体
pub struct DynamicCachePage<'a, T: CachePolicy> {
    cache: &'a DynamicCache,
    _marker: std::marker::PhantomData<T>,
}

impl<'a, T: CachePolicy> DynamicCachePage<'a, T> {
    /// 记忆化分页/列表查询
    pub async fn load<Q, P, F, Fut, E>(
        &self,
        query: &Q,
        ctx: &impl CacheContext,
        loader: F,
    ) -> Result<P, E>
    where
        Q: Serialize + Send + Sync,
        P: Serialize + DeserializeOwned + Send + Sync,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<P, E>>,
    {
        self.cache.get_or_load_page::<T, _, Q, P, F, Fut, E>(query, ctx, loader).await
    }
}

/// 单记录缓存操作代理结构体
pub struct DynamicCacheItem<'a, T: CachePolicy> {
    cache: &'a DynamicCache,
    _marker: std::marker::PhantomData<T>,
}

impl<'a, T: CachePolicy> DynamicCacheItem<'a, T> {
    /// 记忆化单记录查询
    pub async fn load<F, Fut, E>(
        &self,
        id: impl AsRef<str>,
        ctx: &impl CacheContext,
        loader: F,
    ) -> Result<Option<T>, E>
    where
        T: Serialize + DeserializeOwned + Send + Sync,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Option<T>, E>>,
    {
        self.cache.get_or_load_with_ctx::<T, F, Fut, E>(id, ctx, loader).await
    }
}
