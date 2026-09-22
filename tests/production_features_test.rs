use pecs_cache::{
    cache_evict, cache_policy, cacheable, CacheConfig, CacheOpts, DynamicCache, LocalMemoryStore,
    SimpleContext,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Product {
    id: u64,
    name: String,
    price: u64,
}

cache_policy!(Product, biz = "product", strategy = Shared, ttl = 300);

#[tokio::test]
async fn test_singleflight_stampede_protection() {
    let store = Arc::new(LocalMemoryStore::new());
    let cache = Arc::new(DynamicCache::new(store, CacheConfig::default()));

    let loader_calls = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();

    // 模拟 50 个并发请求同一时间涌入查询同一个未命中的热点 Key
    for _ in 0..50 {
        let cache_clone = cache.clone();
        let counter = loader_calls.clone();
        handles.push(tokio::spawn(async move {
            cache_clone
                .item::<Product>()
                .load("item_888", &(), || async move {
                    // 模拟慢查询 DB 延迟 50ms
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, String>(Some(Product {
                        id: 888,
                        name: "Super Phone".into(),
                        price: 5999,
                    }))
                })
                .await
        }));
    }

    let mut results = Vec::new();
    for h in handles {
        let r = h.await.unwrap().unwrap().unwrap();
        results.push(r);
    }

    // 断言 1：所有 50 个并发请求均成功获取到正确数据
    assert_eq!(results.len(), 50);
    for p in results {
        assert_eq!(p.id, 888);
        assert_eq!(p.name, "Super Phone");
    }

    // 断言 2：Single-flight 彻底生效，底层数据库 loader 只被调用了 1 次！
    assert_eq!(loader_calls.load(Ordering::SeqCst), 1);

    // 断言 3：省去的并发调用次数为 49 次
    assert_eq!(cache.coalesced_calls(), 49);
}

#[tokio::test]
async fn test_null_caching_penetration_protection() {
    let store = Arc::new(LocalMemoryStore::new());
    let mut config = CacheConfig::default();
    config.null_ttl_seconds = 60;

    let cache = Arc::new(DynamicCache::new(store, config));
    let loader_calls = Arc::new(AtomicUsize::new(0));

    // 第一次查询：数据库中不存在该 ID (loader 返回 Ok(None))
    let counter = loader_calls.clone();
    let res1 = cache
        .item::<Product>()
        .load("non_existent_id", &(), || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(None)
        })
        .await
        .unwrap();

    assert_eq!(res1, None);
    assert_eq!(loader_calls.load(Ordering::SeqCst), 1);

    // 第二次查询：命中防穿透空值哨兵，loader 绝不会被调用！
    let counter = loader_calls.clone();
    let res2 = cache
        .item::<Product>()
        .load("non_existent_id", &(), || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(None)
        })
        .await
        .unwrap();

    assert_eq!(res2, None);
    assert_eq!(loader_calls.load(Ordering::SeqCst), 1); // loader 仍然是 1 次！

    // 检查统计指标
    let stats = cache.stats();
    assert_eq!(stats.null_hits, 1);
    assert_eq!(stats.misses, 1);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OrderRecord {
    id: u64,
    buyer_uid: u64,
    amount: u64,
}

cache_policy!(OrderRecord, biz = "order_rec", strategy = Subject, ttl = 300);

#[tokio::test]
async fn test_owner_eviction_in_privileged_mode() {
    let store = Arc::new(LocalMemoryStore::new());
    let cache = Arc::new(DynamicCache::new(store, CacheConfig::default()));

    let buyer_ctx = CacheOpts::subject("buyer_10026");
    let loader_calls = Arc::new(AtomicUsize::new(0));

    // 1. 买家 buyer_10026 查询自身订单 9999
    let counter = loader_calls.clone();
    let o1 = cache
        .item::<OrderRecord>()
        .load("9999", &buyer_ctx, || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(Some(OrderRecord {
                id: 9999,
                buyer_uid: 10026,
                amount: 100,
            }))
        })
        .await
        .unwrap();

    assert_eq!(o1.unwrap().amount, 100);
    assert_eq!(loader_calls.load(Ordering::SeqCst), 1);

    // 2. 第二次买家查询：命中私有缓存
    let counter = loader_calls.clone();
    let _o2 = cache
        .item::<OrderRecord>()
        .load("9999", &buyer_ctx, || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(None)
        })
        .await
        .unwrap();
    assert_eq!(loader_calls.load(Ordering::SeqCst), 1);

    // 3. 管理员在后台代客修改/审核订单，明确指定 target_subject / owner 为 "buyer_10026"
    let admin_ctx = CacheOpts::privileged().with_owner("buyer_10026");
    cache
        .evict_with_ctx::<OrderRecord>("9999", &admin_ctx)
        .await
        .unwrap();

    // 4. 买家再次查询：缓存已精准失效，重新回源加载最新数据！
    let counter = loader_calls.clone();
    let o3 = cache
        .item::<OrderRecord>()
        .load("9999", &buyer_ctx, || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(Some(OrderRecord {
                id: 9999,
                buyer_uid: 10026,
                amount: 200, // 最新修改金额
            }))
        })
        .await
        .unwrap();

    assert_eq!(o3.unwrap().amount, 200);
    assert_eq!(loader_calls.load(Ordering::SeqCst), 2);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PublicArticle {
    id: u64,
    title: String,
}

