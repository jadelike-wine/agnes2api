//! 事件落存储的 sink。移植自 TS 版 `src/adapters/logger-store.ts`（218 行，
//! 判据 `logger-store` 27 格 🔴 + 契约 `admin-events` 38 格 🔴）。
//!
//! # 存储形态：`event:<窗口>:<槽位>`，**没有索引**
//!
//! 读路径的候选键由 [`ring::candidate_keys`] 从时钟**纯计算**得出 ——
//! **零 `list()`、零索引读改写**。「索引无上限增长 / 索引读改写丢更新」因此根除；
//! 代价换成「同窗口同槽位真并发覆盖」（最后写的赢，丢一次交错，下一轮各写各的）。
//!
//! # 🔴 有界性走 TTL，**不走 `delete()`**
//!
//! TS 版第一版是「每次成功落盘顺手 delete 一个按固定偏移算出来的旧窗口键」，
//! 被下一轮评审逐场景实测推翻：那个方案的有界性**绑在「落盘节奏恰好规律」上**，
//! 稀疏落盘（gap 超过保留期）时清理率跌到 0。现在用 [`crate::ports::storage::Storage::put`]
//! 的 `expires_at`：**有界性变成存储自己的性质**，与落盘节奏、槽位选择、
//! 进程是否被回收全部无关。
//!
//! # 🔴 最小间隔闸挡的是一根杠杆，不是「省点写」
//!
//! 白名单事件里的 `admin.login_failed` 是**任何未鉴权请求都能触发**的
//! （零凭据、零限速）。没有 [`ring::EVENT_FLUSH_MIN_INTERVAL_MS`] 这道闸，
//! 攻击者反复发失败登录就能**按批驱动存储写** —— 而 `FileStorage` 每次 `put`
//! 都整份重写 `store.json`，与 key 池的状态回写共用同一条写队列。
//! **DoS 面会从「猜口令」扩大到「拖垮 key 池的状态回写」。**
//!
//! ⚠️ 闸的另一半同样承重：`last_flush_at` 的初值必须是**构造那一刻的 `now()`**，
//! 不是「没刷过」这种哨兵。TS 版原来是 `null`、第一次 `maybe_flush` 跳过闸门直接写
//! ⇒ **每次进程冷启动送一次零门槛写**（实测「每次冷启动 = 2 次 KV 写」）。
//!
//! # 本文件的一处**临时安置**，读之前先知道
//!
//! 🔴 下面的 [`ring`] 子模块（窗口/槽位算术、环形追加、归并、写预算、逐条窄化）
//! **最终归宿是 `core::admin::event_ring` 与 `core::admin::event_entry`**
//! （规格 §2.1 的模块表就是这么排的，归期 P1-c）。它现在住在适配器里只有一个理由：
//! **本轮 `core/admin/` 还是空的，而 sink 没有这套算术就写不出来**。
//!
//! P1-c 落 `core/admin/` 时的动作是**整体搬走 + 从这里删掉**，不是「在那边再写一份」——
//! 两份必然漂移，而漂移的现象是「面板读到的窗口和 sink 写进去的窗口不是同一个」。
//! 搬走时判据一起搬：下面 `mod ring_tests` 那几格钉的是纯算术，换个家照样成立。
//!
//! ⚠️ 这套算术**本身是零 IO 纯函数**（时间一律从参数进），所以它现在住在 `adapters`
//! 里并没有破坏 `core` 的零 IO 约束，只是放错了房间。

use crate::ports::logger::{LogEntry, LogLevel, Logger};
use crate::ports::storage::{Storage, StorageError, StorageResult};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────────────────────
// 事件环算术（临时安置，见文件头）
// ─────────────────────────────────────────────────────────────────────────────

pub mod ring {
    //! 事件环的**纯算术**：窗口、槽位、键名、TTL、环形追加、归并、写预算、逐条窄化。
    //!
    //! 🔴 **临时安置**：归宿是 `core::admin::event_ring` / `core::admin::event_entry`
    //! （P1-c）。见本文件头最后一节。
    //!
    //! 这里**一行 IO 都没有**：时间一律从参数进，没有随机、没有存储、没有 env。

    use serde_json::Value;

    /// 单个分片键最多留多少条，超了丢**最旧**的。（`event-ring.ts:50`）
    pub const EVENT_RING_SIZE: usize = 100;
    /// 两次落盘之间的最小间隔。🔴 见文件头「它挡的是一根杠杆」。（`event-ring.ts:70`）
    pub const EVENT_FLUSH_MIN_INTERVAL_MS: i64 = 60_000;
    /// 每实例每天最多落几次盘。（`event-ring.ts:95`）
    ///
    /// 🟡 **依据强度如实登记**：这个数是照着「1,000 次/天的 9.6%」挑的，
    /// **那个分母（Cloudflare KV 每日写配额）已经不存在了，没有人重算过它该是多少**。
    /// 规格 §8.3 第 8 条的裁定是「P1 照抄，不重算」—— 重算它要先定一个新分母
    /// （磁盘写放大能接受到什么程度），那是一次独立的取舍。
    pub const EVENT_WRITES_PER_DAY: u32 = 12;
    /// 时间窗 = 1 小时。（`event-ring.ts:104`）
    pub const EVENT_WINDOW_MS: i64 = 3_600_000;
    /// 槽位数。（`event-ring.ts:112`）
    pub const EVENT_SLOTS: usize = 2;
    /// 回看多少个窗口 ⇒ 24 小时可见历史 / 冷读 48 把键。（`event-ring.ts:120`）
    pub const EVENT_WINDOW_RETAIN: i64 = 24;
    /// TTL 余量。（`event-ring.ts:179`）
    ///
    /// ⚠️ **取一整个窗口是有理由的**：判过期用的时钟与算 `expires_at` 用的时钟
    /// 不保证逐毫秒同步，没有余量的话一个恰好卡在窗口边界的请求会读到空
    /// —— 本身不是错误，但没有必要制造这种边界竞态。
    pub const EVENT_TTL_MARGIN_MS: i64 = EVENT_WINDOW_MS;
    /// 一天。写预算的窗口长度。
    pub const DAY_MS: i64 = 86_400_000;
    /// 分片键前缀。（`event-ring.ts:156`）
    pub const EVENT_KEY_PREFIX: &str = "event:";

    /// `at`（epoch 毫秒）落在第几个时间窗。
    ///
    /// ⚠️ 用 `div_euclid` 而不是 `/`：JS 那边是 `Math.floor`，**负时间戳要向下取整**。
    /// Rust 的 `/` 对负数是向零截断 ⇒ 1970 年之前的时钟会算出**错一格**的窗口。
    pub fn window_index(at: i64) -> i64 {
        at.div_euclid(EVENT_WINDOW_MS)
    }

    /// 稳定的 `shard_id` → 槽位映射。**纯函数、确定性。**
    ///
    /// 🔴 **必须是 `i32` 回绕**（`event-ring.ts:134` 的 `| 0`）。用 `u64` 或
    /// checked 运算会得到**不同的槽位分布** —— 那不影响正确性，但会让升级时
    /// 同一个 `shard_id` 换槽位，⇒ **丢一批已经攒在旧槽位里的事件**。
    /// checked 运算还会在 debug 下 panic，而 release 下回绕 ——
    /// **debug 与 release 行为不一致的判据等于没有判据。**
    ///
    /// ⚠️ 与 JS 的一处已知差异：JS 的 `charCodeAt` 遍历 **UTF-16 码元**，
    /// 这里遍历 **码点**。`shard_id` 在本仓是 uuid 的前 8 位（纯 ASCII），
    /// 两者逐字相同；**非 ASCII 的 `shard_id` 会算出不同槽位**，
    /// 而那只影响「哪个槽位」，不影响任何不变量。
    pub fn slot_of(shard_id: &str) -> usize {
        let mut h: i32 = 0;
        for c in shard_id.chars() {
            h = h.wrapping_mul(31).wrapping_add(c as i32);
        }
        // `rem_euclid` = JS 的 `((h % n) + n) % n`。
        (h.rem_euclid(EVENT_SLOTS as i32)) as usize
    }

