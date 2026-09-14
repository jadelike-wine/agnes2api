//! 注册机的配置装载。
//!
//! 移植自 TS 版 `src/core/registrar/config.ts`（615 行，注册机里最大的单文件），
//! 规则真源 `docs/porting/01-core-rules.md §10.2`。**零 IO**。
//!
//! # 🔴 装载器**零 panic**（唯一豁免是消费方护栏 [`require_channel`]）
//!
//! 注册机是**可选子系统**，它缺凭据不该让转发、`/health`、面板一起死。
//! TS 那一版抛错的结果是**容器起不来**。
//! ⇒ 装载器**只产出 [`RegistrarLoad::blockers`]，启不启动由消费方 gate**。
//!
//! # 🔴 两个集合不许合并
//!
//! - **blocker** =「注册机本次不启动」；
//! - **notice** = **不拦人、但必须说出来**，恰恰配着「注册机照常跑」。
//!
//! ⚠️ **notices 是状态，不是事件。** 走 `GET /admin/api/config`（零写）带出去、
//! 渲染成常驻横幅。做成事件的两条硬理由**各自单独成立**：
//! 1. 装载器每 30 秒刷一次 ⇒ 每实例每天约 2880 次装载，**而事件环只有 100 格，
//!    它会在运维升级完来查问题的那一刻把诊断挤出去**；
//! 2. 补池每轮都重新装载一次，装载期的无条件 warn 会把「健康的一轮零事件零写」
//!    变成「每轮至少一条事件 ⇒ 跨过刷新窗后每轮一次写」。

use super::types::{SCHEDULED_ROUND_BUDGET_MS, worst_attempt_ms, worst_round_ms};
use serde::Serialize;
use serde_json::{Map, Value};

/// 两条邮箱通道 —— **二选一，没有主备、没有自动降级**。
///
/// # 🔴 从 `primary` + `fallback` 改名成 `channel`，**改名不是换措辞**
///
/// **只要类型、存储键、补丁路径、错误码里还留着「主」这个字，内部就还是主备，
/// 界面上把第二个下拉藏起来只是又一个谎。**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Yyds,
    Moemail,
}

impl Channel {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "yyds" => Some(Channel::Yyds),
            "moemail" => Some(Channel::Moemail),
            _ => None,
        }
    }
    /// 公开的解析入口（管理端点用）。
    ///
    /// ⚠️ 与 [`resolve_channel`] 里那条四级链**用的是同一个 [`Channel::parse`]** ——
    /// 各写一份的话，面板认得的通道名与配置认得的会慢慢漂开。
    pub fn parse_public(s: &str) -> Option<Self> {
        Self::parse(s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Yyds => "yyds",
            Channel::Moemail => "moemail",
        }
    }
}

/// 一条通道的凭据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelCreds {
    pub base_url: String,
    pub api_key: String,
}

/// 装载出来的注册机配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrarConfig {
    pub enabled: bool,
    pub channel: Channel,
    pub target_keys: u32,
    pub mint_batch: u32,
    pub tend_interval_ms: i64,
    pub code_timeout_ms: i64,
    pub mint_delay_min_ms: i64,
    pub mint_delay_max_ms: i64,
    pub max_domain_attempts: u32,
    pub token_name: String,
    pub agnes_platform_url: String,
    /// ⚠️ **两条通道都解析**，不是只解析选中那条。理由见 [`resolve_creds`]。
    pub yyds: Option<ChannelCreds>,
    pub moemail: Option<ChannelCreds>,
    /// **装载的产物，不是旋钮。**
    ///
    /// ⚠️ **它为真时 [`enabled`](RegistrarConfig::enabled) 一个字都不改**：
    /// 「运维明明打开了，面板却说未启用」是另一种撒谎。
    /// **真话是「已启用 · 本次没跑起来」。** 消费方一律**先判 `enabled` 再判 `blocked`**。
    /// ⚠️ 它**可以在 `enabled == false` 时为真**（`delay_min_gt_max` 不受门控）。
    pub blocked: bool,
}

pub const DEFAULT_TARGET_KEYS: u32 = 20;
pub const DEFAULT_MINT_BATCH: u32 = 5;
pub const DEFAULT_TEND_INTERVAL_MS: i64 = 1_800_000;
pub const DEFAULT_CODE_TIMEOUT_MS: i64 = 120_000;

/// 🟢 **量出来的下界**：≥60 秒间隔时同一出口连续四次发码全部成功，
/// **第七次才撞上应用层限流**；**无间隔连发时第三次就被上游前置的边缘限流挡下**。
///
/// ⚠️ **这个数是观测不是承诺**：来自单一出口、单日样本，换出口或换时段可能完全不同。
/// **所以它留在可配那一侧。**
pub const DEFAULT_MINT_DELAY_MIN_MS: i64 = 60_000;

/// 🟡 抖动上界，**不是实测**。
pub const DEFAULT_MINT_DELAY_MAX_MS: i64 = 90_000;

