//! `KeyPoolRepo` —— key 池的**存储形态**：索引、进程级快照、写消除、损坏恢复。
//!
//! # 一条原则贯穿全文件
//!
//! > **记录（`key:<id>`）是真源，索引（`pool:index`）是派生缓存。**
//!
//! 两条写序都从这一句推出来，**方向相反，都不许颠倒**：
//!
//! | 操作 | 写序 | 中途失败留下什么 |
//! |---|---|---|
//! | `add` | ① 写记录 → ② 进索引 | **孤儿记录**（不被使用，对账能收养） |
//! | `delete` | ① 出索引 → ② 删记录 | **幽灵索引项**（读路径 filter 掉） |
//!
//! 颠倒 `delete` ⇒ 中途失败留下孤儿记录，而对账以 `list()` 为准会把它**加回索引**
//! —— 等于把一次删除悄悄撤销了，**这是正确性问题**。
//! 颠倒 `add` ⇒ 中途失败时 key 材料压根没落盘，而调用方此刻已经在上游真实建号并领到
//! token —— **丢了就再也找不回来，对账也修不了**（对账只认存储里已有的记录）。
//!
//! # 索引存在的唯一理由
//!
//! 把 `list()` 从转发热路径上摘掉。`FileStorage::list()` 是一次整份 `store.json` 的
//! 读 + 反序列化，挂在每个转发请求上，池子一大就是逐请求的全量反序列化。
//!
//! ⚠️ **不退回「整池存成单条 JSON」**：那样确实只要 1 次 get，
//! 但一 key 一记录正是为了避免整池的 last-write-wins
//! （两个并发请求各改一把 key，后写的覆盖先写的整份池子）。

use crate::core::keys::api_keys::constant_time_equal;
use crate::core::narrow::narrow_key_records;
use crate::core::pool_index::{
    KEY_PREFIX, POOL_INDEX_KEY, ids_from_key_names, make_pool_index, same_id_set,
};
use crate::core::refreshable::Refreshable;
use crate::core::stats::{
    StatsDelta, add_delta, apply_delta, is_zero_delta, max_stats, stats_delta, stats_or_empty,
};
use crate::core::types::{KeyRecord, KeyStats};
use crate::ports::logger::{LogEntry, LogLevel, Logger};
use crate::ports::storage::{Storage, StorageError, StorageResult};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

// ── 五个退避 / 上界常量（改任何一个先回 01 §4.4 读理由）────────────────────

/// 索引重建写失败之后的静默窗口。
///
/// **必须显著大于 `pool_cache_ttl_ms`，否则这条退避是一条死条件**：
/// 曾经两者都是 60_000 而判据是 `since < RETRY` ⇒ 读路径每个 TTL 回落一次，
/// 两次尝试恰好间隔 60_000，`since == 60_000` 不小于 60_000 ⇒ **一次都不被抑制**
/// （实测跨 10 个 TTL 有 10 次 put 尝试）。取 5 倍，留出「TTL 被调小一档」的余量。
pub const INDEX_WRITE_RETRY_MS: i64 = 300_000;

/// 读路径两次 `list()` 的最小间隔。**空池兜底与索引缺失回落共用这一个窗口**
/// —— 两扇门都通向同一个 `list` 桶，只堵一扇等于没堵。
///
/// 🔴 **代价必须写进部署文档**：池子为空、或索引坏掉时，手工导入的 key 不再是
/// 「下一个请求即生效」，而是最多等 `READ_PATH_LIST_BACKOFF_MS + 一个 pool_cache_ttl_ms`。
/// 想立刻生效就同时更新 `pool:index`。
pub const READ_PATH_LIST_BACKOFF_MS: i64 = 600_000;

/// 头几次连续失败用的快窗口。
///
/// 只有一个长窗口时，**一次**瞬时 list 抖动会把那份异常粘住整整 10 分钟：
/// 期间一个**真的是空的**池子会报 500 而不是 503 `pool_empty` —— 把运维指向完全相反的方向。
pub const LIST_FAIL_FAST_RETRY_MS: i64 = 60_000;

/// 连续失败几次之后升到长退避。
///
/// ⚠️ 判据是 `failures > 0 && failures < 3`，门槛看的是**已经失败的次数**，
/// 不是尝试的序号 ⇒ **一共只有 2 个快窗口、累计 2 分钟**，不是 3 个、3 分钟。
pub const LIST_FAIL_ESCALATE_AFTER: u32 = 3;

/// 单把 key 的长度上界。
///
/// **它挡的不是攻击，是一次手滑**（把整个日志文件粘进导入框）：
/// `FileStorage` 每次 put 都重写整个 `store.json`，没有上限时一次粘贴就能把整份存储
/// 撑成几十 MB，此后**每一次** key 池状态回写都要重写它。
pub const MAX_KEY_LENGTH: usize = 1024;

// ── 事件名（运维的 grep 锚点，一个字都不许改）──────────────────────────────

pub const EVENT_POOL_LOAD_FAILED: &str = "pool.load_failed";
pub const EVENT_POOL_INDEX_UNREADABLE: &str = "pool.index_unreadable";
pub const EVENT_POOL_INDEX_BACKFILLED: &str = "pool.index_backfilled";
pub const EVENT_POOL_INDEX_WRITE_FAILED: &str = "pool.index_write_failed";
/// 对账发现索引**整个缺失**（全新部署的常态）⇒ info。
pub const EVENT_POOL_INDEX_BOOTSTRAPPED: &str = "pool.index_bootstrapped";
/// 对账发现索引**在、但与实际记录不符** ⇒ warn。
///
/// ⚠️ 这两条**不许合并成一条**，级别不同是有意的：
/// 「缺失」是全新部署每次都会走一遍的路，「不符」意味着真出过事
/// （写记录成功而写索引失败、或别的副本覆盖了它），运维要能只 grep 后者。
pub const EVENT_POOL_INDEX_REPAIRED: &str = "pool.index_repaired";
pub const EVENT_POOL_STALE_WRITE_DROPPED: &str = "pool.stale_write_dropped";
pub const EVENT_POOL_RECORDS_MALFORMED: &str = "pool.records_malformed";

/// key 的 id = SHA-256 的**前 8 字节**的十六进制（16 字符）。
#[must_use]
pub fn key_id(key: &str) -> String {
    let d = Sha256::digest(key.as_bytes());
    hex::encode(&d[..8])
}

/// 这串东西能不能被导入。
///
/// key 唯一的用途是拼进 `authorization: Bearer <key>`，而请求头的值是 ByteString ——
/// 码点 > U+00FF 与 NUL/CR/LF 在 `HeaderValue::from_str` 上直接失败。
/// 放进池子就是一把**每次被选中都让转发炸掉**的 key，而它看起来完全正常。
///
/// ⚠️ **它不是 `ADMIN_TOKEN` 那条 `SENDABLE` 的第二份实现，两条规则真的不同**：
/// 那一条量词是 `*` 且**刻意放行内部空格**（passphrase 是有价值的口令形态）。
/// 上游 key 是**机器发的 token，不是人记的口令**，中间出现空格只有一种可能 —— 粘贴事故。
///
/// 🔴 **别看到本函数就以为池子里每一把 key 都在门口被拦过**：同一个判据在两条路上
/// 处置不同，而这个不对称是刻意的 —— **导入**拒绝（那串东西还在运维剪贴板里，拒绝是免费的），
/// **铸号**照存不误（拒绝就是销毁凭据：上游账号已真实建出、key 材料只有手上这一份）。
#[must_use]
pub fn is_importable_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY_LENGTH
        && key.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// 写消除白名单。🔴 **判据是白名单，不是 `FIELD_ROLE`。**
///
/// ⚠️⚠️ 这是一次判据翻转，方向很重要，别再翻回去。在此之前判据是黑名单式的
/// （「scheduling 字段都没变 ⇒ 可以消除」），于是 telemetry 那一档里的**每一个**字段
/// 都自动获得了「可以被静默丢弃」的许可，而其中两个根本不该有：
/// - `note`：一次「只改备注」的写会被**整个丢弃**，面板显示保存成功，备注哪儿都不在；
/// - `added_at`：**一条兜底都没有**（`last_used_at` 至少有 `touch_interval_ms` 强制落盘）。
///
/// 白名单两个成员各自的许可证：
/// - `last_used_at` —— 高频、纯展示，且有 `touch_interval_ms` 兜底，**写消除的全部收益来自它**；
/// - `stats` —— 同样高频，且被消除的增量**不会丢**（`pending_stats` 攒着补写）。
///   **它是唯一一个「消除 ≠ 丢弃」的字段。**
pub const MAY_ELIDE_FIELDS: [&str; 2] = ["last_used_at", "stats"];

/// 白名单**之外**的字段逐一比对。全相等 ⇒ 这次改动只动了可消除的字段。
///
/// 🔴 `disabled` 在这里（Scheduling）：把它放进白名单的话，「停用一把 key」这次写
/// 就会被写消除**整个吃掉** —— 面板显示已停用、调度器照常用它。
/// 而且它**只对「`last_used_at` 距今不足 `touch_interval_ms`」的 key 发生**，
/// 也就是**只对正在被使用的那些发生** —— 恰恰是最需要能停下来的那些。
#[must_use]
pub fn only_elidable_changed(a: &KeyRecord, b: &KeyRecord) -> bool {
    a.id == b.id
        && a.key == b.key
        && a.upstream == b.upstream
        && a.cooldown_until == b.cooldown_until
        && a.cooldown_reason == b.cooldown_reason
        && a.strikes == b.strikes
        && a.evicted == b.evicted
        && a.evicted_reason == b.evicted_reason
        && a.disabled == b.disabled
        && a.added_at == b.added_at
        && a.note == b.note
        && a.priority == b.priority
        && a.weight == b.weight
}