// 正常公共实体：默认 ISOLATE_PAGE = false
cache_policy!(PublicArticle, biz = "pub_article", strategy = Shared, ttl = 300);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MyPersonalWiki {
    id: u64,
    title: String,
}

// 含有行级个人权限的实体：配置 ISOLATE_PAGE = true
cache_policy!(
    MyPersonalWiki,
    biz = "wiki_with_perms",
    strategy = Shared,
    ttl = 300,
    isolate_page = true
);

#[tokio::test]
async fn test_page_shared_and_isolate_page_policy() {
    let store = Arc::new(LocalMemoryStore::new());
    let cache = Arc::new(DynamicCache::new(store, CacheConfig::default()));

    let anon_ctx = SimpleContext::new();
    let user_ctx = SimpleContext::new().with_user_id("10026");

    // 1. 公共文章 (ISOLATE_PAGE = false):
    // 无论是未登录访客还是已登录用户，分页 Scope 均为 "shared"！实现真正的 100% 缓存共享！
    assert_eq!(cache.resolve_page_scope::<PublicArticle>(&anon_ctx), "shared");
    assert_eq!(cache.resolve_page_scope::<PublicArticle>(&user_ctx), "shared");

    // 2. 带个人行级权限的实体 (ISOLATE_PAGE = true):
    // 登录用户自动隔离至自身私有分页 sub:10026，绝不污染公共域！
    assert_eq!(cache.resolve_page_scope::<MyPersonalWiki>(&user_ctx), "sub:10026");
    assert_eq!(cache.resolve_page_scope::<MyPersonalWiki>(&anon_ctx), "shared");

    // 3. 动态请求参数显式隔离：公共文章通过 .with_isolate_page(true) 临时隔离
    let isolated_req = user_ctx.clone().with_isolate_page(true);
    assert_eq!(cache.resolve_page_scope::<PublicArticle>(&isolated_req), "sub:10026");
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct UserItem {
    id: u64,
    name: String,
}

cache_policy!(UserItem, biz = "user_item", strategy = Subject, ttl = 300);

struct MockItemService {
    cache: Arc<DynamicCache>,
    db_calls: Arc<AtomicUsize>,
}

impl MockItemService {
    // 支持 Result<Option<T>, E> 的直接宏修饰！
    #[cacheable(UserItem, id = id, optional)]
    pub async fn find_user(&self, id: u64, ctx: &CacheOpts) -> Result<Option<UserItem>, String> {
        self.db_calls.fetch_add(1, Ordering::SeqCst);
        if id == 404 {
            return Ok(None);
        }
        Ok(Some(UserItem {
            id,
            name: format!("Item_{id}"),
        }))
    }

    #[cache_evict(UserItem, id = id, all, owner = owner)]
    pub async fn admin_delete_user(&self, id: u64, owner: &str) -> Result<(), String> {
        self.db_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn test_macro_optional_return_and_owner_evict() {
    let store = Arc::new(LocalMemoryStore::new());
    let cache = Arc::new(DynamicCache::new(store, CacheConfig::default()));
    let db_calls = Arc::new(AtomicUsize::new(0));

    let svc = MockItemService {
        cache: cache.clone(),
        db_calls: db_calls.clone(),
    };

    let user_ctx = CacheOpts::subject("uid_99");

    // 1. 正常存在数据：第一次查库
    let u1 = svc.find_user(1, &user_ctx).await.unwrap();
    assert_eq!(u1.unwrap().name, "Item_1");
    assert_eq!(db_calls.load(Ordering::SeqCst), 1);

    // 2. 第二次查询：命中缓存
    let u2 = svc.find_user(1, &user_ctx).await.unwrap();
    assert_eq!(u2.unwrap().name, "Item_1");
    assert_eq!(db_calls.load(Ordering::SeqCst), 1);

    // 3. 不存在的数据 (404)：自动写入防穿透空值缓存
    let none1 = svc.find_user(404, &user_ctx).await.unwrap();
    assert_eq!(none1, None);
    assert_eq!(db_calls.load(Ordering::SeqCst), 2);

    // 再次查 404：命中空值哨兵，不查库！
    let none2 = svc.find_user(404, &user_ctx).await.unwrap();
    assert_eq!(none2, None);
    assert_eq!(db_calls.load(Ordering::SeqCst), 2);

    // 4. 管理员代客淘汰：通过 owner 语法自动精准淘汰 uid_99
    svc.admin_delete_user(1, "uid_99").await.unwrap();
    assert_eq!(db_calls.load(Ordering::SeqCst), 3);

    // 再次查询 ID 1：已被成功失效淘汰，重新回源！
    let u3 = svc.find_user(1, &user_ctx).await.unwrap();
    assert_eq!(u3.unwrap().name, "Item_1");
    assert_eq!(db_calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn test_memory_store_sharding_and_capacity_eviction() {
    // 创建最大容量仅为 16 条的微型内存缓存
    let store = LocalMemoryStore::with_capacity(16);

    // 写入 32 条记录（触发分片超限淘汰）
    for i in 0..32 {
        store.set(&format!("k_{i}"), "val", None).await.unwrap();
    }

    // 确保记录了淘汰计数
    let stats = store.stats();
    assert!(stats.evictions > 0);
}
