//! 网关配置本体。取值规则**一条都不在这里**，全在 [`crate::core::config_provenance`]。

use crate::core::config_errors::ConfigRefusal;
use crate::core::config_provenance::{EnvMap, LoadFlags, bool_field, defaults, num, string_field};
use serde_json::Value;

/// 网关的生效配置。
///
/// # 缺席的字段，逐个写清楚为什么缺席
///
/// - **`registrar`（注册机子树）**：**刻意不在这里，而且是永久的。**
///   它由 [`crate::registrar::runtime::load`] 单独装载，装载器零 panic。
///   合进本结构体意味着「注册机配坏了」要和「网关配坏了」走同一条装载路径，
///   而 `config-degrade` 那条 🔴 判据（真机 74% 请求 500 那次事故）要的恰恰是
///   **注册机装不起来时网关照常活着** —— 两份配置必须能分开失败。
/// - **`degraded`**：它不是旋钮，是**本次装载的产物**，所以住在
///   [`crate::core::config_provenance::LoadFlags`] 里随装载一起返回，
///   而不是塞进这个结构体让人以为能配。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayConfig {
    /// 🔴 **唯一 fail-closed 字段**：没有它整个网关拒绝启动。
    ///
    /// ⚠️ 它是凭据。**永不进任何响应体、永不进日志、永不进事件的 `fields`。**
    pub gateway_token: String,
    /// 上游基址。
    pub agnes_base_url: String,
    /// 对话类端点的首字节超时。
    pub upstream_timeout_ms: i64,
    /// 同步端点（图片 / 视频建任务）的上游超时。**只在同步端点上生效**，
    /// 不影响对话的「快速甩掉慢 key」。
    pub upstream_sync_timeout_ms: i64,
    pub max_strikes: i64,
    pub cooldown_rate_limit_ms: i64,
    pub cooldown_payment_ms: i64,
    pub cooldown_strike_ms: i64,
    /// ⚠️ **建 app 时读一次**，不是逐次生效：它绑定的是部署形态（活跃副本数 × 池大小），
    /// 不是逐次可调的策略。改它要重启容器，**面板文案不许写「立即生效」**。
    pub pool_cache_ttl_ms: i64,
    /// 同上，生效时机与上一条一致。0 = 关闭写消除。
    pub pool_touch_interval_ms: i64,
    /// Tier-2 时间序列统计。**默认 false，且「关」必须是零成本**：
    /// 关闭时不建累加器、不挂中间件、**一次 `put` 都没有**。
    ///
    /// 两条理由（都要按今天的形态读，别按 Cloudflare 时代的旧话读）：
    /// ① `FileStorage` 每次 `put` 都**重写整个 `store.json`**，多一条写路径就是实打实
    ///    的磁盘写放大，而它与 key 池的状态回写写的是同一个文件、走同一条写队列；
    /// ② sink 构造时 `last_flush_at = now()`，**存活不足一个落盘间隔（默认 2 小时）的
    ///    进程一个字都存不下** —— 频繁重启、崩溃重启循环、一天里滚动更新好几次的部署
    ///    都够得着。**开着它会得到一份看起来完整、实际残缺的数字。**
    ///
    /// ⚠️ 它同样是「建 app 时读一次」的旋钮：装配时用它决定要不要**构造**那个 sink，
    /// 而全局约束不允许「先建好、再靠一个 if 拦住写」—— 那条路径迟早会被某次改动接上写。
    pub usage_stats_enabled: bool,
}

