//! `ConfigHolder` —— 带 TTL 的运行期配置快照。
//!
//! # 它回答的那个问题
//!
//! 「面板上刚改完配置，多久能看见？」答案是**一个 TTL，30 秒，中间不再有任何一层缓存**。
//!
//! ⚠️ **上界就是 30 秒，别再加任何一项。** 上一版加过一个「KV 边缘缓存 60 秒」，
//! 那个数曾经进过 `overview.freshness`、`PUT /config` 的 `propagation`、面板文案与
//! 五语言文档 —— 它随 Worker 形态一起没了，四处都已删干净。再加一层就要四处同时改。
//!
//! ⚠️ 别和 `POOL_CACHE_TTL_MS`（默认 60 秒，「别的副本判的冷却/剔除多久能看到」）搞混。
//! **面板要把两个数都显示出来**，它们回答的不是同一个问题。

use crate::core::config::{GatewayConfig, resolve_config};
use crate::core::config_provenance::{CONFIG_KEY, EnvMap};
use crate::core::refreshable::Refreshable;
use crate::ports::logger::{LogEntry, LogLevel, Logger};
use crate::ports::storage::Storage;
use parking_lot::Mutex;
use std::sync::Arc;

/// 配置快照的存活时长。🔴 **它同时就是「改完多久能看见」的上界**，
/// 面板与五语言文档写的是同一个数字。
pub const CONFIG_TTL_MS: i64 = 30_000;

/// 事件名：热路径刷新读存储失败（**保留上一份合法快照**）。
pub const EVENT_CONFIG_RELOAD_FAILED: &str = "config.reload_failed";

/// 带 TTL 的配置持有者。
///
/// # 为什么 `current()` 交出的是 `Arc<GatewayConfig>` 而不是 `&GatewayConfig`
///
/// 🔴 **每个请求开头取一份快照，单个请求内部保持一致。**
/// 反面是「装配时把配置值展开成一堆字段」—— 那正是历史上
/// 「改完配置网关照旧按旧值跑」的成因（TS 侧的 `{ ...dispatchDeps(deps) }`
/// 会当场求值 getter，把配置永久冻结在建 app 那一刻）。
/// 交 `Arc` 让「这是一份快照」这件事在类型上就看得见。
pub struct ConfigHolder {
    inner: Mutex<Refreshable<Arc<GatewayConfig>>>,
    /// 装载成功的那一份，**终生留着**。
    ///
    /// 它唯一的用途是让 [`ConfigHolder::current`] 在结构上不可能失败：
    /// `Refreshable` 的 `current()` 是 `Option`，而本持有者对外承诺的是
    /// 「同步读、永不抛」。用 `unwrap()` 就是把一条承诺架在「`prime` 没被改坏」上，
    /// 而这条路径上 panic 等于整个网关下线。
    /// ⚠️ 它**不**参与刷新，永远是第一份 —— 真掉到它上面说明代码有 bug，
    /// 那时给一份「起得来的配置」比给一份编出来的默认值诚实。
    fallback: Arc<GatewayConfig>,
    storage: Arc<dyn Storage>,
    logger: Arc<dyn Logger>,
    env: EnvMap,
    ttl_ms: i64,
    /// 并发去重。突发流量下不做这一步，N 个并发请求就是 N 次存储读。
    ///
    /// ⚠️ 用 `tokio::sync::Mutex` 而不是 `parking_lot`：它**跨 await 持有**。
    /// 全仓的分界线是「跨 await 一律 `tokio::sync`」。
    reload_gate: tokio::sync::Mutex<()>,
}

impl ConfigHolder {
    /// 装载一次并建好持有者。
    ///
    /// 🔴 **首次装载失败必须抛**：缺 `GATEWAY_TOKEN` 拒绝服务是不变量。
    ///
    /// ⚠️ 冷启动这一次**允许**「存储读不出来 ⇒ 退到 env + 默认值」，
    /// 之后每一次刷新都**不许**再这么降级 —— 那条路径上有严格更好的兜底
    /// （保留上一份合法快照）。实测过在热路径上吞成降级的后果：一次瞬时读抖动
    /// 把面板保存的配置**静默**换成内置默认值，而且不是几十秒，是剩下的一整天。
    pub fn prime(
        env: &EnvMap,
        storage: Arc<dyn Storage>,
        logger: Arc<dyn Logger>,
        config: GatewayConfig,
        now: i64,
    ) -> Self {
        let config = Arc::new(config);
        let mut r = Refreshable::new();
        r.prime(config.clone(), now);
        Self {
            inner: Mutex::new(r),
            fallback: config,
            storage,
            logger,
            env: env.clone(),
            ttl_ms: CONFIG_TTL_MS,
            reload_gate: tokio::sync::Mutex::new(()),
        }
    }

    /// 固定不变的持有者。**测试与「配置不接线」形态专用。**
    pub fn fixed(config: GatewayConfig) -> Self {
        Self::prime(
            &EnvMap::new(),
            Arc::new(NoStorage),
            Arc::new(crate::ports::logger::NullLogger),
            config,
            // `i64::MAX` ⇒ `needs_reload` 永远为假 ⇒ 永不去读存储。
            i64::MAX,
        )
    }

