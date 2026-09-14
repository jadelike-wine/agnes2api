//! 存储端口的**测试替身**。`#[cfg(test)]`，不进产物。
//!
//! 这个文件是[端口层](crate::ports)存在的**唯一理由**的实物。没有它，
//! 「1000 次错口令恰好产生 1 次存储读」「`/health` 零存储 I/O」这一族判据写不出来。
//!
//! # 🔴 两条从 TS 版买过单的红线
//!
//! 1. **计数桩必须 `get`/`put`/`delete`/`list` 四种全数。** 上一版只数 `get`，
//!    而被违反的恰恰是**写**那一侧 —— 一族本该红的判据全绿。
//! 2. **替身不许瞬时返回。** 瞬时返回的替身让任何 happens-before 性质都不可观测
//!    （「A 一定发生在 B 之前」在零耗时下永远成立，测不出反例）。
//!    ⇒ [`MemoryStorage`] 带一个可注入的 `delay`。

use crate::ports::storage::{Storage, StorageError, StorageResult};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// 内存存储。**是 `Storage` 契约判据里「第二种实现」的那一种。**
///
/// 契约判据是「**对每种实现各跑一遍**」，不是「挑一种跑」——
/// 这正是端口层的代价与它买到的东西。
#[derive(Default)]
pub struct MemoryStorage {
    data: Mutex<BTreeMap<String, (Value, Option<i64>)>>,
    /// 可注入的假时钟（epoch 毫秒）。`None` = 用真实墙钟。
    clock: Mutex<Option<i64>>,
    /// 每次操作前的延迟。见模块头第 2 条。
    delay: Duration,
    /// 下一次操作要不要失败。用来构造「存储抛错」的路径。
    fail_next: Mutex<Option<String>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// 带延迟的替身。**测 happens-before 性质时必须用它。**
    pub fn with_delay(delay: Duration) -> Self {
        Self {
            delay,
            ..Self::default()
        }
    }

    /// 把时钟钉在某一刻（测 TTL 用）。
    pub fn set_clock(&self, now: i64) {
        *self.clock.lock() = Some(now);
    }

    /// 让**下一次**操作失败。只失败一次，之后恢复正常 ——
    /// 这样才能测「失败之后还能不能自愈」。
    pub fn fail_next(&self, msg: &str) {
        *self.fail_next.lock() = Some(msg.to_string());
    }

    fn now(&self) -> i64 {
        self.clock.lock().unwrap_or_else(super::now_ms)
    }

    async fn gate(&self) -> StorageResult<()> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        match self.fail_next.lock().take() {
            Some(msg) => Err(StorageError::new(msg)),
            None => Ok(()),
        }
    }

    fn alive(&self, key: &str, now: i64) -> bool {
        self.data
            .lock()
            .get(key)
            .is_some_and(|(_, exp)| exp.is_none_or(|e| e > now))
    }
}

#[async_trait]
impl Storage for MemoryStorage {
    async fn get(&self, key: &str) -> StorageResult<Option<Value>> {
        self.gate().await?;
        let now = self.now();
        if !self.alive(key, now) {
            return Ok(None);
        }
        Ok(self.data.lock().get(key).map(|(v, _)| v.clone()))
    }

    async fn put(&self, key: &str, value: Value, expires_at: Option<i64>) -> StorageResult<()> {
        self.gate().await?;
        self.data
            .lock()
            .insert(key.to_string(), (value, expires_at));
        Ok(())
    }

    async fn delete(&self, key: &str) -> StorageResult<()> {
        self.gate().await?;
        self.data.lock().remove(key);
        Ok(())
    }

    async fn list(&self, prefix: &str) -> StorageResult<Vec<String>> {
        self.gate().await?;
        let now = self.now();
        let keys: Vec<String> = self
            .data
            .lock()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        Ok(keys.into_iter().filter(|k| self.alive(k, now)).collect())
    }
}

/// 四个桶**分开数**的计数装饰器。
#[derive(Default, Debug)]
pub struct Counts {
    pub get: AtomicU64,
    pub put: AtomicU64,
    pub delete: AtomicU64,
    pub list: AtomicU64,
}

impl Counts {
    /// 四个桶的快照，顺序固定 `(get, put, delete, list)`。
    pub fn snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.get.load(Ordering::Relaxed),
            self.put.load(Ordering::Relaxed),
            self.delete.load(Ordering::Relaxed),
            self.list.load(Ordering::Relaxed),
        )
    }

    /// 四个桶全为 0。**这是「零存储 I/O」那一族判据的唯一正确写法** ——
    /// 只断言 `get == 0` 的版本漏掉的恰恰是写那一侧。
    pub fn is_silent(&self) -> bool {
        self.snapshot() == (0, 0, 0, 0)
    }
}