/// 攒着的、还没落盘的计数增量。
#[derive(Debug, Clone)]
struct PendingStats {
    /// 🔴 **必须存 base，不能只存 delta。**
    ///
    /// 只存 delta 时，落盘写的是「调用方交上来的 `next.stats` + 攒着的」，
    /// 而这**默认了调用方的视图不落后于存储**。`dispatch` 的 `commit()` 恰恰打破这条：
    /// 它把**未合并的 next** 存回自己的视图，于是同一次请求里第二次提交同一把 key 时，
    /// `prev` 比存储旧、`pending` 又已在第一次落盘时清空 ⇒ 第二次写把已经落盘的累计
    /// **往回退**（实测：21 次成功落盘后被抹成 1）。
    /// 而「同一次请求里连着提交同一把 key」不是假想 —— 池里只剩一把可用时会反复选中它。
    base: KeyStats,
    delta: StatsDelta,
    /// 当前这批增量**最早**那一条的时刻。
    ///
    /// 只靠 `last_used_at` 的间隔判据的话，上游 4xx 直通（既不改调度字段也不改
    /// `last_used_at`）会让 `n - p == 0` 恒成立 ⇒ **一个只被打 4xx 的 key 的计数永远不落盘**。
    since: i64,
}

/// 可变状态。
///
/// ⚠️ **全部塞进一把 `tokio::sync::Mutex`**，而不是逐字段各上各的锁。
/// 理由：这些字段之间有不变量（快照 / 退避时刻 / 失败计数 / pending 基线要一起改），
/// 分开加锁就会出现「读到一半别人改了另一半」的组合，而那种 bug 只在并发下出现。
/// 代价老实写：`all()` 在缓存命中时也要过一次锁。那条路径上**零 await、零 IO**，
/// 争用只是一次原子交换 —— 而它挡住的是一整族不可复现的竞态。
#[derive(Default)]
struct RepoState {
    snapshot: Refreshable<Vec<KeyRecord>>,
    pending_stats: HashMap<String, PendingStats>,
    last_read_list_at: Option<i64>,
    last_read_list_error: Option<StorageError>,
    consecutive_list_failures: u32,
    /// 本实例最后一次 `list` 见过的 id 清单。索引缺失且在退避窗口内时拿它顶一阵。
    last_known_ids: Vec<String>,
    index_write_failed_at: Option<i64>,
}

/// 一次对账的结果。**`added`/`removed` 分开报，不是两边长度之差** ——
/// 「剪掉 1 个幽灵 + 捡回 1 个孤儿」两边一样长，长度差会把最忙的一轮说成没事发生。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileResult {
    pub indexed: usize,
    pub actual: usize,
    /// 索引**缺失**或**与实际不符** —— 两种都算，而且都写了。
    pub repaired: bool,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

pub struct KeyPoolRepo {
    storage: Arc<dyn Storage>,
    logger: Arc<dyn Logger>,
    /// 进程级快照存活时长。**0 = 关闭缓存**（每次 `all()` 真读存储）。
    ///
    /// 🔴 代价不只是「看得晚」：陈旧快照上的任意一次调度写会把整条记录覆写回去，
    /// **抹掉别的副本在这个窗口里刚写下的 `evicted` / `cooldown_until`**
    /// —— 那次判定要重新发生一遍。
    cache_ttl_ms: i64,
    /// `last_used_at` 最多多久落盘一次。**0 = 关闭写消除。**
    touch_interval_ms: i64,
    state: tokio::sync::Mutex<RepoState>,
}

impl KeyPoolRepo {
    /// ⚠️ 两个旋钮都是**建 app 时读一次**的：它们绑定的是部署形态
    /// （活跃副本数 × 池大小），不是逐次可调的策略。
    /// **改它要重启容器，面板文案不许写「立即生效」。**
    #[must_use]
    pub fn new(
        storage: Arc<dyn Storage>,
        logger: Arc<dyn Logger>,
        cache_ttl_ms: i64,
        touch_interval_ms: i64,
    ) -> Self {
        Self {
            storage,
            logger,
            cache_ttl_ms,
            touch_interval_ms,
            state: tokio::sync::Mutex::new(RepoState::default()),
        }
    }

    /// 让下一次 `all()` 一定重新装载。
    pub async fn invalidate(&self) {
        self.state.lock().await.snapshot.invalidate();
    }

    // ── 读路径 ──────────────────────────────────────────────────────────────

    /// 整池。**转发热路径唯一的入口，且这条路径上零 `list()`。**
    pub async fn all(&self, now: i64) -> StorageResult<Vec<KeyRecord>> {
        if self.cache_ttl_ms <= 0 {
            let mut st = self.state.lock().await;
            return self.load_all(&mut st, now).await;
        }
        let mut st = self.state.lock().await;
        if st.snapshot.needs_reload(now, self.cache_ttl_ms) {
            let token = st.snapshot.begin_reload();
            match self.load_all(&mut st, now).await {
                Ok(v) => st.snapshot.complete_ok(token, v, now),
                Err(e) => {
                    st.snapshot.complete_err(token, now);
                    self.logger.log(
                        LogEntry::new(LogLevel::Error, EVENT_POOL_LOAD_FAILED, "装载 key 池失败")
                            .with("err", e.to_string()),
                    );
                }
            }
        }
        // ① 从未成功装载过（冷启动就撞上存储故障）：再走一次真加载，把**真实异常**抛出来。
        //    代价老实说：这条分支下每个请求读两遍存储。接受它的理由是 ——
        //    这段时间网关本来就在返 500、根本没有有效流量，而把「哪个文件、哪个键」
        //    交到运维手上，比省一次注定失败的读重要得多。
        let Some(snapshot) = st.snapshot.current().cloned() else {
            return self.load_all(&mut st, now).await;
        };
        // ② 空池态的 list 兜底失败了：🔴 **绝不许把「存储读不出来」伪装成「池子是空的」**。
        //    沿用 `[]` 的话调用方得到的是 503 `pool_empty` —— 一条把运维指向
        //    「你还没导 key」的错误结论，而真相是存储故障，且此时对账读同一个桶同样在抛，
        //    **文档指定的两条自愈路径都已经死了**。
        if snapshot.is_empty()
            && let Some(e) = &st.last_read_list_error
        {
            return Err(e.clone());
        }
        // ③ **浅拷贝**（上面那次 `cloned()`）：`dispatch` 的 commit 会就地改数组元素，
        //    直接把缓存数组交出去等于让一次请求的中间状态污染进程级缓存。
        Ok(snapshot)
    }

    /// 真装载一次。**一个字都不许精简。**
    async fn load_all(&self, st: &mut RepoState, now: i64) -> StorageResult<Vec<KeyRecord>> {
        let idx = self.read_index().await;
        let indexed = match &idx {
            Some(i) => i.clone(),
            None => self.bootstrap_from_list_throttled(st, now).await?,
        };
        let alive = self.load_records(&indexed).await?;
        // 这一趟没经过空池兜底 ⇒ 上一次兜底失败的记忆就此作废
        // （否则陈年失败会永远把 `all()` 钉在抛异常上）。
        if !alive.is_empty() || idx.is_none() {
            st.last_read_list_error = None;
            return Ok(alive);
        }
        self.rescan_empty_result(st, &indexed, now).await
    }

    /// 读索引。**读取本身抛错也一律当「索引缺失」，返回 `None`。**
    ///
    /// 🔴 这一层不是防御性编程，是补一条真实的防线漏洞：
    /// 「结构脏 ⇒ 当作缺失 ⇒ 重建」架在 **JSON 之上**，而存储层遇到坏字节是**先抛**的，
    /// `parse` 根本没机会执行。后果是每个转发请求 500，**并且被指定为修复者的对账
    /// 读同一个键同样抛** —— 它每 30 分钟徒劳地挂一次，永远修不好，只能人工删键。
    /// 抛了就当缺失 ⇒ 调用方回落 `list()` 并顺手把坏值覆盖掉，**自愈**。
    async fn read_index(&self) -> Option<Vec<String>> {
        match self.storage.get(POOL_INDEX_KEY).await {
            Ok(v) => crate::core::narrow::narrow_pool_index(v.as_ref()).map(|i| i.ids),
            Err(e) => {
                self.logger.log(
                    LogEntry::new(
                        LogLevel::Warn,
                        EVENT_POOL_INDEX_UNREADABLE,
                        "读 pool:index 失败，按「索引缺失」处理并回落 list()（顺手把坏值覆盖掉）",
                    )
                    .with("err", e.to_string()),
                );
                None
            }
        }
    }

    /// 读路径 `list()` 的**唯一入口**。
    ///
    /// `Ok(None)` = 「退避窗口内，这次不 list」。
    async fn list_on_read_path(
        &self,
        st: &mut RepoState,
        now: i64,
    ) -> StorageResult<Option<Vec<String>>> {
        let failures = st.consecutive_list_failures;
        // **默认（0 次连续失败）用长退避** —— 这才是这扇闸真正要防的稳态。
        let backoff = if failures > 0 && failures < LIST_FAIL_ESCALATE_AFTER {
            LIST_FAIL_FAST_RETRY_MS
        } else {
            READ_PATH_LIST_BACKOFF_MS
        };
        if let Some(last) = st.last_read_list_at {
            let since = now - last;
            // `since < 0` = 时钟回拨（NTP）。不挡的话这扇闸会冻结「回拨量 + 一个窗口」那么久。
            if since >= 0 && since < backoff {
                // 窗口内若上一次是失败的，**如实抛**而不是返回 `None`。
                if let Some(e) = &st.last_read_list_error {
                    return Err(e.clone());
                }
                return Ok(None);
            }
        }
        // **窗口在发起尝试之前就打上** —— 否则一次慢 list 期间涌进来的请求会各发一次。
        st.last_read_list_at = Some(now);
        match self.storage.list(KEY_PREFIX).await {
            Ok(names) => {
                st.consecutive_list_failures = 0;
                st.last_read_list_error = None;
                Ok(Some(names))
            }
            Err(e) => {
                st.consecutive_list_failures += 1;
                st.last_read_list_error = Some(e.clone());
                Err(e)
            }
        }
    }