/// 装载配置：**优先级 env > 存储 > 内置默认值，全仓唯一的一份实现。**
///
/// `stored` 是**已经从存储里读出来并窄化过**的那份面板配置（`config` 键的值）。
/// 🔴 **存储的读取刻意不在这里做**：`core` 零 IO，不许碰端口。
/// 这么切还有一个附带收益 —— 「读存储」与「按优先级定值」成了两件可以分别测的事，
/// 而 TS 版把它们绑在一个 async 函数里，导致每一格判据都要搭一套存储替身。
///
/// `storage_unreadable`：调用方告诉我们「这次连存储都没读成」。
/// 它只影响 `degraded` 标记（**存储读不出来比存储值非法更轻，不该得到更重的处置**），
/// 唯一仍然 fatal 的还是「两边都没有网关口令」。
///
/// # 调用方要知道的两件事
///
/// 1. **`flags.notices` 必须真的被打出去**（见 `config_provenance` 模块头）。
/// 2. **不许在调用点补任何一条「顺手的」优先级判断。** 只要第二条规则出现在别处，
///    面板（走来源推导）与网关（走这里）就有了两份实现，而它们的差恰好落在
///    「面板说的生效值」与「真正的生效值」不一致的那一格上。
pub fn resolve_config(
    env: &EnvMap,
    stored: Option<&Value>,
    storage_unreadable: bool,
) -> Result<(GatewayConfig, LoadFlags), ConfigRefusal> {
    // 逃生口 `RESET_CONFIG=1`：存储里的 config 键被写坏到连降级都救不回来时，
    // **完全忽略它**。🔴 只忽略不删 —— 删了用户就再也拿不回原值了。
    let reset = env.get("RESET_CONFIG").map(String::as_str) == Some("1");
    let stored = if reset { None } else { stored };

    let at = |name: &str| -> Option<&Value> {
        stored.and_then(|v| v.get(name)).filter(|v| !v.is_null())
    };

    let mut flags = LoadFlags {
        degraded: storage_unreadable,
        notices: Vec::new(),
    };

    // ⚠️ env 侧一旦出现就赢，**哪怕是空串** —— 与 TS 的 `??` 逐字同义。
    // 于是 `GATEWAY_TOKEN=` 不会掉回存储里那份，而是直接走下面那条拒绝启动。
    // 这是有意的：写了这个变量却留空，最可能的真相是「部署脚本没填上」。
    let gateway_token = match env.get("GATEWAY_TOKEN") {
        Some(v) => v.clone(),
        // 存储侧只收字符串，别的形状一律当没有。
        None => match at("gatewayToken") {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        },
    };
    if gateway_token.is_empty() {
        // 🔴 **message 逐字不动**：入口打的就是它，五语言部署文档的故障排查条目
        // 引的也是这句原文。
        return Err(ConfigRefusal::new("缺少 GATEWAY_TOKEN，网关无法启动"));
    }

    let config = GatewayConfig {
        gateway_token,
        agnes_base_url: string_field(
            env,
            "AGNES_BASE_URL",
            at("agnesBaseUrl"),
            defaults::AGNES_BASE_URL,
        ),
        upstream_timeout_ms: num(
            env,
            "UPSTREAM_TIMEOUT_MS",
            "upstreamTimeoutMs",
            at("upstreamTimeoutMs"),
            defaults::UPSTREAM_TIMEOUT_MS,
            1,
            &mut flags,
        )?,
        upstream_sync_timeout_ms: num(
            env,
            "UPSTREAM_SYNC_TIMEOUT_MS",
            "upstreamSyncTimeoutMs",
            at("upstreamSyncTimeoutMs"),
            defaults::UPSTREAM_SYNC_TIMEOUT_MS,
            1,
            &mut flags,
        )?,
        max_strikes: num(
            env,
            "MAX_STRIKES",
            "maxStrikes",
            at("maxStrikes"),
            defaults::MAX_STRIKES,
            1,
            &mut flags,
        )?,
        cooldown_rate_limit_ms: num(
            env,
            "COOLDOWN_RATE_LIMIT_MS",
            "cooldownRateLimitMs",
            at("cooldownRateLimitMs"),
            defaults::COOLDOWN_RATE_LIMIT_MS,
            1,
            &mut flags,
        )?,
        cooldown_payment_ms: num(
            env,
            "COOLDOWN_PAYMENT_MS",
            "cooldownPaymentMs",
            at("cooldownPaymentMs"),
            defaults::COOLDOWN_PAYMENT_MS,
            1,
            &mut flags,
        )?,
        cooldown_strike_ms: num(
            env,
            "COOLDOWN_STRIKE_MS",
            "cooldownStrikeMs",
            at("cooldownStrikeMs"),
            defaults::COOLDOWN_STRIKE_MS,
            1,
            &mut flags,
        )?,
        // 🔴 这两个传 `min = 0`：0 对它们是「关闭」这个有意义的取值，是用户的逃生口。
        pool_cache_ttl_ms: num(
            env,
            "POOL_CACHE_TTL_MS",
            "poolCacheTtlMs",
            at("poolCacheTtlMs"),
            defaults::POOL_CACHE_TTL_MS,
            0,
            &mut flags,
        )?,
        pool_touch_interval_ms: num(
            env,
            "POOL_TOUCH_INTERVAL_MS",
            "poolTouchIntervalMs",
            at("poolTouchIntervalMs"),
            defaults::POOL_TOUCH_INTERVAL_MS,
            0,
            &mut flags,
        )?,
        usage_stats_enabled: bool_field(
            env,
            "USAGE_STATS_ENABLED",
            at("usageStatsEnabled"),
            defaults::USAGE_STATS_ENABLED,
        ),
    };

    Ok((config, flags))
}

