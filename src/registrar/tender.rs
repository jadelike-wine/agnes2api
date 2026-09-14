//! 补池一轮的编排。
//!
//! 移植自 TS 版 `src/core/registrar/tender.ts`（889 行，注册机的核心），
//! 规则真源 `docs/porting/01-core-rules.md §10.5`。
//!
//! # 它不是零 IO 的，但依赖全部从外面注进来
//!
//! 时钟、随机、睡眠、存储、上游都走 [`TendDeps`]。这不是为了「可测」这句空话 ——
//! 这一轮里**每一条分支都对应一次真实的上游请求或一次真实的落盘**，
//! 而这些分支的正确性只有在**能精确控制时钟与上游回话**的前提下才验得了。

use super::backoff::{BackoffKind, BackoffState, in_backoff, next_backoff};
use super::config::RegistrarConfig;
use super::domain_ledger::{
    CommitOutcome, DomainLedger, DomainState, Observation, commit_journal, select_domains,
};
use super::types::worst_attempt_ms;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};

/// 一轮里可能出现的失败归因。
///
/// # 🔴 表里有两个成员**不是「失败」**，别按字面读
///
/// - [`KeySuspicious`](TendFailureReason::KeySuspicious) —— **铸出来了**。
///   这一轮的 `minted` 照常 +1，因为**上游那边账号是真的建出来了、
///   一个临时邮箱是真的花掉了，说 0 是伪造**。它出现在 `failures` 里，
///   是因为 `failures` 是这份结果里**唯一**能逐条带 `channel` 说明
///   「这一轮有什么不对劲」的通道，**而运维必须在补池历史那一行上看得见它**。
/// - [`UpstreamBackoff`](TendFailureReason::UpstreamBackoff) —— 这一轮**一次都没开始**。
///
/// # 🔴 `RoundCrashed` / `UpstreamBackoff` 为什么各占一个成员
///
/// **「这一轮根本没跑」不该靠合读 `attempted == 0 && failures[0]` 去推** ——
/// 面板的补池历史上这一行必须**能自己说清发生了什么**。
///
/// 🔴 **绝不许改用 `skipped: true` 表示它们**：
/// **[`TendResult::skipped`] 有且只有一个含义（`config.enabled == false`），
/// 拿它表示别的就是伪造。**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TendFailureReason {
    DomainBlockedAll,
    UpstreamError,
    CodeTimeout,
    RegisterFailed,
    LoginFailed,
    KeyFailed,
    ProviderError,
    NetworkError,
    /// ⚠️ **不区分撞的是哪一层限流**：两档都归到这一个，
    /// 层级由退避键的 `kind` 与事件带出去。
    /// **代价如实登记**：只看补池历史那一行分不出上一次撞的是哪一层。
    RateLimited,
    /// 选中的通道压根没构造出 provider —— 是**接线错误**。
    ProviderMissing,
    /// **整轮抛出了异常。**
    ///
    /// 🔴 **Rust 侧没有任何产生点，而这是如实登记、不是漏写。**
    /// [`super::runtime::run_round`] 是**全函数**：每条路径都返回一个
    /// [`TendResult`]，不存在「整轮抛出去」这种收场；真 panic 是 bug，
    /// 它会一路冒到 HTTP 层变成 500，而不是变成一行补池历史。
    ///
    /// ⚠️ **留着它不是为了将来用，是为了读得懂存量历史**：
    /// TS 版写进 `tend:history` 的行里有 `round_crashed`，
    /// 而那张表「任一字段不合整条丢」—— 删掉这个成员 =
    /// **升级那一刻把历史上所有崩过的那几轮从面板上抹掉**。
    RoundCrashed,
    /// **铸出来了，但那串 key 材料本身可疑。**
    KeySuspicious,
    /// **这一轮还在退避窗口里，一次都没开始。**
    UpstreamBackoff,
}

