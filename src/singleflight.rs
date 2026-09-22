//! 并发请求合并组件 (Single-flight)
//!
//! 彻底解决缓存击穿 (Cache Stampede) 问题：
//! 当热点 Key 过期或初次加载时，成千上万个并发请求同时涌入，
//! Singleflight 保证同一时间针对同一个 Key 仅有一个异步任务回源执行，
//! 其余所有等待协程直接共享该任务的返回值，从而保护数据库不被打崩。

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::OnceCell;

/// 单 Key 并发合并器
#[derive(Clone, Default)]
pub struct Singleflight {
    calls: Arc<Mutex<HashMap<String, Arc<OnceCell<Option<String>>>>>>,
    coalesced_count: Arc<AtomicU64>,
}

impl Singleflight {
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取被并发合并省去的查库调用总次数
    pub fn coalesced_calls(&self) -> u64 {
        self.coalesced_count.load(Ordering::Relaxed)
    }

    /// 执行并发合并异步调用
    ///
    /// - 若当前没有针对 `key` 的正在运行的任务，当前任务成为 Leader 并执行 `work`；
    /// - 若已有正在执行的 Leader，当前任务自动挂起等待 Leader 完成，并直接克隆其执行结果；
    /// - 任务完成（无论成功还是失败）后自动从活跃表中注销。
    pub async fn execute<F, Fut>(&self, key: &str, work: F) -> Option<String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Option<String>>,
    {
        let (cell, is_leader) = {
            let mut guard = self.calls.lock().unwrap();
            if let Some(existing) = guard.get(key) {
                (existing.clone(), false)
            } else {
                let new_cell = Arc::new(OnceCell::new());
                guard.insert(key.to_string(), new_cell.clone());
                (new_cell, true)
            }
        };

        if !is_leader {
            self.coalesced_count.fetch_add(1, Ordering::Relaxed);
        }

        let result = cell
            .get_or_init(|| async move {
                work().await
            })
            .await
            .clone();

        // Leader 负责在任务完成后清理全局活跃 Map
        if is_leader {
            let mut guard = self.calls.lock().unwrap();
            guard.remove(key);
        }

        result
    }
}