/// 只用环境变量 + 内置默认值装载（没有存储那一层）。
///
/// **它是 [`resolve_config`] 的一个调用，不是第二份实现** —— 这一点是硬要求：
/// 两份实现的差会恰好落在「面板显示的生效值」与「真正的生效值」不一致的那一格上。
///
/// 这条路径上 `degraded` 恒为 false：纯 env + 内置默认值不存在「保存了却没生效」
/// 这种可能，没有什么好提示的。
pub fn config_from_env(env: &EnvMap) -> Result<GatewayConfig, ConfigRefusal> {
    let (config, flags) = resolve_config(env, None, false)?;
    debug_assert!(!flags.degraded, "没有存储那一层就不该有降级");
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config_provenance::EnvMap;
    use serde_json::json;

    fn env_of(pairs: &[(&str, &str)]) -> EnvMap {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn with_token(mut env: EnvMap) -> EnvMap {
        env.entry("GATEWAY_TOKEN".into())
            .or_insert_with(|| "t-abcdefghijklmnop".into());
        env
    }

    /// 🔴 **缺 `GATEWAY_TOKEN` 一律拒绝启动**，且 message 逐字不动。
    ///
    /// 这是全仓唯一保留的 fail-closed：没有口令就无法鉴权，
    /// **继续跑比停下来更危险**（跑起来的是一个谁都能用的中转）。
    #[test]
    fn missing_gateway_token_is_fatal() {
        let err = resolve_config(&EnvMap::new(), None, false).unwrap_err();
        assert_eq!(err.to_string(), "缺少 GATEWAY_TOKEN，网关无法启动");
    }

    /// 空串的 `GATEWAY_TOKEN=` **也是缺**，而且不许掉回存储里那份。
    ///
    /// 写了这个变量却留空，最可能的真相是部署脚本没填上；
    /// 此时悄悄改用存储里的旧口令，会让「我明明换过口令了」变成一次静默失败。
    #[test]
    fn empty_env_gateway_token_does_not_fall_back_to_stored() {
        let env = env_of(&[("GATEWAY_TOKEN", "")]);
        let stored = json!({ "gatewayToken": "stored-token" });
        let err = resolve_config(&env, Some(&stored), false).unwrap_err();
        assert_eq!(err.to_string(), "缺少 GATEWAY_TOKEN，网关无法启动");
    }

    /// 环境变量里没有口令时，存储里那份能把网关救起来 ——
    /// 这是「没设 `GATEWAY_TOKEN` 环境变量的部署，口令由面板管」这条形态的前提。
    #[test]
    fn stored_gateway_token_is_accepted_when_env_absent() {
        let stored = json!({ "gatewayToken": "stored-token" });
        let (c, _) = resolve_config(&EnvMap::new(), Some(&stored), false).unwrap();
        assert_eq!(c.gateway_token, "stored-token");
    }

    /// 内置默认值一格一个手写字面量。
    ///
    /// **期望值不许写成 `defaults::X`** —— 那是同义反复（04 §3.3 第 ⑥ 条假阳性），
    /// 改了默认值当场全绿。改这里的任何一个数之前，先回规格读它的理由。
    #[test]
    fn defaults_are_pinned_literals() {
        let (c, flags) = resolve_config(&with_token(EnvMap::new()), None, false).unwrap();
        assert_eq!(c.agnes_base_url, "https://apihub.agnes-ai.com/v1");
        assert_eq!(c.upstream_timeout_ms, 8_000);
        assert_eq!(
            c.upstream_sync_timeout_ms, 120_000,
            "实测图片生成约 12 秒，8 秒会让它 100% 失败"
        );
        assert_eq!(c.max_strikes, 3);
        assert_eq!(c.cooldown_rate_limit_ms, 60_000);
        assert_eq!(c.cooldown_payment_ms, 3_600_000);
        assert_eq!(c.cooldown_strike_ms, 1_800_000);
        assert_eq!(c.pool_cache_ttl_ms, 60_000);
        assert_eq!(c.pool_touch_interval_ms, 21_600_000);
        assert!(!c.usage_stats_enabled, "Tier-2 默认关，且关必须是零成本");
        assert!(!flags.degraded);
    }

    /// 存储里那份能逐字段盖过默认值。
    #[test]
    fn stored_values_override_defaults() {
        let stored = json!({
            "agnesBaseUrl": "https://example.invalid/v1",
            "maxStrikes": 9,
            "cooldownStrikeMs": 7_777_000,
            "usageStatsEnabled": true,
        });
        let (c, flags) = resolve_config(&with_token(EnvMap::new()), Some(&stored), false).unwrap();
        assert_eq!(c.agnes_base_url, "https://example.invalid/v1");
        assert_eq!(c.max_strikes, 9);
        assert_eq!(c.cooldown_strike_ms, 7_777_000);
        assert!(c.usage_stats_enabled);
        assert!(!flags.degraded, "合法的存储值不是降级");
    }

    /// 🔴 `RESET_CONFIG=1` **完全忽略**存储里那份（只忽略，不删）。
    ///
    /// 它是「存储里的 config 被写坏到连降级都救不回来」时的逃生口。
    #[test]
    fn reset_config_ignores_stored_entirely() {
        let env = with_token(env_of(&[("RESET_CONFIG", "1")]));
        let stored = json!({ "maxStrikes": 9, "agnesBaseUrl": "https://example.invalid/v1" });
        let (c, _) = resolve_config(&env, Some(&stored), false).unwrap();
        assert_eq!(c.max_strikes, 3);
        assert_eq!(c.agnes_base_url, "https://apihub.agnes-ai.com/v1");
    }

    /// `RESET_CONFIG=1` **不影响**存储里那份口令以外的救命作用……
    /// 更准确地说：它连口令也一起忽略，所以这种部署必须自带 `GATEWAY_TOKEN`。
    ///
    /// 这一格把那个后果钉出来，免得有人在生产上按下逃生口之后才发现起不来。
    #[test]
    fn reset_config_also_drops_stored_gateway_token() {
        let env = env_of(&[("RESET_CONFIG", "1")]);
        let stored = json!({ "gatewayToken": "stored-token" });
        let err = resolve_config(&env, Some(&stored), false).unwrap_err();
        assert_eq!(err.to_string(), "缺少 GATEWAY_TOKEN，网关无法启动");
    }

    /// 存储读不出来 ⇒ 只标 `degraded`，**不拒绝启动**。
    ///
    /// 原则：存储**读不出来**比存储值非法更轻，不该得到更重的处置。
    /// 这条路径上的口令来自环境变量，网关照常能跑。
    #[test]
    fn unreadable_storage_degrades_instead_of_refusing() {
        let (c, flags) = resolve_config(&with_token(EnvMap::new()), None, true).unwrap();
        assert_eq!(c.max_strikes, 3);
        assert!(flags.degraded, "面板必须看得见这次装载降级了");
    }

    /// 存储里一个字段非法 ⇒ 那个字段回落默认值，**别的字段照常生效**。
    ///
    /// 回归对象：整块丢弃会让一次坏写把整份面板配置清回默认值，
    /// 而用户在面板上看到的仍然是他保存过的那些数字。
    #[test]
    fn one_bad_field_does_not_poison_the_rest() {
        let stored = json!({ "maxStrikes": "九", "cooldownStrikeMs": 7_777_000 });
        let (c, flags) = resolve_config(&with_token(EnvMap::new()), Some(&stored), false).unwrap();
        assert_eq!(c.max_strikes, 3, "坏字段回落默认值");
        assert_eq!(c.cooldown_strike_ms, 7_777_000, "好字段照常生效");
        assert!(flags.degraded);
        assert_eq!(flags.notices.len(), 1, "只该为坏的那一个字段留通知");
    }

    /// `config_from_env` 与 `resolve_config(env, None, false)` **必须逐字段相同**。
    ///
    /// 它钉的是「只有一份实现」这件事本身：哪天有人在薄封装里补一条
    /// 「顺手的」判断，这一格当场红。
    #[test]
    fn config_from_env_is_a_thin_wrapper_not_a_second_implementation() {
        let env = with_token(env_of(&[
            ("MAX_STRIKES", "5"),
            ("AGNES_BASE_URL", "https://example.invalid/v1"),
            ("USAGE_STATS_ENABLED", "true"),
        ]));
        let a = config_from_env(&env).unwrap();
        let (b, _) = resolve_config(&env, None, false).unwrap();
        assert_eq!(a, b);
    }

    /// 环境变量压过存储 —— 端到端再钉一次（`num` 那一层已经有单测，这里是装配级）。
    #[test]
    fn env_beats_stored_end_to_end() {
        let env = with_token(env_of(&[("MAX_STRIKES", "7")]));
        let stored = json!({ "maxStrikes": 9 });
        let (c, _) = resolve_config(&env, Some(&stored), false).unwrap();
        assert_eq!(c.max_strikes, 7);
    }
}