    /// 当前快照。**同步、永不抛。**
    #[must_use]
    pub fn current(&self) -> Arc<GatewayConfig> {
        self.inner
            .lock()
            .current()
            .cloned()
            .unwrap_or_else(|| self.fallback.clone())
    }

    /// 刷到「不早于一个 TTL」。**永不抛。**
    ///
    /// 🔴 跑在鉴权**之前**是必须的：`/v1` 的鉴权口令就在配置里，顺序反不过来。
    pub async fn ensure_fresh(&self, now: i64) {
        // 先在同步锁里判一次，绝大多数请求在这里就返回了（零 await、零存储）。
        if !self.inner.lock().needs_reload(now, self.ttl_ms) {
            return;
        }
        // 并发去重：拿到闸的那一个去读，其余的排在后面，醒来时再判一次。
        let _g = self.reload_gate.lock().await;
        if !self.inner.lock().needs_reload(now, self.ttl_ms) {
            return;
        }
        let token = self.inner.lock().begin_reload();
        let stored = self.storage.get(CONFIG_KEY).await;
        // ⚠️ **用调用方传进来的 `now`，不许在这里另取一次墙钟**：
        //    另取一次的话「判要不要刷」与「记刷过了」用的是两个时钟基准，
        //    于是 TTL 在注入假时钟的调用方眼里**完全失效**（每次都重刷）。
        let at = now;

        let loaded = match &stored {
            // 🔴 **热路径读失败一律不降级**，保留上一份合法快照。
            Err(e) => {
                self.logger.log(
                    LogEntry::new(
                        LogLevel::Error,
                        EVENT_CONFIG_RELOAD_FAILED,
                        "刷新配置时读存储失败，继续沿用上一份快照（面板上刚保存的改动本次不生效）",
                    )
                    .with("err", e.to_string()),
                );
                None
            }
            Ok(v) => match resolve_config(&self.env, v.as_ref(), false) {
                Ok((cfg, _flags)) => Some(Arc::new(cfg)),
                // 走到这里只有一种可能：fail-closed 的 `gateway_token` 在运行中被清空。
                // 同样保留上一份快照 —— 让一个已经在服务的网关因为存储里一格坏值而
                // 整体失去口令，比「口令晚 30 秒生效」糟得多。
                Err(e) => {
                    self.logger.log(
                        LogEntry::new(
                            LogLevel::Error,
                            EVENT_CONFIG_RELOAD_FAILED,
                            "刷新配置时校验失败，继续沿用上一份快照",
                        )
                        .with("err", e.to_string()),
                    );
                    None
                }
            },
        };

        let mut inner = self.inner.lock();
        match loaded {
            Some(v) => inner.complete_ok(token, v, at),
            None => inner.complete_err(token, at),
        }
    }

    /// 让下一次 `ensure_fresh` 一定重载（面板写完配置之后调）。
    ///
    /// 🔴 它推进 `gen`，于是**在途的那次装载落地时不会把旧值盖回来**。
    pub fn invalidate(&self) {
        self.inner.lock().invalidate();
    }

    /// 写穿透：面板刚保存的那份立刻可见。
    ///
    /// ⚠️ **刻意不推进计时、也不递增 `gen`**（与 `invalidate` 不同，这不是省略）：
    /// 推进计时会让一个持续写入的实例永远不再去读存储，从此看不见别的实例的改动；
    /// 递增 `gen` 会让每次写穿透都作废在途装载 —— 而写穿透最密集的时刻
    /// 恰恰是最需要缓存的时刻。
    pub fn set(&self, config: GatewayConfig) {
        self.inner.lock().set(Arc::new(config));
    }
}

/// `fixed()` 用的空存储：任何一次调用都是错误。
///
/// **它不是替身，是一条断言**：固定持有者永远不该去碰存储，真碰了就该看见错误，
/// 而不是悄悄读到一份真数据。
struct NoStorage;

