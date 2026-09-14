//! `ApiKeyHolder` —— 带 TTL 的**子密钥表**快照。鉴权第②段的唯一数据源。
//!
//! # 🔴 这个模块存在的全部意义是一条事故判据
//!
//! `admin-apikeys-quota`（8 格，🔴 真实事故）钉的是：
//! **拿错口令打业务口时，1000 次错口令只许产生 1 次存储读。**
//!
//! ⇒ 「子密钥表不存在」与「读不出来」**也必须当成一份合法快照按 TTL 缓存**。
//! 不缓存否定结果的实现会让「一条零凭据、零限速的公开路径」变成一根撬动存储的杠杆。
//!
//! # 结构性的零
//!
//! ⚠️ 它**刻意不挂在任何 `use("*")` / 全局 layer 上**：刷新只发生在鉴权第②段
//! （presented ≠ 主口令）之后。于是「全部客户端都用主口令」的部署**每天 0 次读**
//! —— 因为那条路径上根本没有调用点，**不是因为某个 `if`**。

use crate::core::keys::api_keys::ApiKeyTableRead;
use crate::core::narrow::narrow_api_key_table;
use crate::core::refreshable::Refreshable;
use crate::ports::logger::{LogEntry, LogLevel, Logger};
use crate::ports::storage::Storage;
use parking_lot::Mutex;
use std::sync::Arc;

/// 子密钥表的存储键。**字面量，与 TS 版逐字相同。**
pub const APIKEYS_KEY: &str = "apikeys";

/// 默认缓存 TTL：5 分钟。
///
/// 🔴 **它同时就是一条安全承诺的上界**：「面板上停用一把密钥，它最多还能再用约 5 分钟」。
/// 面板 / ADMIN.md / DEPLOY.md **三处都要写这个具体数字**，不许写「稍后生效」。
pub const APIKEY_CACHE_TTL_MS: i64 = 300_000;

/// 读失败降级（**只在还没有任何合法快照可退时**）。
pub const EVENT_APIKEYS_UNREADABLE: &str = "apikeys.unreadable";
/// 读到了、但结构不认。🔴 **绝不静默当空表。**
pub const EVENT_APIKEYS_INVALID: &str = "apikeys.invalid";
/// 已有快照时的刷新失败（沿用旧快照）。
pub const EVENT_APIKEYS_RELOAD_FAILED: &str = "apikeys.reload_failed";

pub struct ApiKeyHolder {
    inner: Mutex<Refreshable<ApiKeyTableRead>>,
    storage: Arc<dyn Storage>,
    logger: Arc<dyn Logger>,
    ttl_ms: i64,
    /// 并发去重。跨 await 持有 ⇒ `tokio::sync`。
    reload_gate: tokio::sync::Mutex<()>,
}

impl ApiKeyHolder {
    /// 建一个持有者。**装配期不读存储**（第 11 步与用量 sink 刻意不同：它无条件建，
    /// 但在「没签发过子密钥」的部署上一次存储调用都不产生）。
    #[must_use]
    pub fn new(storage: Arc<dyn Storage>, logger: Arc<dyn Logger>, ttl_ms: i64) -> Self {
        Self {
            inner: Mutex::new(Refreshable::new()),
            storage,
            logger,
            ttl_ms,
            reload_gate: tokio::sync::Mutex::new(()),
        }
    }

    /// 当前快照。从未装载过时是 [`ApiKeyTableRead::Absent`]
    /// —— 与「还没签发过任何密钥」同义，这是安全的那一侧（谁都过不了第②段）。
    #[must_use]
    pub fn current(&self) -> ApiKeyTableRead {
        self.inner
            .lock()
            .current()
            .cloned()
            .unwrap_or(ApiKeyTableRead::Absent)
    }

    /// 刷到不早于一个 TTL。**永不抛。**
    ///
    /// 🔴 冷启动那一档**必须降级成一份合法快照而不是抛**：抛出去的话
    /// `Refreshable` 在 `ever_loaded == false` 时刻意不推进计时 ⇒
    /// **每个错口令请求一次存储读**，而那正是「零凭据零限速的路径变成撬动读配额的杠杆」。
    /// 降级成 `Absent` 之后它是一份合法快照，照样吃满一个 TTL。
    pub async fn ensure_fresh(&self, now: i64) {
        if !self.inner.lock().needs_reload(now, self.ttl_ms) {
            return;
        }
        let _g = self.reload_gate.lock().await;
        if !self.inner.lock().needs_reload(now, self.ttl_ms) {
            return;
        }
        let token = self.inner.lock().begin_reload();
        let raw = self.storage.get(APIKEYS_KEY).await;
        // ⚠️ 同 `ConfigHolder`：**用调用方的 `now`**，别另取一次墙钟。
        let at = now;
        let ever_loaded = self.inner.lock().current().is_some();

        let next = match raw {
            Ok(v) => {
                let table = narrow_api_key_table(v.as_ref());
                if table == ApiKeyTableRead::Invalid {
                    self.logger.log(LogEntry::new(
                        LogLevel::Error,
                        EVENT_APIKEYS_INVALID,
                        "存储里的 apikeys 结构不认；本次一把子密钥都不认（原字节没被动过，写路径会拒绝覆盖它）",
                    ));
                }
                Some(table)
            }
            Err(e) => {
                // 已经有一份合法快照 ⇒ 沿用它，只记一条刷新失败。
                if ever_loaded {
                    self.logger.log(
                        LogEntry::new(
                            LogLevel::Error,
                            EVENT_APIKEYS_RELOAD_FAILED,
                            "刷新子密钥表失败，继续沿用上一份快照",
                        )
                        .with("err", e.to_string()),
                    );
                    None
                } else {
                    // 🔴 冷启动：降级成一份**合法**快照（等价于「还没签发过」），
                    //    这样它照样吃满一个 TTL。见本函数文档。
                    self.logger.log(
                        LogEntry::new(
                            LogLevel::Error,
                            EVENT_APIKEYS_UNREADABLE,
                            "读不到子密钥表，本次按「还没签发过任何子密钥」处理（主口令不受影响）",
                        )
                        .with("err", e.to_string()),
                    );
                    Some(ApiKeyTableRead::Absent)
                }
            }
        };

        let mut inner = self.inner.lock();
        match next {
            Some(v) => inner.complete_ok(token, v, at),
            None => inner.complete_err(token, at),
        }
    }