impl TendFailureReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::DomainBlockedAll => "domain_blocked_all",
            Self::UpstreamError => "upstream_error",
            Self::CodeTimeout => "code_timeout",
            Self::RegisterFailed => "register_failed",
            Self::LoginFailed => "login_failed",
            Self::KeyFailed => "key_failed",
            Self::ProviderError => "provider_error",
            Self::NetworkError => "network_error",
            Self::RateLimited => "rate_limited",
            Self::ProviderMissing => "provider_missing",
            Self::RoundCrashed => "round_crashed",
            Self::KeySuspicious => "key_suspicious",
            Self::UpstreamBackoff => "upstream_backoff",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Failure {
    pub reason: TendFailureReason,
    pub channel: String,
}

/// 一轮的结果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TendResult {
    /// 🔴 **有且只有一个含义：`config.enabled == false`。**
    pub skipped: bool,
    /// 本轮开始时**占着 `target_keys` 名额**的 key 数。
    ///
    /// ⚠️ **不要读成「能打上游的 key 数」**：**被管理员停用的、以及正在冷却的 key
    /// 都计入这个数**，而这两种恰恰都是不能打上游的。
    /// **整池 3 把全在冷却时这个数仍然说 3，实际能服务的是 0。**
    /// 这一栏等于「池子里没被剔除的把数」。
    ///
    /// 名字没改是因为它已经落进 `tend:history` 持久化、也进了运维日志。
    /// 🔴 **给建补池历史板块的人：这一栏不许按「可用」渲染。**
    pub available: u32,
    pub attempted: u32,
    pub minted: u32,
    /// ⚠️ 今天与 `minted` + `primary_channel` 等价（二选一之后一轮只可能有一条通道产出）。
    /// **保留它的唯一理由是存量历史条目的形状。**
    /// 没铸出来的通道**不出现在表里**（不是记 0）。
    pub minted_by_channel: HashMap<String, u32>,
    pub failures: Vec<Failure>,
    /// 这一轮**开始**的时刻（不是结束时刻）。
    pub at: i64,
    /// ⚠️ 名字里的 `primary` 是**历史格式**留下的，今天系统里已经没有主备了。
    /// **名字不改**：它已经逐字持久化进 `tend:history`
    ///（改名会让升级前的历史条目被整条判成 malformed 丢掉），
    /// 而且写在五语言 API.md 的响应示例里 —— **那是已发过 tag 的公开仓的响应体契约。**
    pub primary_channel: String,
    pub duration_ms: i64,
}