    /// 🔴 空池兜底 —— **不加这一步会出人命的场景**：
    ///
    /// 全新部署上定时轮先在空存储上跑了一次对账，写下**权威的空索引** `{"v":1,"ids":[]}`；
    /// 用户随后按文档手工往 `store.json` 里塞 key。此时索引「合法」，永远不会走缺失回落，
    /// `all()` 恒返回 0 条，**网关一直 503 `pool_empty`**。
    async fn rescan_empty_result(
        &self,
        st: &mut RepoState,
        indexed: &[String],
        now: i64,
    ) -> StorageResult<Vec<KeyRecord>> {
        let Some(names) = self.list_on_read_path(st, now).await? else {
            return Ok(Vec::new());
        };
        let actual = ids_from_key_names(&names);
        st.last_known_ids = actual.clone();
        // ⚠️ **写回只增不减**：只把 list 发现而索引不知道的补进去，**绝不从索引里删**。
        //    剪枝一律交给对账。于是「整池都成了幽灵索引项」时这里不写、不剪，
        //    只是每个退避窗口多一次 list，直到对账。
        let mut merged = indexed.to_vec();
        for id in &actual {
            if !merged.contains(id) {
                merged.push(id.clone());
            }
        }
        if !same_id_set(&merged, indexed) {
            self.write_index_best_effort(
                st,
                &merged,
                now,
                "索引说池子是空的，但 list 找到了记录（多半是手工导入），已把它们补进索引",
            )
            .await;
        }
        self.load_records(&actual).await
    }

    /// 索引缺失时的回落。走**同一道**退避闸。
    ///
    /// 窗口内返回本实例内存里的上一份 id 清单；一条都没有过就返回空 ——
    /// 那时 `all()` 会走到空池兜底，兜底同样在窗口内 ⇒ `[]`，
    /// **语义与「池子是空的」一致，而这正是索引缺失且 list 不可用时唯一诚实的答案**。
    async fn bootstrap_from_list_throttled(
        &self,
        st: &mut RepoState,
        now: i64,
    ) -> StorageResult<Vec<String>> {
        let Some(names) = self.list_on_read_path(st, now).await? else {
            return Ok(st.last_known_ids.clone());
        };
        let ids = ids_from_key_names(&names);
        st.last_known_ids = ids.clone();
        self.write_index_best_effort(st, &ids, now, "索引缺失，已按 list 的结果重建")
            .await;
        Ok(ids)
    }

    /// 按 id 取记录。
    ///
    /// ⚠️ 取不到的（幽灵索引项）被 filter 掉，**刻意不顺手把它从索引里剪掉**：
    /// 剪枝要写索引，而那是**热路径上多出来的一次 put**；
    /// 次要理由是 `add` 的写序天然存在「索引已更新、记录尚未可见」的窗口，
    /// 读路径剪枝会把一把刚铸出来的新 key 打成孤儿。
    async fn load_records(&self, ids: &[String]) -> StorageResult<Vec<KeyRecord>> {
        let mut raws: Vec<Option<Value>> = Vec::with_capacity(ids.len());
        for id in ids {
            raws.push(self.storage.get(&format!("{KEY_PREFIX}{id}")).await?);
        }
        let (records, loss) = narrow_key_records(raws.iter().map(Option::as_ref));
        // 🔴 「一把 key 悄悄消失」不许是一件没有任何现象的事。
        //    ⚠️ 只报 `malformed`：`missing` 是写序中途失败留下的**正常** fail-safe 残留，
        //    混在一起数会让一次正常的删除看起来像一次数据损坏。
        if loss.malformed > 0 {
            self.logger.log(
                LogEntry::new(
                    LogLevel::Error,
                    EVENT_POOL_RECORDS_MALFORMED,
                    "有 key 记录读得到但认不出来，本次装载把它们跳过了（它们仍在存储里，没有被覆盖）",
                )
                .with("malformed", u64::try_from(loss.malformed).unwrap_or(u64::MAX))
                .with("missing", u64::try_from(loss.missing).unwrap_or(u64::MAX)),
            );
        }
        Ok(records)
    }

    /// 尽力而为地写索引。**失败不影响读路径**（存储只读时读路径本来是能工作的）。
    ///
    /// ⚠️ 成功之后**刻意不把 `index_write_failed_at` 清回 `None`**：那行代码是死的，
    /// 不是省略 —— 一次尝试能发生的前提就是 `now - failed_at >= RETRY`；时钟单调，
    /// 之后每次调用只会让 `now` 更大。时钟**不**单调那一种（NTP 回拨）显式挡了 `since < 0`。
    async fn write_index_best_effort(
        &self,
        st: &mut RepoState,
        ids: &[String],
        now: i64,
        why: &'static str,
    ) {
        if let Some(failed_at) = st.index_write_failed_at {
            let since = now - failed_at;
            // `since < 0` = 时钟回拨（NTP）：立刻放行，与本仓其余几处同一套语义。
            if (0..INDEX_WRITE_RETRY_MS).contains(&since) {
                return;
            }
        }
        let idx = make_pool_index(ids);
        let Ok(value) = serde_json::to_value(&idx) else {
            return;
        };
        match self.storage.put(POOL_INDEX_KEY, value, None).await {
            Ok(()) => self.logger.log(
                LogEntry::new(LogLevel::Info, EVENT_POOL_INDEX_BACKFILLED, why)
                    .with("count", u64::try_from(ids.len()).unwrap_or(u64::MAX)),
            ),
            Err(e) => {
                st.index_write_failed_at = Some(now);
                self.logger.log(
                    LogEntry::new(
                        LogLevel::Warn,
                        EVENT_POOL_INDEX_WRITE_FAILED,
                        "写 pool:index 失败；读路径不受影响（会回落 list），下次按退避再试",
                    )
                    .with("err", e.to_string()),
                );
            }
        }
    }

    // ── 写路径 ──────────────────────────────────────────────────────────────

    /// 回写一条记录。
    ///
    /// `prev` 的**两个**作用：① 写消除的对照；② **区分「更新」与「新建」** ——
    /// 给了 prev 就意味着「我手上有一份旧的，要把它改成新的」，于是要先确认那份旧的还在。
    /// 🔴 **不给 prev 就一定落盘**：写消除是可选优化，缺少对照时必须保守。
    pub async fn save(
        &self,
        next: &KeyRecord,
        prev: Option<&KeyRecord>,
        now: i64,
    ) -> StorageResult<()> {
        let mut st = self.state.lock().await;

        let Some(prev) = prev else {
            // 新建：写的是一条**本来就不存在**的记录。
            // 🔴 上一世攒着的增量绝不许跟过来 —— 裸存储删除之后没有任何后续 save、
            //    直接重新导入同一把 key 时，这一行是唯一的闸。
            st.pending_stats.remove(&next.id);
            self.put_record(next).await?;
            replace_in_snapshot(&mut st, next);
            return Ok(());
        };

        if next.id != prev.id {
            // 写的是**另一个键**。`id` 是 scheduling 字段 ⇒ `should_elide` 恒为假，
            // 这里照样调一次是为了让「白名单是唯一判据」这件事在代码里只有一处。
            if self.should_elide(prev, next) {
                return Ok(());
            }
            self.put_record(next).await?;
            replace_in_snapshot(&mut st, next);
            return Ok(());
        }

        // `prev` 手上已经是窄化过的结构体，这里只要把 `None` 摊平成全零。
        let seen = stats_or_empty(prev.stats.as_ref());
        track_baseline(&mut st, &next.id, seen, now);
        let delta = stats_delta(prev.stats.as_ref(), next.stats.as_ref());

        // 攒得比一个触达间隔还久就别再消除了，否则只被打 4xx 的 key 永远落不了盘。
        if self.should_elide(prev, next) && !self.pending_is_stale(&st, &next.id, now) {
            stash_pending(&mut st, &next.id, &delta, now);
            return Ok(());
        }

        if !self.still_exists(&mut st, &next.id).await? {
            // 增量没有归属 —— 留着它就等于给复活留了一条路。
            st.pending_stats.remove(&next.id);
            return Ok(());
        }

        // 落盘 = **本实例记的基线** + 先前被消除掉的那些 + 本次这一笔。
        // 🔴 刻意**不**用 `next.stats` 当基数：那是调用方的视图，它可能落后于存储。
        let entry = st.pending_stats.get(&next.id).cloned();
        let merged_stats = match &entry {
            Some(e) => apply_delta(Some(&e.base), Some(&add_delta(&e.delta, &delta))),
            None => apply_delta(next.stats.as_ref(), None),
        };
        let merged = KeyRecord {
            stats: Some(merged_stats.clone()),
            ..next.clone()
        };
        self.put_record(&merged).await?;
        // **put 成功之后才推进基线**：put 抛错时增量必须留着，下一次再合并一遍。
        if let Some(e) = st.pending_stats.get_mut(&next.id) {
            e.base = merged_stats;
            e.delta = StatsDelta::zero();
            e.since = now;
        }
        replace_in_snapshot(&mut st, &merged);
        Ok(())
    }