/// 🟢 有了域名台账之后，一个名额里的**第二个**域名期望收益为负
///（多烧一格限流预算，而暖机后成功率提升趋近 0）；
/// 而「一个名额里连打好几次」正是实测中触发边缘限流的那个形态。
///
/// ⚠️ **域名轮换没有消失，只是从「一个名额之内」搬到「两个名额之间」。**
pub const DEFAULT_MAX_DOMAIN_ATTEMPTS: u32 = 1;

pub const DEFAULT_TOKEN_NAME: &str = "auto";
pub const DEFAULT_AGNES_PLATFORM_URL: &str = "https://platform-backend.agnes-ai.com";
/// YYDS 有内置取值；MoeMail **没有**（自建服务，地址只能由运维给）。
pub const DEFAULT_YYDS_BASE_URL: &str = "https://maliapi.215.im";

/// 一条拦人的理由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Blocker {
    pub code: &'static str,
    /// 它说的是哪一格（面板路径，例如 `registrar.yyds.apiKey`）。
    ///
    /// 🔴 **构造时就知道，所以在构造时写下来** —— 让配置面去解析 `msg` 把字段名
    /// 抠出来，是本仓点名禁止的那条路：措辞一改，抠的那一下就静默失效。
    pub field: String,
    pub msg: String,
}

/// 一条**不拦人、但必须说出来**的理由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Notice {
    pub code: &'static str,
    /// 同 [`Blocker::field`]。
    pub field: String,
    pub msg: String,
}

/// 一条启动期告警（只 warn、不产 blocker）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Warning {
    pub event: &'static str,
    pub msg: String,
}

/// 装载的完整产物。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegistrarLoad {
    pub config: RegistrarConfig,
    pub blockers: Vec<Blocker>,
    pub notices: Vec<Notice>,
    pub warnings: Vec<Warning>,
}

// ── 取值的四级解析 ────────────────────────────────────────────────────────

/// 「缺席」的判据：`None` / `null` / **空串**都算没写。
///
/// ⚠️ 空串那一条不是洁癖：env 侧的 `REGISTRAR_CHANNEL=` 在 compose 里**极常见**。
fn non_empty(v: Option<&str>) -> Option<&str> {
    v.map(str::trim).filter(|s| !s.is_empty())
}

fn stored_str<'a>(stored: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    non_empty(stored.get(key).and_then(Value::as_str))
}