/// 把失败聚合成 `yyds:register_failed×3 moemail:code_timeout×1`。
///
/// **两个入口的收尾日志必须给出同一份口径**，否则排障方式就得按形态写两套。
pub fn summarize_failures(failures: &[Failure]) -> String {
    let mut counts: Vec<(String, u32)> = Vec::new();
    for f in failures {
        let k = format!("{}:{}", f.channel, f.reason.as_str());
        match counts.iter_mut().find(|(s, _)| *s == k) {
            Some((_, n)) => *n += 1,
            None => counts.push((k, 1)),
        }
    }
    counts
        .into_iter()
        .map(|(k, n)| format!("{k}×{n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

// ── 注进来的依赖 ──────────────────────────────────────────────────────────

/// 一次铸号的结果（[`Minter`] 的产物）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintOutcome {
    Ok {
        key: String,
    },
    RateLimited {
        limit_kind: BackoffKind,
        marker: Option<String>,
    },
    Failed {
        reason: TendFailureReason,
    },
}

/// 一次铸号要用到的那几样。
pub struct MintRequest {
    pub candidates: Vec<String>,
    pub code_timeout_ms: i64,
}

/// 铸号器。真正的实现在 `mint`（还没写）。
#[async_trait]
pub trait Minter: Send + Sync {
    /// 铸一把 key。**观测本子由实现方往里记**（哪个域名被上游怎么看待）。
    async fn mint_one(&self, req: MintRequest, journal: &mut Vec<Observation>) -> MintOutcome;
}

/// 一轮要用到的存储面。
#[async_trait]
pub trait TendStore: Send + Sync {
    async fn load_backoff(&self) -> Option<BackoffState>;
    /// `None` = 把那把键删掉。
    async fn save_backoff(&self, next: Option<BackoffState>) -> Result<(), String>;
    async fn load_domain_ledger(&self) -> DomainLedger;
    async fn save_domain_ledger(&self, next: &DomainLedger) -> Result<(), String>;
}

/// 一轮要用到的池面。
#[async_trait]
pub trait TendRepo: Send + Sync {
    /// 占着 `target_keys` 名额的把数。
    async fn counts_toward_target(&self) -> u32;
    /// **照存不误**：刻意不加校验、也不 `trim()`。见 [`TendDeps::is_importable_key`]。
    async fn add(&self, key: &str) -> Result<(), String>;
    /// 真的铸出 key 之后再对一次账。
    async fn reconcile_after_mint(&self) -> Result<(), String>;
}

/// 上游邮箱通道。
#[async_trait]
pub trait DomainSource: Send + Sync {
    async fn list_domains(&self) -> Result<Vec<String>, String>;
}

/// 一条日志。`tender` 不认识具体的 logger，只往外扔。
pub struct TendEvent {
    pub level: &'static str,
    pub event: &'static str,
    pub msg: String,
    pub fields: Vec<(&'static str, String)>,
}

/// 一轮的全部外部依赖。
pub struct TendDeps<'a> {
    pub config: &'a RegistrarConfig,
    pub repo: &'a dyn TendRepo,
    pub store: &'a dyn TendStore,
    pub domains: &'a dyn DomainSource,
    pub minter: &'a dyn Minter,
    pub now: &'a (dyn Fn() -> i64 + Sync),
    /// `[0, 1)`。
    pub rand: &'a (dyn Fn() -> f64 + Sync),
    pub sleep: &'a (dyn Fn(i64) -> futures_core::future::BoxFuture<'static, ()> + Sync),
    pub emit: &'a (dyn Fn(TendEvent) + Sync),
    /// 🔴 **只有手动那一轮传**，定时轮一直不传。见 `types::SCHEDULED_ROUND_BUDGET_MS`。
    pub round_budget_ms: Option<i64>,
    /// 这一串 key 材料能不能安全地拼进请求头。
    pub is_importable_key: &'a (dyn Fn(&str) -> bool + Sync),
}

fn ev(
    level: &'static str,
    event: &'static str,
    msg: impl Into<String>,
    fields: Vec<(&'static str, String)>,
) -> TendEvent {
    TendEvent {
        level,
        event,
        msg: msg.into(),
        fields,
    }
}

/// 本轮已被上游**明确拒过**的域名（从本轮观测本子现算）。
///
/// **同一个域名以最后一条观测为准**（先被拒、后来又成功过的，这一轮不该再被让到后面 ——
/// 口径与 [`commit_journal`] 的折叠逐字相同）。
///
/// ⚠️ **不落盘、不跨轮**：下一轮从空集重新开始 —— **一个好域名不可能被它永久降权。**
fn rejected_this_round(journal: &[Observation]) -> HashSet<String> {
    let mut last: HashMap<&str, DomainState> = HashMap::new();
    for o in journal {
        last.insert(&o.domain, o.verdict);
    }
    last.into_iter()
        .filter(|(_, v)| *v == DomainState::Blocked)
        .map(|(d, _)| d.to_string())
        .collect()
}

/// 跑一轮补池。
///
/// # 顶层流程
///
/// ```text
/// 1. enabled == false            ⇒ skipped: true，一次上游请求都不发
/// 2. need = target_keys - available；need <= 0 ⇒ 直接返回（**走不到收尾**）
/// 3. 退避闸：还在窗口里 ⇒ 整轮跳过（failures: [upstream_backoff]）
/// 4. 准备：读台账一次 + 列域名一次
/// 5. for i in 0..rounds：睡间隔 → 预算判据 → 选域名 → 铸一把 → 按 reason 处置
/// 6. 收尾（**一定会跑**）：台账最多写 1 次、退避键按需写 1 次
/// 7. minted > 0 ⇒ 再对一次账
/// ```
pub async fn tend_once(deps: &TendDeps<'_>) -> TendResult {
    let cfg = deps.config;
    let channel = cfg.channel.as_str().to_string();
    let started_at = (deps.now)();

    // ── 1. 关着 ──────────────────────────────────────────────────────
    if !cfg.enabled {
        return TendResult {
            skipped: true,
            available: 0,
            attempted: 0,
            minted: 0,
            minted_by_channel: HashMap::new(),
            failures: Vec::new(),
            at: started_at,
            // `skipped` 时记空串（历史格式如此）。
            primary_channel: String::new(),
            duration_ms: 0,
        };
    }

    let available = deps.repo.counts_toward_target().await;
    let mk = |attempted: u32,
              minted: u32,
              minted_by_channel: HashMap<String, u32>,
              failures: Vec<Failure>,
              now: i64| TendResult {
        skipped: false,
        available,
        attempted,
        minted,
        minted_by_channel,
        failures,
        at: started_at,
        primary_channel: channel.clone(),
        duration_ms: now - started_at,
    };

    // ── 2. 没有缺口 ──────────────────────────────────────────────────
    //
    // 🔴 **这一支走不到收尾**，所以一把 `hits` 很高的退避键会一直留在存储里 ——
    // 那正是 `backoff::streak_broken` 要治的那条驻留路径。
    let need = cfg.target_keys.saturating_sub(available);
    if need == 0 {
        return mk(0, 0, HashMap::new(), Vec::new(), (deps.now)());
    }

    // ── 3. 退避闸 ────────────────────────────────────────────────────
    let backoff = deps.store.load_backoff().await;
    if in_backoff(backoff.as_ref(), (deps.now)()) {
        let until = backoff.as_ref().map_or(0, |b| b.until);
        (deps.emit)(ev(
            "info",
            "registrar.round_skipped_backoff",
            "还在上游限流的退避窗口里，这一轮一次请求都不发。",
            vec![("until", until.to_string())],
        ));
        return mk(
            0,
            0,
            HashMap::new(),
            vec![Failure {
                reason: TendFailureReason::UpstreamBackoff,
                channel: channel.clone(),
            }],
            (deps.now)(),
        );
    }

    // ── 4. 准备阶段 ──────────────────────────────────────────────────
    let ledger = deps.store.load_domain_ledger().await;
    let listed = match deps.domains.list_domains().await {
        Ok(v) => v,
        Err(e) => {
            (deps.emit)(ev(
                "warn",
                "registrar.list_domains_failed",
                e,
                vec![("channel", channel.clone())],
            ));
            Vec::new()
        }
    };

    // 🔴 **起点挪到准备阶段之后，这是一条修复不是整理。**
    //
    // 预算判据问的是「**还开不开得起一次完整的尝试**」。而从整轮开头算起时，
    // `elapsed` 里装着读池、读退避、读台账、列域名 —— **其中列域名单请求就允许 15 秒**。
    // 手动轮预算 70 秒、最坏尝试恒 60 秒 ⇒ **留给 elapsed 的只有 10 秒**
    // ⇒ 上游邮箱服务挂起一次（15 > 10），i=0 那次判定就不通过
    // ⇒ 整颗按钮诚实空转，而且打出的是「这是配置问题，请调小 CODE_TIMEOUT_MS」——
    // **一条确切而错误的处置**。
    // **人恰恰是在池子出问题时才会去点那颗按钮，也就是最可能撞上这一格的时刻。**
    //
    // ⚠️ **代价明写（不是零成本）**：准备阶段不再计入预算 ⇒ 定时轮的实际墙钟最坏
    // 比 780 秒多出一段准备时间 —— **本来就是那份预算留了余量的东西**，仍在 900 秒之内。
    let round_started_at = (deps.now)();

    let rounds = need.min(cfg.mint_batch);
    let worst_attempt = worst_attempt_ms(
        cfg.max_domain_attempts,
        cfg.code_timeout_ms,
        cfg.mint_delay_max_ms,
    );

    let mut journal: Vec<Observation> = Vec::new();
    let mut used_this_round: HashMap<String, u32> = HashMap::new();
    let mut attempted = 0u32;
    let mut minted = 0u32;
    let mut minted_by_channel: HashMap<String, u32> = HashMap::new();
    let mut failures: Vec<Failure> = Vec::new();
    let mut to_save_backoff: Option<BackoffState> = None;
    let mut probed: HashSet<String> = HashSet::new();

    // ── 5. 主循环 ────────────────────────────────────────────────────
    for i in 0..rounds {
        let delay = if i == 0 {
            0
        } else {
            let span = (cfg.mint_delay_max_ms - cfg.mint_delay_min_ms).max(0);
            cfg.mint_delay_min_ms + ((deps.rand)() * span as f64) as i64
        };

        // 预算判据。
        if let Some(budget) = deps.round_budget_ms {
            let elapsed = (deps.now)() - round_started_at;
            if elapsed + delay + worst_attempt > budget {
                if attempted == 0 {
                    // 🔴 **走 error，措辞必须与下面那条分开**：`failures` 是空的、
                    // `minted < attempted` 是 `0 < 0` 为假 ⇒ 两个入口的归因日志一条都不会打，
                    // **这里是唯一能说破它的地方**。
                    (deps.emit)(ev(
                        "error",
                        "registrar.round_budget_impossible",
                        "这个配置在本形态下**永远**铸不出 key：一次完整的尝试装不进这一轮的预算，\
                         每一轮都会原地打转。这不是「这轮差一点」。",
                        vec![
                            ("worstAttemptMs", worst_attempt.to_string()),
                            ("roundBudgetMs", budget.to_string()),
                        ],
                    ));
                } else {
                    (deps.emit)(ev(
                        "warn",
                        "registrar.round_budget_exhausted",
                        "预算不够再开一次完整的尝试了，提前收尾；剩下的名额留给下次调度。",
                        vec![
                            ("elapsedMs", elapsed.to_string()),
                            ("budgetMs", budget.to_string()),
                            ("worstAttemptMs", worst_attempt.to_string()),
                            ("attempted", attempted.to_string()),
                            ("minted", minted.to_string()),
                        ],
                    ));
                }
                // 🔴 **关键在于「不开始」而不是「跑到一半停下」**：
                // 被从中间砍断时铸号的收尾不会执行，**邮箱就漏了**；
                // 主动提前收尾则每一次尝试都完整走完。
                break;
            }
        }

        if delay > 0 {
            (deps.sleep)(delay).await;
        }
        attempted += 1;

        let candidates = select_domains(
            &ledger,
            &listed,
            (deps.now)(),
            cfg.max_domain_attempts as usize,
            &used_this_round,
            &rejected_this_round(&journal),
        );
        // ⚠️ **装的是「派出去的候选」而不是「真的打过的域名」，是刻意的取舍。**
        //
        // 🔴 为什么非有它不可：台账**轮内不更新**（落盘统一在收尾），
        // 而选域名对「已知 ok」那一档是按 `at` 全序排序 —— `at` 轮内不变
        // ⇒ 默认 `max_domain_attempts = 1` 时**一轮 5 个名额确定性地全落在同一个域名上**
        //（6 分钟内 5 个账号挂在同一个域名下），而那一档的说明逐字写着
        // 「LRU 轮换，别把一个好域名打成上游风控的焦点」——
        // **而「打成焦点」发生的正是轮内那几次连续注册。**
        //
        // 多标几个的代价只有一条：**档内多转几格，而档与档的优先级一格都不动。**
        for d in &candidates {
            *used_this_round.entry(d.clone()).or_insert(0) += 1;
            probed.insert(d.clone());
        }

        let out = deps
            .minter
            .mint_one(
                MintRequest {
                    candidates,
                    code_timeout_ms: cfg.code_timeout_ms,
                },
                &mut journal,
            )
            .await;

        match out {
            MintOutcome::Ok { key } => {
                // **照存不误**：在这里拒收 = 销毁凭据 = 数据丢失。
                if let Err(e) = deps.repo.add(&key).await {
                    (deps.emit)(ev(
                        "error",
                        "registrar.key_save_failed",
                        e,
                        vec![("channel", channel.clone())],
                    ));
                }
                minted += 1;
                *minted_by_channel.entry(channel.clone()).or_insert(0) += 1;

                if !(deps.is_importable_key)(&key) {
                    failures.push(Failure {
                        reason: TendFailureReason::KeySuspicious,
                        channel: channel.clone(),
                    });
                    (deps.emit)(ev(
                        "error",
                        "registrar.minted_key_suspicious",
                        "上游发回来的 key 含有不可打印字符或空白，已照常存进池子，\
                         但它多半每次被选中都会让转发失败；请在面板上停用或删除它。",
                        // **不带明文**：只报长度与通道。
                        vec![
                            ("channel", channel.clone()),
                            ("keyLength", key.len().to_string()),
                        ],
                    ));
                }
            }
            MintOutcome::RateLimited { limit_kind, marker } => {
                failures.push(Failure {
                    reason: TendFailureReason::RateLimited,
                    channel: channel.clone(),
                });
                // 🔴 **这一轮铸出过 key 就重新起一串。**
                //
                // 从前这里无条件传旧的退避键，于是「前 4 把成功、第 5 把撞限流」
                // 这种**默认参数下最常见的一轮**照样把 `hits` 一路推上去：
                // 连着几轮都长这样 ⇒ 15min → 30min → 1h → 2h → 4h(封顶)，
                // **而每一轮其实都在正常出 key**。
                // **指数退避要治的是「越打越死」，不是「出着 key 顺带撞了一次上限」。**
                //
                // ⚠️ `minted` 在这一行已经是这一轮的终值：本支紧跟着中止整轮。
                let prev = if minted > 0 { None } else { backoff.as_ref() };
                to_save_backoff = Some(next_backoff(prev, limit_kind, (deps.now)()));
                (deps.emit)(ev(
                    "warn",
                    "registrar.rate_limited",
                    "撞上上游限流，整轮立刻中止并记退避窗口。",
                    vec![
                        ("kind", format!("{limit_kind:?}")),
                        ("marker", marker.unwrap_or_default()),
                    ],
                ));
                break;
            }
            MintOutcome::Failed { reason } => {
                failures.push(Failure {
                    reason,
                    channel: channel.clone(),
                });
                // 只决定「要不要中止整轮」，**不再决定「换不换通道」**。
                //
                // ⚠️⚠️ **两条通道是二选一，一条通道失败绝不会去碰另一条。**
                // 从前 `provider_error` 与 `code_timeout` 会降级到备通道；现在与其余几支一样。
                // **变坏的地方点名写出来**：从前「这条通道收不到验证码」还有一次换通道的机会；
                // 现在同一条通道收不到验证码 = **这一轮乃至这一天一把都铸不出来**，
                // 直到运维自己去面板换通道。故障形态从「补池变慢但还在出 key」
                // 变成「补池零产出、池子慢慢耗干、几小时到几天后以 pool_empty 503 炸出来」——
                // **把一个自愈的故障换成了一个要人管的故障。这是「二选一」的固有代价，不是实现缺陷。**
                //
                // ⚠️ 这个 `match` 本身必须留着：它还担着穷尽性断言。
                let abort = match reason {
                    // Agnes 后端整体故障，继续本轮只会在故障期间制造更多注定失败的请求。
                    TendFailureReason::UpstreamError => true,
                    TendFailureReason::ProviderError
                    | TendFailureReason::NetworkError
                    | TendFailureReason::CodeTimeout
                    | TendFailureReason::DomainBlockedAll
                    | TendFailureReason::RegisterFailed
                    | TendFailureReason::LoginFailed
                    | TendFailureReason::KeyFailed => false,
                    // 这几个不该从 `mint_one` 出来 —— 它们是 `tender` 自己的产物。
                    TendFailureReason::RateLimited
                    | TendFailureReason::ProviderMissing
                    | TendFailureReason::RoundCrashed
                    | TendFailureReason::KeySuspicious
                    | TendFailureReason::UpstreamBackoff => false,
                };
                if abort {
                    break;
                }
            }
        }
    }

    // ── 6. 收尾 ─────────────────────────────────────────────────────
    let committed = commit_journal(
        &ledger,
        &journal,
        u32::try_from(listed.len()).ok(),
        (deps.now)(),
    );
    finish_round(
        deps,
        &committed,
        &probed,
        &listed,
        &journal,
        minted,
        backoff.as_ref(),
        &mut to_save_backoff,
    )
    .await;

    // ── 7. 铸出来了就再对一次账 ─────────────────────────────────────
    //
    // 🔴 索引更新是**没有 CAS 的读-改-写** ⇒ 同一轮里刚被捡回来的孤儿转眼又成了孤儿。
    // **连带后果比「索引脏了」严重得多**：孤儿不被池子看到 ⇒ `available` 偏小
    // ⇒ 缺口被高估 ⇒ **超额补铸**，而每铸一把都是一次真实的 Agnes 建号，
    // 同时撞注册风控与活跃邮箱上限。**补铸出来的 key 还会再次触发同一条覆盖，一轮一轮地滚。**
    //
    // 这一步**不消灭那个窗口**（没有 CAS 就消灭不了），只把它从「一整轮」压到「一轮之内」。
    if minted > 0
        && let Err(e) = deps.repo.reconcile_after_mint().await
    {
        // **失败只记一条 warn 就算了**：索引残留是 fail-safe 的，
        // 让它把一轮**已经成功铸出了 key** 的补池变成异常，
        // 只会让入口打出误导性的「补池失败」，**而实际上 key 已经落盘了**。
        (deps.emit)(ev(
            "warn",
            "registrar.post_mint_reconcile_failed",
            e,
            vec![],
        ));
    }

    mk(attempted, minted, minted_by_channel, failures, (deps.now)())
}

/// 收尾：台账最多写 1 次、退避键按需写 1 次。
#[allow(clippy::too_many_arguments)]
async fn finish_round(
    deps: &TendDeps<'_>,
    committed: &CommitOutcome,
    probed: &HashSet<String>,
    listed: &[String],
    journal: &[Observation],
    minted: u32,
    backoff: Option<&BackoffState>,
    to_save: &mut Option<BackoffState>,
) {
    // 🔴 **「结论没变就一次写都不发」是写账的一根轴**：定时轮每 30 分钟一轮，
    // 稳态下台账的结论不再变动 ⇒ 这把键的写次数是 **0** 而不是 48 次/天。
    if committed.dirty
        && let Err(e) = deps.store.save_domain_ledger(&committed.ledger).await
    {
        (deps.emit)(ev("warn", "registrar.ledger_save_failed", e, vec![]));
    }

    // 🔴 `cluster` 那一档的形状证据，**两条，取并集**。
    let rejected = rejected_this_round(journal);
    // ⚠️ **「一个不落全试过」这半句是承重的，不许省**：观测本子里只有拿到过可分类回话的域名。
    // 省掉它的话，「池子快满、这一轮只开了 1 个名额、而它恰好撞上一个真被拉黑的域名」
    // 也会记退避。
    let round_all_rejected =
        !rejected.is_empty() && rejected.len() == probed.len() && probed.len() == listed.len();
    // ① 钳位生效 = 同一轮里 ≥2 个域名被判成「域名被屏蔽」
    // ② 上游列出来的域名这一轮**一个不落**全试过了，而且**全**被判成「域名被屏蔽」
    //
    // 🔴🔴 **② 是补上来的，它治的是一条实测出来的、对整类部署完全失效的漏洞**：
    // **只配了一个邮箱域名的部署永远凑不满 ① 要的那 2 个**，于是上游一改限流文案，
    // 那种部署每一轮都打满 `mint_batch` 次注定失败的发码请求、**一个退避键都不写**。
    // 实测（测试替身、内置值、20 轮）：逐轮请求数恒为 5、`minted` 恒为 0、退避 20 轮全是空
    // ⇒ 稳态 **240 次/天**；接上 ② 之后 ⇒ 稳态 **约 30 次/天**。
    //
    // ⚠️ ② 真正多接住的只有「上游只给出一个域名」那一种（≥2 个域名时 ① 早就成立）。
    // 写成更一般的形状，只是因为**这一档的语义本来就该是「这一轮的候选全军覆没」**，
    // 而不是「凑够两个」——
    // **「凑够两个」是钳位的口径，被顺手借用成了退避的口径，那次借用就是这条漏洞的来源。**
    let cluster_shape = committed.discarded > 0 || round_all_rejected;

    // ⚠️ **`minted == 0` 这个前提是承重的，不是保险起见**：这两条证据在
    // 「上游真的成批拉黑了好几个域名」时同样会成立，而那一档**这一轮照样铸得出 key**。
    // **那时上游明明还在给我们发号，记退避等于自己把补池按停。**
    if to_save.is_none() && minted == 0 && cluster_shape {
        *to_save = Some(next_backoff(backoff, BackoffKind::Cluster, (deps.now)()));
    }

    // 三类事件。
    if committed.discarded > 0 {
        // 🔴 **代价登记（可证不是推断）**：钳位生效时 `commit_journal` 把**全部**
        // blocked 判定滤光 ⇒ `newly_blocked` **可证恒为空** ⇒
        // 那条 `registrar.domain_blocked` **一条都发不出来**，
        // 而这条事件的 fields **一个字的上游原话都不带**。
        // ⇒ **面板横幅与五语言文档都按支分开写，不许说成「去翻 domain_blocked 就能分辨」。**
        (deps.emit)(ev(
            "warn",
            "registrar.domain_verdicts_discarded",
            "这一轮有两个以上域名同时被判成「被屏蔽」—— 那更像是上游在限流而不是域名真被拉黑，\
             所以这一轮一条域名结论都不学。",
            vec![("discarded", committed.discarded.to_string())],
        ));
    }
    if round_all_rejected {
        // **照 `domain_verdicts_discarded` 那条事件的名字发出去就是假话，所以另起一条。**
        (deps.emit)(ev(
            "warn",
            "registrar.round_all_domains_rejected",
            "上游列出来的域名这一轮一个不落全试过了，而且全被拒 —— \
             两种可能都还开着：域名真的都被拉黑了，或者上游改了限流文案而我们的词表没认出来。",
            vec![("listed", listed.len().to_string())],
        ));
    }
    for d in &committed.newly_blocked {
        (deps.emit)(ev(
            "warn",
            "registrar.domain_blocked",
            "这个域名连续两轮被上游拒绝，已记进台账、后续排到候选最后。",
            vec![("domain", d.clone())],
        ));
    }

    // 退避键的最终落盘。
    if let Some(s) = *to_save {
        if let Err(e) = deps.store.save_backoff(Some(s)).await {
            (deps.emit)(ev("warn", "registrar.backoff_save_failed", e, vec![]));
        }
    } else if minted > 0 && backoff.is_some() {
        // 铸出来了 ⇒ 上游现在认我们 ⇒ 把那把陈旧的退避键清掉，指数从头数。
        // ⚠️ **只有键真的存在时才写**：否则每一轮成功补池都要白付一次写。
        if let Err(e) = deps.store.save_backoff(None).await {
            (deps.emit)(ev("warn", "registrar.backoff_clear_failed", e, vec![]));
        }
    }
}

#[cfg(test)]
mod tests;
