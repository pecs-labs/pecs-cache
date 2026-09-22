use pecs_cache::{
    cache_policy, CacheConfig, CacheEntryConfig, DynamicCache, LocalMemoryStore, SimpleContext,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct UserProfile {
    id: u64,
    nickname: String,
}

cache_policy!(UserProfile, biz = "user_profile", strategy = User, ttl = 300);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Article {
    id: u64,
    title: String,
}

cache_policy!(Article, biz = "article", strategy = Shared, ttl = 300);

#[tokio::test]
async fn test_dynamic_scope_resolution() {
    let store = Arc::new(LocalMemoryStore::new());
    let cache = DynamicCache::new(store, CacheConfig::default());

    // 1. 普通主体/用户访问 Subject/User 策略实体 -> 隔离至 sub:100
    let user_ctx = SimpleContext::new().with_user_id("100");
    assert_eq!(
        cache.resolve_scope::<UserProfile>(&user_ctx),
        "sub:100"
    );

    // 2. 未认证访客访问 Subject/User 策略实体 -> none (直接穿透底层数据库，不写缓存)
    let anon_ctx = SimpleContext::new();
    assert_eq!(cache.resolve_scope::<UserProfile>(&anon_ctx), "none");

    // 3. 特权访问（管理员/Supervisor） -> 隔离至 privileged 特权视图
    let admin_ctx = SimpleContext::new().with_admin(true);
    assert_eq!(cache.resolve_scope::<UserProfile>(&admin_ctx), "privileged");

    // 4. 访问 Shared 策略实体 -> 全局 shared
    assert_eq!(cache.resolve_scope::<Article>(&user_ctx), "shared");
    assert_eq!(cache.resolve_scope::<Article>(&anon_ctx), "shared");

    // 5. 多租户隔离测试 -> tenant:{id}
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct TenantConfig {
        key: String,
        val: String,
    }
    cache_policy!(TenantConfig, biz = "tenant_config", strategy = Tenant, ttl = 300);
    let tenant_ctx = pecs_cache::CacheOpts::tenant("org_42");
    assert_eq!(cache.resolve_scope::<TenantConfig>(&tenant_ctx), "tenant:org_42");
}

#[tokio::test]
async fn test_cache_aside_with_ctx() {
    let store = Arc::new(LocalMemoryStore::new());
    let cache = DynamicCache::new(store, CacheConfig::default());

    let user_ctx = SimpleContext::new().with_user_id("100");
    let load_count = Arc::new(AtomicUsize::new(0));

    // 第 1 次读取：未命中缓存，执行回源 loader
    let counter = load_count.clone();
    let res = cache
        .item::<UserProfile>()
        .load("100", &user_ctx, || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(Some(UserProfile {
                id: 100,
                nickname: "Alice".to_string(),
            }))
        })
        .await
        .unwrap();

    assert_eq!(res.unwrap().nickname, "Alice");
    assert_eq!(load_count.load(Ordering::SeqCst), 1);

    // 第 2 次读取：命中缓存，不执行回源 loader
    let counter = load_count.clone();
    let res2 = cache
        .item::<UserProfile>()
        .load("100", &user_ctx, || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(Some(UserProfile {
                id: 100,
                nickname: "Bob".to_string(),
            }))
        })
        .await
        .unwrap();

    assert_eq!(res2.unwrap().nickname, "Alice"); // 仍是缓存中的 Alice
    assert_eq!(load_count.load(Ordering::SeqCst), 1); // loader 未被调用！

    // 写操作淘汰缓存
    cache
        .evict_with_ctx::<UserProfile>("100", &user_ctx)
        .await
        .unwrap();

    // 第 3 次读取：缓存已淘汰，重新回源
    let counter = load_count.clone();
    let res3 = cache
        .item::<UserProfile>()
        .load("100", &user_ctx, || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(Some(UserProfile {
                id: 100,
                nickname: "Alice_Updated".to_string(),
            }))
        })
        .await
        .unwrap();

    assert_eq!(res3.unwrap().nickname, "Alice_Updated");
    assert_eq!(load_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_paginated_cache_and_evict() {
    let store = Arc::new(LocalMemoryStore::new());
    let cache = DynamicCache::new(store, CacheConfig::default());
    let user_ctx = SimpleContext::new().with_user_id("100");

    #[derive(Serialize)]
    struct ArticleQuery {
        page_no: u32,
        page_size: u32,
    }

    let query = ArticleQuery {
        page_no: 1,
        page_size: 10,
    };
    let load_count = Arc::new(AtomicUsize::new(0));

    // 第一次查分页
    let counter = load_count.clone();
    let page1: Vec<Article> = cache
        .page::<Article>()
        .load(&query, &user_ctx, || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(vec![Article {
                id: 1,
                title: "Rust Cache".to_string(),
            }])
        })
        .await
        .unwrap();

    assert_eq!(page1.len(), 1);
    assert_eq!(load_count.load(Ordering::SeqCst), 1);

    // 第二次查分页（命中缓存）
    let counter = load_count.clone();
    let page2: Vec<Article> = cache
        .page::<Article>()
        .load(&query, &user_ctx, || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(vec![])
        })
        .await
        .unwrap();

    assert_eq!(page2.len(), 1);
    assert_eq!(load_count.load(Ordering::SeqCst), 1);

    // 淘汰分页缓存
    cache.evict_page::<Article>(&user_ctx).await.unwrap();

    // 第三次查分页（重新加载）
    let counter = load_count.clone();
    let _page3: Vec<Article> = cache
        .page::<Article>()
        .load(&query, &user_ctx, || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(vec![])
        })
        .await
        .unwrap();

    assert_eq!(load_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_dynamic_config_disable_override() {
    let store = Arc::new(LocalMemoryStore::new());
    let mut config = CacheConfig::default();

    // 动态禁用 article 业务的缓存
    config.entries.insert(
        "article".to_string(),
        CacheEntryConfig {
            enable: false,
            strategy: None,
            ttl: None,
        },
    );

    let cache = DynamicCache::new(store, config);
    let (enabled, _, _) = cache.resolve_policy::<Article>();
    assert!(!enabled); // 动态关闭成功
}

struct MyCustomRequestContext {
    session_uid: u64,
    ip_addr: String,
    is_super_admin: bool,
}

// 仅需实现这一个方法！返回包含必要字段的小对象 CacheOpts！
impl pecs_cache::CacheContext for MyCustomRequestContext {
    fn to_cache_opts(&self) -> pecs_cache::CacheOpts {
        pecs_cache::CacheOpts {
            subject: Some(self.session_uid.to_string()),
            origin: Some(self.ip_addr.clone()),
            is_privileged: self.is_super_admin,
            tenant: None,
            explicit_scope: None,
        }
    }
}

#[tokio::test]
async fn test_single_method_to_cache_opts() {
    let store = Arc::new(LocalMemoryStore::new());
    let cache = DynamicCache::new(store, CacheConfig::default());

    let ctx = MyCustomRequestContext {
        session_uid: 777,
        ip_addr: "127.0.0.1".into(),
        is_super_admin: false,
    };

    assert_eq!(cache.resolve_scope::<UserProfile>(&ctx), "sub:777");

    let admin_ctx = MyCustomRequestContext {
        session_uid: 1,
        ip_addr: "127.0.0.1".into(),
        is_super_admin: true,
    };

    assert_eq!(cache.resolve_scope::<UserProfile>(&admin_ctx), "privileged");
}
