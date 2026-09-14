//! 用量统计的**写端**。
//!
//! 移植自 TS 版 `src/http/usage-sink.ts`。读端在 [`crate::admin::usage`]，
//! 两侧共用同一份键格式与桶字段 —— **这里产出的东西必须与那边读的逐字段对上**。
//!
//! # 🔴 统计是旁路，绝不影响响应
//!
//! 这一层的**任何**失败都只落进一条事件，不往调用方抛、不改任何响应。
//! 用量数字错了是一件要修的事；**因为用量数字而让一次真实的转发失败，是一件更糟的事**。

use crate::admin::usage::{
    USAGE_SLOTS, UsageBucket, usage_day_index, usage_day_key, usage_expires_at,
};
use crate::ports::logger::{LogEntry, LogLevel, Logger};
use crate::ports::storage::Storage;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// 一次转发的用量。
#[derive(Debug, Clone)]
pub struct UsageOutcome {
    pub ok: bool,
    pub stream: bool,
    pub latency_ms: i64,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub model: String,
    pub protocol: &'static str,
    /// 归属到哪一把对外密钥。
    ///
    /// ⚠️ **`None` 与空串是同一件事** —— 一个空字符串键在面板上是**一格没有名字的行**。
    pub api_key_id: Option<String>,
}

/// `byModel` 那一维最多留几个键。
///
/// 🔴 **模型名是客户端随便填的** —— 不设上界时，一个乱填模型名的客户端
/// 就能把这份日累加器撑成任意大，而它每次落盘都要整份写出去。
pub const USAGE_MODEL_MAX_KEYS: usize = 32;

/// `byApiKey` 那一维最多留几个键。
///
/// ⚠️ **与 [`USAGE_MODEL_MAX_KEYS`] 不是同一个数**（201 vs 32）：
/// 这一维的键是**我们自己发的**（密钥 id 或那三个保留串），客户端选不了自己落进哪一格，
/// 所以上界按「一个部署合理能签发多少把密钥」定，而不是按「防乱填」定。
pub const USAGE_APIKEY_MAX_KEYS: usize = 201;

/// 没归属到任何密钥时落进的那一格。
pub const USAGE_UNATTRIBUTED_BUCKET: &str = "__unattributed__";
/// 超出上界之后落进的那一格。
pub const USAGE_OTHER_BUCKET: &str = "__other__";

/// 两条报错通道。**措辞必须分开。**
///
/// 🔴 `record` 那一条说的是「**这一条计数永久丢了**」——
/// 它不会被下一次落盘补上。共用 flush 那条事件等于**对运维说一句假的承诺**。
pub const EVENT_RECORD_FAILED: &str = "usage.record_failed";
pub const EVENT_FLUSH_FAILED: &str = "usage.flush_failed";

/// 落盘间隔的默认值。
///
/// ⚠️ **这个值就是「未落盘的尾巴最长多久」的上界。** 调小它不会更准，
/// 只会让写更频繁 —— 而用量是**旁路**，它的写应当让路给转发。
pub const USAGE_FLUSH_DEFAULT_MS: i64 = 7_200_000; // 2 小时

/// 一天的累加器。
#[derive(Debug, Clone, Default)]
struct DayAcc {
    total: UsageBucket,
    hours: BTreeMap<u8, UsageBucket>,
    by_model: BTreeMap<String, UsageBucket>,
    by_protocol: BTreeMap<String, UsageBucket>,
    by_api_key: BTreeMap<String, UsageBucket>,
}

/// 把一次结果加进一个桶。
fn add(b: UsageBucket, u: &UsageOutcome) -> UsageBucket {
    b.plus(UsageBucket {
        requests: 1,
        success: i64::from(u.ok),
        errors: i64::from(!u.ok),
        // ⚠️ **只有非流式、且协议路由解析过响应体的那几条**才有 token。
        tokens_in: u.tokens_in,
        tokens_out: u.tokens_out,
        streaming_requests: i64::from(u.stream),
        // ⚠️ **只累计成功请求的延迟。**
        //
        // 上一版对失败请求同样 `+1` —— 后果不是措辞问题：
        // 一个 8 秒超时和一个 30 毫秒的 401 都会进平均延迟，
        // 于是面板上那个「平均延迟」**不是任何真实的东西**。
        // 失败请求的耗时由「错误率」那张卡负责，不掺进延迟。
        latency_sum: if u.ok { u.latency_ms } else { 0 },
        latency_count: i64::from(u.ok),
    })
}

