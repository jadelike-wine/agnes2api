//! 上游结果 → 对这把 key 做什么。**这张表是全局唯一的一份**（`errors.ts:1-25`，01 §2.6）。
//!
//! 🔴 **任何地方都不许自己判上游状态码。** 转发热路径、探测端点、模型测试端点
//! 全部调这里的两个函数。各写各的后果是「面板说这把 key 被限流了、调度器把它剔除了」
//! 这种两套语义 —— 而它们分歧的那一格恰好是最少见、最难复现的那一格。

use crate::core::types::PoolPolicy;

/// 一次上游交互之后，对**刚才用的那把 key** 该做什么。
///
/// ⚠️ 它只回答「对 key 做什么」。**对客户端回什么**是 `pool/dispatcher.rs` 的事，
/// 两者不是一回事：`Passthrough` 的 4xx 照样原样回给客户端，只是不记在这把 key 头上。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamAction {
    /// 上游 2xx。
    Success,
    /// 进冷却（会自己回来）。
    Cooldown { ms: i64, reason: String },
    /// 🔴 **永久剔除**。只保留给凭据失效（401 / 403）。
    Evict { reason: String },
    /// 累计一次瞬时故障。攒满 `max_strikes` 之后进长冷却，**不是**永久剔除。
    Strike { reason: String },
    /// 其余 4xx：**客户端的错，不是这把 key 的错**，直通，不记账在 key 头上。
    Passthrough,
}

/// 调用上游时抛出来的那一类错误（还没拿到状态码）。
///
/// # 为什么是一个自造的枚举而不是 `reqwest::Error`
///
/// `core` 是零 IO 岛，不许依赖 `reqwest`。映射规则写死在这里、由适配器执行：
/// **`reqwest::Error::is_timeout()` 或 `tokio::time::error::Elapsed`
/// ⇒ [`ThrownError::Timeout`]，其余一律 [`ThrownError::Network`]。**
/// （TS 侧的判据是 `err.name === "AbortError"`，语义逐字相同：
/// 「是我们自己把它掐了」vs「连都没连上 / 半路断了」。）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrownError {
    /// 我们自己的超时闸掐断了它。
    Timeout,
    /// 连不上 / 半路断了 / TLS 出错……
    Network,
}

/// 🔴 `KeyStats::last_error_kind` 的取值之一。**这个字符串会进面板，改它就是改对外可见的东西。**
pub const ERR_KIND_TIMEOUT: &str = "timeout";
/// 🔴 同上。
pub const ERR_KIND_NETWORK: &str = "network error";
/// 429 的冷却理由。**会进 `KeyRecord::cooldown_reason`、会进面板。**
pub const REASON_RATE_LIMITED: &str = "rate limited";
/// 402 的冷却理由。同上。
pub const REASON_PAYMENT_REQUIRED: &str = "payment required";

/// 按上游状态码分档。**全仓唯一的一份**（`errors.ts:5-16`）。
///
/// | 状态码 | 处置 | 理由（逐条都是买过单的） |
/// |---|---|---|
/// | `200..=299` | `Success` | — |
/// | `429` | `Cooldown(cooldown_rate_limit_ms)` | 限流**会自己过去**，剔除它就是把一次瞬时限流变成永久损失 |
/// | `402` | `Cooldown(cooldown_payment_ms)` | 欠费同理，但恢复要人去充值 ⇒ 冷却窗口长一个量级（默认 1 小时 vs 1 分钟） |
/// | `401` / `403` | `Evict` | 凭据失效是**确定性的**，重试无意义；这是唯一一条通向永久剔除的路 |
/// | `>= 500` | `Strike` | 上游故障是**暂时的** ⇒ 累计，攒满进长冷却（见 [`crate::core::keypool::apply_strike`]） |
/// | 其余 4xx | `Passthrough` | 客户端发错了请求，记在 key 头上会让运维去查一把没问题的 key |
///
/// 🔴 **`401/403 ⇒ Evict` 与 `5xx ⇒ Strike` 的分界不许挪。**
/// 5xx 走 evict 的那一版（历史实现）后果是：三把 key 的池子在上游持续 503 时
/// **只需五个请求即全部报废**，而当时没有任何 un-evict 路径，
/// **上游恢复之后网关也永远起不来**。
///
/// ⚠️ **`>= 500` 刻意不写上界**（不是 `500..=599`）：上游经反代之后回过什么数都可能，
/// 而「不是 2xx、不是已知的 4xx、还大得离谱」的正确处置与 5xx 完全一样。
/// 走到 `Passthrough` 那一支的只有 `100..=199` 与 `300..=399` 与未点名的 4xx。
pub fn classify_status(status: u16, policy: &PoolPolicy) -> UpstreamAction {
    if (200..300).contains(&status) {
        return UpstreamAction::Success;
    }
    match status {
        429 => UpstreamAction::Cooldown {
            ms: policy.cooldown_rate_limit_ms,
            reason: REASON_RATE_LIMITED.to_string(),
        },
        402 => UpstreamAction::Cooldown {
            ms: policy.cooldown_payment_ms,
            reason: REASON_PAYMENT_REQUIRED.to_string(),
        },
        401 | 403 => UpstreamAction::Evict {
            reason: format!("upstream {status}"),
        },
        s if s >= 500 => UpstreamAction::Strike {
            reason: format!("upstream {status}"),
        },
        _ => UpstreamAction::Passthrough,
    }
}