    /// 分片键：`event:<窗口>:<槽位>`。
    pub fn shard_key(window: i64, slot: usize) -> String {
        format!("{EVENT_KEY_PREFIX}{window}:{slot}")
    }

    /// 这把键什么时候该消失。见文件头「有界性走 TTL」。
    pub fn event_expires_at(at: i64) -> i64 {
        (window_index(at) + EVENT_WINDOW_RETAIN) * EVENT_WINDOW_MS + EVENT_TTL_MARGIN_MS
    }

    /// 面板读路径要打哪些键。
    ///
    /// 🔴 **`floor` 那条钳位是承重的。** 没有它，一个陈旧游标能让候选键数随
    /// 「游标陈旧度」**线性增长、无上界** —— 评审实测 `after = 0` 时单次请求
    /// 打出约 **99 万次 get**。而「游标长期冻结、系统长期安静」恰恰是本项目
    /// 最常见的稳态。
    ///
    /// 钳位之后返回长度**对任何 `after`（含负数、含极端未来值）都不超过
    /// `EVENT_WINDOW_RETAIN × EVENT_SLOTS = 48`**，这个函数自身是这条不变量
    /// **唯一**需要守住的地方。
    ///
    /// - `after == None`：冷读，回看整个保留期；
    /// - `after` 有值：暖读，只看 `after` 所在窗口到当前窗口（通常 1 个，跨窗口那一刻 2 个）。
    ///
    /// 返回顺序：窗口升序、同窗口内槽位升序（顺序本身无语义，只为确定性）。
    pub fn candidate_keys(now: i64, after: Option<i64>) -> Vec<String> {
        let now_window = window_index(now);
        let floor = now_window - EVENT_WINDOW_RETAIN + 1;
        let from_window = match after {
            None => floor,
            Some(a) => floor.max(window_index(a)),
        };
        let mut keys = Vec::new();
        let mut w = from_window;
        while w <= now_window {
            for s in 0..EVENT_SLOTS {
                keys.push(shard_key(w, s));
            }
            w += 1;
        }
        keys
    }

    /// 环形追加：超出上限丢**最旧**的。不就地改入参。
    pub fn append_ring(cur: &[Value], add: &[Value], size: usize) -> Vec<Value> {
        let mut merged: Vec<Value> = Vec::with_capacity(cur.len() + add.len());
        merged.extend_from_slice(cur);
        merged.extend_from_slice(add);
        if merged.len() > size {
            merged.split_off(merged.len() - size)
        } else {
            merged
        }
    }

    /// 这一次 [`append_ring`] 会截掉多少条**已经在存储里**的旧事件。
    ///
    /// 🔴 **它存在的理由**：`append_ring` 本身静默丢弃超限的最旧条目，
    /// 而 `dropped` 计数器原来只在**内存缓冲**溢出时计数 ——
    /// 分片攒满 100 条之后，**每一次落盘都在这里静默丢弃**
    /// （任何有持续流量的部署的稳态），**而面板此时显示 `dropped: 0`**。
    pub fn truncated_count(cur_len: usize, add_len: usize, size: usize) -> usize {
        (cur_len + add_len).saturating_sub(size)
    }

    /// 多分片归并：按 `ts` **降序**（最新在前），同 `ts` 用 `(分片序号, 条目序号)`
    /// **显式**稳定破平，截到 `limit`。
    ///
    /// ⚠️ **不依赖排序算法天然稳定**：显式的二级键让结果与标准库实现无关。
    /// 不这样做的话面板每次轮询的顺序都可能在跳。
    pub fn merge_shards(shards: &[Vec<Value>], limit: usize) -> Vec<Value> {
        let mut tagged: Vec<(f64, usize, usize, &Value)> = Vec::new();
        for (si, shard) in shards.iter().enumerate() {
            for (seq, e) in shard.iter().enumerate() {
                // 入参一律来自 `narrow_shard`，`ts` 必定是有限数字；
                // `0.0` 只是「不可达分支也不许 panic」的兜底。
                let ts = entry_ts(e).unwrap_or(0.0);
                tagged.push((ts, si, seq, e));
            }
        }
        tagged.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        tagged
            .into_iter()
            .take(limit)
            .map(|t| t.3.clone())
            .collect()
    }