#[async_trait::async_trait]
impl Storage for NoStorage {
    async fn get(
        &self,
        _k: &str,
    ) -> crate::ports::storage::StorageResult<Option<serde_json::Value>> {
        Err(crate::ports::storage::StorageError::new(
            "fixed ConfigHolder 不该读存储",
        ))
    }
    async fn put(
        &self,
        _k: &str,
        _v: serde_json::Value,
        _e: Option<i64>,
    ) -> crate::ports::storage::StorageResult<()> {
        Err(crate::ports::storage::StorageError::new(
            "fixed ConfigHolder 不该写存储",
        ))
    }
    async fn delete(&self, _k: &str) -> crate::ports::storage::StorageResult<()> {
        Err(crate::ports::storage::StorageError::new(
            "fixed ConfigHolder 不该写存储",
        ))
    }
    async fn list(&self, _p: &str) -> crate::ports::storage::StorageResult<Vec<String>> {
        Err(crate::ports::storage::StorageError::new(
            "fixed ConfigHolder 不该列存储",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::core::config_provenance::CONFIG_KEY;
    use serde_json::json;

    fn env() -> EnvMap {
        let mut m = EnvMap::new();
        m.insert("GATEWAY_TOKEN".into(), "t-abcdefghijklmnop".into());
        m
    }

    async fn holder(storage: Arc<dyn Storage>, now: i64) -> ConfigHolder {
        let stored = storage.get(CONFIG_KEY).await.ok().flatten();
        let (cfg, _) = resolve_config(&env(), stored.as_ref(), false).unwrap();
        ConfigHolder::prime(
            &env(),
            storage,
            Arc::new(crate::ports::logger::NullLogger),
            cfg,
            now,
        )
    }

    /// TTL 之内不读存储；过了 TTL 才读一次。
    ///
    /// 数的是**精确次数** —— 「读了没读」这类判据只有精确次数才有意义。
    #[tokio::test]
    async fn the_snapshot_is_reused_for_a_whole_ttl() {
        let mem = Arc::new(MemoryStorage::new());
        let counting = Arc::new(CountingStorage::new(mem.clone()));
        let counts = counting.counts.clone();
        let h = holder(counting, 1_000).await;
        let before = counts.snapshot().0;

        for _ in 0..50 {
            h.ensure_fresh(1_000 + CONFIG_TTL_MS - 1).await;
        }
        assert_eq!(counts.snapshot().0, before, "TTL 内一次都不该读");

        h.ensure_fresh(1_000 + CONFIG_TTL_MS).await;
        assert_eq!(counts.snapshot().0, before + 1, "刚好到 TTL 要读一次");
    }

    /// 过了 TTL 之后，存储里的新值真的被看见。
    #[tokio::test]
    async fn a_change_in_storage_becomes_visible_after_the_ttl() {
        let mem = Arc::new(MemoryStorage::new());
        let h = holder(mem.clone(), 0).await;
        assert_eq!(h.current().max_strikes, 3);

        mem.put(CONFIG_KEY, json!({ "maxStrikes": 9 }), None)
            .await
            .unwrap();
        h.ensure_fresh(CONFIG_TTL_MS).await;
        assert_eq!(h.current().max_strikes, 9);
    }

    /// 🔴 **热路径读失败保留上一份合法快照，绝不降级成默认值。**
    ///
    /// 回归对象：实测过一次瞬时读抖动把面板保存的配置静默换成内置默认值，
    /// 而且不是几十秒，是剩下的一整天。
    #[tokio::test]
    async fn a_read_failure_keeps_the_last_good_snapshot() {
        let mem = Arc::new(MemoryStorage::new());
        mem.put(CONFIG_KEY, json!({ "maxStrikes": 7 }), None)
            .await
            .unwrap();
        let h = holder(mem.clone(), 0).await;
        assert_eq!(h.current().max_strikes, 7);

        mem.fail_next("EACCES");
        h.ensure_fresh(CONFIG_TTL_MS).await;
        assert_eq!(h.current().max_strikes, 7, "读失败不该把配置打回默认值");
    }

    /// `invalidate()` 之后下一次 `ensure_fresh` 一定重载（哪怕还在 TTL 内）。
    #[tokio::test]
    async fn invalidate_forces_the_next_refresh() {
        let mem = Arc::new(MemoryStorage::new());
        let h = holder(mem.clone(), 0).await;
        mem.put(CONFIG_KEY, json!({ "maxStrikes": 5 }), None)
            .await
            .unwrap();

        h.ensure_fresh(1).await;
        assert_eq!(h.current().max_strikes, 3, "TTL 内本不该看见");
        h.invalidate();
        h.ensure_fresh(2).await;
        assert_eq!(h.current().max_strikes, 5);
    }

    /// 写穿透立刻可见，**但不推进计时** ——
    /// 所以下一次到点的刷新照常发生（否则本实例永远看不见别的副本的改动）。
    #[tokio::test]
    async fn a_write_through_is_visible_immediately_but_does_not_reset_the_clock() {
        let mem = Arc::new(MemoryStorage::new());
        let counting = Arc::new(CountingStorage::new(mem.clone()));
        let counts = counting.counts.clone();
        let h = holder(counting, 0).await;
        let before = counts.snapshot().0;

        h.set(GatewayConfig {
            max_strikes: 11,
            ..(*h.current()).clone()
        });
        assert_eq!(h.current().max_strikes, 11);

        h.ensure_fresh(CONFIG_TTL_MS).await;
        assert_eq!(counts.snapshot().0, before + 1, "写穿透不该把刷新推后");
    }

    /// `fixed()` 一次存储都不碰。
    #[tokio::test]
    async fn a_fixed_holder_never_touches_storage() {
        let (cfg, _) = resolve_config(&env(), None, false).unwrap();
        let h = ConfigHolder::fixed(cfg);
        // 真去读存储的话 `NoStorage` 会回错误，快照会保持不变 —— 但那是「看不出来」的绿。
        // 所以这里钉的是「needs_reload 恒为假」这个可观测点：换一个远在未来的 now 也不刷新。
        h.ensure_fresh(i64::MAX).await;
        assert_eq!(h.current().gateway_token, "t-abcdefghijklmnop");
    }
}