    /// 对一次账：拿 `list()` 的结果覆盖索引。
    ///
    /// # 🔴 它治的是「孤儿」，而孤儿的连带后果比「索引脏了」严重得多
    ///
    /// 索引更新是**没有 CAS 的读-改-写**。并发写时后一次可能读到「前一次修好之前」
    /// 的那份索引 ⇒ **刚被捡回来的孤儿转眼又成了孤儿**。
    /// 孤儿不被 [`Self::all`] 看到 ⇒ 缺口被高估 ⇒ **超额补铸**，
    /// 而每铸一把都是一次真实的上游建号，同时撞注册风控与活跃邮箱上限。
    /// **补铸出来的 key 还会再次触发同一条覆盖，一轮一轮地滚。**
    ///
    /// ⚠️ **它不消灭那个窗口**（没有 CAS 就消灭不了），只把它从「一整轮」压到「一轮之内」。
    /// **这句限定不许被改写成「并发已解决」。**
    ///
    /// # ⚠️ 这是全仓**唯一**会从索引里**删**东西的地方
    ///
    /// 读路径上的两处回落都逐字写着「只增不减、剪枝一律交给对账」——
    /// 因为读路径看到的 `list()` 失败与「这把 key 真的没了」**分不开**，
    /// 而在读路径上剪枝意味着**一次 list 抖动就能把整池 key 从索引里抹掉**。
    /// 这里敢剪，是因为它**只在真的铸出过 key 之后被调**，
    /// 且 `list()` 失败时**一个字都不写**（见下）。
    /// ⚠️ 这里的 `list()` **绕开**了 [`Self::list_on_read_path`] 那道退避闸，而那是对的：
    /// 那道闸写着「**读路径** `list()` 的唯一入口」，对账不是读路径。
    /// 它本来就被调用方天然限了频（定时轮默认半小时一跳），
    /// 而**把唯一的剪枝者节流掉 = 幽灵索引项永远没人剪**。
    ///
    /// ⚠️ 写索引也**不走** [`Self::write_index_best_effort`]，理由有三条，每条都够独立成立：
    /// ① 那条路打的是 `pool.index_backfilled`，而对账要打的是下面两条**级别不同**的事件；
    /// ② 那条路吞掉写失败，对账必须如实往上报（调用方据此记 warn）；
    /// ③ 那条路上有 `INDEX_WRITE_RETRY_MS` 退避闸 ——
    /// 🔴 **它会让对账「报告修好了、其实一个字没写」**，而那是最难查的一种沉默。
    pub async fn reconcile(&self) -> StorageResult<ReconcileResult> {
        let actual = ids_from_key_names(&self.storage.list(KEY_PREFIX).await?);
        // ⚠️ `None` = 缺失**或**读坏了（`read_index` 把两者合成了一个 `None`）。
        // 下面的 `cur.is_none() ||` 正是为它写的：
        // 🔴 少了那一截，「池子空 + 索引读坏了」两边都是空集 ⇒ 判等成立 ⇒ 坏值原地不动，
        // 而 `read_index` 上方那段注释把自愈的指望**写在了对账头上**。
        let cur = self.read_index().await;
        let indexed = cur.clone().unwrap_or_default();
        let added: Vec<String> = actual
            .iter()
            .filter(|x| !indexed.contains(x))
            .cloned()
            .collect();
        let removed: Vec<String> = indexed
            .iter()
            .filter(|x| !actual.contains(x))
            .cloned()
            .collect();
        let repaired = cur.is_none() || !same_id_set(&indexed, &actual);

        if repaired {
            let idx = make_pool_index(&actual);
            let value = serde_json::to_value(&idx)
                .map_err(|e| StorageError::new(format!("序列化 pool:index 失败：{e}")))?;
            self.storage.put(POOL_INDEX_KEY, value, None).await?;
            {
                let mut st = self.state.lock().await;
                st.last_known_ids = actual.clone();
                // 🔴 对账刚刚改变了「`all()` 会返回哪些 id」（捡回孤儿、剪掉幽灵），
                // 与 `add`/`delete` 是同一类写，**同样要让快照失效**。
                // 不失效的表现形式是「刚对完账，池子在整个 TTL 里没反应」。
                st.snapshot.invalidate();
            }
            let (level, event, msg) = if cur.is_none() {
                (
                    LogLevel::Info,
                    EVENT_POOL_INDEX_BOOTSTRAPPED,
                    "key 池索引缺失，已按实际记录建立",
                )
            } else {
                (
                    LogLevel::Warn,
                    EVENT_POOL_INDEX_REPAIRED,
                    "key 池索引与实际记录不一致，已按实际记录修复",
                )
            };
            self.logger.log(
                LogEntry::new(level, event, msg)
                    .with("indexed", u64::try_from(indexed.len()).unwrap_or(u64::MAX))
                    .with("actual", u64::try_from(actual.len()).unwrap_or(u64::MAX))
                    .with("added", u64::try_from(added.len()).unwrap_or(u64::MAX))
                    .with("removed", u64::try_from(removed.len()).unwrap_or(u64::MAX)),
            );
        }

        Ok(ReconcileResult {
            indexed: indexed.len(),
            actual: actual.len(),
            repaired,
            added,
            removed,
        })
    }

    /// 新建一把 key。**写序：① 写记录 → ② 进索引**（颠倒 = 可能丢失 key 材料）。
    pub async fn add(&self, key: &str, now: i64) -> StorageResult<KeyRecord> {
        let record = KeyRecord::fresh(key_id(key), key, now);
        // ① 记录先落盘。
        self.save(&record, None, now).await?;
        // ② 再进索引。中途失败留下的是**孤儿记录**（不被使用，对账能收养）。
        let mut st = self.state.lock().await;
        let mut ids = self.read_index().await.unwrap_or_default();
        if !ids.contains(&record.id) {
            ids.push(record.id.clone());
            // 这里**不走**退避（它不是读路径的兜底重建，是一次确定的结构变更）。
            let idx = make_pool_index(&ids);
            if let Ok(v) = serde_json::to_value(&idx) {
                self.storage.put(POOL_INDEX_KEY, v, None).await?;
            }
        }
        st.snapshot.invalidate();
        Ok(record)
    }

    /// 删一把 key。**写序：① 出索引 → ② 删记录**（颠倒 = 删除可能被对账悄悄撤销）。
    ///
    /// 返回「它本来在不在」。
    pub async fn delete(&self, id: &str, _now: i64) -> StorageResult<bool> {
        let existed = self
            .storage
            .get(&format!("{KEY_PREFIX}{id}"))
            .await?
            .is_some();
        // ① 先出索引。
        let ids = self.read_index().await.unwrap_or_default();
        let kept: Vec<String> = ids.iter().filter(|x| x.as_str() != id).cloned().collect();
        if kept.len() != ids.len()
            && let Ok(v) = serde_json::to_value(make_pool_index(&kept))
        {
            self.storage.put(POOL_INDEX_KEY, v, None).await?;
        }
        // ② 再删记录。
        self.storage.delete(&format!("{KEY_PREFIX}{id}")).await?;
        let mut st = self.state.lock().await;
        // 攒着的增量必须跟着走，否则重新导入同一把 key 时它会「继承前任的计数」。
        st.pending_stats.remove(id);
        st.snapshot.invalidate();
        Ok(existed)
    }

    async fn put_record(&self, r: &KeyRecord) -> StorageResult<()> {
        let v = serde_json::to_value(r)
            .map_err(|e| StorageError::new(format!("序列化 key 记录失败: {e}")))?;
        // 🔴 **一律不传 `expires_at`**：key 记录靠人管，不靠 TTL 蒸发。
        self.storage
            .put(&format!("{KEY_PREFIX}{}", r.id), v, None)
            .await
    }

    /// 🔴 **「删除不许被陈旧写回撤销」这条不变量的全部实现。**
    ///
    /// 失效链（评审实测复现，**单副本 Docker 也中招**）：加了池快照缓存之后 `all()`
    /// 交出的是一份最长 `pool_cache_ttl_ms` 的进程级快照，而运维吊销一把泄漏的 key 时
    /// 若绕开面板、**直接编辑 `store.json`**，记录没了，可本实例的快照里它还在：
    /// 下一个请求照样选中它，上游一报错就 `apply_strike` ⇒ 调度字段变了 ⇒
    /// 无条件 put 把整条记录原样写回去 ⇒ **被吊销的凭据复活**，而且此后不会自愈。
    ///
    /// 三条备选都评估过、都不选：**墓碑法**只护得住 `delete()` 这一条路径
    /// （而唯一存在的吊销姿势恰恰是绕过它的裸存储删除），何况墓碑里仍躺着 key 材料；
    /// **对账不再收养孤儿**要求一份「已删 id」的持久集合，同样拿不到，且会拆掉
    /// `add` 崩在①②之间时唯一的补救。**本条改在写侧**，对三种删除路径一视同仁。
    async fn still_exists(&self, st: &mut RepoState, id: &str) -> StorageResult<bool> {
        if self
            .storage
            .get(&format!("{KEY_PREFIX}{id}"))
            .await?
            .is_some()
        {
            return Ok(true);
        }
        self.logger.log(
            LogEntry::new(
                LogLevel::Warn,
                EVENT_POOL_STALE_WRITE_DROPPED,
                "记录已被删除，丢弃这次基于陈旧快照的写回（删除不会被撤销），并立刻刷新快照",
            )
            .with("id", id.to_string()),
        );
        // 记录确实没了，而本实例的快照里还留着它 —— 不失效的话它会继续被选中整整一个 TTL。
        st.snapshot.invalidate();
        Ok(false)
    }

    fn should_elide(&self, prev: &KeyRecord, next: &KeyRecord) -> bool {
        if self.touch_interval_ms <= 0 {
            return false;
        }
        if !only_elidable_changed(prev, next) {
            return false;
        }
        // 从未用过（`None`）⇒ 差值是 +∞ ⇒ **首次使用一定落盘**，
        // 否则面板会永远显示「从未使用」。
        let p = prev.last_used_at.unwrap_or(i64::MIN);
        let n = next.last_used_at.unwrap_or(i64::MIN);
        if n < p {
            // 时钟回拨：老实写，不当成「没变化」。
            return false;
        }
        n.saturating_sub(p) < self.touch_interval_ms
    }

