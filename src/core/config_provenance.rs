//! 配置取值的**唯一实现**：优先级、内置默认值、字段级降级。
//!
//! 🔴 **不允许在面板层另写一套来源推导 —— 那必然与这里漂移**，而漂移的形态很具体：
//! 下面 [`num`] 里有一条「存储值非法 ⇒ 字段级降级回默认值」的分支。面板若照着
//! 「有 env 就显示 env，否则显示存储」推一遍，**在那条降级分支上就会说假话**：
//! 面板显示存储里那个非法值是「生效值」，而网关实际在用默认值。
//! **两份实现之间的差，恰好落在最需要面板说实话的那一格上。**
//!
//! # 与 TS 版的一处**刻意偏离**（登记在此，理由写全）
//!
//! TS 的 `num()` 直接持一个 `Logger` 并在降级时打 `config.invalid`。
//! Rust 侧**不能照抄**：规格 §2.2 的边界表写死「`core` 不许依赖 `ports/`」。
//! ⇒ 这里改成**把该打的事件攒进 [`LoadFlags::notices`] 返回给调用方**，
//! 由边界上那一层（`server::wire`）翻译成 `LogEntry` 真正打出去。
//!
//! 换来的：`num()` 成了不需要任何替身就能测的纯函数。
//! 代价明写：**事件必须真的被打出去，否则「面板说实话」这条链就断在半路**——
//! 调用方漏掉 `notices` 是一个新的失效形态，P1-b 接 `ConfigHolder` 时要有一格判据钉住它。
//! 事件名与字段**逐字保留**（`config.invalid` / `field` / `source` / `raw` / `fallback`），
//! 它们是运维的 grep 锚点。

use crate::core::config_errors::ConfigRefusal;
use serde_json::Value;
use std::collections::BTreeMap;

/// 环境变量快照。
///
/// **core 不许读 `std::env`**，所以它必须由调用方在边界上抓一份传进来。
/// 顺带的好处：整个配置体系在测试里就是「给一张表，看返回什么」。
///
/// ⚠️ **空串是一个真实存在的值，不是「没设」**：`.env` 文件里留空的键送进来就是空串，
/// 而数值字段留空时 fail-fast 是**有意的**（见 [`num`]）。
/// 只有 `DATA_DIR` / `PORT` 两个进程级开关把空串当没设，那条特例住在 `cli.rs`。
pub type EnvMap = BTreeMap<String, String>;

/// 存储里放面板那份配置的键。
pub const CONFIG_KEY: &str = "config";

/// 🔴 事件名是运维的 grep 锚点，一个字都不许改。
pub const EVENT_CONFIG_INVALID: &str = "config.invalid";
/// 同上。
pub const EVENT_CONFIG_STORAGE_UNREADABLE: &str = "config.storage_unreadable";

/// 内置默认值。
///
/// **导出是因为将来面板那条路也用它** —— 抄第二份的后果是
/// 「面板显示的默认值」与「网关实际回落到的默认值」可以不一样。
pub mod defaults {
    /// 上游基址。⚠️ 它是**上游厂商的公开 API 基址**，不是本部署的私有域名。
    /// 运维要改走 `AGNES_BASE_URL` 或面板。
    pub const AGNES_BASE_URL: &str = "https://apihub.agnes-ai.com/v1";
    /// 对话类端点的**首字节**超时。
    pub const UPSTREAM_TIMEOUT_MS: i64 = 8_000;
    /// 同步端点（图片生成、视频建任务）的上游超时。
    ///
    /// 与上一条分开是因为两者度量的**根本不是同一件事**：流式对话的首字节只代表
    /// 「上游开始说话」，8 秒够；同步端点的首字节要等整张图渲染完才到达 ——
    /// 实测直连上游图片生成首字节约 12 秒。用 8 秒去卡它的结果是图片生成 100% 失败，
    /// 且每次请求会把池中每把 key 各记一次 strike，**三次请求就能把整池打进 30 分钟
    /// 长冷却，连对话一起拖死**。留出「慢 key 比快 key 慢 3~4 倍」的余量，取 2 分钟。
    pub const UPSTREAM_SYNC_TIMEOUT_MS: i64 = 120_000;
    /// 连续失败多少次判停用。
    pub const MAX_STRIKES: i64 = 3;
    /// 429 之后的冷却。
    pub const COOLDOWN_RATE_LIMIT_MS: i64 = 60_000;
    /// 402 之后的冷却。
    pub const COOLDOWN_PAYMENT_MS: i64 = 3_600_000;
    /// strike 攒满之后的冷却。
    pub const COOLDOWN_STRIKE_MS: i64 = 1_800_000;
    /// 池快照缓存 TTL。**0 = 关**。⚠️ 建 app 时读一次，改了要重启。
    pub const POOL_CACHE_TTL_MS: i64 = 60_000;
    /// 写消除的时间窗。**0 = 关**。⚠️ 建 app 时读一次，改了要重启。
    pub const POOL_TOUCH_INTERVAL_MS: i64 = 21_600_000;
    /// Tier-2 时间序列统计。**默认关，且「关」必须是零成本**。
    pub const USAGE_STATS_ENABLED: bool = false;
}

