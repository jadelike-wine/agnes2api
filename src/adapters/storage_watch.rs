//! 存储可写性的**IO 那一半**：装饰器 + 启动探测。
//!
//! 状态本体在 [`crate::core::storage_health`]，那边写着这件事为什么存在。

use crate::adapters::now_ms;
use crate::core::storage_health::StorageHealth;
use crate::ports::storage::{Storage, StorageError, StorageResult};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

/// 健康探针用的键。
///
/// ⚠️ 它**刻意不带 `key:` 前缀**：key 池兜底那条 `list("key:")` 会把带前缀的东西
/// 扫进来当成一把 key 记录。于是即便下面「删探针」那一步失败，
/// 残留的探针键也不会被当成一把坏 key。
pub const HEALTH_PROBE_KEY: &str = "health:probe";

/// 包一层，把**每一次真实写操作**的成败反映到 [`StorageHealth`] 上。
///
/// 🔴 **只观测 `put` / `delete`。** 读操作在数据目录只读时照样成功，
/// 用它们判断可写性会得出「健康」的错误结论 —— 真机上正是读得到、写不进。
///
/// 🔴 **异常一律原样抛出，不吞**：调用方的错误处理路径完全不变。
/// 这个装饰器只是顺路观察，它不许改变任何一条业务路径的行为。
pub struct WatchedStorage {
    inner: Arc<dyn Storage>,
    health: Arc<StorageHealth>,
}

impl WatchedStorage {
    pub fn new(inner: Arc<dyn Storage>, health: Arc<StorageHealth>) -> Self {
        Self { inner, health }
    }

    fn observe<T>(&self, r: StorageResult<T>) -> StorageResult<T> {
        self.health.record(r.is_ok(), now_ms());
        r
    }
}

#[async_trait]
impl Storage for WatchedStorage {
    async fn get(&self, key: &str) -> StorageResult<Option<Value>> {
        // 刻意不观测：读成功证明不了可写。
        self.inner.get(key).await
    }

    async fn put(&self, key: &str, value: Value, expires_at: Option<i64>) -> StorageResult<()> {
        let r = self.inner.put(key, value, expires_at).await;
        self.observe(r)
    }

    async fn delete(&self, key: &str) -> StorageResult<()> {
        let r = self.inner.delete(key).await;
        self.observe(r)
    }

    async fn list(&self, prefix: &str) -> StorageResult<Vec<String>> {
        // 同 `get`：刻意不观测。
        self.inner.list(prefix).await
    }
}