/// 包一层、把四种操作各记一笔的存储。
pub struct CountingStorage {
    inner: Arc<dyn Storage>,
    pub counts: Arc<Counts>,
}

impl CountingStorage {
    pub fn new(inner: Arc<dyn Storage>) -> Self {
        Self {
            inner,
            counts: Arc::new(Counts::default()),
        }
    }
}

#[async_trait]
impl Storage for CountingStorage {
    async fn get(&self, key: &str) -> StorageResult<Option<Value>> {
        self.counts.get.fetch_add(1, Ordering::Relaxed);
        self.inner.get(key).await
    }

    async fn put(&self, key: &str, value: Value, expires_at: Option<i64>) -> StorageResult<()> {
        self.counts.put.fetch_add(1, Ordering::Relaxed);
        self.inner.put(key, value, expires_at).await
    }

    async fn delete(&self, key: &str) -> StorageResult<()> {
        self.counts.delete.fetch_add(1, Ordering::Relaxed);
        self.inner.delete(key).await
    }

    async fn list(&self, prefix: &str) -> StorageResult<Vec<String>> {
        self.counts.list.fetch_add(1, Ordering::Relaxed);
        self.inner.list(prefix).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 🔴 `Storage` 端口契约，**对每种实现各跑一遍**。
    ///
    /// 这个函数就是那份契约。P1 的「每种实现」＝ `FileStorage` + `MemoryStorage`。
    /// `FileStorage` 那一遍在它自己的模块里（那边还要检落盘文件的字节），
    /// 这里跑内存实现这一遍。
    async fn contract(s: &dyn Storage, now: i64) {
        assert_eq!(s.get("k").await.unwrap(), None);
        s.put("k", json!({ "a": 1 }), None).await.unwrap();
        assert_eq!(s.get("k").await.unwrap(), Some(json!({ "a": 1 })));

        s.put("p:1", json!(1), None).await.unwrap();
        s.put("p:2", json!(2), Some(now + 60_000)).await.unwrap();
        s.put("p:3", json!(3), Some(now - 1)).await.unwrap();

        let mut keys = s.list("p:").await.unwrap();
        keys.sort();
        assert_eq!(
            keys,
            vec!["p:1".to_string(), "p:2".to_string()],
            "过期键不许出现在 list 里"
        );
        assert_eq!(s.get("p:3").await.unwrap(), None, "过期键 get 也必须看不见");

        s.delete("k").await.unwrap();
        assert_eq!(s.get("k").await.unwrap(), None);
        s.delete("k").await.unwrap();
    }

    #[tokio::test]
    async fn memory_storage_satisfies_the_port_contract() {
        let s = MemoryStorage::new();
        s.set_clock(1_700_000_000_000);
        contract(&s, 1_700_000_000_000).await;
    }

    /// 假时钟**能往前走** —— 冻结的时钟测不出 TTL 过期
    /// （04 §3.3 第 ④ 条假阳性：冻结时钟下 TTL 类变异的结果是「挂起」而非失败）。
    #[tokio::test]
    async fn fake_clock_advances_so_ttl_expiry_is_observable() {
        let s = MemoryStorage::new();
        s.set_clock(1_000);
        s.put("k", json!(1), Some(2_000)).await.unwrap();
        assert_eq!(s.get("k").await.unwrap(), Some(json!(1)), "还没到期");
        s.set_clock(2_001);
        assert_eq!(s.get("k").await.unwrap(), None, "到期之后必须看不见");
    }

    /// 🔴 计数桩**四个桶全数**。
    ///
    /// 回归对象：只数 `get` 的版本让「写侧多了一次落盘」这一族判据全绿。
    #[tokio::test]
    async fn counting_storage_counts_all_four_buckets() {
        let c = CountingStorage::new(Arc::new(MemoryStorage::new()));
        c.get("a").await.unwrap();
        c.put("a", json!(1), None).await.unwrap();
        c.put("b", json!(1), None).await.unwrap();
        c.delete("a").await.unwrap();
        c.list("").await.unwrap();
        assert_eq!(c.counts.snapshot(), (1, 2, 1, 1));
        assert!(!c.counts.is_silent());
    }

    /// 一个全新的计数器是「静默」的 —— `is_silent` 的反向控制。
    #[test]
    fn a_fresh_counter_is_silent() {
        assert!(Counts::default().is_silent());
    }

    /// 注入的失败**只发生一次**，之后恢复正常。
    #[tokio::test]
    async fn injected_failure_happens_exactly_once() {
        let s = MemoryStorage::new();
        s.fail_next("boom");
        assert!(s.get("k").await.is_err());
        assert!(s.get("k").await.is_ok(), "只失败一次，否则测不出自愈");
    }
}