    /// 写预算。纯数据：状态进、状态出，时间从参数进。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct WriteBudget {
        /// 当前这一天窗口的起点。`0` 只是哨兵，不承载语义。
        pub day_start: i64,
        pub used: u32,
    }

    /// 全新的预算。
    pub const FRESH_BUDGET: WriteBudget = WriteBudget {
        day_start: 0,
        used: 0,
    };

    /// `at` 还在 `b` 记的那一天窗口里吗。
    ///
    /// ⚠️ `at < day_start`（**时钟回拨**）与 `at - day_start >= DAY_MS`（正常跨天）
    /// **都**判成「不在窗口内」，两者都按「新的一天」处理 ——
    /// 与本仓其余几处 `age < 0` 立刻恢复是同一套语义：
    /// 回拨立刻恢复可写，不会把预算冻结到「回拨量走完」才解封，也不会算出负的 `used`。
    fn in_window(b: WriteBudget, at: i64) -> bool {
        at >= b.day_start && at - b.day_start < DAY_MS
    }

    pub fn can_write(b: WriteBudget, at: i64, per_day: u32) -> bool {
        let used = if in_window(b, at) { b.used } else { 0 };
        used < per_day
    }

    pub fn consume(b: WriteBudget, at: i64) -> WriteBudget {
        if in_window(b, at) {
            WriteBudget {
                day_start: b.day_start,
                used: b.used.saturating_add(1),
            }
        } else {
            WriteBudget {
                day_start: at,
                used: 1,
            }
        }
    }

    /// 一条从存储里读回来的东西的 `ts`。**唯一被校验过的字段。**
    ///
    /// 判据只有一条：**是有限数字**。`serde_json` 解析出来的数字天然有限，
    /// 但进程内构造的 `Value` 可以是 `f64::NAN`/`INFINITY`，所以这里真的要判。
    pub fn entry_ts(v: &Value) -> Option<f64> {
        if !v.is_object() {
            return None;
        }
        v.get("ts")?.as_f64().filter(|f| f.is_finite())
    }

    /// 一条能不能当事件用。
    ///
    /// 🔴 **这不是「校验得不够」，是一条刻意划出来的界线**：
    ///
    /// | 档 | 处置 | 理由 |
    /// |---|---|---|
    /// | **结构性不可用**（`ts` 缺失/非数字/NaN/±Inf） | **丢** | 排不了序、当不了游标 ⇒ **面板的游标永远推不动**（稳态读吞吐从 70,560 涨到 276,480 次/天） |
    /// | **显示不好看**（`level` 不在档、`event`/`msg` 缺失、`fields` 不是对象） | **留** | 只影响某一列怎么渲染；丢掉它们就是在一次上游异常之后**把整段证据清空** |
    ///
    /// ⚠️ `level` 后端**一个字都不动**：归一化由前端那一份在跑，后端再做一次
    /// 就是同一个判据的第二份实现。代价明写：畸形 `level` 的条目在「按级别筛选」
    /// 时选不中，只在「全部级别」下可见 —— **已知限制，登记不修**。
    pub fn is_event_entry(v: &Value) -> bool {
        entry_ts(v).is_some()
    }

    /// 整个分片的窄化 + 丢弃计数。**读写两侧共用这一份。**
    ///
    /// 🔴 `raw` 不是数组（含 `None` / 字符串 / 数字 / 对象）⇒ 回空分片、
    /// `malformed` 计 **0**：键还没被写过是完全正常的路径，把它计进 `malformed`
    /// 会让一个全新部署一上来就报「有畸形数据」。
    /// **只有「分片本身是数组、里面某几条不是」才计数**，那才是需要被说出来的事。
    ///
    /// ⚠️ `malformed` **恒为 0 正是它的价值**：本仓没有任何路径能产出畸形条目，
    /// 所以它是生产环境里的对照组 —— 运维手改存储、`store.json` 被写坏时，
    /// **它是唯一能把「面板少了几条事件」和「本来就没有事件」区分开的信号**。
    pub fn narrow_shard(raw: Option<&Value>) -> (Vec<Value>, usize) {
        let Some(Value::Array(items)) = raw else {
            return (Vec::new(), 0);
        };
        let entries: Vec<Value> = items
            .iter()
            .filter(|v| is_event_entry(v))
            .cloned()
            .collect();
        let malformed = items.len() - entries.len();
        (entries, malformed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// sink 本体
// ─────────────────────────────────────────────────────────────────────────────

/// `read_events()` 的返回形状：**只归并，不过滤不截断**。
///
/// ⚠️ 「归并 → 过滤 → 截到 limit」这个顺序不能倒过来：先截断再按 level 过滤
/// 会把本该出现的旧事件漏掉。过滤与截断由 events handler 在这份结果之上做。
#[derive(Debug)]
pub struct ReadEventsResult {
    /// 归并后的条目。每一条都保证是**带有限数字 `ts` 的对象**，其余字段未经校验。
    pub items: Vec<Value>,
    /// 🔴 **这一次读**里被逐条丢掉的畸形条目数。
    ///
    /// **随每次读返回，刻意不做成实例字段**：实例字段会跨请求粘住，
    /// 两个并发请求各读一次之后，谁的 handler 先读到那个字段就说了算，
    /// 面板会看到**另一个请求的数**。
    pub malformed: usize,
}

/// sink 的自述状态。面板的「事件」板块要显示它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkStatus {
    pub shard_id: String,
    pub buffered: usize,
    /// 🔴 缓冲侧与落盘侧两类丢弃的**总数**。
    ///
    /// 只数缓冲侧那一版漏掉的是「分片攒满之后每次落盘都在静默截断」——
    /// 那是任何有持续流量的部署的稳态，而面板当时显示 `dropped: 0`。
    pub dropped: u64,
    pub budget_exhausted: bool,
}

struct State {
    buffer: Vec<Value>,
    last_flush_at: i64,
    budget: ring::WriteBudget,
    buffer_dropped: u64,
    persisted_dropped: u64,
}

/// 事件落存储的 sink。
///
/// [`Logger::log`] 是**同步**的（端口如此），所以这里**只缓冲、不落盘**。
/// 落盘由中间件在请求收尾时 `await`（`server::middleware::log_flush`，P1-b 的对象）
/// —— 写一定在响应返回前完成，`admin-events` 那 38 格里有一条直接钉着这件事。
pub struct StoreLogger {
    storage: Arc<dyn Storage>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    shard_id: String,
    /// 这个实例稳定落在哪个槽位。**构造时算一次，终生不变。**
    ///
    /// ⚠️ 每次落盘换槽位的话环形语义就退化成「每把键只有一条」。
    slot: usize,
    /// 🔴 **sink 自身故障的出口，必须是一条与存储无关的路**（通常是 ConsoleLogger）。
    ///
    /// 把「落盘失败」这条诊断再塞回同一个**正在故障的**落盘 sink 没有意义。
    /// 装配时**不要**把 fan-out 之后的那个 logger 传进来（那里面就有本 sink）。
    fallback: Arc<dyn Logger>,
    state: Mutex<State>,
}

impl StoreLogger {
    /// 生产装配用：时钟是真墙钟。
    pub fn new(
        storage: Arc<dyn Storage>,
        shard_id: impl Into<String>,
        fallback: Arc<dyn Logger>,
    ) -> Self {
        Self::with_clock(storage, shard_id, fallback, Arc::new(super::now_ms))
    }

    /// 可注入时钟。**判据要它**：TTL、最小间隔闸、写预算跨天全都要假时钟往前走。
    ///
    /// 🔴 `last_flush_at` 初值 = **构造那一刻的 `now()`**，不是哨兵。
    /// 见文件头「闸的另一半同样承重」。
    pub fn with_clock(
        storage: Arc<dyn Storage>,
        shard_id: impl Into<String>,
        fallback: Arc<dyn Logger>,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        let shard_id = shard_id.into();
        let slot = ring::slot_of(&shard_id);
        let started_at = now();
        Self {
            storage,
            now,
            shard_id,
            slot,
            fallback,
            state: Mutex::new(State {
                buffer: Vec::new(),
                last_flush_at: started_at,
                budget: ring::FRESH_BUDGET,
                buffer_dropped: 0,
                persisted_dropped: 0,
            }),
        }
    }

    /// 面板要看的自述状态。
    pub fn status(&self) -> SinkStatus {
        let at = (self.now)();
        let st = self.state.lock();
        SinkStatus {
            shard_id: self.shard_id.clone(),
            buffered: st.buffer.len(),
            dropped: st.buffer_dropped + st.persisted_dropped,
            budget_exhausted: !ring::can_write(st.budget, at, ring::EVENT_WRITES_PER_DAY),
        }
    }

    /// 到点就落盘。**`fetch` 路径只许走这一条。**
    ///
    /// 🔴 **不许改调下面的 [`StoreLogger::flush`]**：这条最小间隔闸是
    /// `admin.login_failed` 那根杠杆的唯一约束（见文件头）。
    ///
    /// `since < 0`（时钟回拨）= **立刻恢复**，与本仓其余几处同一套语义。
    pub async fn maybe_flush(&self) {
        let at = (self.now)();
        {
            let st = self.state.lock();
            if st.buffer.is_empty() {
                return;
            }
            let since = at - st.last_flush_at;
            // `since < 0` = **时钟回拨**：立刻恢复，不等回拨量走完
            // —— 与本仓其余几处 `age < 0` 是同一套语义。
            let rolled_back = since < 0;
            if !rolled_back && since < ring::EVENT_FLUSH_MIN_INTERVAL_MS {
                return;
            }
        }
        self.write_batch(at).await;
    }

    /// **无条件落盘，绕过最小间隔闸。**
    ///
    /// ⚠️ **只给「调用频率本身已经有上界」的调用方用**（TS 侧是补池收尾那两处：
    /// 定时轮与带 10 分钟冷却的手动轮）。
    ///
    /// ⚠️ **别把「还有每日预算兜着」当成这条路径的护栏** —— 在 TS 侧唯一的两个
    /// 调用点上那条约束**从不生效**：两处**每一轮都新建一个 sink 实例**，
    /// 实例一诞生就带一份全新预算 ⇒ `can_write` 恒为真（实测 60 轮全部写出）。
    /// **它的上界不在这里，在调用方的频率上。**
    ///
    /// **为什么非有它不可**（实测出来的）：补池那条路每一轮都新建 sink，
    /// 构造那一刻 `last_flush_at = now()`，而那一轮如果跑得快（健康稳态、接线错误、
    /// 预算不可能 —— 三种都是毫秒级返回），收尾 `maybe_flush()` 时
    /// `since` 远小于最小间隔 ⇒ **一条都写不出去，而且不抛不报**。
    /// 实测：24 个窗口 × 48 轮，写入 432 条、可见 **0** 条，丢失率 **100%**。
    pub async fn flush(&self) {
        let at = (self.now)();
        self.write_batch(at).await;
    }

    async fn write_batch(&self, at: i64) {
        // ── 锁内：决定这一批写不写、把状态先推进；**一个 `.await` 都不许在锁内** ──
        let batch = {
            let mut st = self.state.lock();
            if st.buffer.is_empty() {
                return;
            }
            if !ring::can_write(st.budget, at, ring::EVENT_WRITES_PER_DAY) {
                // 预算用完 ⇒ **不写**，缓冲区继续接（满了丢最旧的并计数），
                // 端点如实报 `budget_exhausted`。Console 那一路一条都不丢。
                return;
            }
            // 🔴 **窗口与预算在发起写之前就推进**：写失败时不重试同一批
            // （下一轮再攒），否则存储持续故障会让每个请求都白付一次 put。
            // 连带好处（Rust 侧独有）：两个并发的 `maybe_flush` 里第二个
            // 看到的已经是推进后的 `last_flush_at`，闸门对并发同样成立。
            st.last_flush_at = at;
            st.budget = ring::consume(st.budget, at);
            std::mem::take(&mut st.buffer)
        };

        let key = ring::shard_key(ring::window_index(at), self.slot);

        // 🔴 **写侧也要逐条窄化。** TS 版这里一度连 `Array.isArray` 都没有，
        // 两条真实后果（实测过的）：
        // ① `cur` 是**字符串**时 `[...cur]` 把它**逐字符展开**，`"abc"` 变成三条
        //    单字符条目原样写回存储 —— 不抛不报不留痕；而单字符条目没有 `ts`，
        //    **正是让读侧游标冻住的那一种**。一个标量进来，出去的是三份燃料。
        // ② `cur` 是**其它标量或对象**时 `[...cur]` 抛 `TypeError`，而缓冲区
        //    已经先清空 ⇒ **这一批永久丢失**；更糟的是 `cur.length` 是
        //    `undefined` ⇒ `persistedDropped` 变成 `NaN` 并**终生粘住** ⇒
        //    面板黄条永远不亮，序列化成 `null` ⇒ 面板读到「没有数据」。
        // ⇒ **事件缺口是一小时的（键每小时换一把），「面板说不出自己缺了东西」是终生的。**
        // Rust 侧 `narrow_shard` 把这两条一起根除：非数组一律当空分片。
        let cur = match self.storage.get(&key).await {
            Ok(v) => ring::narrow_shard(v.as_ref()).0,
            Err(e) => {
                self.report_failure(&e);
                return;
            }
        };

        let truncated = ring::truncated_count(cur.len(), batch.len(), ring::EVENT_RING_SIZE);
        self.state.lock().persisted_dropped += truncated as u64;

        let merged = ring::append_ring(&cur, &batch, ring::EVENT_RING_SIZE);
        if let Err(e) = self
            .storage
            .put(&key, Value::Array(merged), Some(ring::event_expires_at(at)))
            .await
        {
            self.report_failure(&e);
        }
    }

    /// 面板读路径。**候选键从时钟直接算出来，零 `list()`、零索引读。**
    ///
    /// 存储抛错时**如实往上抛**，不吞：events handler 要把它变成一个错误信封，
    /// 而「读不出来」与「本来就没有事件」在面板上必须分得开。
    pub async fn read_events(&self, after: Option<i64>) -> StorageResult<ReadEventsResult> {
        let keys = ring::candidate_keys((self.now)(), after);
        let mut shards: Vec<Vec<Value>> = Vec::with_capacity(keys.len());
        let mut malformed = 0usize;
        // ⚠️ 与 TS 的 `Promise.all` 的唯一差别是**并发度**（这里顺序发）：
        // 键数、顺序、归并结果逐字相同。`FileStorage` 上每次 `get` 都是整份
        // 文件读，并发也不会变便宜。
        for k in &keys {
            let raw = self.storage.get(k).await?;
            let (entries, m) = ring::narrow_shard(raw.as_ref());
            malformed += m;
            shards.push(entries);
        }
        let limit = keys.len() * ring::EVENT_RING_SIZE;
        Ok(ReadEventsResult {
            items: ring::merge_shards(&shards, limit),
            malformed,
        })
    }

    /// 🔴 事件名是运维的 grep 锚点，**一个字都不许改**。
    fn report_failure(&self, err: &StorageError) {
        self.fallback.log(
            LogEntry::new(
                LogLevel::Error,
                "storage.event_flush_failed",
                "事件落盘失败，本轮缓冲已丢弃（不重试同一批，下一轮再攒新的）",
            )
            .with("error", err.to_string()),
        );
    }
}

/// 一条 [`LogEntry`] 写到存储里长什么样。
///
/// ⚠️ **这是有兼容包袱的形状**：事件一旦开始落库，面板就按这些字段名读。
/// `fields` 恒在（哪怕是空对象）让形状稳定；`corr` 只在真有值时出现
/// （TS 侧 `corr` 是可选字段，装配里大多不传）。
fn entry_to_value(e: &LogEntry, ts: i64) -> Value {
    let mut m = Map::new();
    m.insert("ts".into(), json!(ts));
    m.insert("level".into(), json!(e.level.as_str()));
    m.insert("event".into(), json!(e.event));
    m.insert("msg".into(), json!(e.msg));
    if let Some(corr) = &e.corr {
        m.insert("corr".into(), json!(corr));
    }
    m.insert(
        "fields".into(),
        Value::Object(
            e.fields
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
    );
    Value::Object(m)
}

impl Logger for StoreLogger {
    /// 🔴 **同步、零 IO、只缓冲。** 落盘时机是显式的（中间件择机），
    /// 不是散落在每个「只想记一句」的调用点上。
    fn log(&self, entry: LogEntry) {
        // `ts` 在**入队这一刻**取，不是落盘那一刻 ——
        // 否则一整批事件会共用同一个时间戳，面板上的时间线全塌成一个点。
        let v = entry_to_value(&entry, (self.now)());
        let mut st = self.state.lock();
        st.buffer.push(v);
        if st.buffer.len() > ring::EVENT_RING_SIZE {
            // 丢**最旧**的。丢掉几条要能被面板看见 —— **静默丢弃就是撒谎。**
            st.buffer.remove(0);
            st.buffer_dropped += 1;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod ring_tests {
    use super::ring::*;
    use serde_json::{Value, json};

    /// 常量是对外可见的行为，**期望值手写字面量**，不从被测对象现算。
    #[test]
    fn constants_are_pinned_literals() {
        assert_eq!(EVENT_RING_SIZE, 100);
        assert_eq!(EVENT_FLUSH_MIN_INTERVAL_MS, 60_000);
        assert_eq!(EVENT_WRITES_PER_DAY, 12);
        assert_eq!(EVENT_WINDOW_MS, 3_600_000);
        assert_eq!(EVENT_SLOTS, 2);
        assert_eq!(EVENT_WINDOW_RETAIN, 24);
        assert_eq!(EVENT_TTL_MARGIN_MS, 3_600_000);
        assert_eq!(EVENT_KEY_PREFIX, "event:");
    }

    /// 窗口是**向下取整**，负时间戳同样。
    ///
    /// 回归对象：把 `div_euclid` 换成 `/` —— 正数全绿，1970 年之前错一格。
    #[test]
    fn window_index_floors_including_negative_times() {
        assert_eq!(window_index(0), 0);
        assert_eq!(window_index(3_599_999), 0);
        assert_eq!(window_index(3_600_000), 1);
        assert_eq!(window_index(-1), -1);
        assert_eq!(window_index(-3_600_000), -1);
        assert_eq!(window_index(-3_600_001), -2);
    }

    /// 槽位确定性、值域有界，**且长串不 panic**。
    ///
    /// 🔴 回归对象：把 `wrapping_*` 换成普通算术 —— debug 下溢出 panic、
    /// release 下回绕，**行为不一致的判据等于没有判据**。
    #[test]
    fn slot_of_is_deterministic_bounded_and_never_overflows() {
        let long = "x".repeat(4096);
        for id in ["", "a", "0f3c9a1b", "深圳", &long] {
            let s = slot_of(id);
            assert!(s < EVENT_SLOTS, "槽位越界: {id} -> {s}");
            assert_eq!(s, slot_of(id), "同一个 id 两次算出不同槽位");
        }
    }

    /// 分片键的字面形状是存储契约。
    #[test]
    fn shard_key_shape_is_pinned() {
        assert_eq!(shard_key(475_000, 1), "event:475000:1");
        assert_eq!(shard_key(-2, 0), "event:-2:0");
    }

    /// TTL = 保留期末尾 + 一整个窗口的余量。**期望值手算。**
    #[test]
    fn event_expires_at_is_retention_end_plus_one_window() {
        // at 落在窗口 1（1 小时整）⇒ (1 + 24) * 3_600_000 + 3_600_000
        assert_eq!(event_expires_at(3_600_000), 93_600_000);
        assert!(event_expires_at(0) > 0);
    }

    /// 🔴 **本函数存在的全部理由**：陈旧游标不许把候选键数撑爆。
    ///
    /// 回归对象：去掉 `floor` 钳位 —— `after = 0` 那一格会从 48 变成几十万，
    /// 实测过一次请求约 99 万次 get。
    #[test]
    fn candidate_keys_are_bounded_for_any_after() {
        let now = 1_800_000_000_000i64; // 某个正常的现代时间戳
        let cap = EVENT_WINDOW_RETAIN as usize * EVENT_SLOTS;
        assert_eq!(cap, 48);
        for after in [
            None,
            Some(0),
            Some(-1),
            Some(i64::MIN / 4),
            Some(i64::MAX / 4),
        ] {
            let n = candidate_keys(now, after).len();
            assert!(n <= cap, "after={after:?} 算出 {n} 把键，超过上界 {cap}");
        }
        // 冷读恰好是整个保留期。
        assert_eq!(candidate_keys(now, None).len(), cap);
    }

    /// 暖读只看 `after` 那个窗口到当前窗口：同窗口 = 2 把，跨一次窗口 = 4 把。
    #[test]
    fn warm_read_spans_only_the_windows_in_play() {
        let now = 100 * EVENT_WINDOW_MS + 5;
        assert_eq!(candidate_keys(now, Some(now - 1)).len(), EVENT_SLOTS);
        assert_eq!(
            candidate_keys(now, Some(now - EVENT_WINDOW_MS)).len(),
            2 * EVENT_SLOTS
        );
        // 顺序：窗口升序、槽位升序。
        assert_eq!(
            candidate_keys(now, Some(now - EVENT_WINDOW_MS)),
            vec!["event:99:0", "event:99:1", "event:100:0", "event:100:1"]
        );
    }

    /// 环形追加丢**最旧**的，且不改入参。
    #[test]
    fn append_ring_drops_the_oldest() {
        let cur: Vec<_> = (0..3).map(|i| json!({ "ts": i })).collect();
        let add: Vec<_> = (3..5).map(|i| json!({ "ts": i })).collect();
        let out = append_ring(&cur, &add, 4);
        let ts: Vec<i64> = out.iter().map(|v| v["ts"].as_i64().unwrap()).collect();
        assert_eq!(ts, vec![1, 2, 3, 4]);
        assert_eq!(cur.len(), 3, "入参不许被就地改");
    }

    /// 没超上限就原样拼。
    #[test]
    fn append_ring_keeps_everything_under_the_cap() {
        let cur = vec![json!({ "ts": 1 })];
        let add = vec![json!({ "ts": 2 })];
        assert_eq!(append_ring(&cur, &add, 100).len(), 2);
    }

    /// 截断计数是纯算术，**不允许出现负数**（TS 那边是 `Math.max(0, …)`）。
    #[test]
    fn truncated_count_saturates_at_zero() {
        assert_eq!(truncated_count(98, 5, 100), 3);
        assert_eq!(truncated_count(1, 1, 100), 0);
        assert_eq!(truncated_count(0, 0, 100), 0);
    }

    /// 归并：`ts` 降序；同 `ts` 按 `(分片, 序号)` 破平；截到 limit。
    ///
    /// 回归对象：依赖排序天然稳定 —— 同 `ts` 的两条顺序会随实现漂，
    /// 现象是「面板每次轮询顺序都在跳」。
    #[test]
    fn merge_shards_sorts_desc_and_breaks_ties_explicitly() {
        let s0 = vec![
            json!({ "ts": 5, "tag": "a" }),
            json!({ "ts": 9, "tag": "b" }),
        ];
        let s1 = vec![json!({ "ts": 5, "tag": "c" })];
        let out = merge_shards(&[s0, s1], 10);
        let tags: Vec<&str> = out.iter().map(|v| v["tag"].as_str().unwrap()).collect();
        assert_eq!(tags, vec!["b", "a", "c"]);
    }

    #[test]
    fn merge_shards_truncates_to_limit() {
        let s: Vec<_> = (0..5).map(|i| json!({ "ts": i })).collect();
        assert_eq!(merge_shards(&[s], 2).len(), 2);
        let s2: Vec<_> = (0..5).map(|i| json!({ "ts": i })).collect();
        assert!(merge_shards(&[s2], 0).is_empty());
    }

    /// 🔴 窄化的界线：`ts` 坏 ⇒ 丢；别的坏 ⇒ 留。
    #[test]
    fn narrowing_drops_only_structurally_unusable_entries() {
        let raw = json!([
            { "ts": 1, "level": "loud", "fields": "不是对象" },   // 只是不好看 ⇒ 留
            { "ts": 2 },                                          // 缺一堆字段 ⇒ 留
            { "level": "info" },                                  // 没 ts ⇒ 丢
            { "ts": "3" },                                        // ts 是字符串 ⇒ 丢
            { "ts": null },                                       // ⇒ 丢
            "裸字符串",                                            // ⇒ 丢
            42,                                                   // ⇒ 丢
            null,                                                 // ⇒ 丢
        ]);
        let (entries, malformed) = narrow_shard(Some(&raw));
        assert_eq!(entries.len(), 2, "两条只是「显示不好看」，必须留下");
        assert_eq!(malformed, 6);
    }

    /// `NaN` / `±Inf` 同样是结构性不可用。
    ///
    /// 回归对象：只判 `is_number()` —— 进程内构造的 `NaN` 会漏进去，
    /// 而它正是让面板游标冻住的那一种。
    #[test]
    fn non_finite_ts_is_not_an_event() {
        assert!(!is_event_entry(&json!({ "ts": f64::NAN })));
        assert!(!is_event_entry(&json!({ "ts": f64::INFINITY })));
        assert!(is_event_entry(&json!({ "ts": 0 })));
        assert!(is_event_entry(&json!({ "ts": -1.5 })));
    }

    /// 🔴 **键还没被写过不是「畸形」**：`malformed` 必须是 0，
    /// 否则一个全新部署一上来就报「有畸形数据」。
    #[test]
    fn absent_or_non_array_shard_reports_zero_malformed() {
        for raw in [
            None,
            Some(&Value::Null),
            Some(&json!("abc")),
            Some(&json!({ "a": 1 })),
        ] {
            let (entries, malformed) = narrow_shard(raw);
            assert!(entries.is_empty());
            assert_eq!(malformed, 0, "raw={raw:?} 不该被计成畸形");
        }
    }

    /// 预算：窗口内累加，跨天重置。
    #[test]
    fn budget_counts_within_a_day_and_resets_across_it() {
        let mut b = FRESH_BUDGET;
        assert!(can_write(b, 1_000, 2));
        b = consume(b, 1_000);
        assert!(can_write(b, 1_000, 2));
        b = consume(b, 1_000);
        assert!(!can_write(b, 1_000, 2), "用满就不许再写");
        assert!(can_write(b, 1_000 + DAY_MS, 2), "跨天必须重置");
    }

    /// ⚠️ **时钟回拨立刻恢复可写**，不把预算冻结到回拨量走完。
    ///
    /// 回归对象：`in_window` 只判上界 —— 回拨之后 `at - day_start` 是负数，
    /// 会被当成「还在窗口内」，于是 `used` 继续锁死。
    #[test]
    fn clock_rollback_immediately_restores_the_budget() {
        // ⚠️ 起点必须是一个**现代**时间戳：`FRESH_BUDGET` 的 `day_start` 是 0，
        //    小时间戳会落在那个哨兵窗口里，测的就不是想测的那件事了。
        let t = 1_800_000_000_000i64;
        let mut b = FRESH_BUDGET;
        for _ in 0..EVENT_WRITES_PER_DAY {
            b = consume(b, t);
        }
        assert_eq!(b.day_start, t, "第一次 consume 必须把哨兵换成真实的 at");
        assert!(!can_write(b, t, EVENT_WRITES_PER_DAY));
        assert!(
            can_write(b, t - 1_000, EVENT_WRITES_PER_DAY),
            "回拨之后必须立刻可写"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use serde_json::json;

    /// 记录型 fallback：观察 sink 自身故障有没有被如实说出来。
    #[derive(Default)]
    struct Recording(Mutex<Vec<LogEntry>>);

    impl Recording {
        fn events(&self) -> Vec<&'static str> {
            self.0.lock().iter().map(|e| e.event).collect()
        }
    }

    impl Logger for Recording {
        fn log(&self, entry: LogEntry) {
            self.0.lock().push(entry);
        }
    }

    /// 可推进的假时钟。**必须能往前走** —— 冻结时钟下 TTL 与间隔闸类变异
    /// 的结果是「挂起」而不是「失败」（04 §3.3 第 ④ 条假阳性）。
    #[derive(Clone)]
    struct Clock(Arc<Mutex<i64>>);

    impl Clock {
        fn new(at: i64) -> Self {
            Self(Arc::new(Mutex::new(at)))
        }
        fn advance(&self, ms: i64) {
            *self.0.lock() += ms;
        }
        fn set(&self, at: i64) {
            *self.0.lock() = at;
        }
        fn as_fn(&self) -> Arc<dyn Fn() -> i64 + Send + Sync> {
            let c = self.0.clone();
            Arc::new(move || *c.lock())
        }
    }

    /// 起点选在一个正常的现代时间戳上，且**不是窗口边界**。
    const T0: i64 = 1_800_000_000_123;

    struct Rig {
        sink: StoreLogger,
        clock: Clock,
        counts: Arc<crate::adapters::storage_test_doubles::Counts>,
        mem: Arc<MemoryStorage>,
        fallback: Arc<Recording>,
    }

    fn rig() -> Rig {
        let mem = Arc::new(MemoryStorage::new());
        let counting = Arc::new(CountingStorage::new(mem.clone()));
        let counts = counting.counts.clone();
        let clock = Clock::new(T0);
        let fallback = Arc::new(Recording::default());
        let sink = StoreLogger::with_clock(counting, "0f3c9a1b", fallback.clone(), clock.as_fn());
        Rig {
            sink,
            clock,
            counts,
            mem,
            fallback,
        }
    }

    fn ev(msg: &str) -> LogEntry {
        LogEntry::new(LogLevel::Warn, "admin.login_failed", msg)
    }

    /// 🔴 `log()` 是同步的、**零存储 I/O**：只缓冲。
    ///
    /// 回归对象：有人给 sink 加一条「攒够就顺手写」的捷径 ——
    /// 那等于把落盘时机交还给调用方，而最小间隔闸挡的正是这件事。
    /// 🔴 四个桶全数，不是只数 `get`。
    #[tokio::test]
    async fn log_buffers_and_touches_storage_zero_times() {
        let r = rig();
        for i in 0..50 {
            r.sink.log(ev(&format!("第{i}条")));
        }
        assert!(
            r.counts.is_silent(),
            "log() 不许碰存储: {:?}",
            r.counts.snapshot()
        );
        assert_eq!(r.sink.status().buffered, 50);
        assert_eq!(r.sink.status().dropped, 0);
    }

    /// 缓冲满了丢**最旧**的，而且**丢了几条要数得出来**。
    #[tokio::test]
    async fn buffer_overflow_drops_oldest_and_counts_it() {
        let r = rig();
        for i in 0..(ring::EVENT_RING_SIZE + 7) {
            r.sink.log(ev(&format!("m{i}")));
        }
        let st = r.sink.status();
        assert_eq!(st.buffered, ring::EVENT_RING_SIZE);
        assert_eq!(st.dropped, 7);

        // 丢的是最旧的：落盘之后读回来，最早那几条不在。
        r.sink.flush().await;
        let got = r.sink.read_events(None).await.unwrap();
        let msgs: Vec<&str> = got
            .items
            .iter()
            .map(|v| v["msg"].as_str().unwrap())
            .collect();
        // 记了 107 条、环上限 100 ⇒ 被丢的恰好是最旧的 m0..m6，m7 是第一条幸存者。
        assert!(!msgs.contains(&"m0"), "最旧的必须被丢掉");
        assert!(!msgs.contains(&"m6"), "m6 是最后一条被丢的");
        assert!(msgs.contains(&"m7"), "m7 是第一条幸存者");
        assert_eq!(msgs.len(), ring::EVENT_RING_SIZE);
    }

    /// 🔴 **冷启动的第一次 `maybe_flush` 同样受闸门约束。**
    ///
    /// 回归对象：`last_flush_at` 初值写成哨兵（`None` / `0` / `i64::MIN`）——
    /// 那等于**每次进程冷启动送一次零门槛写**，而 `admin.login_failed`
    /// 是任何未鉴权请求都能触发的，攻击者按批重启不了进程，但**按批触发
    /// 新建 sink 的那条路**同理。
    #[tokio::test]
    async fn cold_start_first_maybe_flush_is_throttled_too() {
        let r = rig();
        r.sink.log(ev("一"));
        r.sink.maybe_flush().await;
        assert!(r.counts.is_silent(), "冷启动首刷必须被闸住");
        assert_eq!(r.sink.status().buffered, 1, "缓冲不许被闸门吃掉");
    }

    /// 🔴 最小间隔闸：不到 60 秒不写，到了就写。
    ///
    /// **边界两侧都测**：`59_999` 不写、`60_000` 写。
    #[tokio::test]
    async fn min_interval_gate_holds_at_the_boundary() {
        let r = rig();
        r.sink.log(ev("一"));
        r.clock.advance(ring::EVENT_FLUSH_MIN_INTERVAL_MS - 1);
        r.sink.maybe_flush().await;
        assert_eq!(r.counts.snapshot().1, 0, "差 1 毫秒就不许写");

        r.clock.advance(1);
        r.sink.maybe_flush().await;
        assert_eq!(r.counts.snapshot().1, 1, "到点必须写");
        assert_eq!(r.sink.status().buffered, 0);
    }

    /// 🔴 挨得很近的两次收尾**只落盘一次**（杠杆那条判据的直接形态）。
    #[tokio::test]
    async fn two_close_requests_flush_only_once() {
        let r = rig();
        r.sink.log(ev("一"));
        r.clock.advance(ring::EVENT_FLUSH_MIN_INTERVAL_MS);
        r.sink.maybe_flush().await;

        r.sink.log(ev("二"));
        r.clock.advance(50);
        r.sink.maybe_flush().await;

        assert_eq!(r.counts.snapshot().1, 1, "第二次必须被闸住");
        assert_eq!(r.sink.status().buffered, 1, "第二条还在缓冲里等下一轮");
    }

    /// `flush()` **绕过闸门**。这是它存在的全部理由。
    ///
    /// 回归对象：有人为了「统一」把 `flush` 改成调 `maybe_flush` ——
    /// 实测后果是补池那条路 432 条写入、可见 0 条，丢失率 100%。
    #[tokio::test]
    async fn flush_bypasses_the_min_interval_gate() {
        let r = rig();
        r.sink.log(ev("一"));
        r.sink.flush().await; // 距构造 0 毫秒
        assert_eq!(r.counts.snapshot().1, 1);
        assert_eq!(r.sink.status().buffered, 0);
    }

    /// 空缓冲不产生任何存储操作（连 `get` 都不许）。
    #[tokio::test]
    async fn flushing_an_empty_buffer_is_free() {
        let r = rig();
        r.sink.flush().await;
        r.clock.advance(ring::EVENT_FLUSH_MIN_INTERVAL_MS * 10);
        r.sink.maybe_flush().await;
        assert!(r.counts.is_silent(), "{:?}", r.counts.snapshot());
    }

    /// ⚠️ 时钟回拨（`since < 0`）**立刻恢复**，不等回拨量走完。
    #[tokio::test]
    async fn clock_rollback_lets_maybe_flush_proceed_immediately() {
        let r = rig();
        r.sink.log(ev("一"));
        r.clock.set(T0 - 5_000);
        r.sink.maybe_flush().await;
        assert_eq!(r.counts.snapshot().1, 1, "回拨之后必须立刻能落盘");
    }

    /// 🟡 每日写预算：满 12 次就停，且**缓冲区继续接、端点如实报**。
    ///
    /// Console 那一路一条都不丢，排障能力不受影响 —— 这一格只钉存储那一侧。
    #[tokio::test]
    async fn daily_write_budget_stops_writing_but_keeps_buffering() {
        let r = rig();
        for i in 0..ring::EVENT_WRITES_PER_DAY {
            r.sink.log(ev(&format!("m{i}")));
            r.sink.flush().await;
        }
        assert_eq!(r.counts.snapshot().1, u64::from(ring::EVENT_WRITES_PER_DAY));
        assert!(r.sink.status().budget_exhausted);

        r.sink.log(ev("超预算的那条"));
        r.sink.flush().await;
        assert_eq!(
            r.counts.snapshot().1,
            u64::from(ring::EVENT_WRITES_PER_DAY),
            "预算用完之后不许再写"
        );
        assert_eq!(r.sink.status().buffered, 1, "缓冲区必须继续接");

        // 跨天之后恢复。
        r.clock.advance(ring::DAY_MS);
        r.sink.flush().await;
        assert_eq!(
            r.counts.snapshot().1,
            u64::from(ring::EVENT_WRITES_PER_DAY) + 1
        );
        assert!(!r.sink.status().budget_exhausted);
    }

    /// 🔴 **TS 缺陷 ①**：存储里那把键是**字符串**时，绝不许被逐字符展开。
    ///
    /// 回归对象：把 `narrow_shard` 换成「直接当数组用」。TS 那边 `[..."abc"]`
    /// 会写回三条单字符条目 —— 不抛不报不留痕，而单字符条目没有 `ts`，
    /// **正是让读侧游标冻住的那一种**。
    #[tokio::test]
    async fn a_string_shard_is_never_expanded_char_by_char() {
        let r = rig();
        let key = ring::shard_key(ring::window_index(T0), ring::slot_of("0f3c9a1b"));
        r.mem.put(&key, json!("abc"), None).await.unwrap();

        r.sink.log(ev("真事件"));
        r.sink.flush().await;

        let stored = r.mem.get(&key).await.unwrap().unwrap();
        let arr = stored.as_array().expect("落盘结果必须是数组");
        assert_eq!(arr.len(), 1, "脏字符串不许变成三条单字符条目: {arr:?}");
        assert_eq!(arr[0]["msg"], json!("真事件"));
    }

    /// 🔴 **TS 缺陷 ②**：存储里那把键是标量/对象时，既不许 panic，
    /// 也不许把这一批永久丢掉，更不许把 `dropped` 污染成一个终生粘住的坏值。
    #[tokio::test]
    async fn a_scalar_or_object_shard_neither_panics_nor_eats_the_batch() {
        for junk in [json!(42), json!({ "a": 1 }), Value::Null, json!(true)] {
            let r = rig();
            let key = ring::shard_key(ring::window_index(T0), ring::slot_of("0f3c9a1b"));
            r.mem.put(&key, junk.clone(), None).await.unwrap();

            r.sink.log(ev("真事件"));
            r.sink.flush().await;

            let stored = r.mem.get(&key).await.unwrap().unwrap();
            let arr = stored.as_array().expect("落盘结果必须是数组");
            assert_eq!(arr.len(), 1, "junk={junk} 时这一批被吃掉了");
            assert_eq!(
                r.sink.status().dropped,
                0,
                "junk={junk} 时 dropped 被污染了"
            );
        }
    }

    /// 存储里那把键是数组、里面混了畸形条目 ⇒ **逐条剔掉**，好条目原样保留。
    #[tokio::test]
    async fn malformed_entries_inside_a_shard_are_dropped_one_by_one() {
        let r = rig();
        let key = ring::shard_key(ring::window_index(T0), ring::slot_of("0f3c9a1b"));
        r.mem
            .put(
                &key,
                json!([{ "ts": 1, "msg": "老的" }, "脏", { "no_ts": true }]),
                None,
            )
            .await
            .unwrap();

        r.sink.log(ev("新的"));
        r.sink.flush().await;

        let stored = r.mem.get(&key).await.unwrap().unwrap();
        let arr = stored.as_array().unwrap();
        assert_eq!(arr.len(), 2, "一条老的 + 一条新的: {arr:?}");
        assert_eq!(arr[0]["msg"], json!("老的"));
        assert_eq!(arr[1]["msg"], json!("新的"));
    }

    /// 🔴 分片攒满之后的**静默截断要被数出来**。
    ///
    /// 回归对象：`dropped` 只数缓冲侧 —— 有持续流量的部署稳态下每次落盘
    /// 都在截断，而面板显示 `dropped: 0`。
    #[tokio::test]
    async fn ring_truncation_on_persist_is_counted_into_dropped() {
        let r = rig();
        let key = ring::shard_key(ring::window_index(T0), ring::slot_of("0f3c9a1b"));
        let full: Vec<Value> = (0..ring::EVENT_RING_SIZE as i64)
            .map(|i| json!({ "ts": i, "msg": "旧" }))
            .collect();
        r.mem.put(&key, Value::Array(full), None).await.unwrap();

        for i in 0..3 {
            r.sink.log(ev(&format!("新{i}")));
        }
        r.sink.flush().await;

        assert_eq!(r.sink.status().dropped, 3, "截掉的 3 条必须进 dropped");
        let arr = r.mem.get(&key).await.unwrap().unwrap();
        assert_eq!(arr.as_array().unwrap().len(), ring::EVENT_RING_SIZE);
    }

    /// 落盘失败 ⇒ 走 fallback 如实报，**且不重试同一批**（缓冲已清）。
    ///
    /// 事件名是 grep 锚点，钉成字面量。
    #[tokio::test]
    async fn a_failed_flush_reports_through_fallback_and_does_not_retry() {
        let r = rig();
        r.sink.log(ev("一"));
        r.mem.fail_next("磁盘炸了");
        r.sink.flush().await;

        assert_eq!(r.fallback.events(), vec!["storage.event_flush_failed"]);
        assert_eq!(r.sink.status().buffered, 0, "这一批不重试，下一轮再攒");

        // 下一轮照常能写（故障不许把 sink 毒死）。
        r.sink.log(ev("二"));
        r.sink.flush().await;
        let got = r.sink.read_events(None).await.unwrap();
        assert_eq!(got.items.len(), 1);
        assert_eq!(got.items[0]["msg"], json!("二"));
    }

    /// 🔴 **落盘失败不许把错误抛给调用方。** 日志是旁路。
    ///
    /// 这一格钉的是「`flush()` 返回了」这件事本身 —— 它没有 `Result`，
    /// 回归对象是有人把签名改成 `-> StorageResult<()>` 并让中间件 `?` 掉。
    #[tokio::test]
    async fn a_failed_flush_never_becomes_the_callers_problem() {
        let r = rig();
        r.sink.log(ev("一"));
        r.mem.fail_next("磁盘炸了");
        r.sink.flush().await; // 不 panic、不返回 Err —— 编译期就保证了
        assert!(!r.fallback.events().is_empty());
    }

    /// 写出去的形状是有兼容包袱的契约：字段名与级别串逐字钉住。
    #[tokio::test]
    async fn persisted_entry_shape_is_pinned() {
        let r = rig();
        r.sink.log(
            LogEntry::new(LogLevel::Error, "pool.load_failed", "炸了")
                .with("count", 3)
                .with("kind", "timeout"),
        );
        r.sink.flush().await;

        let got = r.sink.read_events(None).await.unwrap();
        let e = &got.items[0];
        assert_eq!(e["ts"], json!(T0));
        assert_eq!(e["level"], json!("error"));
        assert_eq!(e["event"], json!("pool.load_failed"));
        assert_eq!(e["msg"], json!("炸了"));
        assert_eq!(e["fields"], json!({ "count": 3, "kind": "timeout" }));
        assert!(e.get("corr").is_none(), "没有 corr 时不许写一个 null 进去");
    }

    /// `ts` 在**入队那一刻**取，不是落盘那一刻。
    ///
    /// 回归对象：在 `write_batch` 里统一补 `ts` —— 一整批事件会共用同一个
    /// 时间戳，面板上的时间线塌成一个点，而游标也跟着失去分辨率。
    #[tokio::test]
    async fn ts_is_stamped_at_enqueue_time_not_at_flush_time() {
        let r = rig();
        r.sink.log(ev("一"));
        r.clock.advance(1_000);
        r.sink.log(ev("二"));
        r.clock.advance(ring::EVENT_FLUSH_MIN_INTERVAL_MS);
        r.sink.flush().await;

        let got = r.sink.read_events(None).await.unwrap();
        let ts: Vec<i64> = got
            .items
            .iter()
            .map(|v| v["ts"].as_i64().unwrap())
            .collect();
        assert_eq!(ts, vec![T0 + 1_000, T0], "降序，且两条时间戳必须不同");
    }

    /// 读侧：畸形条目**逐条丢，而且丢了几条要数得出来**，
    /// 🔴 **并且这个数不许跨读粘住**。
    ///
    /// 回归对象：把 `malformed` 做成实例字段 —— 两个并发请求各读一次之后，
    /// 面板会看到另一个请求的数。
    #[tokio::test]
    async fn read_side_counts_malformed_per_read_and_never_sticks() {
        let r = rig();
        let key = ring::shard_key(ring::window_index(T0), ring::slot_of("0f3c9a1b"));
        r.mem
            .put(
                &key,
                json!([{ "ts": 1 }, "脏一", "脏二", { "no_ts": 1 }]),
                None,
            )
            .await
            .unwrap();

        let first = r.sink.read_events(None).await.unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.malformed, 3);

        // 换一把干净的键再读：这一次必须是 0，不许继承上一次的 3。
        r.mem.delete(&key).await.unwrap();
        let second = r.sink.read_events(None).await.unwrap();
        assert!(second.items.is_empty());
        assert_eq!(second.malformed, 0);
    }

    /// 🔴 读路径**零 `list()`**，且候选键数对陈旧游标有硬上界。
    ///
    /// 回归对象：用 `list("event:")` 找键 —— 那正是被推翻的索引形态；
    /// 以及去掉 `candidate_keys` 的钳位 —— 实测一次请求约 99 万次 get。
    #[tokio::test]
    async fn read_path_never_lists_and_is_bounded_by_a_stale_cursor() {
        let r = rig();
        let _ = r.sink.read_events(Some(0)).await.unwrap();
        let (get, put, delete, list) = r.counts.snapshot();
        assert_eq!(list, 0, "读路径不许 list");
        assert_eq!(put, 0);
        assert_eq!(delete, 0);
        assert!(get <= 48, "陈旧游标打出了 {get} 次 get，上界是 48");
    }

    /// 🔴 有界性是**存储自己的性质**：落盘时必须带上 TTL。
    ///
    /// 回归对象：`put` 第三参传 `None` —— `store.json` 里的 `event:*` 从此
    /// 只增不减，而这正是 TS 那两版被推翻的形态要根治的东西。
    #[tokio::test]
    async fn persisted_shards_carry_a_ttl() {
        let r = rig();
        r.sink.log(ev("一"));
        r.sink.flush().await;
        let key = ring::shard_key(ring::window_index(T0), ring::slot_of("0f3c9a1b"));

        // 保留期内看得见。
        r.mem.set_clock(T0);
        assert!(r.mem.get(&key).await.unwrap().is_some());

        // 过了 TTL 就看不见了（`get` 与 `list` 都看不见，这是端口契约）。
        r.mem.set_clock(ring::event_expires_at(T0) + 1);
        assert!(r.mem.get(&key).await.unwrap().is_none(), "TTL 必须真的生效");
        assert!(r.mem.list(ring::EVENT_KEY_PREFIX).await.unwrap().is_empty());
    }

    /// 同一个实例的连续多次落盘必须稳定落在**同一把键**上，
    /// 否则环形累积退化成「每把键只有一条」。
    #[tokio::test]
    async fn consecutive_flushes_land_on_the_same_shard_key() {
        let r = rig();
        r.sink.log(ev("一"));
        r.sink.flush().await;
        r.sink.log(ev("二"));
        r.sink.flush().await;

        let keys = r.mem.list(ring::EVENT_KEY_PREFIX).await.unwrap();
        assert_eq!(keys.len(), 1, "同一个窗口内不许开第二把键: {keys:?}");
        let arr = r.mem.get(&keys[0]).await.unwrap().unwrap();
        assert_eq!(arr.as_array().unwrap().len(), 2, "第二次必须追加而不是覆盖");
    }

    /// 跨时间窗之后写到新键，而读路径（冷读）把两个窗口一起归并回来。
    #[tokio::test]
    async fn events_span_windows_and_are_merged_back_on_a_cold_read() {
        let r = rig();
        r.sink.log(ev("旧窗口"));
        r.sink.flush().await;
        r.clock.advance(ring::EVENT_WINDOW_MS);
        r.sink.log(ev("新窗口"));
        r.sink.flush().await;

        assert_eq!(r.mem.list(ring::EVENT_KEY_PREFIX).await.unwrap().len(), 2);
        let got = r.sink.read_events(None).await.unwrap();
        let msgs: Vec<&str> = got
            .items
            .iter()
            .map(|v| v["msg"].as_str().unwrap())
            .collect();
        assert_eq!(msgs, vec!["新窗口", "旧窗口"], "最新在前");
    }

    /// 存储读不出来时**如实往上抛**，不吞成「本来就没有事件」。
    #[tokio::test]
    async fn read_events_surfaces_storage_failures() {
        let r = rig();
        r.mem.fail_next("读不动");
        let Err(err) = r.sink.read_events(None).await else {
            panic!("存储读失败必须如实往上抛，不许吞成「本来就没有事件」");
        };
        assert!(err.to_string().contains("读不动"));
    }

    /// `status()` 如实报 shard、缓冲量、丢弃总数。
    #[tokio::test]
    async fn status_reports_shard_and_both_kinds_of_drops() {
        let r = rig();
        assert_eq!(
            r.sink.status(),
            SinkStatus {
                shard_id: "0f3c9a1b".into(),
                buffered: 0,
                dropped: 0,
                budget_exhausted: false,
            }
        );
    }
}