/// 把一个键收进上界之内。
///
/// 🔴 **这一步必须排在所有写入之前** —— 先写再判的话，那一次的计数已经落进了
/// 一个越界的键，而随后的「归并到 `__other__`」并不会把它取回来。
fn bound_key(map: &BTreeMap<String, UsageBucket>, raw: &str, max: usize) -> String {
    if map.contains_key(raw) || map.len() < max {
        raw.to_string()
    } else {
        USAGE_OTHER_BUCKET.to_string()
    }
}

fn bucket_json(b: UsageBucket) -> Value {
    json!({
        "requests": b.requests,
        "success": b.success,
        "errors": b.errors,
        "tokensIn": b.tokens_in,
        "tokensOut": b.tokens_out,
        "streamingRequests": b.streaming_requests,
        "latencySum": b.latency_sum,
        "latencyCount": b.latency_count,
    })
}

fn record_json(m: &BTreeMap<String, UsageBucket>) -> Value {
    Value::Object(
        m.iter()
            .map(|(k, v)| (k.clone(), bucket_json(*v)))
            .collect::<Map<_, _>>(),
    )
}

impl DayAcc {
    fn to_json(&self) -> Value {
        let hours = Value::Object(
            self.hours
                .iter()
                .map(|(h, v)| (h.to_string(), bucket_json(*v)))
                .collect::<Map<_, _>>(),
        );
        json!({
            "total": bucket_json(self.total),
            "hours": hours,
            "byModel": record_json(&self.by_model),
            "byProtocol": record_json(&self.by_protocol),
            "byApiKey": record_json(&self.by_api_key),
        })
    }
}

struct Inner {
    /// UTC 日序号 → 那一天的**累计**值。
    ///
    /// ⚠️ **落盘成功之后这里不清零**（同一天的下一次落盘要把整份再覆写一遍），
    /// 跨天之后旧日的累加器也仍然留着。
    /// **上界是这个进程的存活天数**：跑满一年也就 365 个小对象。
    /// 为它加一条清理路径要在热路径上多一次判断，**换不回任何可观测的东西** ——
    /// 明写在这里，免得下一个人以为它「只保留有未落盘增量的那些日」。
    days: HashMap<i64, DayAcc>,
    /// 有未落盘增量的日序号。落盘成功一个就删一个。
    dirty: HashSet<i64>,
    /// 日序号 → 被 `record` 改过多少次。
    ///
    /// ⚠️ **只用来判「我发起写的那一刻之后，有没有新的增量进来」**，不参与任何计数。
    version: HashMap<i64, u64>,
    pending: u64,
    last_flush_at: i64,
}

/// 用量 sink。
pub struct UsageSink {
    inner: Mutex<Inner>,
    storage: Arc<dyn Storage>,
    logger: Arc<dyn Logger>,
    /// 这个进程的分片槽位。**每进程一次、终生不变** ——
    /// 多副本各写各的槽位，读端把它们相加。
    slot: usize,
    flush_interval_ms: i64,
}