/// 一条「本次装载降级了」的通知，等着被调用方翻译成事件打出去。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigNotice {
    /// 事件名，取自上面那几个常量。
    pub event: &'static str,
    /// 面板路径，例如 `maxStrikes`。整体读不出来那一条没有字段，是 `None`。
    pub field: Option<String>,
    /// 降级来源。今天只有 `"stored"`（env 侧非法值是 fail-fast，不降级）。
    pub source: Option<&'static str>,
    /// 被拒的原始值的**字符串形态**。🔴 只用于诊断，**永不是凭据字段的值**：
    /// 凭据字段一律不走 [`num`]，也不进这张通知。
    pub raw: Option<String>,
    /// 实际回落到的值。
    pub fallback: Option<String>,
}

/// 一次装载的产物标记。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadFlags {
    /// 本次装载有没有降级（存储读不出来 / 某些字段回落了默认值）。
    ///
    /// 它有一条完整的三级消费链：响应体 → 前端窄化成 `bool | null` → 显示红色横幅。
    /// 🔴 **「保存了却没生效」是这个项目最高频的用户困惑，不让它可见就等于让面板撒谎。**
    pub degraded: bool,
    /// 待打的事件。**调用方必须真的打出去**，见模块头的偏离说明。
    pub notices: Vec<ConfigNotice>,
}

/// 读一个数值配置项，优先级：**环境变量 > 存储 > 内置默认值**。
///
/// `min` 默认应传 1。理由不是「防负数」这种泛泛而谈：
/// `UPSTREAM_TIMEOUT_MS=-1` 会让超时立刻触发，**一个请求就能把整池打成 strike**；
/// `MAX_STRIKES=0` 更糟 —— `strikes >= max_strikes` 在第一次失败时即成立（`1 >= 0`），
/// **等于跳过了整个容错机制**。故只校验「是有限数」是不够的。
///
/// **只有 `pool_cache_ttl_ms` / `pool_touch_interval_ms` 传 `min = 0`**：
/// 对它们来说 0 是「关闭」这个有意义的取值，是用户的逃生口。
///
/// # 两侧非法值的处置**刻意相反**
///
/// - **env 侧非法 ⇒ fail-fast（返回 [`ConfigRefusal`]）**：那是部署时错误，运维必须
///   立刻看得见，而且它不可能是面板写坏的 —— 面板永远碰不到环境变量。
/// - **存储侧非法 ⇒ 字段级降级**：抛错的后果是进程 exit(1) 进入重启循环，
///   而且**没有面板可以进去改回来**；一次误操作把网关砖掉是不可接受的。
///
/// ⚠️⚠️ 注册机子树（P3）对 env 侧非法值**降级不抛**，与这里正相反，**那是刻意的例外**。
/// 判据不是「哪种更严格」，而是**这个子系统有没有安全的降能模式**：注册机停跑 =
/// 池子不再自动补、转发照常；而这里这几个旋钮（超时、strike 阈值、冷却时长）
/// **一个都关不掉** —— 回落默认值就是**静默改掉转发行为**，那比拒绝启动更坏。
/// ⇒ 两边的不一致是有理由的取舍，不是疏漏。**要改先推翻这条理由。**
pub fn num(
    env: &EnvMap,
    env_name: &str,
    field_name: &str,
    stored: Option<&Value>,
    fallback: i64,
    min: i64,
    flags: &mut LoadFlags,
) -> Result<i64, ConfigRefusal> {
    if let Some(raw) = env.get(env_name) {
        return match parse_int_at_least(&Value::String(raw.clone()), min) {
            Some(n) => Ok(n),
            None => Err(ConfigRefusal::new(format!(
                "环境变量 {env_name} 必须是不小于 {min} 的整数: {raw}"
            ))),
        };
    }

    if let Some(v) = stored.filter(|v| !v.is_null()) {
        return match parse_int_at_least(v, min) {
            Some(n) => Ok(n),
            None => {
                flags.degraded = true;
                flags.notices.push(ConfigNotice {
                    event: EVENT_CONFIG_INVALID,
                    field: Some(field_name.to_string()),
                    source: Some("stored"),
                    raw: Some(scalar_to_string(v)),
                    fallback: Some(fallback.to_string()),
                });
                Ok(fallback)
            }
        };
    }

    Ok(fallback)
}