/// 存储里那一格读成字符串，**数字与布尔一并收下**。
///
/// # 🔴 它治的是一个真缺陷，不是「宽容一点」
///
/// 注册机这一侧从一开始就只按**字符串**读存储（存量数据确实全是字符串），
/// 而配置面 `PUT /admin/api/config` 写进去的是 **JSON 原生类型**
///（`registrar.enabled` 是真布尔、`targetKeys` 是真数字 —— 那是 `Spec` 那张表要求的）。
/// 两边不一致的形态**不是报错，是静默忽略**：
/// 运维在面板上把注册机打开、保存成功，而装载器读不出那个 `true`
/// ⇒ **它一直是关着的，面板上却显示已启用**。
///
/// ⚠️ **不许反过来「让配置面去写字符串」**：那会让面板存进去的
/// `"true"` / `"20"` 与 `Spec::Bool` / `Spec::Int` 的校验互相打架，
/// 而且把一个类型错误埋进存储里。**要收的是读这一侧。**
pub(super) fn stored_scalar(stored: &Map<String, Value>, key: &str) -> Option<String> {
    match stored.get(key)? {
        Value::String(s) => non_empty(Some(s.as_str())).map(str::to_string),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 通道的四级解析：`REGISTRAR_CHANNEL > REGISTRAR_PRIMARY > registrar.channel > registrar.primary`。
///
/// # 🔴 **本级判死就不往下穿透** —— 绝不能写成 `??` 链
///
/// `??` 链会让 `REGISTRAR_CHANNEL=yydss` **静默穿透**成别名或存储里那条通道 ——
/// 运维写错一个字母，网关拿**另一条通道**去跑，**一句话都不说**。
///
/// ⇒ 某一级**在场但认不出**时，这里产出一条 blocker 并**停在这一级**。
/// 第三个返回值 = **有没有人真的选过**。
///
/// 🔴 **「没选」是一个必须说出来的状态，不是「那就用 yyds 吧」。**
/// 设计上两条通道**完全平级**；静默挑一条等于替运维做了一个他没做的决定，
/// 而后果要到「凭据缺失」那条报错指着一条他从没选过的通道时才浮上来。
/// 调用方在 `enabled` 为真时据它产出 `channel_required`
///（与被移植的那一版逐字一致：写错了说 `not_a_channel`，
/// **else if** 压根没选才说 `channel_required` —— 两句话不同时说，
/// 同时说只会让运维以为有两处要改）。
pub fn resolve_channel(
    env_channel: Option<&str>,
    env_primary: Option<&str>,
    stored: &Map<String, Value>,
) -> (Channel, Option<Blocker>, bool) {
    let levels: [(&str, Option<&str>); 4] = [
        ("REGISTRAR_CHANNEL", non_empty(env_channel)),
        ("REGISTRAR_PRIMARY", non_empty(env_primary)),
        ("registrar.channel", stored_str(stored, "channel")),
        ("registrar.primary", stored_str(stored, "primary")),
    ];
    for (source, raw) in levels {
        let Some(raw) = raw else { continue };
        return match Channel::parse(raw) {
            Some(c) => (c, None, true),
            // 🔴 **停在这一级**，不往下穿透。
            None => (
                Channel::Yyds,
                Some(Blocker {
                    code: "registrar_channel_unknown",
                    field: "registrar.channel".into(),
                    msg: format!(
                        "{source} 写的是「{raw}」，不是 yyds 或 moemail —— \
                         本次不启动注册机。**没有往下一级取值**：往下取会让一个笔误变成\
                         「网关拿另一条通道在跑，而且一句话都不说」。"
                    ),
                }),
                // 写错了也算「选过」—— 那一支已经有自己的话要说。
                true,
            ),
        };
    }
    // ⚠️ 这里给的 `Channel::Yyds` 是**占位**，不是取值：`enabled` 为真时
    // 调用方会产出 `channel_required` blocker ⇒ 这一轮压根不会跑，它用不上。
    (Channel::Yyds, None, false)
}

/// 正整数取值：**非法值两侧都降级，不 panic**。
///
/// # 🔴 这是本模块唯一一处**主动放弃的 fail-fast**
///
/// 它与网关 env 那一侧的 `num()` 策略**故意不一致**：那边是「配错就别启动」，
/// 这边是「配错就回落默认值，注册机照样跑」。
/// 依据是**有没有安全的降能模式** —— 注册机有（少铸几把 key），网关没有。
///
/// ⚠️ **不产出 blocker**：字段回落之后注册机照样跑得起来。
/// **代价明写**：运维**不能再靠「容器崩了」发现部署笔误**，只能靠面板红横幅与事件。
///
/// ⚠️ 事件名取 `config.invalid` 而不是 `registrar.*`：
/// 面板与文档里「字段级降级」这件事**只有一条事件名**。
/// 代价是它拿不到 `[registrar]` 前缀，字段路径里的 `registrar.` 是唯一的归属线索。
pub fn pos_int(raw: Option<&str>, default: i64, field: &str) -> (i64, Option<Notice>) {
    let Some(raw) = non_empty(raw) else {
        return (default, None);
    };
    match raw.parse::<i64>() {
        Ok(n) if n >= 1 => (n, None),
        _ => (
            default,
            Some(Notice {
                code: "config.invalid",
                field: format!("registrar.{field}"),
                msg: format!(
                    "registrar.{field} 的值「{raw}」不是不小于 1 的整数，已回落到内置默认值 {default}。\
                     ⚠️ 注册机照常运行 —— 这条只在这里说，容器不会因此起不来。"
                ),
            }),
        ),
    }
}

// ── 旧键规整 ──────────────────────────────────────────────────────────────

/// 旧主备键的规整结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    /// 规整之后该写回存储的那一份。
    pub next: Map<String, Value>,
    pub notices: Vec<Notice>,
    /// 有没有真的动过。为假时**写路径一次写都不许发**。
    pub changed: bool,
}

/// 旧主备键的**唯一真源**。
///
/// 读路径拿 [`Migration::notices`]，写路径拿 [`Migration::next`]。
/// 🔴 **两处共用这一份，不许各写一份** ——
/// 这一条一旦分叉，**面板上的横幅会与存储里的实际键说两句不同的话**。
///
/// 规整动作只有两步：`channel` 缺席时把 `primary` 的值抬上来；然后删掉 `primary` 与 `fallback`。
/// ⇒ **运维在面板上保存任意一次设置，旧键消失、横幅随之消失。**
pub fn migrate_stored_registrar(stored: &Map<String, Value>) -> Migration {
    let mut next = stored.clone();
    let mut notices = Vec::new();

    let had_primary = stored.contains_key("primary");
    let had_fallback = stored.contains_key("fallback");
    if !had_primary && !had_fallback {
        return Migration {
            next,
            notices,
            changed: false,
        };
    }

    // ① `channel` 缺席时把 `primary` 的值抬上来。
    if stored_str(stored, "channel").is_none()
        && let Some(p) = stored_str(stored, "primary")
    {
        next.insert("channel".into(), Value::String(p.to_string()));
    }
    // 抬完之后**本次生效的那条通道**是谁 —— 下面判 fallback 要用它。
    let effective = stored_str(&next, "channel").and_then(Channel::parse);

    // ② 删掉旧键。
    next.remove("primary");
    next.remove("fallback");

    if had_primary {
        notices.push(Notice {
            code: "registrar_legacy_primary_key",
            field: "registrar.channel".into(),
            msg: "存储里还留着旧的 registrar.primary 键。它已被规整成 registrar.channel —— \
                  在面板上保存任意一次设置，旧键就会消失、这条提示也会随之消失。"
                .into(),
        });
    }
    if had_fallback {
        let fb = stored_str(stored, "fallback").and_then(Channel::parse);
        match fb {
            // ⚠️ **`fallback == 本次生效的那条通道` 时一条 notice 都不产**：
            // 那份配置从前就被「备通道等于主通道」那条 blocker 拦着、**从来没有生效过**，
            // 说「丢弃了一条降级路径」是假话。
            Some(f) if Some(f) == effective => {}
            Some(f) => notices.push(Notice {
                code: "registrar_legacy_fallback_dropped",
                field: "registrar.channel".into(),
                msg: format!(
                    "存储里那条备用通道（{}）已被丢弃：今天是**二选一**，没有主备、没有自动降级。",
                    f.as_str()
                ),
            }),
            // ⚠️ **已知边界**：`fallback` 写的是一个非法值时**只删键、不产 notice**
            //（那条通道一把 key 都没铸过，说「丢弃了一条降级路径」同样是假话）。
            None => {}
        }
    }

    Migration {
        next,
        notices,
        changed: true,
    }
}

// ── 凭据 ──────────────────────────────────────────────────────────────────

/// 解析一条通道的凭据。**缺什么就产出哪一条 blocker，不 panic、也不发明取值。**
///
/// # 🔴 两条通道都解析，但**只有选中那条的缺凭据产 blocker**
///
/// 只给「选中那条」解析的话，未选中通道的 `yyds` / `moemail` 恒为 `None`，
/// 而面板的「这条通道配好了吗」判据正是**「这一格非 null」**。三个消费者会连锁失真：
/// `/status` 的 `configured` 变成假话、「测试通道」对未选中那条恒 409、
/// 「自动注册」指定它也恒 409。
/// **于是「想在切换前先测一下另一条」必须先切过去保存 —— 鸡生蛋，
/// 而那恰恰是「二选一」模型下最核心的工作流。**
/// 📜 这里曾登记「今天还没有生产调用方，只被判据调用」。
/// **那条登记已经作废**：装载器（`runtime::load`）就是它的生产调用方，
/// 两条通道各调一次。留下这行是因为那条登记本来是防「被当成死代码删掉」的，
/// 而今天防它的是调用方本身。
pub fn resolve_creds(
    which: Channel,
    base_url: Option<&str>,
    api_key: Option<&str>,
    selected: Channel,
) -> (Option<ChannelCreds>, Vec<Blocker>) {
    let name = which.as_str();
    let default_base = match which {
        Channel::Yyds => Some(DEFAULT_YYDS_BASE_URL),
        // MoeMail 是自建服务，**没有内置取值**。
        Channel::Moemail => None,
    };
    let base = non_empty(base_url).or(default_base);
    let key = non_empty(api_key);

    let mut blockers = Vec::new();
    // 只有选中那条缺凭据才拦人。
    if which == selected {
        if base.is_none() {
            blockers.push(Blocker {
                code: "registrar_missing_base_url",
                field: format!("registrar.{}.baseUrl", which.as_str()),
                msg: format!(
                    "选中的通道 {name} 没有地址，而它没有内置默认值 —— 本次不启动注册机。"
                ),
            });
        }
        if key.is_none() {
            blockers.push(Blocker {
                code: "registrar_missing_api_key",
                field: format!("registrar.{}.apiKey", which.as_str()),
                msg: format!("选中的通道 {name} 没有凭据 —— 本次不启动注册机。"),
            });
        }
    }

    let creds = match (base, key) {
        (Some(b), Some(k)) => Some(ChannelCreds {
            base_url: b.to_string(),
            api_key: k.to_string(),
        }),
        _ => None,
    };
    (creds, blockers)
}

// ── 三条启动期告警 ────────────────────────────────────────────────────────

/// 三条启动期告警（**只 warn、不产 blocker**）。
///
/// 都受 `enabled` 门控：关着的注册机不打这三条。
pub fn startup_warnings(cfg: &RegistrarConfig) -> Vec<Warning> {
    let mut out = Vec::new();
    if !cfg.enabled {
        return out;
    }

    // ① 间隔比单轮最坏耗时还短 ⇒ 两轮会重叠。
    let worst_round = worst_round_ms(cfg.mint_batch, cfg.code_timeout_ms, cfg.mint_delay_max_ms);
    if cfg.tend_interval_ms < worst_round {
        out.push(Warning {
            event: "registrar.interval_shorter_than_worst_round",
            msg: format!(
                "TEND_INTERVAL_MS（{}）小于单轮最坏耗时（{worst_round}）—— \
                 两轮可能重叠。单轮最坏 = MINT_BATCH×CODE_TIMEOUT_MS 加上名额之间的 MINT_DELAY_MAX_MS 间隔。",
                cfg.tend_interval_ms,
            ),
        });
    }

    // ② 调大域名尝试数的代价。
    //
    // ⚠️ **这条 warn 是「调大它的代价」唯一说得出口的地方** ——
    // 面板上它只是一个数字输入框。
    if cfg.max_domain_attempts > 2 {
        out.push(Warning {
            event: "registrar.domain_attempts_costly",
            msg: format!(
                "MAX_DOMAIN_ATTEMPTS = {} ——「一个名额里连打好几次」正是实测中触发边缘限流的那个形态，\
                 而暖机后第二个域名的期望收益趋近 0。域名轮换本来就在「两个名额之间」做。",
                cfg.max_domain_attempts,
            ),
        });
    }

    // ③ 一次尝试的最坏耗时超过定时轮预算。
    let worst_attempt = worst_attempt_ms(
        cfg.max_domain_attempts,
        cfg.code_timeout_ms,
        cfg.mint_delay_max_ms,
    );
    if worst_attempt > SCHEDULED_ROUND_BUDGET_MS {
        out.push(Warning {
            // 🟡 **事件名里那个 `worker` 是一处已知残留，刻意没改**：
            // 它是五语言文档逐字给运维的 grep 锚点，改名必须**源码与五份文档同一次改完**。
            // 建议的新名是 `registrar.attempt_exceeds_round_budget`。
            event: "registrar.attempt_exceeds_worker_budget",
            msg: format!(
                "一次尝试的最坏耗时（{worst_attempt}）超过定时轮预算（{SCHEDULED_ROUND_BUDGET_MS}）。\
                 真实后果是：一轮补池没有任何东西会把它截断，而补池锁的有效期是 900 秒 —— \
                 **这一轮可能跑过自己那把锁的有效期**，锁一过期，下一轮触发或另一个共卷副本\
                 就会在它还活着的时候并发开跑。",
            ),
        });
    }
    out
}

// ── 消费方护栏 ────────────────────────────────────────────────────────────

/// 本模块**唯一的 panic 豁免**。
///
/// 🔴 **它不在装载路径上** —— 装载器全函数化说的是「一份坏配置不该让网关起不来」，
/// 而这里是**消费方护栏**：走到这里还没有可用通道，
/// 说明某个消费者**跳过了 `enabled` / `blocked` 两道 gate**，**那是代码 bug，必须响。**
///
/// # Panics
/// 配置被 blocker 拦下（`blocked`）或整个关着（`!enabled`）时 panic。
pub fn require_channel(cfg: &RegistrarConfig) -> (Channel, &ChannelCreds) {
    assert!(
        cfg.enabled,
        "require_channel 在注册机关着的时候被调用了 —— \
         调用方跳过了 enabled 这道 gate，这是代码 bug",
    );
    assert!(
        !cfg.blocked,
        "require_channel 在注册机被 blocker 拦下的时候被调用了 —— \
         调用方跳过了 blocked 这道 gate，这是代码 bug",
    );
    let creds = match cfg.channel {
        Channel::Yyds => cfg.yyds.as_ref(),
        Channel::Moemail => cfg.moemail.as_ref(),
    };
    (
        cfg.channel,
        creds.expect("blocked 为假却没有选中通道的凭据 —— 装载器自相矛盾，这是代码 bug"),
    )
}

/// 注册机自己的 blocker / notice 码 → 配置面那张 22 码全表里的一格。
///
/// 🔴 **一张显式的表，而不是让配置面去解析 `msg`。**
/// 认不出来的一律回 [`ConfigErrorCode::ConfigUnloadable`]：那是「说不出是哪一格」
/// 的兜底档，**比编一个具体错误码出来诚实**。
///
/// ⚠️ **一个码今天没有产生点，如实登记**：`LegacyChannelEnv`
///（旧的 `REGISTRAR_PRIMARY` 环境变量走的是 `env` 优先级，不产 notice）。
/// **别为了凑满一张表去造产生点。**
///
/// 📜 `ChannelRequired` 曾经也在这一行里，理由写着「今天通道恒有默认值，
/// "没选通道" 这个状态不存在」。**那条理由本身就是缺陷**：
/// 恒有默认值 = 替运维挑了一条，而两条通道是完全平级的。已修，它现在有产生点了。
#[must_use]
pub fn config_error_code_of(code: &str) -> crate::core::config_errors::ConfigErrorCode {
    use crate::core::config_errors::ConfigErrorCode as C;
    match code {
        "registrar_delay_min_gt_max" => C::DelayMinGtMax,
        "registrar_channel_unknown" => C::NotAChannel,
        "registrar_channel_required" => C::ChannelRequired,
        // ⚠️ 地址与凭据合成一档：两者缺任一，这条通道都**用不了**，
        // 而面板那句话与运维的下一步动作（去把这条通道填全）逐字相同。
        "registrar_missing_api_key" | "registrar_missing_base_url" => C::ChannelCredentialsMissing,
        "registrar_legacy_primary_key" => C::LegacyChannelKey,
        "registrar_legacy_fallback_dropped" => C::LegacyFallbackIgnored,
        _ => C::ConfigUnloadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stored(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap_or_default()
    }
    fn cfg() -> RegistrarConfig {
        RegistrarConfig {
            enabled: true,
            channel: Channel::Yyds,
            target_keys: DEFAULT_TARGET_KEYS,
            mint_batch: DEFAULT_MINT_BATCH,
            tend_interval_ms: DEFAULT_TEND_INTERVAL_MS,
            code_timeout_ms: DEFAULT_CODE_TIMEOUT_MS,
            mint_delay_min_ms: DEFAULT_MINT_DELAY_MIN_MS,
            mint_delay_max_ms: DEFAULT_MINT_DELAY_MAX_MS,
            max_domain_attempts: DEFAULT_MAX_DOMAIN_ATTEMPTS,
            token_name: DEFAULT_TOKEN_NAME.into(),
            agnes_platform_url: DEFAULT_AGNES_PLATFORM_URL.into(),
            yyds: Some(ChannelCreds {
                base_url: DEFAULT_YYDS_BASE_URL.into(),
                api_key: "k".into(),
            }),
            moemail: None,
            blocked: false,
        }
    }

    // ── 通道解析 ──────────────────────────────────────────────────────

    /// 🔴 **这一格钉的是那条「本级判死就不往下穿透」。**
    ///
    /// 变异对象：把 `resolve_channel` 写成 `??` 链（认不出就 `continue`）——
    /// 运维写错一个字母，网关拿**另一条通道**去跑，**一句话都不说**。
    #[test]
    fn a_typo_in_the_env_channel_never_falls_through_to_a_lower_level() {
        let s = stored(json!({"channel": "moemail"}));
        let (c, b, chosen) = resolve_channel(Some("yydss"), None, &s);
        let b = b.expect("写错一个字母却没有任何人说话 —— 那正是这条规则要挡的");
        assert_eq!(b.code, "registrar_channel_unknown");
        assert!(b.msg.contains("yydss"), "报文里没有那个笔误：{}", b.msg);
        // 返回值虽然有个占位，但 blocker 在场 ⇒ 消费方不会用它。
        let _ = c;
        assert!(
            chosen,
            "⚠️ 写错了也算「选过」—— 否则会再叠一条 `channel_required`，\
             而两句话同时说只会让运维以为有两处要改",
        );
    }

    /// 🔴 **一条都没选 ⇒ 如实说「没选」，不静默挑一条。**
    ///
    /// 静默挑一条等于替运维做了一个他没做的决定，而后果要到
    /// 「凭据缺失」那条报错**指着一条他从没选过的通道**时才浮上来。
    #[test]
    fn nothing_chosen_is_reported_as_nothing_chosen() {
        let (_, b, chosen) = resolve_channel(None, None, &stored(json!({})));
        assert!(
            b.is_none(),
            "「没选」本身不在这一层产 blocker（它受 enabled 门控）"
        );
        assert!(
            !chosen,
            "🔴 一条都没选却报告「选过了」—— 两条通道就不再是平级的了",
        );
    }

    #[test]
    fn the_four_levels_are_tried_in_order() {
        let s = stored(json!({"channel": "yyds", "primary": "moemail"}));
        assert_eq!(
            resolve_channel(Some("moemail"), None, &s).0,
            Channel::Moemail
        );
        assert_eq!(
            resolve_channel(None, Some("moemail"), &s).0,
            Channel::Moemail
        );
        assert_eq!(resolve_channel(None, None, &s).0, Channel::Yyds);
        let only_primary = stored(json!({"primary": "moemail"}));
        assert_eq!(
            resolve_channel(None, None, &only_primary).0,
            Channel::Moemail
        );
    }

    /// ⚠️ 「缺席」四级同源：`None` / 空串 / 纯空白都算没写。
    ///
    /// 空串那一条不是洁癖 —— `REGISTRAR_CHANNEL=` 在 compose 里极常见。
    #[test]
    fn an_empty_string_counts_as_absent_at_every_level() {
        let s = stored(json!({"channel": "", "primary": "moemail"}));
        assert_eq!(
            resolve_channel(Some(""), Some("   "), &s).0,
            Channel::Moemail
        );
        assert!(resolve_channel(Some(""), Some("   "), &s).1.is_none());
    }

    // ── pos_int ──────────────────────────────────────────────────────

    /// 🔴 非法值**两侧都降级，不 panic，也不产 blocker**。
    #[test]
    fn an_illegal_number_degrades_to_the_default_and_only_says_so() {
        for bad in ["0", "-3", "abc", "1.5", "999999999999999999999"] {
            let (v, n) = pos_int(Some(bad), 20, "targetKeys");
            assert_eq!(v, 20, "「{bad}」没有回落到默认值");
            let n = n.expect("回落了却一个字都不说");
            assert_eq!(n.code, "config.invalid", "字段级降级只该有这一条事件名");
            assert!(n.msg.contains("registrar.targetKeys"), "{}", n.msg);
        }
    }

    #[test]
    fn a_legal_number_passes_through_silently() {
        assert_eq!(pos_int(Some("7"), 20, "x"), (7, None));
        assert_eq!(pos_int(None, 20, "x"), (20, None));
        assert_eq!(pos_int(Some(""), 20, "x"), (20, None));
    }

    // ── 旧键规整 ──────────────────────────────────────────────────────

    #[test]
    fn a_store_without_legacy_keys_is_left_completely_alone() {
        let m = migrate_stored_registrar(&stored(json!({"channel": "yyds"})));
        assert!(!m.changed, "没有旧键却说自己动过 ⇒ 写路径会白写一次");
        assert!(m.notices.is_empty());
    }

    #[test]
    fn the_primary_value_is_lifted_into_channel_and_the_old_keys_are_removed() {
        let m = migrate_stored_registrar(&stored(json!({"primary": "moemail"})));
        assert_eq!(
            m.next.get("channel").and_then(Value::as_str),
            Some("moemail")
        );
        assert!(!m.next.contains_key("primary"));
        assert!(m.changed);
        assert!(
            m.notices
                .iter()
                .any(|n| n.code == "registrar_legacy_primary_key")
        );
    }

    /// ⚠️ **`fallback` 等于本次生效的那条通道时，一条 notice 都不产。**
    ///
    /// 那份配置从前就被「备通道等于主通道」那条 blocker 拦着、**从来没有生效过**，
    /// 说「丢弃了一条降级路径」是假话。
    #[test]
    fn a_fallback_equal_to_the_effective_channel_is_dropped_without_claiming_a_loss() {
        let m = migrate_stored_registrar(&stored(json!({"primary": "yyds", "fallback": "yyds"})));
        assert!(!m.next.contains_key("fallback"));
        assert!(
            !m.notices
                .iter()
                .any(|n| n.code == "registrar_legacy_fallback_dropped"),
            "说了一句「丢弃了一条降级路径」，而那条路从来没生效过",
        );
    }

    #[test]
    fn a_real_fallback_is_reported_as_dropped() {
        let m =
            migrate_stored_registrar(&stored(json!({"primary": "yyds", "fallback": "moemail"})));
        let n = m
            .notices
            .iter()
            .find(|n| n.code == "registrar_legacy_fallback_dropped");
        assert!(n.is_some(), "真丢了一条降级路径却没说");
        assert!(n.unwrap().msg.contains("moemail"));
    }

    /// ⚠️ **已知边界**：`fallback` 是非法值时**只删键、不产 notice**。
    #[test]
    fn an_unparseable_fallback_is_removed_quietly() {
        let m =
            migrate_stored_registrar(&stored(json!({"primary": "yyds", "fallback": "nonsense"})));
        assert!(!m.next.contains_key("fallback"));
        assert!(
            !m.notices
                .iter()
                .any(|n| n.code == "registrar_legacy_fallback_dropped")
        );
    }

    // ── 凭据 ──────────────────────────────────────────────────────────

    /// 🔴 **这一格钉的是那条「两条通道都解析」。**
    ///
    /// 变异对象：只给选中那条解析 —— 未选中通道那一格恒 `None`，
    /// 于是面板的「这条通道配好了吗」恒为假，「想在切换前先测一下另一条」
    /// 必须先切过去保存，**鸡生蛋**。
    #[test]
    fn the_unselected_channel_is_still_parsed_so_the_panel_can_tell_it_is_configured() {
        let (creds, blockers) = resolve_creds(
            Channel::Moemail,
            Some("https://m.test"),
            Some("k"),
            Channel::Yyds,
        );
        assert!(creds.is_some(), "未选中的通道没被解析 —— 面板会说它没配置");
        assert!(blockers.is_empty(), "未选中的通道不该拦人");
    }

    /// 只有**选中那条**缺凭据才拦人。
    #[test]
    fn only_the_selected_channel_can_block_the_registrar() {
        let (_, sel) = resolve_creds(Channel::Moemail, None, None, Channel::Moemail);
        assert_eq!(sel.len(), 2, "选中的通道缺地址缺凭据，该有两条 blocker");
        let (_, unsel) = resolve_creds(Channel::Moemail, None, None, Channel::Yyds);
        assert!(unsel.is_empty(), "未选中的通道缺凭据也拦了人");
    }

    /// YYDS 有内置地址，MoeMail **没有**（自建服务）。
    #[test]
    fn only_yyds_has_a_built_in_base_url() {
        let (y, yb) = resolve_creds(Channel::Yyds, None, Some("k"), Channel::Yyds);
        assert_eq!(y.unwrap().base_url, DEFAULT_YYDS_BASE_URL);
        assert!(yb.is_empty());
        let (m, mb) = resolve_creds(Channel::Moemail, None, Some("k"), Channel::Moemail);
        assert!(m.is_none(), "给自建服务发明了一个地址");
        assert!(mb.iter().any(|b| b.code == "registrar_missing_base_url"));
    }

    // ── 启动期告警 ────────────────────────────────────────────────────

    /// 关着的注册机三条都不打。
    #[test]
    fn a_disabled_registrar_emits_no_startup_warnings() {
        let mut c = cfg();
        c.enabled = false;
        c.max_domain_attempts = 9;
        c.tend_interval_ms = 1;
        assert!(startup_warnings(&c).is_empty());
    }

    /// 默认配置一条告警都不该打 —— 否则每次启动都刷一条噪声。
    #[test]
    fn the_default_configuration_is_quiet() {
        assert!(
            startup_warnings(&cfg()).is_empty(),
            "默认配置就在报警，那三条会被当噪声忽略"
        );
    }

    /// ⚠️ 名额之间那一项**必须**算进去，否则这条告警会在真的会重叠的配置上沉默。
    #[test]
    fn the_overlap_warning_counts_the_gaps_between_slots() {
        let mut c = cfg();
        // 5 个名额 × 120 秒 = 600 秒；加上 4 段 × 90 秒 = 960 秒。
        // 间隔取 700 秒：漏掉 gaps 时不会报，算上才会报。
        c.tend_interval_ms = 700_000;
        let w = startup_warnings(&c);
        assert!(
            w.iter()
                .any(|w| w.event == "registrar.interval_shorter_than_worst_round"),
            "漏算了名额之间的间隔 —— 一份真会重叠的配置上这条告警沉默了",
        );
    }

    #[test]
    fn raising_the_domain_attempts_past_two_says_what_it_costs() {
        let mut c = cfg();
        c.max_domain_attempts = 3;
        let w = startup_warnings(&c);
        let one = w
            .iter()
            .find(|w| w.event == "registrar.domain_attempts_costly");
        assert!(
            one.is_some(),
            "面板上它只是个数字输入框 —— 这条 warn 是代价唯一说得出口的地方"
        );
        // 2 不报（边界）。
        c.max_domain_attempts = 2;
        assert!(
            !startup_warnings(&c)
                .iter()
                .any(|w| w.event == "registrar.domain_attempts_costly")
        );
    }

    /// 🔴 这一格钉的是那条**报文已经被订正过**的告警。
    ///
    /// ⚠️ 原报文后半句写着「Node/Docker 上同样铸不出来」——
    /// **那句话在摘掉 Worker 形态之前就是假的**：手动补池把等码超时压到 60 秒，
    /// 永远撞不到这个预算。今天真实的后果是**一轮可能跑过补池锁的有效期**。
    #[test]
    fn the_budget_warning_says_the_real_consequence_not_the_old_false_one() {
        let mut c = cfg();
        c.code_timeout_ms = 800_000; // 单独一项就超过 780 秒预算
        let w = startup_warnings(&c);
        let one = w
            .iter()
            .find(|w| w.event == "registrar.attempt_exceeds_worker_budget")
            .expect("超预算却没告警");
        assert!(
            one.msg.contains("锁的有效期"),
            "报文没说真实后果：{}",
            one.msg
        );
        assert!(
            !one.msg.contains("铸不出"),
            "报文里还留着那句已被证伪的话：{}",
            one.msg,
        );
    }

    /// 🟡 事件名里那个 `worker` 是**刻意留的**残留（五语言文档的 grep 锚点）。
    ///
    /// 这一格不是在夸它，是**把「刻意」钉住** —— 哪天有人顺手改名，
    /// 这一格会红并提醒他：**源码与五份文档必须同一次改完**。
    #[test]
    fn the_event_name_keeps_its_known_leftover_on_purpose() {
        let mut c = cfg();
        c.code_timeout_ms = 800_000;
        let w = startup_warnings(&c);
        assert!(
            w.iter()
                .any(|w| w.event == "registrar.attempt_exceeds_worker_budget"),
            "事件名被改了 —— 它是五语言 REGISTRAR.md 逐字给运维的 grep 锚点，\
             改名必须源码与五份文档同一次改完（建议新名 registrar.attempt_exceeds_round_budget）",
        );
    }

    // ── 消费方护栏 ────────────────────────────────────────────────────

    #[test]
    fn require_channel_returns_the_selected_channel_and_its_creds() {
        let c = cfg();
        let (ch, creds) = require_channel(&c);
        assert_eq!(ch, Channel::Yyds);
        assert_eq!(creds.api_key, "k");
    }

    /// 🔴 **唯一的 panic 豁免** —— 它守的是「消费者跳过了 gate」这个代码 bug。
    #[test]
    #[should_panic(expected = "跳过了 enabled")]
    fn require_channel_panics_when_the_caller_skipped_the_enabled_gate() {
        let mut c = cfg();
        c.enabled = false;
        require_channel(&c);
    }

    #[test]
    #[should_panic(expected = "跳过了 blocked")]
    fn require_channel_panics_when_the_caller_skipped_the_blocked_gate() {
        let mut c = cfg();
        c.blocked = true;
        require_channel(&c);
    }

    /// ⚠️ `blocked` 为真时 `enabled` **一个字都不改** ——
    /// 「运维明明打开了，面板却说未启用」是另一种撒谎。
    #[test]
    fn being_blocked_never_flips_enabled_to_false() {
        let mut c = cfg();
        c.blocked = true;
        assert!(c.enabled, "真话是「已启用 · 本次没跑起来」，不是「未启用」");
    }
}