    /// ⚠️ **空增量一律不算陈旧**：落盘之后 `delta` 清零而条目留着（基线要留），
    /// `since` 却还停在旧时刻。
    ///
    /// ⚠️ **真正被它挡住的是「什么都没变」的 no-op save，不是「只有 `last_used_at` 在动」那种**
    /// —— 这句话第一版写反了，是实测订正的。
    fn pending_is_stale(&self, st: &RepoState, id: &str, now: i64) -> bool {
        if self.touch_interval_ms <= 0 {
            return false;
        }
        let Some(e) = st.pending_stats.get(id) else {
            return false;
        };
        if is_zero_delta(&e.delta) {
            return false;
        }
        let age = now - e.since;
        age < 0 || age >= self.touch_interval_ms
    }
}

/// 🔴 **只在还没有未落盘增量时才校准基线。**
///
/// `delta` 非零时 `seen` 里含着的正是**仍攒在 `delta` 里、尚未落盘**的那一段
/// （调用方每次都把 next 回写进自己的视图），把它顶成新基线 ⇒
/// 落盘写成 `base + 2×delta + 本次` ⇒ **计数虚高**。
/// 实测：2 次成功被消除 + 1 次失败落盘，真值 `requests:3 / success:2`，
/// 不加这个条件落盘写出 `5 / 4`。
/// **虚高比少计更糟**：面板已经对用户承诺过「并发下会少计」，虚高是在说一句它不再保证的话。
fn track_baseline(st: &mut RepoState, id: &str, seen: KeyStats, now: i64) {
    match st.pending_stats.get_mut(id) {
        None => {
            st.pending_stats.insert(
                id.to_string(),
                PendingStats {
                    base: seen,
                    delta: StatsDelta::zero(),
                    since: now,
                },
            );
        }
        Some(cur) => {
            if is_zero_delta(&cur.delta) {
                cur.base = max_stats(&cur.base, &seen);
            }
        }
    }
}

fn stash_pending(st: &mut RepoState, id: &str, delta: &StatsDelta, now: i64) {
    if let Some(e) = st.pending_stats.get_mut(id) {
        let was_zero = is_zero_delta(&e.delta);
        e.delta = add_delta(&e.delta, delta);
        if was_zero && !is_zero_delta(&e.delta) {
            // `since` 是当前这批增量**最早**那一条的时刻。
            e.since = now;
        }
    }
}

/// 写穿透：在当前快照里把同 id 的那一项换掉。
///
/// **不在快照里（刚 add 的）就什么都不做** —— 交给 `invalidate` 后的下一次刷新。
fn replace_in_snapshot(st: &mut RepoState, r: &KeyRecord) {
    let Some(cur) = st.snapshot.current() else {
        return;
    };
    let Some(i) = cur.iter().position(|x| x.id == r.id) else {
        return;
    };
    let mut next = cur.clone();
    next[i] = r.clone();
    st.snapshot.set(next);
}