impl UsageSink {
    pub fn new(
        storage: Arc<dyn Storage>,
        logger: Arc<dyn Logger>,
        shard_id: &str,
        flush_interval_ms: i64,
        now: i64,
    ) -> Self {
        // 分片槽位由 shard_id 稳定地散出来 —— 同一个进程重启后落在同一格，
        // 于是它**覆写自己那一份**而不是再多占一格。
        let slot = shard_id
            .bytes()
            .fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize))
            % USAGE_SLOTS;
        Self {
            inner: Mutex::new(Inner {
                days: HashMap::new(),
                dirty: HashSet::new(),
                version: HashMap::new(),
                pending: 0,
                last_flush_at: now,
            }),
            storage,
            logger,
            slot,
            flush_interval_ms,
        }
    }

    /// 记一次。
    ///
    /// 🔴 **绝不往外抛。** 统计是旁路 —— 说一声，不静默吞掉。
    pub fn record(&self, u: &UsageOutcome, at: i64) {
        let day = usage_day_index(at);
        let hour = u8::try_from((at.rem_euclid(86_400_000)) / 3_600_000).unwrap_or(0);

        let mut g = self.inner.lock();
        let acc = g.days.entry(day).or_default();

        // 🔴 **两处收上界都排在所有写入之前**（见 `bound_key`）。
        let model_key = bound_key(&acc.by_model, &u.model, USAGE_MODEL_MAX_KEYS);
        // ⚠️ **归属的兜底就在这一行，不在四条路由上。**
        //   `None` 只在「有路绕过了鉴权中间件」时出现，今天结构上不可达 ——
        //   留着它是因为**那条不可达性是装配的性质，不是类型的性质**。
        //   ⚠️ 空串一并归到这里。
        let raw_key_id = match u.api_key_id.as_deref() {
            Some(s) if !s.is_empty() => s,
            _ => USAGE_UNATTRIBUTED_BUCKET,
        };
        let api_key_key = bound_key(&acc.by_api_key, raw_key_id, USAGE_APIKEY_MAX_KEYS);

        acc.total = add(acc.total, u);
        let h = acc.hours.entry(hour).or_default();
        *h = add(*h, u);
        let m = acc.by_model.entry(model_key).or_default();
        *m = add(*m, u);
        // `byProtocol` **刻意不收上界**：它的值是四条路由里的字面量，外部碰不到。
        let p = acc.by_protocol.entry(u.protocol.to_string()).or_default();
        *p = add(*p, u);
        let k = acc.by_api_key.entry(api_key_key).or_default();
        *k = add(*k, u);

        g.dirty.insert(day);
        *g.version.entry(day).or_insert(0) += 1;
        g.pending += 1;
    }

    /// 到点了就落一次盘。
    ///
    /// ⚠️ **失败的那一次也把 `last_flush_at` 推到当下** —— 那是**节流**要的语义：
    /// 不推的话存储一直失败时**每个请求都会重试一次**，白烧延迟。
    /// ⇒ 那两种失败态由另外两个字段说话（`pending > 0`）。
    pub async fn maybe_flush(&self, now: i64) {
        let due = {
            let mut g = self.inner.lock();
            if g.dirty.is_empty() || now - g.last_flush_at < self.flush_interval_ms {
                false
            } else {
                g.last_flush_at = now;
                true
            }
        };
        if due {
            self.flush(now).await;
        }
    }

    /// 立刻落一次盘。
    pub async fn flush(&self, now: i64) {
        // 先把要写的快照与版本号一起取出来 —— **锁不跨 await**。
        let todo: Vec<(i64, Value, u64)> = {
            let g = self.inner.lock();
            g.dirty
                .iter()
                .filter_map(|d| {
                    let acc = g.days.get(d)?;
                    Some((*d, acc.to_json(), g.version.get(d).copied().unwrap_or(0)))
                })
                .collect()
        };

        for (day, body, ver) in todo {
            let key = usage_day_key(day, self.slot);
            let res = self
                .storage
                .put(&key, body, Some(usage_expires_at(day)))
                .await;
            match res {
                Ok(()) => {
                    let mut g = self.inner.lock();
                    // 🔴 **只有版本号没变时才算干净。**
                    //
                    // 我发起这次写之后可能又有新的 `record` 进来 ——
                    // 那一份增量**不在我刚写出去的那个快照里**。
                    // 无条件清 dirty 会让它**永远落不了盘**，而计数看起来一切正常。
                    if g.version.get(&day).copied().unwrap_or(0) == ver {
                        g.dirty.remove(&day);
                        g.pending = 0;
                    }
                }
                Err(e) => {
                    // ⚠️ **dirty 保留** —— 下一次落盘会把整份再写一遍，这一次没丢东西。
                    self.logger.log(LogEntry::new(
                        LogLevel::Warn,
                        EVENT_FLUSH_FAILED,
                        format!("用量落盘失败，下一次会把整份再写一遍：{e}"),
                    ));
                }
            }
        }
        let _ = now;
    }

    /// 面板要看的自述状态。
    ///
    /// ⚠️ **`pending_ms` 数的是「距上一次落盘*尝试*多久」，不是「距上一次写成功多久」。**
    /// 两者在稳态下相同，**在失败态下不同** ——
    /// `pending > 0 && pending_ms ≈ 0` 读作「**刚试过、没写成**」，不是「刚写完」。
    pub fn status(&self, now: i64) -> (usize, u64, i64) {
        let g = self.inner.lock();
        (self.slot, g.pending, now - g.last_flush_at)
    }
}

#[cfg(test)]
mod tests;