/// 读一个字符串配置项，优先级同上。
///
/// ⚠️ **env 侧一旦出现（哪怕是空串）就赢**，不会掉回存储 —— 与 TS 的 `??` 逐字同义。
/// 存储侧**只收字符串**，别的形状（数字 / 对象）一律当没有：存储里什么形状都可能来。
pub fn string_field(
    env: &EnvMap,
    env_name: &str,
    stored: Option<&Value>,
    fallback: &str,
) -> String {
    if let Some(raw) = env.get(env_name) {
        return raw.clone();
    }
    if let Some(Value::String(s)) = stored {
        return s.clone();
    }
    fallback.to_string()
}

/// 读一个布尔配置项。
///
/// 🔴 **判据一律是 `== "true"`**，不是「非空即真」：`USAGE_STATS_ENABLED=0` /
/// `=false` 必须是关，那是运维写下「我不要它」时最自然的两种写法。
///
/// ⚠️ **布尔不走 [`num`]，所以没有「存储值非法 ⇒ 字段级降级」这一支**：
/// 布尔没有非法值，读不懂一律落到 `false` —— 而那正是「默认关」该有的失败方向
/// （关是零成本，读不懂时必须往关的方向倒）。
pub fn bool_field(env: &EnvMap, env_name: &str, stored: Option<&Value>, fallback: bool) -> bool {
    if let Some(raw) = env.get(env_name) {
        return raw == "true";
    }
    match stored {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true",
        // 数字 / 数组 / 对象 / null / 缺席：一律回落。
        _ => fallback,
    }
}