/// 连状态码都没拿到的那一族（`errors.ts:18-25`）。**两档，都是 `Strike`。**
///
/// 都走 strike 是有依据的：这两种都**可能**是上游抖动、也**可能**是这把 key 特别慢，
/// 而 strike 恰恰是「先记一笔，攒够了再处置」—— 一次孤立的网络抖动不改变任何东西。
///
/// 🔴 `reason` 的两个字符串会原样落进 [`crate::core::types::KeyStats::last_error_kind`]
/// 并显示在面板上，**改字符串就是改对外可见的东西**（也会打断运维按它 grep 的习惯）。
pub fn classify_thrown(err: ThrownError) -> UpstreamAction {
    match err {
        ThrownError::Timeout => UpstreamAction::Strike {
            reason: ERR_KIND_TIMEOUT.to_string(),
        },
        ThrownError::Network => UpstreamAction::Strike {
            reason: ERR_KIND_NETWORK.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> PoolPolicy {
        PoolPolicy {
            max_strikes: 3,
            cooldown_rate_limit_ms: 60_000,
            cooldown_payment_ms: 3_600_000,
            cooldown_strike_ms: 1_800_000,
        }
    }

    /// 2xx 的**整个区间**都是成功，不只是 200。
    /// （上游的 `204` / `206` 在媒体端点上真的出现过。）
    #[test]
    fn every_2xx_is_success() {
        for s in [200_u16, 201, 202, 204, 206, 299] {
            assert_eq!(
                classify_status(s, &policy()),
                UpstreamAction::Success,
                "{s}"
            );
        }
    }

    /// 🔴 两档冷却各自取各自的时长，**不许共用一个**：
    /// 限流 1 分钟就能回来，欠费要人去充值。共用一个值意味着其中一个必然是错的。
    #[test]
    fn rate_limit_and_payment_use_their_own_windows() {
        assert_eq!(
            classify_status(429, &policy()),
            UpstreamAction::Cooldown {
                ms: 60_000,
                reason: "rate limited".into()
            }
        );
        assert_eq!(
            classify_status(402, &policy()),
            UpstreamAction::Cooldown {
                ms: 3_600_000,
                reason: "payment required".into()
            }
        );
    }

    /// 🔴 **只有 401/403 通向永久剔除。**
    #[test]
    fn only_credential_failures_evict() {
        for s in [401_u16, 403] {
            assert_eq!(
                classify_status(s, &policy()),
                UpstreamAction::Evict {
                    reason: format!("upstream {s}")
                }
            );
        }
    }

    /// 🔴 **5xx 是 strike 不是 evict。**
    ///
    /// 回归对象是一条实测事故：5xx 走 evict 时，三把 key 的池子在上游持续 503 下
    /// 五个请求就全报废，且没有 un-evict 路径 —— 上游恢复后网关永远起不来。
    #[test]
    fn server_errors_strike_and_never_evict() {
        for s in [500_u16, 502, 503, 504, 520, 599, 999] {
            match classify_status(s, &policy()) {
                UpstreamAction::Strike { reason } => assert_eq!(reason, format!("upstream {s}")),
                other => panic!("{s} 不该是 {other:?}，5xx 一次抖动不许摧毁池子"),
            }
        }
    }

    /// 其余 4xx 直通：**不记在这把 key 头上。**
    /// 记了的话运维会被指去查一把其实没问题的 key。
    #[test]
    fn other_client_errors_pass_through_untouched() {
        for s in [400_u16, 404, 405, 409, 413, 418, 422, 428, 451] {
            assert_eq!(
                classify_status(s, &policy()),
                UpstreamAction::Passthrough,
                "{s}"
            );
        }
    }

    /// 1xx / 3xx 也走直通 —— 它们既不是这把 key 的错，也不代表上游挂了。
    #[test]
    fn informational_and_redirects_pass_through() {
        for s in [100_u16, 101, 301, 302, 304, 399] {
            assert_eq!(
                classify_status(s, &policy()),
                UpstreamAction::Passthrough,
                "{s}"
            );
        }
    }

    /// 🔴 两个 `last_error_kind` 字符串是**对外可见**的，逐字钉住。
    #[test]
    fn thrown_error_kinds_are_the_two_panel_visible_strings() {
        assert_eq!(
            classify_thrown(ThrownError::Timeout),
            UpstreamAction::Strike {
                reason: "timeout".into()
            }
        );
        assert_eq!(
            classify_thrown(ThrownError::Network),
            UpstreamAction::Strike {
                reason: "network error".into()
            }
        );
    }

    /// 冷却的两条 `reason` 同样是对外可见的字面量。
    #[test]
    fn cooldown_reasons_are_stable_literals() {
        assert_eq!(REASON_RATE_LIMITED, "rate limited");
        assert_eq!(REASON_PAYMENT_REQUIRED, "payment required");
    }

    /// 分档表**不读配置以外的任何东西**：同一个状态码 + 同一份 policy，
    /// 结果必须逐次相同（零 IO 岛的最基本性质）。
    #[test]
    fn classification_is_a_pure_function_of_status_and_policy() {
        let a = classify_status(429, &policy());
        let b = classify_status(429, &policy());
        assert_eq!(a, b);
    }
}
