use pecs_cache::{
    cache_evict, cache_policy, cacheable, CacheConfig, DynamicCache,
    LocalMemoryStore, SimpleContext,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct User {
    id: u64,
    name: String,
}

cache_policy!(User, biz = "user", strategy = User, ttl = 300);

struct UserService {
    cache: Arc<DynamicCache>,
    db_calls: Arc<AtomicUsize>,
}

impl UserService {
    #[cacheable(User, id = id)]
    pub async fn get_user(&self, id: u64, ctx: &SimpleContext) -> Result<User, String> {
        self.db_calls.fetch_add(1, Ordering::SeqCst);
        Ok(User {
            id,
            name: format!("User_{id}"),
        })
    }

    #[cache_evict(User, id = user.id, all)]
    pub async fn update_user(&self, user: &User, ctx: &SimpleContext) -> Result<(), String> {
        self.db_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn test_macro_cacheable_and_evict() {
    let store = Arc::new(LocalMemoryStore::new());
    let cache = Arc::new(DynamicCache::new(store, CacheConfig::default()));
    let db_calls = Arc::new(AtomicUsize::new(0));

    let svc = UserService {
        cache,
        db_calls: db_calls.clone(),
    };

    let ctx = SimpleContext::new().with_user_id("10");

    // 1. 第一次调用 get_user：未命中，执行方法体，DB 次数 +1
    let u1 = svc.get_user(10, &ctx).await.unwrap();
    assert_eq!(u1.name, "User_10");
    assert_eq!(db_calls.load(Ordering::SeqCst), 1);

    // 2. 第二次调用 get_user：命中缓存，透明拦截直接返回，DB 次数不变！
    let u2 = svc.get_user(10, &ctx).await.unwrap();
    assert_eq!(u2.name, "User_10");
    assert_eq!(db_calls.load(Ordering::SeqCst), 1);

    // 3. 执行更新方法 update_user：业务成功后自动触发 cache_evict 淘汰
    svc.update_user(&u1, &ctx).await.unwrap();
    assert_eq!(db_calls.load(Ordering::SeqCst), 2);

    // 4. 第三次调用 get_user：缓存已失效，再次回源，DB 次数 +1
    let u3 = svc.get_user(10, &ctx).await.unwrap();
    assert_eq!(u3.name, "User_10");
    assert_eq!(db_calls.load(Ordering::SeqCst), 3);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Order {
    id: u64,
    buyer_id: u64,
    title: String,
}

cache_policy!(Order, biz = "order", strategy = User, ttl = 300);

struct OrderService {
    cache: Arc<DynamicCache>,
    db_calls: Arc<AtomicUsize>,
}

impl OrderService {
    // 纯 Opts / UID 方式：方法签名完全不需要任何 Context 结构体！直接通过 uid 配置！
    #[cacheable(Order, id = id, uid = buyer_id)]
    pub async fn get_order(&self, id: u64, buyer_id: u64) -> Result<Order, String> {
        self.db_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Order {
            id,
            buyer_id,
            title: format!("Order_{id}"),
        })
    }

    #[cache_evict(Order, id = order.id, uid = order.buyer_id, all)]
    pub async fn update_order(&self, order: &Order) -> Result<(), String> {
        self.db_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn test_macro_with_pure_uid_opts() {
    let store = Arc::new(LocalMemoryStore::new());
    let cache = Arc::new(DynamicCache::new(store, CacheConfig::default()));
    let db_calls = Arc::new(AtomicUsize::new(0));

    let svc = OrderService {
        cache,
        db_calls: db_calls.clone(),
    };

    // 第一次查询：回源查库
    let o1 = svc.get_order(1001, 888).await.unwrap();
    assert_eq!(o1.title, "Order_1001");
    assert_eq!(db_calls.load(Ordering::SeqCst), 1);

    // 第二次查询：命中缓存（Key 格式：app_cache_order:detail:uid:888:id:1001）
    let o2 = svc.get_order(1001, 888).await.unwrap();
    assert_eq!(o2.title, "Order_1001");
    assert_eq!(db_calls.load(Ordering::SeqCst), 1);

    // 另一个用户 999 查询同一个订单 1001：由于是 User 隔离策略，999 无法读到 888 的缓存，必须回源！
    let _o3 = svc.get_order(1001, 999).await.unwrap();
    assert_eq!(db_calls.load(Ordering::SeqCst), 2);

    // 执行更新：自动淘汰 uid:888 下的详情与分页
    svc.update_order(&o1).await.unwrap();
    assert_eq!(db_calls.load(Ordering::SeqCst), 3);

    // 再次读取：已失效，重新回源
    let _ = svc.get_order(1001, 888).await.unwrap();
    assert_eq!(db_calls.load(Ordering::SeqCst), 4);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DeviceConfig {
    id: u64,
    device_id: String,
    config_data: String,
}

cache_policy!(DeviceConfig, biz = "device_cfg", strategy = Subject, ttl = 300);

struct DeviceService {
    cache: Arc<DynamicCache>,
    db_calls: Arc<AtomicUsize>,
}

impl DeviceService {
    #[cacheable(DeviceConfig, id = id, sub = device_id)]
    pub async fn get_config(&self, id: u64, device_id: &str) -> Result<DeviceConfig, String> {
        self.db_calls.fetch_add(1, Ordering::SeqCst);
        Ok(DeviceConfig {
            id,
            device_id: device_id.to_string(),
            config_data: format!("Config_{id}"),
        })
    }

    #[cache_evict(DeviceConfig, id = cfg.id, sub = cfg.device_id, all)]
    pub async fn update_config(&self, cfg: &DeviceConfig) -> Result<(), String> {
        self.db_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn test_macro_with_subject_opts() {
    let store = Arc::new(LocalMemoryStore::new());
    let cache = Arc::new(DynamicCache::new(store, CacheConfig::default()));
    let db_calls = Arc::new(AtomicUsize::new(0));

    let svc = DeviceService {
        cache,
        db_calls: db_calls.clone(),
    };

    let c1 = svc.get_config(1, "dev_alpha").await.unwrap();
    assert_eq!(c1.config_data, "Config_1");
    assert_eq!(db_calls.load(Ordering::SeqCst), 1);

    // 第二次命中缓存
    let c2 = svc.get_config(1, "dev_alpha").await.unwrap();
    assert_eq!(c2.config_data, "Config_1");
    assert_eq!(db_calls.load(Ordering::SeqCst), 1);

    // 另一个设备 dev_beta 查询 -> 跨主体严格隔离，必须回源！
    let _ = svc.get_config(1, "dev_beta").await.unwrap();
    assert_eq!(db_calls.load(Ordering::SeqCst), 2);

    // 淘汰 dev_alpha
    svc.update_config(&c1).await.unwrap();
    assert_eq!(db_calls.load(Ordering::SeqCst), 3);

    // 再次回源
    let _ = svc.get_config(1, "dev_alpha").await.unwrap();
    assert_eq!(db_calls.load(Ordering::SeqCst), 4);
}
