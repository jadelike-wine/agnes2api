//! 端口的**生产实现**。文件 IO、reqwest、时钟、随机**全部住在这一层**。
//!
//! 边界：**不许依赖 `core` / `server` / `admin`**。方向是单向的 ——
//! `core` 不知道适配器存在，`server` 只经端口用它们。
//!
//! # 🔴 端口层最大的风险：**替身全绿而真实现坏了**
//!
//! 所以每个适配器都要有**它自己的**判据，不能靠端口替身的绿色代言：
//! - [`storage_file`] 跑完整的 `Storage` 端口契约（含 TTL 物理删除）；
//! - [`fetcher_reqwest`] 用 **wiremock** + 一台手写 TCP 服务端跑「真 HTTP」判据
//!   （状态码、解压、分块**时序**、两种断法、两档超时）——
//!   那是引入 wiremock 的**唯一**理由。
//!
//! # 本轮落地的三个适配器，各自最要紧的那一条
//!
//! | 文件 | 放什么 | 最要紧的那一条 |
//! |---|---|---|
//! | [`fetcher_reqwest`] | 两个客户端工厂（数据面 / 控制面）+ **显式决定 `accept-encoding`** | 响应体**必须是流**；超时**必须在响应头到手那一刻按面解除**；三档失败不许混成一句 |
//! | [`logger_store`] | 事件环落存储：最小间隔闸、`maybe_flush`/`flush`、畸形条目逐条丢并计数 | 🔴 最小间隔闸挡的是「未鉴权请求驱动存储写」这根杠杆，**不是省点写** |
//! | [`logger_multi`] | 扇出：一个 sink 坏掉不影响别的 sink，**更不影响调用方** | Rust 侧「坏掉」只剩 panic 一种形态 ⇒ 落法是 `catch_unwind` |
//!
//! ⚠️ [`logger_store`] 里那个 `ring` 子模块是**临时安置**：它的归宿是
//! `core::admin::event_ring` / `core::admin::event_entry`（P1-c）。
//! 搬走时是**整体搬 + 从那边删掉**，不是「在两处各留一份」。理由写在它自己的模块头上。

pub mod fetcher_reqwest;
pub mod logger_console;
pub mod logger_multi;
pub mod logger_store;
pub mod storage_file;
pub mod storage_watch;

/// 测试用的存储替身。**不进产物**。
///
/// 它是端口层存在的**唯一理由**的实物：
/// - `MemoryStorage`：可注入假时钟（测 TTL）、可注入延迟（
///   「瞬时返回的替身让任何 happens-before 性质都不可观测」）；
/// - `CountingStorage`：🔴 **`get`/`put`/`delete`/`list` 四个桶分开数** ——
///   上一版只数 `get`，而被违反的恰恰是写那一侧。
#[cfg(test)]
pub mod storage_test_doubles;

/// `[0, 1)` 的随机数。
///
/// # 🔴 为什么不引一个随机数 crate
///
/// 注册机是这个数**唯一**的消费者，而它要的性质只有一条：
/// **「别让一轮里的几次铸号在同一毫秒扎堆」**。
/// 那是个抖动，不是密码学随机 —— `getrandom`（本仓已有，用于别处）够用且不多一条依赖。
///
/// ⚠️ **绝不许拿它去生成凭据。** 口令生成走的是同一个接口，
/// 但那一处的安全性来自**上游的复杂度要求 + 口令只活在一次注册与紧接着那次登录之间**，
/// 不来自这里的熵。哪天有人要拿它做别的，先回来读这一段。
pub fn rand_unit() -> f64 {
    let mut b = [0u8; 8];
    if getrandom::getrandom(&mut b).is_err() {
        // 取不到熵时回落到时钟低位 —— 抖动仍然成立，而**让补池因为取不到熵就停摆是荒唐的**。
        let t = now_ms() as u64;
        b.copy_from_slice(&t.to_le_bytes());
    }
    // 取 53 位尾数，落在 [0, 1)。
    (u64::from_le_bytes(b) >> 11) as f64 / (1u64 << 53) as f64
}

/// 当前墙钟（epoch 毫秒）。
///
/// **只有这一层能读时钟**（`core` 一律收 `now: i64` 参数）。
/// 系统时钟早于 1970 时回 0 而不是 panic：时钟异常不该让网关崩，
/// 它的后果顶多是某些 TTL 立刻过期，而那是可恢复的。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    /// 时钟至少是个像样的 epoch 毫秒（2020 年之后），而且不是常量。
    #[test]
    fn now_ms_looks_like_epoch_millis() {
        let t = super::now_ms();
        assert!(
            t > 1_577_836_800_000,
            "看起来不像 2020 年之后的 epoch 毫秒: {t}"
        );
    }
}