/// `Value` → 整数，且不小于 `min`。看不懂一律 `None`。
///
/// 接受两种形状：JSON 数字，以及**字符串形态的数字**（环境变量只有这一种）。
/// ⚠️ 与 TS 的 `Number(raw)` 有一处已知差异：JS 认 `"0x10"` / `"Infinity"`，这里不认。
/// 两者在本仓的取值范围内结果相同（都落到「拒绝」那一侧），如实登记在此。
fn parse_int_at_least(v: &Value, min: i64) -> Option<i64> {
    let n: f64 = match v {
        Value::Number(n) => n.as_f64()?,
        // JS 的 `Number(" 8000 ")` 是 8000：前后空白被吃掉。这里对齐。
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if !n.is_finite() || n.fract() != 0.0 {
        return None;
    }
    // f64 转 i64 的边界：超出 i64 表示范围的一律当看不懂，而不是悄悄截断。
    if n < i64::MIN as f64 || n > i64::MAX as f64 {
        return None;
    }
    let n = n as i64;
    (n >= min).then_some(n)
}

/// 把一个被拒的值渲染成一行诊断文本。**只给日志看**，不进任何响应体。
fn scalar_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env_of(pairs: &[(&str, &str)]) -> EnvMap {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// 优先级的三层，一格一层。
    #[test]
    fn env_beats_stored_beats_default() {
        let mut f = LoadFlags::default();
        let env = env_of(&[("MAX_STRIKES", "9")]);
        assert_eq!(
            num(
                &env,
                "MAX_STRIKES",
                "maxStrikes",
                Some(&json!(5)),
                3,
                1,
                &mut f
            )
            .unwrap(),
            9
        );

        let empty = EnvMap::new();
        assert_eq!(
            num(
                &empty,
                "MAX_STRIKES",
                "maxStrikes",
                Some(&json!(5)),
                3,
                1,
                &mut f
            )
            .unwrap(),
            5
        );
        assert_eq!(
            num(&empty, "MAX_STRIKES", "maxStrikes", None, 3, 1, &mut f).unwrap(),
            3
        );
        assert!(!f.degraded, "全是合法值，不该有任何降级");
    }

    /// 🔴 **env 侧非法值 fail-fast**：那是部署时错误，运维必须立刻看得见。
    ///
    /// `MAX_STRIKES=0` 不是「保守取值」，它等于跳过整个容错机制
    /// （`strikes >= 0` 在第一次失败时即成立）。
    #[test]
    fn illegal_env_value_refuses_to_start() {
        let mut f = LoadFlags::default();
        for raw in ["0", "-1", "abc", "", "1.5"] {
            let env = env_of(&[("MAX_STRIKES", raw)]);
            let err = num(&env, "MAX_STRIKES", "maxStrikes", None, 3, 1, &mut f)
                .expect_err(&format!("MAX_STRIKES={raw} 必须拒绝启动"));
            assert!(
                err.to_string().contains("MAX_STRIKES"),
                "报错必须点名是哪个环境变量"
            );
        }
        assert!(!f.degraded, "fail-fast 那一支不许顺手打降级标记");
    }

    /// 🔴 **存储侧非法值只降级不抛**，并且降级必须被上层观测到。
    ///
    /// 回归对象：抛错的后果是进程 exit(1) 进重启循环，而**没有面板可以进去改回来**。
    #[test]
    fn illegal_stored_value_degrades_to_default_and_is_observable() {
        let mut f = LoadFlags::default();
        let empty = EnvMap::new();
        let got = num(
            &empty,
            "MAX_STRIKES",
            "maxStrikes",
            Some(&json!("九")),
            3,
            1,
            &mut f,
        )
        .unwrap();
        assert_eq!(got, 3, "必须回落到默认值");
        assert!(f.degraded, "字段级降级必须让 degraded 为真，否则面板不会报");

        let n = f.notices.first().expect("必须留下一条待打的事件");
        assert_eq!(n.event, "config.invalid", "事件名是运维的 grep 锚点");
        assert_eq!(n.field.as_deref(), Some("maxStrikes"));
        assert_eq!(n.source, Some("stored"));
        assert_eq!(n.raw.as_deref(), Some("九"));
        assert_eq!(n.fallback.as_deref(), Some("3"));
    }

    /// 两个池子旋钮的 `min = 0`：0 是「关闭」这个有意义的取值，不是越界值。
    #[test]
    fn pool_knobs_accept_zero_as_off() {
        let mut f = LoadFlags::default();
        let env = env_of(&[("POOL_CACHE_TTL_MS", "0")]);
        assert_eq!(
            num(
                &env,
                "POOL_CACHE_TTL_MS",
                "poolCacheTtlMs",
                None,
                60_000,
                0,
                &mut f
            )
            .unwrap(),
            0
        );
        // 同一个值在 min=1 的字段上必须被拒 —— 这是这一格的反向控制。
        let env = env_of(&[("MAX_STRIKES", "0")]);
        assert!(num(&env, "MAX_STRIKES", "maxStrikes", None, 3, 1, &mut f).is_err());
    }

    /// `null` 的存储值等同于「没存过」，走默认值且**不算降级**。
    #[test]
    fn stored_null_is_treated_as_absent() {
        let mut f = LoadFlags::default();
        let empty = EnvMap::new();
        assert_eq!(
            num(
                &empty,
                "MAX_STRIKES",
                "maxStrikes",
                Some(&Value::Null),
                3,
                1,
                &mut f
            )
            .unwrap(),
            3
        );
        assert!(!f.degraded, "没存过不是降级");
    }

    /// 字符串字段：env 一旦出现就赢，**哪怕是空串**（与 TS 的 `??` 逐字同义）。
    #[test]
    fn string_env_wins_even_when_empty() {
        let env = env_of(&[("AGNES_BASE_URL", "")]);
        assert_eq!(
            string_field(&env, "AGNES_BASE_URL", Some(&json!("https://stored")), "d"),
            ""
        );
    }

    /// 字符串字段：存储里非字符串的形状一律当没有。
    #[test]
    fn string_stored_accepts_only_strings() {
        let empty = EnvMap::new();
        assert_eq!(
            string_field(&empty, "AGNES_BASE_URL", Some(&json!(42)), "d"),
            "d"
        );
        assert_eq!(
            string_field(&empty, "AGNES_BASE_URL", Some(&json!({"a":1})), "d"),
            "d"
        );
        assert_eq!(
            string_field(&empty, "AGNES_BASE_URL", Some(&json!("u")), "d"),
            "u"
        );
    }

    /// 🔴 布尔的判据是 `== "true"`，不是「非空即真」。
    ///
    /// 回归对象：`USAGE_STATS_ENABLED=0` / `=false` 被当成「开」，
    /// 而运维写下这两种值时的意思恰恰是「我不要它」。
    #[test]
    fn bool_requires_literal_true() {
        for raw in ["0", "false", "no", "1", "yes", "TRUE", ""] {
            let env = env_of(&[("USAGE_STATS_ENABLED", raw)]);
            assert!(
                !bool_field(&env, "USAGE_STATS_ENABLED", None, false),
                "USAGE_STATS_ENABLED={raw} 必须是关"
            );
        }
        let env = env_of(&[("USAGE_STATS_ENABLED", "true")]);
        assert!(bool_field(&env, "USAGE_STATS_ENABLED", None, false));
    }

    /// 布尔在存储侧读不懂时倒向 `false`（关是零成本，那是正确的失败方向）。
    #[test]
    fn bool_stored_falls_back_towards_off() {
        let empty = EnvMap::new();
        assert!(bool_field(
            &empty,
            "USAGE_STATS_ENABLED",
            Some(&json!(true)),
            false
        ));
        assert!(bool_field(
            &empty,
            "USAGE_STATS_ENABLED",
            Some(&json!("true")),
            false
        ));
        assert!(!bool_field(
            &empty,
            "USAGE_STATS_ENABLED",
            Some(&json!("yes")),
            false
        ));
        assert!(!bool_field(
            &empty,
            "USAGE_STATS_ENABLED",
            Some(&json!(1)),
            false
        ));
        assert!(!bool_field(
            &empty,
            "USAGE_STATS_ENABLED",
            Some(&Value::Null),
            false
        ));
    }

    /// 数值解析吃掉前后空白（与 JS 的 `Number(" 8000 ")` 对齐），但拒绝小数与非有限数。
    #[test]
    fn number_parsing_matches_documented_shape() {
        assert_eq!(parse_int_at_least(&json!(" 8000 "), 1), Some(8000));
        assert_eq!(parse_int_at_least(&json!("8e3"), 1), Some(8000));
        assert_eq!(parse_int_at_least(&json!(8000.0), 1), Some(8000));
        assert_eq!(parse_int_at_least(&json!(1.5), 1), None);
        assert_eq!(parse_int_at_least(&json!(true), 1), None);
        assert_eq!(parse_int_at_least(&json!([1]), 1), None);
        // 超出 i64 的一律当看不懂，**不许悄悄截断**。
        assert_eq!(parse_int_at_least(&json!("1e30"), 1), None);
    }
}