    /// 生效的缓存 TTL（毫秒）。
    ///
    /// **消费者**：`GET /admin/api/apikeys` 的 `cacheTtlMs` 与
    /// `GET /admin/api/capabilities` 的 `apiKeys.cacheTtlMs`。
    /// 🔴 **面板据它算「停用之后最多还能用多久」，不许在前端写死**
    /// —— 写死就会在运维调过 `APIKEY_CACHE_TTL_MS` 之后变成一句假话。
    /// ⚠️ 它是 env-only、不进配置面，所以**除了这里没有第二条路能问到它**。
    #[must_use]
    pub fn ttl_ms(&self) -> i64 {
        self.ttl_ms
    }

    /// 面板写完子密钥之后调：下一次鉴权一定重载。
    pub fn invalidate(&self) {
        self.inner.lock().invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::ports::logger::NullLogger;
    use serde_json::json;

    fn holder(storage: Arc<dyn Storage>) -> ApiKeyHolder {
        ApiKeyHolder::new(storage, Arc::new(NullLogger), APIKEY_CACHE_TTL_MS)
    }

    /// 🔴🔴 **事故判据：1000 次错口令 = 恰好 1 次存储读。**
    ///
    /// 回归对象：子密钥表**不存在**也必须当成一份合法快照按 TTL 缓存。
    /// 不缓存否定结果时，一条零凭据、零限速的公开路径就是一根撬动存储的杠杆。
    #[tokio::test]
    async fn a_thousand_bad_tokens_cost_exactly_one_storage_read() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = holder(counting);
        for i in 0..1000 {
            h.ensure_fresh(i).await;
        }
        let (get, put, delete, list) = counts.snapshot();
        assert_eq!(get, 1, "1000 次错口令只许读 1 次");
        assert_eq!((put, delete, list), (0, 0, 0), "鉴权路径一次写都不许有");
    }

    /// 读**失败**同样被缓存（与「不存在」同一条道理）。
    #[tokio::test]
    async fn an_unreadable_table_is_also_cached_for_a_whole_ttl() {
        let mem = Arc::new(MemoryStorage::new());
        mem.fail_next("EACCES");
        let counting = Arc::new(CountingStorage::new(mem));
        let counts = counting.counts.clone();
        let h = holder(counting);
        for i in 0..500 {
            h.ensure_fresh(i).await;
        }
        assert_eq!(counts.snapshot().0, 1, "读失败也必须吃满一个 TTL");
        assert_eq!(h.current(), ApiKeyTableRead::Absent);
    }

    /// 🔴 **结构不认 ≠ 空表**，两者在类型上就是两档。
    ///
    /// 回归对象：当空表的后果是全部客户 401，而**下一次面板写会把幸存记录整份覆掉，不可逆**。
    #[tokio::test]
    async fn a_malformed_table_is_never_treated_as_empty() {
        let mem = Arc::new(MemoryStorage::new());
        mem.put(APIKEYS_KEY, json!("这不是一张表"), None)
            .await
            .unwrap();
        let h = holder(mem);
        h.ensure_fresh(0).await;
        assert_eq!(h.current(), ApiKeyTableRead::Invalid);
    }

    /// 过了 TTL 才重新读；新签发的密钥随之可见。
    #[tokio::test]
    async fn a_newly_issued_key_becomes_visible_after_the_ttl() {
        let mem = Arc::new(MemoryStorage::new());
        let h = holder(mem.clone());
        h.ensure_fresh(0).await;
        assert_eq!(h.current(), ApiKeyTableRead::Absent);

        mem.put(APIKEYS_KEY, json!({ "version": 1, "keys": [] }), None)
            .await
            .unwrap();
        h.ensure_fresh(APIKEY_CACHE_TTL_MS - 1).await;
        assert_eq!(h.current(), ApiKeyTableRead::Absent, "TTL 内不该看见");
        h.ensure_fresh(APIKEY_CACHE_TTL_MS).await;
        assert!(matches!(h.current(), ApiKeyTableRead::Ok(_)));
    }

    /// `invalidate()` 之后下一次鉴权一定重载 ——
    /// 面板停用一把密钥之后不必等满 5 分钟。
    #[tokio::test]
    async fn invalidate_forces_a_reload() {
        let mem = Arc::new(MemoryStorage::new());
        let counting = Arc::new(CountingStorage::new(mem));
        let counts = counting.counts.clone();
        let h = holder(counting);
        h.ensure_fresh(0).await;
        h.invalidate();
        h.ensure_fresh(1).await;
        assert_eq!(counts.snapshot().0, 2);
    }

    /// TTL = 0 是合法的逃生口（关缓存，每次都读）。
    #[tokio::test]
    async fn a_zero_ttl_disables_the_cache() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = ApiKeyHolder::new(counting, Arc::new(NullLogger), 0);
        h.ensure_fresh(0).await;
        h.ensure_fresh(0).await;
        assert_eq!(counts.snapshot().0, 2, "关了缓存就该每次都读");
    }
}