/// 两把 key 是不是同一把。**常数时间比较** —— 导入去重这条路径上
/// 调用方送来的是明文，逐字节短路比较会泄漏「前几位对不对」。
#[must_use]
pub fn same_key(a: &str, b: &str) -> bool {
    constant_time_equal(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::core::stats::{Outcome, with_outcome};
    use crate::ports::logger::NullLogger;
    use serde_json::json;

    const TOUCH: i64 = 21_600_000;

    fn repo(storage: Arc<dyn Storage>) -> KeyPoolRepo {
        KeyPoolRepo::new(storage, Arc::new(NullLogger), 60_000, TOUCH)
    }

    /// key id = SHA-256 前 8 字节的十六进制，**16 个字符**。
    /// 期望值手写字面量（同一把 key 永远得到同一个 id 是它的全部意义）。
    #[test]
    fn the_key_id_is_a_stable_sha256_prefix() {
        assert_eq!(key_id("abc").len(), 16);
        assert_eq!(key_id("abc"), key_id("abc"));
        assert_ne!(key_id("abc"), key_id("abd"));
        // sha256("abc") = ba7816bf8f01cfea414140de5dae2223...
        assert_eq!(key_id("abc"), "ba7816bf8f01cfea");
    }

    /// 导入判据：可打印 ASCII、不含空格、非空、≤1024。
    #[test]
    fn importable_keys_are_sendable_as_a_header_value() {
        assert!(is_importable_key("sk-abc123"));
        assert!(!is_importable_key(""));
        assert!(!is_importable_key("has space"));
        assert!(!is_importable_key("有中文"));
        assert!(!is_importable_key("tab\there"));
        assert!(!is_importable_key(&"x".repeat(MAX_KEY_LENGTH + 1)));
        assert!(is_importable_key(&"x".repeat(MAX_KEY_LENGTH)));
    }

    /// 🔴 **转发热路径零 `list()`**：索引在的时候 `all()` 一次 list 都不发。
    #[tokio::test]
    async fn the_hot_read_path_never_calls_list() {
        let mem = Arc::new(MemoryStorage::new());
        let counting = Arc::new(CountingStorage::new(mem.clone()));
        let r = repo(counting.clone());
        r.add("sk-one", 0).await.unwrap();
        let before = counting.counts.snapshot().3;

        for i in 0..20 {
            // 每次都越过 TTL，强制真装载 —— 即便如此也不许 list。
            r.all(i * 100_000).await.unwrap();
        }
        assert_eq!(counting.counts.snapshot().3, before, "热路径上不许有 list");
    }

    /// 🔴 **权威的空索引不许让手工导入的 key 隐身。**
    ///
    /// 回归对象：全新部署上对账先写下 `{"v":1,"ids":[]}`，运维随后手工塞 key
    /// ⇒ 索引「合法」，永远不走缺失回落，`all()` 恒 0 条，网关一直 503 `pool_empty`。
    #[tokio::test]
    async fn a_manually_imported_key_is_found_even_with_an_authoritative_empty_index() {
        let mem = Arc::new(MemoryStorage::new());
        mem.put(POOL_INDEX_KEY, json!({ "v": 1, "ids": [] }), None)
            .await
            .unwrap();
        let rec = KeyRecord::fresh("aa", "sk-manual", 0);
        mem.put("key:aa", serde_json::to_value(&rec).unwrap(), None)
            .await
            .unwrap();

        let r = repo(mem);
        let all = r.all(0).await.unwrap();
        assert_eq!(all.len(), 1, "手工导入的 key 必须被 list 兜底找回来");
        assert_eq!(all[0].key, "sk-manual");
    }

    // ── 对账 ──────────────────────────────────────────────────────────────
    //
    // 对账是**全仓唯一会从索引里删东西的地方**。两条读路径都逐字写着
    // 「只增不减、剪枝一律交给对账」，所以下面这组判据要同时钉住两头：
    // 剪得掉幽灵（否则没人剪），和剪不掉真 key（否则它自己就是灾难）。

    /// 🔴 **幽灵索引项要真的被剪掉。**
    ///
    /// 不剪的后果不是「索引脏了」：`all()` 每次都要按索引里的 id 去 `get`，
    /// 幽灵 id 每轮都白读一次，而且永远不会自己消失。
    #[tokio::test]
    async fn reconcile_prunes_index_entries_whose_record_is_gone() {
        let mem = Arc::new(MemoryStorage::new());
        let rec = KeyRecord::fresh("aa", "sk-real", 0);
        mem.put("key:aa", serde_json::to_value(&rec).unwrap(), None)
            .await
            .unwrap();
        mem.put(
            POOL_INDEX_KEY,
            json!({ "v": 1, "ids": ["aa", "ghost"] }),
            None,
        )
        .await
        .unwrap();

        let r = repo(mem.clone());
        let out = r.reconcile().await.unwrap();
        assert_eq!(out.removed, ["ghost"], "只该动了 ghost 这一个");
        assert!(out.added.is_empty());
        assert!(out.repaired);

        let idx = mem.get(POOL_INDEX_KEY).await.unwrap().unwrap();
        let ids: Vec<&str> = idx["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(ids, ["aa"], "🔴 幽灵没被剪掉 —— 而这里是全仓唯一会剪的地方");
    }

    /// 🔴 **`list()` 失败时一个字都不许写。**
    ///
    /// 这是对账敢剪枝的全部前提。一次 list 抖动读回空清单、
    /// 而这里照旧「按 list 的结果覆盖」⇒ **整池 key 被一次性从索引里抹掉**，
    /// 随后 `all()` 只能靠退避着的空池兜底 list 才找得回来，期间网关 503。
    #[tokio::test]
    async fn a_failed_list_writes_nothing_at_all_instead_of_pruning_the_whole_pool() {
        let mem = Arc::new(MemoryStorage::new());
        let rec = KeyRecord::fresh("aa", "sk-real", 0);
        mem.put("key:aa", serde_json::to_value(&rec).unwrap(), None)
            .await
            .unwrap();
        mem.put(POOL_INDEX_KEY, json!({ "v": 1, "ids": ["aa"] }), None)
            .await
            .unwrap();

        let r = repo(mem.clone());
        mem.fail_next("list 抖了一下");
        assert!(r.reconcile().await.is_err(), "list 失败必须如实往上报");

        let idx = mem.get(POOL_INDEX_KEY).await.unwrap().unwrap();
        assert_eq!(
            idx["ids"].as_array().unwrap().len(),
            1,
            "🔴 一次 list 抖动就把真 key 从索引里抹掉了",
        );
    }

    /// 🔴 **索引与现实一致时，一次写都不许发。**
    ///
    /// 稳态下补池每一轮都会调对账，而稳态下索引本来就是对的。
    /// 无条件写 = 给每一轮凭空加一次写。
    #[tokio::test]
    async fn reconcile_writes_nothing_when_the_index_already_matches_reality() {
        let mem = Arc::new(MemoryStorage::new());
        let counting = Arc::new(CountingStorage::new(mem.clone()));
        let r = repo(counting.clone());
        r.add("sk-one", 0).await.unwrap();

        let writes_before = counting.counts.snapshot().1;
        let out = r.reconcile().await.unwrap();
        assert!(!out.repaired);
        assert!(out.added.is_empty() && out.removed.is_empty());
        assert_eq!(
            counting.counts.snapshot().1,
            writes_before,
            "🔴 没有漂移也照写 —— 稳态下每半小时白写一次",
        );
    }

    /// 🔴 **读坏了的索引要被覆盖掉，哪怕池子是空的。**
    ///
    /// 回归对象：把「读不出来」当成空清单去比 ⇒ 空池时两边都是空集 ⇒ 判等成立
    /// ⇒ 坏值原地不动。而 `read_index` 上方那段注释恰恰把自愈的指望
    /// **写在了对账头上**：「坏了就当缺失 ⇒ 调用方回落 list 并顺手把坏值覆盖掉」。
    #[tokio::test]
    async fn a_corrupt_index_is_overwritten_even_when_the_pool_is_empty() {
        let mem = Arc::new(MemoryStorage::new());
        mem.put(POOL_INDEX_KEY, json!("这不是索引"), None)
            .await
            .unwrap();

        let r = repo(mem.clone());
        assert!(r.reconcile().await.unwrap().repaired);

        let idx = mem.get(POOL_INDEX_KEY).await.unwrap().unwrap();
        assert!(
            idx["ids"].is_array(),
            "🔴 坏值原地不动 —— 而全仓指望的就是这里把它覆盖掉：{idx}",
        );
    }

    /// 🔴 两个方向**分开报**，不是两边长度之差。
    ///
    /// 「剪掉 1 个幽灵 + 捡回 1 个孤儿」两边一样长 ⇒ 长度差报 0，
    /// 而那一轮恰恰是对账干得最多的一轮。
    #[tokio::test]
    async fn the_reported_drift_counts_both_directions_not_the_length_difference() {
        let mem = Arc::new(MemoryStorage::new());
        let rec = KeyRecord::fresh("orphan", "sk-orphan", 0);
        mem.put("key:orphan", serde_json::to_value(&rec).unwrap(), None)
            .await
            .unwrap();
        mem.put(POOL_INDEX_KEY, json!({ "v": 1, "ids": ["ghost"] }), None)
            .await
            .unwrap();

        let r = repo(mem);
        let out = r.reconcile().await.unwrap();
        assert_eq!(
            (out.added.as_slice(), out.removed.as_slice()),
            (
                ["orphan".to_string()].as_slice(),
                ["ghost".to_string()].as_slice()
            ),
            "🔴 一进一出被报成了 0 —— 长度差把最忙的一轮说成了没事发生",
        );
        assert_eq!(
            (out.indexed, out.actual),
            (1, 1),
            "长度差恰好是 0，正是这条判据存在的理由"
        );
    }

    /// 🔴 **两条事件名不许合并，级别不同是有意的。**
    ///
    /// 「缺失」是全新部署每次都会走一遍的路（info）；
    /// 「不符」意味着真出过事 —— 写记录成功而写索引失败、或别的副本覆盖了它（warn）。
    /// 合并了，运维就没法只 grep 后者；而事件名全表上写着「Rust 侧一个字都不许改，
    /// 它们是运维的 grep 锚点」。
    #[tokio::test]
    async fn a_missing_index_and_a_disagreeing_index_are_two_different_events() {
        // ① 索引整个缺失 ⇒ info / bootstrapped。
        let mem = Arc::new(MemoryStorage::new());
        let rec = KeyRecord::fresh("aa", "sk-real", 0);
        mem.put("key:aa", serde_json::to_value(&rec).unwrap(), None)
            .await
            .unwrap();
        let lg = Arc::new(crate::server::test_support::RecordingLogger::default());
        KeyPoolRepo::new(mem.clone(), lg.clone(), 60_000, TOUCH)
            .reconcile()
            .await
            .unwrap();
        let e = lg.entries();
        let boot = e
            .iter()
            .find(|x| x.event == EVENT_POOL_INDEX_BOOTSTRAPPED)
            .expect("索引缺失该打 bootstrapped");
        assert_eq!(boot.level, LogLevel::Info, "全新部署每次都走，不该是 warn");

        // ② 索引在、但不符 ⇒ warn / repaired。
        mem.put(POOL_INDEX_KEY, json!({ "v": 1, "ids": ["ghost"] }), None)
            .await
            .unwrap();
        let lg2 = Arc::new(crate::server::test_support::RecordingLogger::default());
        KeyPoolRepo::new(mem.clone(), lg2.clone(), 60_000, TOUCH)
            .reconcile()
            .await
            .unwrap();
        let e2 = lg2.entries();
        let rep = e2
            .iter()
            .find(|x| x.event == EVENT_POOL_INDEX_REPAIRED)
            .unwrap_or_else(|| panic!("索引不符该打 repaired：{:?}", lg2.events()));
        assert_eq!(
            rep.level,
            LogLevel::Warn,
            "🔴 「索引与实际不符」被降成了 info —— 那是真出过事，运维要 grep 得到",
        );
    }

    /// 🔴 **对账改变了「`all()` 会返回哪些 id」⇒ 必须让快照失效。**
    ///
    /// 不失效的表现形式是「刚对完账，池子在整个 TTL 里没反应」：
    /// 剪掉的幽灵还在被选中、捡回的孤儿还是看不见。
    #[tokio::test]
    async fn reconcile_invalidates_the_snapshot_because_it_changed_what_all_returns() {
        let mem = Arc::new(MemoryStorage::new());
        let r = repo(mem.clone());
        r.add("sk-one", 0).await.unwrap();
        assert_eq!(r.all(0).await.unwrap().len(), 1, "先把快照烘热");

        // 绕过 repo 直接删记录 —— 模拟别处删掉了它，索引里留下幽灵。
        let id = key_id("sk-one");
        mem.delete(&format!("key:{id}")).await.unwrap();
        r.reconcile().await.unwrap();

        // TTL 还没到。不失效的话这里会拿陈旧快照，仍然报 1 条。
        assert_eq!(
            r.all(1).await.unwrap().len(),
            0,
            "🔴 对完账池子没反应 —— 剪掉的幽灵还在被选中，整整一个 TTL",
        );
    }

    /// 只让**写 `pool:index`** 这一步失败，别的照常。
    ///
    /// ⚠️ 不能用 `MemoryStorage::fail_next` 来测这件事：它失败的是**下一次存储调用**，
    /// 而对账的下一次调用是 `list` —— 判据会因为 list 失败而变绿，
    /// 于是它测的其实是上面那条「list 失败」，**换个名字又测了一遍**。
    struct IndexWriteFails(Arc<MemoryStorage>);
    #[async_trait::async_trait]
    impl Storage for IndexWriteFails {
        async fn get(&self, k: &str) -> StorageResult<Option<Value>> {
            self.0.get(k).await
        }
        async fn put(&self, k: &str, v: Value, e: Option<i64>) -> StorageResult<()> {
            if k == POOL_INDEX_KEY {
                return Err(StorageError::new("写索引失败"));
            }
            self.0.put(k, v, e).await
        }
        async fn delete(&self, k: &str) -> StorageResult<()> {
            self.0.delete(k).await
        }
        async fn list(&self, p: &str) -> StorageResult<Vec<String>> {
            self.0.list(p).await
        }
    }

    /// 🔴 **写索引失败要如实往上报，不许报告「修好了」。**
    ///
    /// 回归对象：图省事走 `write_index_best_effort` —— 那条路吞掉写失败，
    /// 还架着 `INDEX_WRITE_RETRY_MS` 退避闸，
    /// 于是对账会**报告修好了、其实一个字没写**，而那是最难查的一种沉默。
    #[tokio::test]
    async fn a_failed_index_write_is_reported_instead_of_being_swallowed() {
        let mem = Arc::new(MemoryStorage::new());
        let rec = KeyRecord::fresh("aa", "sk-real", 0);
        mem.put("key:aa", serde_json::to_value(&rec).unwrap(), None)
            .await
            .unwrap();
        mem.put(POOL_INDEX_KEY, json!({ "v": 1, "ids": ["ghost"] }), None)
            .await
            .unwrap();

        let r = repo(Arc::new(IndexWriteFails(mem.clone())));
        assert!(
            r.reconcile().await.is_err(),
            "🔴 写失败被吞了，对账报告「修好了」而索引一个字没变",
        );
        // 而且 list 那一步是好的 —— 否则这条判据又退化成上面那条。
        assert_eq!(mem.list(KEY_PREFIX).await.unwrap().len(), 1);
    }

    /// ⚠️ 对账**绕开**读路径那道退避闸，而那是对的。
    ///
    /// 那道闸的职责是压住转发热路径上的 list；把唯一的剪枝者也节流掉，
    /// 等于幽灵索引项永远没人剪。
    #[tokio::test]
    async fn reconcile_is_not_silenced_by_the_read_path_list_backoff() {
        let mem = Arc::new(MemoryStorage::new());
        mem.put(POOL_INDEX_KEY, json!({ "v": 1, "ids": ["ghost"] }), None)
            .await
            .unwrap();
        let counting = Arc::new(CountingStorage::new(mem.clone()));
        let r = repo(counting.clone());

        // 先把读路径的退避闸压满：空池兜底刚 list 过一次。
        r.all(0).await.unwrap();
        let lists_before = counting.counts.snapshot().3;

        // 同一毫秒再对账 —— 闸正关着，而对账必须照样 list。
        r.reconcile().await.unwrap();
        assert!(
            counting.counts.snapshot().3 > lists_before,
            "🔴 对账被读路径的退避闸挡住了 —— 剪枝者一被节流，幽灵就永远在",
        );
    }

    /// 空池兜底的 `list` 走退避闸：窗口内**不再** list 第二次。
    #[tokio::test]
    async fn the_empty_pool_rescan_is_throttled() {
        let mem = Arc::new(MemoryStorage::new());
        mem.put(POOL_INDEX_KEY, json!({ "v": 1, "ids": [] }), None)
            .await
            .unwrap();
        let counting = Arc::new(CountingStorage::new(mem));
        // 关掉快照缓存，让每次 all() 都真装载 —— 这样数到的 list 次数只由退避闸决定。
        let r = KeyPoolRepo::new(counting.clone(), Arc::new(NullLogger), 0, TOUCH);

        r.all(0).await.unwrap();
        let after_first = counting.counts.snapshot().3;
        assert_eq!(after_first, 1);
        r.all(READ_PATH_LIST_BACKOFF_MS - 1).await.unwrap();
        assert_eq!(counting.counts.snapshot().3, 1, "窗口内不许再 list");
        r.all(READ_PATH_LIST_BACKOFF_MS).await.unwrap();
        assert_eq!(counting.counts.snapshot().3, 2, "出了窗口要再试一次");
    }

    /// 🔴 **绝不许把「存储读不出来」伪装成「池子是空的」。**
    ///
    /// 回归对象：报 `pool_empty` 会把运维指向「你还没导 key」，
    /// 而真相是存储故障 —— 两条自愈路径此时都已经死了。
    #[tokio::test]
    async fn an_unreadable_store_is_never_reported_as_an_empty_pool() {
        let mem = Arc::new(MemoryStorage::new());
        mem.put(POOL_INDEX_KEY, json!({ "v": 1, "ids": [] }), None)
            .await
            .unwrap();
        // 索引读得到、**只有 `list` 读不到** —— 这个精确组合正是端口替身存在的理由
        //（用一个坏文件构造不出来：那样 `get` 也一起坏了）。
        let r = repo(Arc::new(ListFails(mem)));
        assert!(r.all(0).await.is_err(), "必须如实抛，而不是回一个空池");
    }

    /// 只让 `list` 失败的装饰器。见上一格的理由。
    struct ListFails(Arc<MemoryStorage>);

    #[async_trait::async_trait]
    impl Storage for ListFails {
        async fn get(&self, k: &str) -> StorageResult<Option<Value>> {
            self.0.get(k).await
        }
        async fn put(&self, k: &str, v: Value, e: Option<i64>) -> StorageResult<()> {
            self.0.put(k, v, e).await
        }
        async fn delete(&self, k: &str) -> StorageResult<()> {
            self.0.delete(k).await
        }
        async fn list(&self, _p: &str) -> StorageResult<Vec<String>> {
            Err(StorageError::new("EIO"))
        }
    }

    /// 🔴 **写消除**：只有 `last_used_at` 变化时不落盘。
    #[tokio::test]
    async fn a_touch_only_change_is_elided() {
        let mem = Arc::new(MemoryStorage::new());
        let counting = Arc::new(CountingStorage::new(mem));
        let r = repo(counting.clone());
        let rec = r.add("sk-one", 0).await.unwrap();
        let before = counting.counts.snapshot().1;

        let next = KeyRecord {
            last_used_at: Some(1_000),
            ..rec.clone()
        };
        // 第一次用：`last_used_at` 从 None 变成 Some ⇒ 差值 +∞ ⇒ **一定落盘**
        // （否则面板永远显示「从未使用」）。
        r.save(&next, Some(&rec), 1_000).await.unwrap();
        assert_eq!(counting.counts.snapshot().1, before + 1);

        let third = KeyRecord {
            last_used_at: Some(2_000),
            ..next.clone()
        };
        r.save(&third, Some(&next), 2_000).await.unwrap();
        assert_eq!(
            counting.counts.snapshot().1,
            before + 1,
            "触达间隔内的第二次触碰必须被消除掉"
        );
    }

    /// 🔴 **停用一把 key 的写绝不许被消除掉。**
    ///
    /// 回归对象：把 `disabled` 放进白名单的后果是「面板显示已停用、调度器照常用它」，
    /// 而且**只对正在被使用的那些 key 发生** —— 恰恰是最需要能停下来的那些。
    #[tokio::test]
    async fn disabling_a_key_is_never_elided() {
        let mem = Arc::new(MemoryStorage::new());
        let counting = Arc::new(CountingStorage::new(mem.clone()));
        let r = repo(counting.clone());
        let rec = r.add("sk-one", 0).await.unwrap();
        let used = KeyRecord {
            last_used_at: Some(1_000),
            ..rec.clone()
        };
        r.save(&used, Some(&rec), 1_000).await.unwrap();
        let before = counting.counts.snapshot().1;

        let off = KeyRecord {
            disabled: Some(true),
            last_used_at: Some(1_001),
            ..used.clone()
        };
        r.save(&off, Some(&used), 1_001).await.unwrap();
        assert_eq!(counting.counts.snapshot().1, before + 1, "停用必须落盘");

        let stored = mem.get(&format!("key:{}", rec.id)).await.unwrap().unwrap();
        assert_eq!(stored["disabled"], json!(true));
    }

    /// 「只改一格」的构造器。
    type Mutator = fn(&KeyRecord) -> KeyRecord;

    /// camelCase（serde 名）→ snake_case（`MAY_ELIDE_FIELDS` 用的名）。
    /// **纯机械转换**，不从任何一张清单推导。
    fn snake_case(camel: &str) -> String {
        let mut out = String::new();
        for c in camel.chars() {
            if c.is_ascii_uppercase() {
                out.push('_');
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    }

    /// 🔴🔴 **写消除的字段覆盖门禁**（照抄 `core/narrow.rs` 给读侧落的那一道）。
    ///
    /// 回归对象：给 `KeyRecord` 加一个字段而忘了来 `only_elidable_changed` 加一行
    /// ⇒ 只改该字段的那次 save 让它返回 `true` ⇒ **写被整个消除** ⇒
    /// 面板显示保存成功、存储里那一格永远是旧值，而任何一格行为断言都不会红。
    ///
    /// 两道门一起落：
    /// ① **覆盖**：期望侧是下面这张**手写**的构造器表，实际侧从序列化结果现算 —— 两侧不互推；
    /// ② **行为**：只改一格时 `only_elidable_changed` 的答案必须**恰好**等于
    ///    「这一格在不在 [`MAY_ELIDE_FIELDS`] 里」—— 删掉函数里任何一行比对，这一格当场红。
    #[test]
    fn every_key_record_field_is_either_compared_or_explicitly_elidable() {
        // 一条**每一格都在场**的记录：`skip_serializing_if` 才不会把可选那三格藏起来。
        let base = KeyRecord {
            upstream: Some(crate::core::types::KeyUpstream::International),
            stats: Some(KeyStats::empty()),
            disabled: Some(false),
            note: Some(String::new()),
            ..KeyRecord::fresh("id0", "sk-plain", 1_000)
        };
        // 每个字段一个「**只改这一格**」的构造器。键名是 serde 名，好与序列化结果对账。
        let cases: [(&str, Mutator); 15] = [
            ("id", |r| KeyRecord {
                id: format!("{}x", r.id),
                ..r.clone()
            }),
            ("key", |r| KeyRecord {
                key: format!("{}x", r.key),
                ..r.clone()
            }),
            ("upstream", |r| KeyRecord {
                upstream: Some(crate::core::types::KeyUpstream::China),
                ..r.clone()
            }),
            ("addedAt", |r| KeyRecord {
                added_at: r.added_at + 1,
                ..r.clone()
            }),
            ("lastUsedAt", |r| KeyRecord {
                last_used_at: Some(r.last_used_at.unwrap_or(0) + 1),
                ..r.clone()
            }),
            ("cooldownUntil", |r| KeyRecord {
                cooldown_until: r.cooldown_until + 1,
                ..r.clone()
            }),
            ("cooldownReason", |r| KeyRecord {
                cooldown_reason: Some("rate limited".into()),
                ..r.clone()
            }),
            ("strikes", |r| KeyRecord {
                strikes: r.strikes + 1,
                ..r.clone()
            }),
            ("evicted", |r| KeyRecord {
                evicted: !r.evicted,
                ..r.clone()
            }),
            ("evictedReason", |r| KeyRecord {
                evicted_reason: Some("upstream 401".into()),
                ..r.clone()
            }),
            ("priority", |r| KeyRecord {
                priority: r.priority + 1,
                ..r.clone()
            }),
            ("weight", |r| KeyRecord {
                weight: r.weight + 1,
                ..r.clone()
            }),
            ("stats", |r| KeyRecord {
                stats: Some(KeyStats {
                    requests: 1,
                    ..KeyStats::empty()
                }),
                ..r.clone()
            }),
            ("disabled", |r| KeyRecord {
                disabled: Some(!r.disabled.unwrap_or(false)),
                ..r.clone()
            }),
            ("note", |r| KeyRecord {
                note: Some("运维备注".into()),
                ..r.clone()
            }),
        ];

        // ① 覆盖门禁。
        let v = serde_json::to_value(&base).unwrap();
        let mut actual: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        let mut expected: Vec<&str> = cases.iter().map(|(n, _)| *n).collect();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(
            actual, expected,
            "KeyRecord 的字段与写消除这一侧表过态的字段对不上：\
             加字段时必须来这里表一次态（它算可消除的，还是必须逐一比对）"
        );

        // ② 行为门禁。
        for (name, mutate) in cases {
            let next = mutate(&base);
            assert_ne!(next, base, "夹具自己没改动 `{name}`，这一格什么都没测");
            let elidable = MAY_ELIDE_FIELDS.contains(&snake_case(name).as_str());
            assert_eq!(
                only_elidable_changed(&base, &next),
                elidable,
                "只改了 `{name}`：它{}在白名单里，`only_elidable_changed` 就该回 {elidable}",
                if elidable { "" } else { "不" }
            );
        }
    }

    /// 🔴 **被消除的计数不许丢**：50 次成功之后落盘的 `requests` 不是 1。
    ///
    /// 回归对象：`stats` 是唯一一个「消除 ≠ 丢弃」的字段。
    /// 实测过的坏行为是「50 次成功之后落盘的 requests 是 1」。
    #[tokio::test]
    async fn elided_counters_are_carried_over_not_dropped() {
        let mem = Arc::new(MemoryStorage::new());
        let r = repo(mem.clone());
        let mut cur = r.add("sk-one", 0).await.unwrap();

        for i in 1..=50 {
            let next = KeyRecord {
                last_used_at: Some(i),
                ..with_outcome(&cur, Outcome::Success, i, None)
            };
            r.save(&next, Some(&cur), i).await.unwrap();
            cur = next;
        }
        // 再来一次**不可消除**的写（改备注），把攒着的增量一起带下去。
        let noted = KeyRecord {
            note: Some("x".into()),
            ..cur.clone()
        };
        r.save(&noted, Some(&cur), 60).await.unwrap();

        let stored = mem.get(&format!("key:{}", cur.id)).await.unwrap().unwrap();
        // 50 次成功 = 50 笔；改备注那一次**不产生**计数。
        // ⚠️ 期望值是手算出来的，不是从被测对象现算的。
        assert_eq!(
            stored["stats"]["requests"],
            json!(50),
            "被写消除吃掉的那 49 笔计数必须在下一次落盘时补回来（坏行为是只剩 1）"
        );
    }

    /// 🔴 **只被打 4xx 的那把 key，它的计数也必须落盘。**（`pending_is_stale` 的存在理由。）
    ///
    /// 回归对象：上游 4xx 直通既不改调度字段、也不碰 `last_used_at`
    /// ⇒ `should_elide` 的 `n - p == 0` 恒成立 ⇒ 不加这条兜底的话这把 key 的计数
    /// **永远落不了盘**，面板上它永远显示 0 次客户端错误。
    #[tokio::test]
    async fn a_key_that_only_ever_gets_client_errors_still_gets_its_counters_persisted() {
        let mem = Arc::new(MemoryStorage::new());
        let r = repo(mem.clone());
        let mut cur = r.add("sk-one", 0).await.unwrap();
        let name = format!("key:{}", cur.id);

        // 5 次上游 4xx 直通：**只有 stats 在动**。
        for i in 1..=5 {
            let next = with_outcome(&cur, Outcome::ClientError, i, None);
            assert_eq!(next.last_used_at, cur.last_used_at, "4xx 不碰 last_used_at");
            r.save(&next, Some(&cur), i).await.unwrap();
            cur = next;
        }
        let stored = mem.get(&name).await.unwrap().unwrap();
        assert_eq!(
            stored["stats"]["clientErrors"],
            json!(null),
            "一个触达间隔之内照常被消除"
        );

        // 攒过一个触达间隔之后，下一笔同样「只有 stats 在动」的写必须真的落盘。
        let next = with_outcome(&cur, Outcome::ClientError, TOUCH + 1, None);
        r.save(&next, Some(&cur), TOUCH + 1).await.unwrap();
        let stored = mem.get(&name).await.unwrap().unwrap();
        assert_eq!(
            stored["stats"]["clientErrors"],
            json!(6),
            "攒过一个触达间隔就不许再消除了（坏行为是这一格永远不落盘）"
        );
    }

    /// ⚠️ **空增量一律不算陈旧。**
    ///
    /// 落盘之后 `delta` 清零而条目留着（基线要留），`since` 却还停在旧时刻 ——
    /// 此时一次 no-op save、或一次「只有 `last_used_at` 在动」的 save
    /// **仍然要被消除掉**，不许被这条兜底顶成一次真实的 put。
    ///
    /// ⚠️ 被它挡住的是「什么都没变」那种，不是「只有 `last_used_at` 在动」那种 ——
    /// 这句话第一版写反了，所以两种形状这里都钉上。
    #[tokio::test]
    async fn a_zero_delta_is_never_stale_so_an_idle_save_stays_elided() {
        let mem = Arc::new(MemoryStorage::new());
        let counting = Arc::new(CountingStorage::new(mem));
        let r = repo(counting.clone());
        let rec = r.add("sk-one", 0).await.unwrap();

        // 一次真落盘：base 就位、delta 清零，而 `since` 从此停在 1_000。
        let used = KeyRecord {
            last_used_at: Some(1_000),
            ..with_outcome(&rec, Outcome::Success, 1_000, None)
        };
        r.save(&used, Some(&rec), 1_000).await.unwrap();
        let before = counting.counts.snapshot().1;

        // ① 什么都没变的 no-op save，发生在一个触达间隔之后。
        r.save(&used, Some(&used), 1_000 + TOUCH + 1).await.unwrap();
        assert_eq!(
            counting.counts.snapshot().1,
            before,
            "空增量不算陈旧：一次 no-op save 不该被顶成一次 put"
        );

        // ② 只有 `last_used_at` 在动（同样在一个触达间隔之后发生）。
        let touched = KeyRecord {
            last_used_at: Some(2_000),
            ..used.clone()
        };
        r.save(&touched, Some(&used), 1_000 + TOUCH + 2)
            .await
            .unwrap();
        assert_eq!(
            counting.counts.snapshot().1,
            before,
            "只有 last_used_at 在动、计数没动 ⇒ 照常消除"
        );
    }

    /// 🔴🔴 **删除不许被陈旧写回撤销**（`still_exists` 那条不变量）。
    ///
    /// 必须**绕过 `delete()`** 来构造：同实例内 `delete()` 会 `invalidate()`，
    /// 被测动作根本发生不了。这里用裸存储删除模拟「运维直接编辑 store.json 吊销泄漏的 key」。
    #[tokio::test]
    async fn a_stale_write_never_resurrects_a_deleted_key() {
        let mem = Arc::new(MemoryStorage::new());
        let r = repo(mem.clone());
        let rec = r.add("sk-leaked", 0).await.unwrap();
        let key_name = format!("key:{}", rec.id);

        // 裸存储删除（绕过 repo，快照里它还在）。
        mem.delete(&key_name).await.unwrap();

        // 一次会改调度字段的写回（strike ⇒ 不可消除 ⇒ 无条件 put 的那条路）。
        let struck = KeyRecord {
            strikes: 1,
            ..rec.clone()
        };
        r.save(&struck, Some(&rec), 1_000).await.unwrap();

        assert_eq!(
            mem.get(&key_name).await.unwrap(),
            None,
            "被吊销的凭据不许复活"
        );
    }

    /// `add` 的写序：**记录先、索引后**。
    #[tokio::test]
    async fn add_writes_the_record_before_the_index() {
        let mem = Arc::new(MemoryStorage::new());
        let r = repo(mem.clone());
        let rec = r.add("sk-one", 0).await.unwrap();
        assert!(mem.get(&format!("key:{}", rec.id)).await.unwrap().is_some());
        let idx = mem.get(POOL_INDEX_KEY).await.unwrap().unwrap();
        assert_eq!(idx["ids"], json!([rec.id]));
    }

    /// `delete` 之后 `all()` 立刻看不见它（同实例内 `invalidate` 是硬保证）。
    #[tokio::test]
    async fn a_deleted_key_disappears_immediately_within_the_same_instance() {
        let mem = Arc::new(MemoryStorage::new());
        let r = repo(mem.clone());
        let rec = r.add("sk-one", 0).await.unwrap();
        assert_eq!(r.all(0).await.unwrap().len(), 1);

        assert!(r.delete(&rec.id, 0).await.unwrap());
        assert_eq!(r.all(0).await.unwrap().len(), 0);
        // 索引里也得干净，否则下一次装载会多一次注定落空的 get。
        let idx = mem.get(POOL_INDEX_KEY).await.unwrap().unwrap();
        assert_eq!(idx["ids"], json!([]));
    }

    /// 🔴 `pool:index` 存了非 JSON 字节时，`all()` **不许抛、不许 panic**。
    ///
    /// 回归对象：存储层遇到坏字节是**先抛**的，`parse` 根本没机会跑；
    /// 不兜住的话每个转发请求 500，而被指定为修复者的对账读同一个键同样抛。
    #[tokio::test]
    async fn a_corrupt_index_heals_instead_of_throwing() {
        let mem = Arc::new(MemoryStorage::new());
        let rec = KeyRecord::fresh("aa", "sk-alive", 0);
        mem.put("key:aa", serde_json::to_value(&rec).unwrap(), None)
            .await
            .unwrap();
        mem.put(POOL_INDEX_KEY, json!("不是索引"), None)
            .await
            .unwrap();

        let r = repo(mem.clone());
        let all = r.all(0).await.expect("坏索引不许把读路径打死");
        assert_eq!(all.len(), 1);
        // 顺手把坏值覆盖掉 —— 自愈。
        let idx = mem.get(POOL_INDEX_KEY).await.unwrap().unwrap();
        assert_eq!(idx["ids"], json!(["aa"]));
    }

    /// 快照是**浅拷贝**交出去的：调用方就地改数组不许污染进程级缓存。
    #[tokio::test]
    async fn the_snapshot_handed_out_is_a_copy() {
        let mem = Arc::new(MemoryStorage::new());
        let r = repo(mem);
        r.add("sk-one", 0).await.unwrap();
        let mut a = r.all(0).await.unwrap();
        a[0].strikes = 99;
        let b = r.all(0).await.unwrap();
        assert_eq!(b[0].strikes, 0, "一次请求的中间状态不许污染进程级缓存");
    }
}