/// 启动时探一次「这个数据目录到底写不写得进」。
///
/// 返回 `Some(err)` 表示写不进（调用方据此大声报错并让 `/health` 转 degraded），
/// `None` 表示能写。
///
/// # 🔴 结论**只看 `put`**
///
/// `delete` 只是「不留痕迹」的收尾，**它失败并不能推翻「刚刚确实写进去了」这个事实**。
/// 原实现把两步一起 try，于是「写得进、删不掉」会被误报成 degraded ——
/// 而那种情况下探针键还留在存储里，**两个后果同时发生**。
///
/// # 🔴 探测失败**不阻止进程启动**
///
/// 让容器起来并把 `/health` 报成不健康，**比崩溃重启循环更容易排障**：
/// 运维能直接 curl 到原因，也仍然会在 `docker ps` 里看到 unhealthy。
pub async fn probe_writable(storage: &dyn Storage, health: &StorageHealth) -> Option<StorageError> {
    let now = now_ms();
    if let Err(e) = storage
        .put(HEALTH_PROBE_KEY, json!({ "at": now }), None)
        .await
    {
        health.record(false, now);
        return Some(e);
    }
    health.record(true, now);
    // 删失败只是留了个痕迹，**按 put 的结论走**。
    let _ = storage.delete(HEALTH_PROBE_KEY).await;
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::MemoryStorage;
    use crate::ports::storage::Storage;

    /// 🔴 **读成功不许被当成「可写」。**
    ///
    /// 回归对象（真机）：绑定挂载属主不匹配时，读得到、写不进，
    /// 而只观测读的实现会把这种部署报成 healthy —— 所有 API 调用都在失败。
    #[tokio::test]
    async fn reads_never_move_the_writable_verdict() {
        let health = Arc::new(StorageHealth::new());
        let inner = Arc::new(MemoryStorage::new());
        let w = WatchedStorage::new(inner, health.clone());

        w.get("k").await.unwrap();
        w.list("").await.unwrap();
        assert!(
            health.status().checked_at.is_none(),
            "读操作不许产生任何观测"
        );
    }

    /// 写失败翻成不可写，写成功再翻回来。
    #[tokio::test]
    async fn writes_move_the_verdict_both_ways() {
        let health = Arc::new(StorageHealth::new());
        let inner = Arc::new(MemoryStorage::new());
        inner.fail_next("EACCES");
        let w = WatchedStorage::new(inner, health.clone());

        assert!(w.put("k", json!(1), None).await.is_err());
        assert!(!health.status().writable);

        w.put("k", json!(1), None).await.unwrap();
        assert!(
            health.status().writable,
            "修好之后必须能恢复，否则只能靠重启"
        );
    }

    /// 🔴 装饰器**不许改变**调用方看到的结果（错误原样抛）。
    #[tokio::test]
    async fn the_decorator_is_transparent_to_callers() {
        let health = Arc::new(StorageHealth::new());
        let inner = Arc::new(MemoryStorage::new());
        inner.fail_next("原始错误文本");
        let w = WatchedStorage::new(inner, health);
        let err = w.delete("k").await.unwrap_err();
        assert_eq!(err.to_string(), "原始错误文本");
    }

    /// 🔴 **可写性的结论只看 `put`**：删不掉不许推翻「刚刚确实写进去了」。
    ///
    /// 回归对象：两步一起 try 的实现会把「写得进、删不掉」误报成 degraded，
    /// 而那种情况下探针键还留在存储里，两个后果同时发生。
    #[tokio::test]
    async fn probe_verdict_ignores_a_failing_cleanup() {
        struct PutOkDeleteFails(MemoryStorage);
        #[async_trait]
        impl Storage for PutOkDeleteFails {
            async fn get(&self, k: &str) -> StorageResult<Option<Value>> {
                self.0.get(k).await
            }
            async fn put(&self, k: &str, v: Value, e: Option<i64>) -> StorageResult<()> {
                self.0.put(k, v, e).await
            }
            async fn delete(&self, _k: &str) -> StorageResult<()> {
                Err(StorageError::new("删不掉"))
            }
            async fn list(&self, p: &str) -> StorageResult<Vec<String>> {
                self.0.list(p).await
            }
        }

        let health = StorageHealth::new();
        let s = PutOkDeleteFails(MemoryStorage::new());
        assert!(
            probe_writable(&s, &health).await.is_none(),
            "删不掉不等于写不进"
        );
        assert!(health.status().writable);
    }

    /// 写不进时探测如实报错，并把状态翻过去。
    #[tokio::test]
    async fn probe_reports_an_unwritable_directory() {
        let health = StorageHealth::new();
        let s = MemoryStorage::new();
        s.fail_next("EACCES: permission denied");
        let err = probe_writable(&s, &health).await.expect("写不进必须报出来");
        assert!(err.to_string().contains("EACCES"));
        assert!(!health.status().writable);
    }

    /// 探针键**不带 `key:` 前缀** —— 否则删除失败时它会被当成一把坏 key。
    #[test]
    fn the_probe_key_can_never_be_mistaken_for_a_pool_key() {
        assert_eq!(HEALTH_PROBE_KEY, "health:probe");
        assert!(!HEALTH_PROBE_KEY.starts_with("key:"));
    }

    /// 探测成功之后不留痕迹。
    #[tokio::test]
    async fn a_successful_probe_leaves_nothing_behind() {
        let health = StorageHealth::new();
        let s = MemoryStorage::new();
        assert!(probe_writable(&s, &health).await.is_none());
        assert_eq!(s.get(HEALTH_PROBE_KEY).await.unwrap(), None);
    }
}
