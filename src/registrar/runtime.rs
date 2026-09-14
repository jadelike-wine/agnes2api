//! 把注册机那十个模块**接到真实的存储、时钟与上游上**。
//!
//! 这一层不含业务规则 —— 规则全在 `config` / `tender` / `mint` / `mailbox` 那几个模块里。
//! 它只负责三件事：**补池锁**、**补池历史**、**定时轮**。

use super::backoff::{BackoffState, merge_backoff, narrow_backoff};
use super::config::{Channel, RegistrarConfig};
use super::domain_ledger::{
    DOMAIN_LEDGER_KEY, DomainEntry, DomainLedger, DomainState, merge_domain_ledger,
};
use super::mailbox::{MailDeps, MailEvent, Moemail, Yyds, moemail_list_domains, yyds_list_domains};
use super::mint::{MailboxProvider, RealMinter};
use super::tender::{
    DomainSource, Failure, TendDeps, TendEvent, TendRepo, TendResult, TendStore, tend_once,
};
use super::types::SCHEDULED_ROUND_WALL_CLOCK_MS;
use crate::ports::logger::{LogEntry, LogLevel, Logger};
use crate::ports::storage::Storage;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

/// 定时轮那一步对账失败。
///
/// ⚠️ **它与 `registrar.post_mint_reconcile_failed` 是两条，不许合并**：
/// 那条只在「真的铸出了 key」之后打，而这条**不挑注册机开没开** ——
/// 合并了就分不清「补池路上出的事」和「关着的部署索引在烂」。
///
/// 🟡 TS 侧这里走的是 `console.error`（没有事件名）。Rust 侧没有那条非结构化通道，
/// 于是**新起一个名字并当场写进事件名全表**（01 §13）。
pub const EVENT_POOL_INDEX_RECONCILE_FAILED: &str = "pool.index_reconcile_failed";

/// 每轮重读配置时读存储失败 ⇒ 沿用上一次已知合法的那份，本轮不补池。
///
/// 🟡 同上，TS 侧是 `console.error`，Rust 侧补名并已登记进全表。
pub const EVENT_REGISTRAR_RELOAD_FAILED: &str = "registrar.config_reload_failed";

/// 跨副本那把锁还占着 ⇒ 这一轮压根没开始。
///
/// 🟡 TS 侧这一档不打事件（锁在 handler 里判、直接回 409）。
/// Rust 侧定时轮那条路也会撞上它，而那条路上没有任何人接得住 ——
/// **补一个名字并已登记进事件名全表**（01 §13）。
pub const EVENT_ROUND_SKIPPED_LOCKED: &str = "registrar.round_skipped_locked";

// ── 补池锁 ────────────────────────────────────────────────────────────────

pub const TEND_LOCK_KEY: &str = "registrar_tend_lock";

/// 定时轮那条路的锁 TTL = 一轮的墙钟上限。
pub const TEND_LOCK_TTL_SCHEDULED_MS: i64 = SCHEDULED_ROUND_WALL_CLOCK_MS;

/// 手动那条路的锁 TTL。
///
/// 🔴 **与定时轮那份分开，而且照「轮次的真实墙钟」定，不照「载体」定。**
/// 取 180 秒的两头夹：
/// - **下界**：手动一轮最坏 ≈ 一次尝试 60 秒 + 准备阶段几次请求（各 15 秒上限）≈ 135 秒；
/// - **上界**：它是「这把锁最久卡住多久」的上界，而手动补池是人点的，
///   卡三分钟已经足够让人以为坏了。
pub const TEND_LOCK_TTL_MANUAL_MS: i64 = 180_000;

// ── 两条交叉校验：**编译期**断言 ─────────────────────────────────────────
//
// 🔴 它们原先是一格 `#[test]`，clippy 报了 `this assertion has a constant value` ——
// **那句话本身就是结论**：这两条全由常量决定，运行期不可能失败。
// 放在测试里只是让人**以为**它在守着什么；挪到这里之后，配错一个常量**编译都过不去**。
//
// ⚠️ 同一处改动我在 `types.rs` 那边栽过一次：删掉了测试而新的 `const _` 因为
// 字符串没匹配上**压根没写进来**，于是四道门禁全绿而三条护栏凭空消失。
// **改完必须立刻用变异验一次它真的在守。**

/// 定时轮的锁 TTL 必须**明显短于**补池间隔。
///
/// 进程被硬杀时收尾一行都不跑，锁只能靠自然过期放开 ——
/// TTL 取到间隔的一半，**代价上界就是最多跳过一轮**。
const _SCHEDULED_LOCK_IS_WELL_UNDER_THE_INTERVAL: () = assert!(
    TEND_LOCK_TTL_SCHEDULED_MS * 2 <= super::config::DEFAULT_TEND_INTERVAL_MS,
    "定时轮的锁 TTL 没有明显短于补池间隔 —— 进程被硬杀之后会跳过不止一轮",
);

/// 🔴 手动那份**不许与定时轮同长** —— 它照「轮次的真实墙钟」定，而手动一轮小一个量级。
const _MANUAL_LOCK_IS_SHORTER_THAN_THE_SCHEDULED_ONE: () = assert!(
    TEND_LOCK_TTL_MANUAL_MS < TEND_LOCK_TTL_SCHEDULED_MS,
    "手动补池的锁与定时轮同长了 —— 人点一下要等一刻钟才放开",
);

/// ⚠️ **诚实限定，不许被改写成「并发已解决」。**
///
/// 存储锁是**尽力而为、不是互斥原语**：读-判-写之间没有 CAS，
/// 两个副本在同一毫秒读到「没锁」时会**同时**拿到锁。
/// 它挡住的是**绝大多数**重叠，不是全部。
///
/// ⚠️ **收尾挡不住进程被硬杀**：`SIGKILL` / OOM kill / 容器重建时它一行都不跑，
/// 锁只能靠自然过期放开 —— 那正是两个 TTL 都必须**明显短于补池间隔**的理由。
/// `Err(Some(until))` = 锁还占着，**并且把它什么时候放开一起带出来** ——
/// 面板那颗按钮要据此说「还要等多久」，只回一个 false 说不出那句话。
/// `Err(None)` = 锁是空的，但这一次写没写进去。
pub async fn acquire_tend_lock(
    storage: &dyn Storage,
    now: i64,
    ttl_ms: i64,
) -> Result<(), Option<i64>> {
    let cur = storage.get(TEND_LOCK_KEY).await.ok().flatten();
    if let Some(v) = cur
        && let Some(until) = v.get("until").and_then(Value::as_i64)
        && until > now
    {
        return Err(Some(until));
    }
    // ⚠️ **这把键写的时候不传过期时间**：有界性靠「单一固定键、数量恒为 1」，
    // 而不是靠存储层的 TTL —— 后者在不同后端上语义不一。
    match storage
        .put(TEND_LOCK_KEY, json!({ "until": now + ttl_ms }), None)
        .await
    {
        Ok(()) => Ok(()),
        Err(_) => Err(None),
    }
}

pub async fn release_tend_lock(storage: &dyn Storage) {
    let _ = storage.delete(TEND_LOCK_KEY).await;
}

// ── 补池历史 ──────────────────────────────────────────────────────────────

pub const TEND_HISTORY_KEY: &str = "tend:history";
pub const TEND_HISTORY_SIZE: usize = 50;

/// 这一轮是谁触发的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TendTrigger {
    Scheduled,
    Manual,
}
impl TendTrigger {
    fn as_str(self) -> &'static str {
        match self {
            // ⚠️ 落盘值是 `cron` 而不是 `scheduled` —— 那是**已经写进存量历史**的字面量，
            // 改它会让升级前的条目在面板上对不上。名字与今天的形态有出入是知情的。
            Self::Scheduled => "cron",
            Self::Manual => "manual",
        }
    }
}

fn to_record(r: &TendResult, trigger: TendTrigger) -> Value {
    json!({
        "at": r.at,
        "trigger": trigger.as_str(),
        "skipped": r.skipped,
        "available": r.available,
        "attempted": r.attempted,
        "minted": r.minted,
        "mintedByChannel": r.minted_by_channel,
        "failures": r.failures,
        "primaryChannel": r.primary_channel,
        "durationMs": r.duration_ms,
    })
}

/// 把一条记录追加进历史环。
///
/// ⚠️ **读不出来的整份历史当成空的重开**，不是报错：
/// 历史是**诊断**，不是账本 —— 让一份写坏的历史把补池本身挡住是本末倒置。
pub async fn append_history(storage: &dyn Storage, record: Value) {
    let mut entries = read_history_entries(storage).await;
    entries.push(record);
    if entries.len() > TEND_HISTORY_SIZE {
        let drop_n = entries.len() - TEND_HISTORY_SIZE;
        entries.drain(0..drop_n);
    }
    // 🔴 **裸数组落盘，不是 `{"entries": [...]}`。**
    // 存量部署里这把键就是裸数组（规格里的值形状写着 `TendRecord[]`）。
    // 包一层的代价**不止是读不到旧历史**：写出去那一份**上一版也读不懂**
    // ⇒ 回滚回去之后历史同样是空的，而回滚恰恰是升级唯一的兜底。
    let _ = storage
        .put(TEND_HISTORY_KEY, Value::Array(entries), None)
        .await;
}

/// 读历史那一环。
///
/// ⚠️ **两种形状都收**：裸数组是存量与今天落盘的那一种；
/// `{"entries": [...]}` 只是为了让某个中途写成包一层的部署也读得回来 ——
/// 一次升级把历史清空这件事，值得多认一种形状。
async fn read_history_entries(storage: &dyn Storage) -> Vec<Value> {
    let Some(v) = storage.get(TEND_HISTORY_KEY).await.ok().flatten() else {
        return Vec::new();
    };
    let raw = match &v {
        Value::Array(a) => Some(a.clone()),
        _ => v.get("entries").and_then(Value::as_array).cloned(),
    };
    // 逐条窄化：坏掉的那几条丢掉，**不是整份丢掉**。
    raw.map(|a| a.into_iter().filter(Value::is_object).collect())
        .unwrap_or_default()
}

// ── 存储面的实现 ──────────────────────────────────────────────────────────

const BACKOFF_KEY: &str = "registrar:backoff";

struct RealStore {
    storage: Arc<dyn Storage>,
}

#[async_trait]
impl TendStore for RealStore {
    async fn load_backoff(&self) -> Option<BackoffState> {
        let v = self.storage.get(BACKOFF_KEY).await.ok().flatten();
        narrow_backoff(v.as_ref())
    }

    async fn save_backoff(&self, next: Option<BackoffState>) -> Result<(), String> {
        match next {
            None => self
                .storage
                .delete(BACKOFF_KEY)
                .await
                .map_err(|e| e.to_string()),
            Some(s) => {
                // 🔴 **先合并再写**：丢掉 `until` 等于退避窗口凭空消失 ⇒ 继续打
                // ⇒ 每打一次续一次窗口 ⇒ **正好回到退避要修的那个缺陷**。
                let cur = self.load_backoff().await;
                let merged = cur.map_or(s, |c| merge_backoff(&c, &s));
                let v = serde_json::to_value(merged).map_err(|e| e.to_string())?;
                self.storage
                    .put(BACKOFF_KEY, v, None)
                    .await
                    .map_err(|e| e.to_string())
            }
        }
    }

    async fn load_domain_ledger(&self) -> DomainLedger {
        let Some(v) = self.storage.get(DOMAIN_LEDGER_KEY).await.ok().flatten() else {
            return DomainLedger::default();
        };
        narrow_domain_ledger(&v)
    }

    async fn save_domain_ledger(&self, next: &DomainLedger) -> Result<(), String> {
        // 同理先合并：丢一条域名结论只是下一轮重学（便宜），
        // 但**整份覆盖**会把别的副本刚学到的东西抹掉。
        let cur = self.load_domain_ledger().await;
        let merged = merge_domain_ledger(&cur, next);
        let v = serde_json::to_value(&merged).map_err(|e| e.to_string())?;
        self.storage
            .put(DOMAIN_LEDGER_KEY, v, None)
            .await
            .map_err(|e| e.to_string())
    }
}

/// 把存储里那份台账窄化出来。
///
/// 🔴 **失败方向：任一字段读不得 ⇒ 丢掉那一条；整个对象读不得 ⇒ 空台账。**
/// 也就是「什么都没记住，多探几次」——
/// **绝不返回「全被屏蔽」**，后者会让注册机一把 key 都铸不出来，
/// **而面板上看不出任何原因。**
pub fn narrow_domain_ledger(v: &Value) -> DomainLedger {
    let Some(o) = v.as_object() else {
        return DomainLedger::default();
    };
    let mut entries = std::collections::HashMap::new();
    if let Some(es) = o.get("entries").and_then(Value::as_object) {
        for (k, ev) in es {
            let Some(eo) = ev.as_object() else { continue };
            let (Some(s), Some(at), Some(n)) = (
                eo.get("s").and_then(Value::as_str),
                eo.get("at").and_then(Value::as_i64),
                eo.get("n").and_then(Value::as_u64),
            ) else {
                continue;
            };
            let s = match s {
                "ok" => DomainState::Ok,
                "blocked" => DomainState::Blocked,
                _ => continue,
            };
            let Ok(n) = u32::try_from(n) else { continue };
            entries.insert(k.clone(), DomainEntry { s, at, n });
        }
    }
    DomainLedger {
        updated_at: o.get("updated_at").and_then(Value::as_i64).unwrap_or(0),
        total: o
            .get("total")
            .and_then(Value::as_u64)
            .and_then(|x| u32::try_from(x).ok()),
        entries,
    }
}

// ── 池面与域名面 ──────────────────────────────────────────────────────────

/// 池面。真正的实现由调用方注进来（它认识 `KeyPoolRepo`）。
pub struct PoolBridge<F1, F2, F3> {
    pub counts: F1,
    pub add: F2,
    pub reconcile: F3,
}

struct DomainsBridge<'a> {
    channel: Channel,
    deps: MailDeps<'a>,
}
#[async_trait]
impl DomainSource for DomainsBridge<'_> {
    async fn list_domains(&self) -> Result<Vec<String>, String> {
        match self.channel {
            Channel::Yyds => yyds_list_domains(&self.deps).await,
            Channel::Moemail => moemail_list_domains(&self.deps).await,
        }
    }
}

// ── 跑一轮 ────────────────────────────────────────────────────────────────

/// 跑一轮补池的全部外部依赖。
pub struct RoundDeps<'a> {
    pub config: &'a RegistrarConfig,
    pub storage: Arc<dyn Storage>,
    pub logger: Arc<dyn Logger>,
    pub fetcher: Arc<dyn crate::ports::fetcher::Fetcher>,
    pub repo: &'a dyn TendRepo,
    pub trigger: TendTrigger,
    /// 只有手动那条路传。
    pub round_budget_ms: Option<i64>,
}

fn bridge_event(logger: &dyn Logger, e: TendEvent) {
    let level = match e.level {
        "error" => LogLevel::Error,
        "warn" => LogLevel::Warn,
        _ => LogLevel::Info,
    };
    logger.log(LogEntry {
        level,
        // ⚠️ 事件名是 `&'static str` 且取自白名单全表 —— 它们是运维的 grep 锚点，
        // 也是面板事件板块的筛选依据。`TendEvent` 那一侧同样是 `&'static str`，两边对得上。
        event: e.event,
        msg: e.msg,
        corr: None,
        fields: e
            .fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), Value::String(v)))
            .collect(),
    });
}

/// 一轮补池的收场。
///
/// 🔴 **「锁被占着」不是一次失败的补池，它是「压根没开始」** ——
/// 混进 [`TendResult::failures`] 会让补池历史多出一行**什么都没发生的失败**，
/// 而面板那颗按钮也就只能说「补池失败了」，
/// 而真话是「另一边正在补，等它 N 秒」。
pub enum RoundOutcome {
    Ran(Box<TendResult>),
    Locked {
        /// 锁什么时候自然放开。
        until: Option<i64>,
    },
}

impl RoundOutcome {
    /// 定时轮那条路不关心区别 —— 它两种都只是「这一跳没干成」。
    pub fn ran(self) -> Option<TendResult> {
        match self {
            Self::Ran(r) => Some(*r),
            Self::Locked { .. } => None,
        }
    }
}

/// 抢锁 → 跑一轮 → 写历史 → 放锁。
///
/// 🔴 **历史一定会写**，哪怕整轮抛错 —— 面板上那一行必须能自己说清发生了什么。
pub async fn run_round(d: RoundDeps<'_>) -> RoundOutcome {
    let now_fn = || crate::adapters::now_ms();
    let ttl = match d.trigger {
        TendTrigger::Scheduled => TEND_LOCK_TTL_SCHEDULED_MS,
        TendTrigger::Manual => TEND_LOCK_TTL_MANUAL_MS,
    };

    if let Err(until) = acquire_tend_lock(d.storage.as_ref(), now_fn(), ttl).await {
        bridge_event(
            d.logger.as_ref(),
            TendEvent {
                level: "info",
                event: EVENT_ROUND_SKIPPED_LOCKED,
                msg: "上一轮还没跑完（补池锁还在），这一轮跳过。".into(),
                fields: vec![],
            },
        );
        // 🔴 **不写补池历史**：这一轮什么都没发生，
        // 记一行进那张定长表等于把一条真实记录挤出去。
        return RoundOutcome::Locked { until };
    }

    let store = RealStore {
        storage: Arc::clone(&d.storage),
    };
    let creds = match d.config.channel {
        Channel::Yyds => d.config.yyds.as_ref(),
        Channel::Moemail => d.config.moemail.as_ref(),
    };
    let Some(creds) = creds else {
        release_tend_lock(d.storage.as_ref()).await;
        // 走到这里说明调用方跳过了 `enabled` / `blocked` 两道 gate。
        // ⚠️ 这里**不 panic**（那是 `require_channel` 的职责，它在装配路径之外）——
        // 定时轮里 panic 会把整个后台任务带走，而它是**可选子系统**。
        bridge_event(
            d.logger.as_ref(),
            TendEvent {
                level: "error",
                event: "registrar.channel_missing",
                msg: "选中的通道没有凭据，而这一轮却被启动了 —— 调用方跳过了 enabled/blocked 两道闸。"
                    .into(),
                fields: vec![],
            },
        );
        return RoundOutcome::Ran(Box::new(TendResult {
            skipped: false,
            available: 0,
            attempted: 0,
            minted: 0,
            minted_by_channel: std::collections::HashMap::new(),
            failures: vec![Failure {
                reason: super::tender::TendFailureReason::ProviderMissing,
                channel: d.config.channel.as_str().into(),
            }],
            at: now_fn(),
            primary_channel: d.config.channel.as_str().into(),
            duration_ms: 0,
        }));
    };

    let emit_mail = |e: MailEvent| {
        bridge_event(
            d.logger.as_ref(),
            TendEvent {
                level: e.level,
                event: e.event,
                msg: e.msg,
                fields: e.fields,
            },
        )
    };
    let sleep = |ms: i64| -> futures_core::future::BoxFuture<'static, ()> {
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_millis(ms.max(0) as u64)).await;
        })
    };
    let rand = crate::adapters::rand_unit;

    let mail_deps = || MailDeps {
        fetcher: d.fetcher.as_ref(),
        base_url: creds.base_url.clone(),
        api_key: creds.api_key.clone(),
        now: &now_fn,
        sleep: &sleep,
        emit: &emit_mail,
    };

    let provider: Box<dyn MailboxProvider> = match d.config.channel {
        Channel::Yyds => Box::new(Yyds { deps: mail_deps() }),
        Channel::Moemail => Box::new(Moemail { deps: mail_deps() }),
    };
    let domains = DomainsBridge {
        channel: d.config.channel,
        deps: mail_deps(),
    };
    let ledger = store.load_domain_ledger().await;
    let agnes = super::agnes::RealAgnes {
        fetcher: d.fetcher.as_ref(),
        platform_url: d.config.agnes_platform_url.clone(),
    };
    let emit_tend = |e: TendEvent| bridge_event(d.logger.as_ref(), e);
    let minter = RealMinter {
        deps: super::mint::MintDeps {
            provider: provider.as_ref(),
            agnes: &agnes,
            ledger: &ledger,
            token_name: &d.config.token_name,
            mint_delay_min_ms: d.config.mint_delay_min_ms,
            mint_delay_max_ms: d.config.mint_delay_max_ms,
            now: &now_fn,
            rand: &rand,
            sleep: &sleep,
            emit: &emit_tend,
        },
    };

    let result = tend_once(&TendDeps {
        config: d.config,
        repo: d.repo,
        store: &store,
        domains: &domains,
        minter: &minter,
        now: &now_fn,
        rand: &rand,
        sleep: &sleep,
        emit: &emit_tend,
        round_budget_ms: d.round_budget_ms,
        is_importable_key: &crate::pool::repo::is_importable_key,
    })
    .await;

    // 🔴 历史一定要写 —— 面板上那一行必须能自己说清发生了什么。
    append_history(d.storage.as_ref(), to_record(&result, d.trigger)).await;
    release_tend_lock(d.storage.as_ref()).await;
    RoundOutcome::Ran(Box::new(result))
}

/// 手动一轮被压小的那几个数 —— **压前压后两组都给**。
///
/// 只报「压成了多少」等于让运维看不出「我配的那个数被动过」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualCapNumbers {
    pub mint_batch: u32,
    pub max_domain_attempts: u32,
    pub code_timeout_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualTendCap {
    pub before: ManualCapNumbers,
    pub after: ManualCapNumbers,
}

/// 真被压小时打这一条。
pub const EVENT_MANUAL_ROUND_CAPPED: &str = "registrar.manual_round_capped";

/// 给手动这一轮套上它自己那一族上限。
///
/// # 🔴 一律 `min()` **压顶**，不是赋值
///
/// 运维把 `CODE_TIMEOUT_MS` 调到 30 秒，是因为他知道自己的上游快；
/// 赋值会把它**抬**回 60 秒 —— 那是拿运维的判断去换一个默认值。
///
/// # 🔴 `MANUAL_MAX_DOMAIN_ATTEMPTS = 1` 不是「保守一点」，它是承重的
///
/// 它让 [`worst_attempt_ms`] **退化成常量**。不压它的话，运维把
/// `MAX_DOMAIN_ATTEMPTS` 调成 2，最坏尝试就跳到 150 秒 > 预算 70 秒
/// ⇒ 预算判据当场判「这一次尝试开不起来」
/// ⇒ **按钮变成永远铸不出 key 的诚实空转，而且空转得毫无征兆。**
pub fn manual_caps(cfg: &RegistrarConfig) -> (RegistrarConfig, Option<ManualTendCap>) {
    let before = ManualCapNumbers {
        mint_batch: cfg.mint_batch,
        max_domain_attempts: cfg.max_domain_attempts,
        code_timeout_ms: cfg.code_timeout_ms,
    };
    let after = ManualCapNumbers {
        mint_batch: before.mint_batch.min(super::types::MANUAL_MINT_BATCH),
        max_domain_attempts: before
            .max_domain_attempts
            .min(super::types::MANUAL_MAX_DOMAIN_ATTEMPTS),
        code_timeout_ms: before
            .code_timeout_ms
            .min(super::types::MANUAL_CODE_TIMEOUT_MS),
    };
    let mut capped_cfg = cfg.clone();
    capped_cfg.mint_batch = after.mint_batch;
    capped_cfg.max_domain_attempts = after.max_domain_attempts;
    capped_cfg.code_timeout_ms = after.code_timeout_ms;
    let capped = (after != before).then_some(ManualTendCap { before, after });
    (capped_cfg, capped)
}

/// 手动一轮的墙钟预算。
///
/// ⚠️ 它**只在 [`manual_caps`] 压过之后才成立** —— 那条编译期断言
///（`MANUAL_ROUND_BUDGET_MS > worst_attempt_ms(MANUAL_MAX_DOMAIN_ATTEMPTS, …)`）
/// 算的是压完之后那组数。拿没压过的配置配它，预算随时可能不够。
pub fn manual_round_budget(_cfg: &RegistrarConfig) -> i64 {
    super::types::MANUAL_ROUND_BUDGET_MS
}

// ── 运行期持有者 ──────────────────────────────────────────────────────────

/// 注册机的运行期。**装不起来就压根不构造它**（`AppState::registrar` 是 `Option`）。
pub struct RegistrarRuntime {
    pub config: RegistrarConfig,
    /// 装载期的 blocker / notice / warning。
    ///
    /// 🔴 **blocker 非空时 `config.blocked` 为真，而 `enabled` 一个字都不改** ——
    /// 「运维明明打开了，面板却说未启用」是另一种撒谎。
    /// **真话是「已启用 · 本次没跑起来」。**
    pub blockers: Vec<super::config::Blocker>,
    pub notices: Vec<super::config::Notice>,
    pub warnings: Vec<super::config::Warning>,
    pub storage: Arc<dyn Storage>,
    pub logger: Arc<dyn Logger>,
    pub fetcher: Arc<dyn crate::ports::fetcher::Fetcher>,
    /// 装配那一刻的环境变量快照。
    ///
    /// ⚠️ **只存 env，不存那份「已装载的配置」** —— 后者每一轮都要重读。
    /// 进程内 env 本来就不会变（TS 侧每轮读的 `process.env` 同理），
    /// 会变的是**存储里那一份，而面板正是它的编辑器**。
    pub env: crate::core::config_provenance::EnvMap,
}

impl RegistrarRuntime {
    /// 这一刻跑不跑得起来。
    ///
    /// 🔴 消费方一律**先判 `enabled` 再判 `blocked`**（两者的含义不同，见 `config`）。
    pub fn runnable(&self) -> bool {
        self.config.enabled && !self.config.blocked
    }

    /// 重新装一份配置：**env 照旧，存储那一份重读**。
    ///
    /// 🔴 **这是「面板改完不必重启」的全部机制。**
    /// 少了它，[`Self::config`] 就是启动那一刻的快照：
    /// 面板是这份配置的编辑器，而编辑器写完没有任何人再读 ⇒
    /// 开关、通道、目标数量、凭据**改了全都不生效**，必须重启容器；
    /// 更糟的是启动时关着就永远打不开、启动时开着就永远关不掉。
    ///
    /// ⚠️ **读存储失败回 `None`，而不是「当成空配置装一份」** ——
    /// 后者会让一次存储抖动把面板上配好的一切**静默降级成 env-only 的那一份**
    /// （通道变回默认、目标数量变回默认），而日志上只有一行读失败。
    /// 调用方拿到 `None` 就沿用上一次已知合法的那份。
    pub async fn reload(&self) -> Option<super::config::RegistrarLoad> {
        let stored = self
            .storage
            .get(crate::core::config_provenance::CONFIG_KEY)
            .await
            .ok()?
            .and_then(|v| v.get("registrar").cloned())
            .unwrap_or(Value::Null);
        Some(load(&self.env, &stored))
    }

    /// 读一份补池历史（**零上游请求**）。
    pub async fn history(&self) -> Vec<Value> {
        read_history_entries(self.storage.as_ref()).await
    }

    /// 读退避键（**零上游请求**）。
    pub async fn backoff(&self) -> Option<BackoffState> {
        let v = self.storage.get(BACKOFF_KEY).await.ok().flatten();
        narrow_backoff(v.as_ref())
    }

    /// 读域名台账（**零上游请求**）。
    pub async fn ledger(&self) -> DomainLedger {
        let Some(v) = self.storage.get(DOMAIN_LEDGER_KEY).await.ok().flatten() else {
            return DomainLedger::default();
        };
        narrow_domain_ledger(&v)
    }

    /// **真的验一次凭据。**
    ///
    /// 🔴 **不是只读一次域名列表就报绿** —— TS 版有过这个真缺陷：
    /// **凭据粘错时「测试连接」照样报绿**，因为列域名那一步在有的站点上不查凭据。
    /// ⇒ 这里**建一个临时邮箱再删掉**，那是唯一能证明「这把凭据今天真的能用」的动作。
    ///
    /// ⚠️ **代价明写：每点一次消耗一个活跃邮箱名额**（用完即删）。
    /// 连点由管理面那把共用的探测护栏挡着，这里不另造一套。
    ///
    /// ⚠️ **不许把「限流 / 连不上」翻译成「凭据无效」** —— 三者的处置完全相反，
    /// 而运维手上只有面板那一句话。做法就是把真实的失败原样交出去。
    /// ⚠️ **凭据从参数来，不从 [`Self::config`] 那份启动快照来。**
    /// 「在面板上粘一把新凭据 → 点测试」必须验的是**刚粘上去那把**；
    /// 读快照会让它去验旧凭据，于是这颗按钮对「凭据粘错了吗」这个唯一的问题
    /// 给出一个与现实无关的答案 —— 而它存在的全部理由就是回答这个问题。
    pub async fn probe_channel(
        &self,
        cfg: &RegistrarConfig,
        channel: Channel,
    ) -> Result<ChannelProbeOk, ChannelProbeErr> {
        // ⚠️ 「这条通道没配凭据」**不落这里** —— 它是调用方那一层的
        // `409 channel_not_configured`，一次上游请求都不该发。
        // 这里到不了那一档，`expect` 会在接线错时当场炸而不是静默变成上游故障。
        let creds = match channel {
            Channel::Yyds => cfg.yyds.as_ref(),
            Channel::Moemail => cfg.moemail.as_ref(),
        }
        .expect("调用方必须先判 channel_not_configured");

        let spy = LastStatus {
            inner: self.fetcher.as_ref(),
            last: parking_lot::Mutex::new(None),
        };
        let fail = |e: String, spy: &LastStatus<'_>| ChannelProbeErr {
            status: *spy.last.lock(),
            detail: e,
        };

        let now_fn = || crate::adapters::now_ms();
        let sleep = |ms: i64| -> futures_core::future::BoxFuture<'static, ()> {
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(ms.max(0) as u64)).await;
            })
        };
        let logger = Arc::clone(&self.logger);
        let emit = move |e: MailEvent| {
            bridge_event(
                logger.as_ref(),
                TendEvent {
                    level: e.level,
                    event: e.event,
                    msg: e.msg,
                    fields: e.fields,
                },
            )
        };
        let deps = || MailDeps {
            fetcher: &spy,
            base_url: creds.base_url.clone(),
            api_key: creds.api_key.clone(),
            now: &now_fn,
            sleep: &sleep,
            emit: &emit,
        };

        // ⚠️ **域名从这一次列表里取，不再列第二次** —— 调用方手上已经有了。
        let domains = match channel {
            Channel::Yyds => yyds_list_domains(&deps()).await,
            Channel::Moemail => moemail_list_domains(&deps()).await,
        }
        .map_err(|e| fail(e, &spy))?;

        // 🔴 **一个域名都没有 ⇒ 早退，不去验凭据，而且如实说没验。**
        // 这不是失败：列域名那一步真的成功了。面板靠 `domains: 0` 说清这件事。
        let Some(domain) = domains.first().cloned() else {
            return Ok(ChannelProbeOk {
                domains: 0,
                credentials: "not_checked",
                cleaned: true,
            });
        };

        let provider: Box<dyn MailboxProvider> = match channel {
            Channel::Yyds => Box::new(Yyds { deps: deps() }),
            Channel::Moemail => Box::new(Moemail { deps: deps() }),
        };
        let mailbox = provider
            .create_mailbox(&domain)
            .await
            .map_err(|e| fail(e, &spy))?;
        // ⚠️ **返回值的含义是「确认删掉了没有」** —— 删不掉要如实说，
        // 因为它**占着这条通道的活跃邮箱名额**，而那个名额是补池能不能继续工作的前提。
        let cleaned = provider.delete_mailbox(&mailbox).await.is_ok();
        Ok(ChannelProbeOk {
            domains: domains.len(),
            credentials: "accepted",
            cleaned,
        })
    }
}

/// 一次通道探测的成功形状。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelProbeOk {
    pub domains: usize,
    /// `"accepted"` | `"not_checked"`。
    ///
    /// 🔴 **一个域名都读不到时不去验凭据，如实回 `"not_checked"`。**
    /// 两条理由缺一不可：① 验凭据那一步要一个域名，**硬编一个就是伪造**；
    /// ② 没域名可用时补池本来就会失败，凭据好不好都不改变处置。
    pub credentials: &'static str,
    /// ⚠️ 含义是「**确认删掉了**」，不是「大概删掉了」。
    /// 为假时那个临时邮箱**占着一条活跃邮箱名额**，而那是补池能继续工作的前提。
    pub cleaned: bool,
}

/// 一次通道探测的失败形状。**状态码单开一格，不留在消息串里。**
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelProbeErr {
    /// `None` = 这一次没拿到状态码。
    ///
    /// ⚠️ **别把 `None` 读成「请求压根没发出去」** —— 传输层失败（连不上 / TLS）
    /// 落这里，而那一次请求确实发出去过。
    /// 「一次上游请求都没发出去」是**另一档**（`not_attempted`），由调用方早退。
    pub status: Option<u16>,
    /// 真实失败原文（已脱敏）。
    ///
    /// 🔴 **分档之外还要给它**：三档 `reason` 是给面板选文案的，
    /// 而「到底哪一步、上游说了什么」只有这一句说得出来。
    pub detail: String,
}

/// 记下**最后一次尝试**的上游状态码。
///
/// # 为什么不是去解析错误消息里的数字
///
/// 那条路正是 `url::http_fail_message` 上方点名禁止的：消息是给人读的、
/// 措辞可以改，**从消息串里抠数字会在改一次措辞时静默失效**。
///
/// # ⚠️ 一处已知的射程边界，如实登记
///
/// 上游回 2xx 但正文解析不出来时（例如建邮箱回 200 却没给 id），
/// 这里记下的是那个 **2xx**，而失败是在它之后发生的。
/// 分档不受影响（2xx 一律落 `upstream_error`），
/// 但响应里的 `status` 那一格会是 200 —— **它说的是「最后一次上游答了什么」**。
struct LastStatus<'a> {
    inner: &'a dyn crate::ports::fetcher::Fetcher,
    last: parking_lot::Mutex<Option<u16>>,
}

#[async_trait]
impl crate::ports::fetcher::Fetcher for LastStatus<'_> {
    async fn send(
        &self,
        req: crate::ports::fetcher::OutboundRequest,
    ) -> Result<crate::ports::fetcher::OutboundResponse, crate::ports::fetcher::FetchError> {
        let out = self.inner.send(req).await;
        // 🔴 **失败要把它清成 `None`，不是留着上一次那个。**
        // 留着的话，「列域名 200 → 建邮箱连不上」会报成 `status: 200`，
        // 而那是一句「上游答了 200 却失败了」的假话。
        *self.last.lock() = match &out {
            Ok(r) => Some(r.status),
            Err(_) => None,
        };
        out
    }
}

/// 把 `KeyPoolRepo` 接到 [`TendRepo`] 上。
pub struct PoolRepoBridge {
    pub repo: Arc<crate::pool::repo::KeyPoolRepo>,
}

#[async_trait]
impl TendRepo for PoolRepoBridge {
    async fn counts_toward_target(&self) -> u32 {
        let now = crate::adapters::now_ms();
        let all = self.repo.all(now).await.unwrap_or_default();
        // ⚠️ **这个数不是「能打上游的把数」**：被停用的、正在冷却的都算进来。
        // 它等于「池子里没被剔除的把数」，与 `TendResult::available` 同一个口径。
        u32::try_from(
            all.iter()
                .filter(|r| crate::core::keypool::counts_toward_target(r))
                .count(),
        )
        .unwrap_or(u32::MAX)
    }

    async fn add(&self, key: &str) -> Result<(), String> {
        self.repo
            .add(key, crate::adapters::now_ms())
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// 本轮真的铸出 key 之后**再对一次账**。
    ///
    /// 🔴 索引更新是**没有 CAS 的读-改-写** ⇒ 同一轮里刚被捡回来的孤儿转眼又成了孤儿。
    /// 连带后果比「索引脏了」严重得多：孤儿不被池子看到 ⇒ 缺口被高估 ⇒ **超额补铸**，
    /// 而每铸一把都是一次真实的上游建号，同时撞注册风控与活跃邮箱上限。
    /// **补铸出来的 key 还会再次触发同一条覆盖，一轮一轮地滚。**
    ///
    /// ⚠️ 它**不消灭那个窗口**（没有 CAS 就消灭不了），只把它从「一整轮」压到「一轮之内」。
    ///
    /// ⚠️ **失败只往上报一句话，由调用方记一条 warn 就算了**：索引残留是 fail-safe 的，
    /// 让它把一轮**已经成功铸出了 key** 的补池变成异常，
    /// 只会让入口打出误导性的「补池失败」，**而实际上 key 已经落盘了**。
    async fn reconcile_after_mint(&self) -> Result<(), String> {
        self.repo
            .reconcile()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

// ── 装载 ──────────────────────────────────────────────────────────────────

/// 从环境变量 + 存储里装出一份注册机配置。
///
/// 🔴 **零 panic。** 注册机是**可选子系统**，它缺凭据不该让转发、`/health`、面板一起死。
/// 装不起来就产出 blocker，由调用方决定启不启动 —— 见 `config` 的模块头。
pub fn load(
    env: &std::collections::BTreeMap<String, String>,
    stored: &Value,
) -> super::config::RegistrarLoad {
    use super::config::*;
    let g = |k: &str| env.get(k).map(String::as_str);
    let so = stored.as_object().cloned().unwrap_or_default();
    // ⚠️ **数字与布尔一并收下**：配置面写进存储的是 JSON 原生类型
    // （`enabled` 是真布尔、`targetKeys` 是真数字 —— `Spec` 那张表要求的），
    // 而这里从前只认字符串 ⇒ **面板保存成功、装载器读不出来、注册机一直关着**。
    let owned = |k: &str| stored_scalar(&so, k);
    // 🔴 **凭据是嵌套的两层**（`registrar.yyds.apiKey`），不是扁平的 `yydsApiKey`：
    // 02 §9.18 那张可编辑字段表逐字写着两层，被移植的 TS 版读的也是
    // `stored.yyds?.apiKey`。**读成扁平的后果不是报错，是静默读不到** ——
    // 切换那一刻，存量部署里配好的注册机凭据会一把都找不到，
    // 而面板上两张卡同时变成「未配置」。
    let nested = |a: &str, b: &str| -> Option<String> {
        so.get(a)
            .and_then(Value::as_object)
            .and_then(|o| stored_scalar(o, b))
    };

    let mut blockers = Vec::new();
    let mut notices = Vec::new();

    // 旧主备键的规整 —— **读路径与写路径共用这一份**。
    let migration = migrate_stored_registrar(&so);
    notices.extend(migration.notices.clone());

    let (channel, ch_blocker, channel_chosen) = resolve_channel(
        g("REGISTRAR_CHANNEL"),
        g("REGISTRAR_PRIMARY"),
        &migration.next,
    );
    if let Some(b) = ch_blocker {
        blockers.push(b);
    }

    let mut num = |env_key: &str, stored_key: &str, default: i64, field: &str| -> i64 {
        // 同上：存储里可能是真数字（配置面写的），也可能是字符串（存量）。
        let owned_raw = g(env_key)
            .map(str::to_string)
            .or_else(|| stored_scalar(&so, stored_key));
        let (v, n) = pos_int(owned_raw.as_deref(), default, field);
        if let Some(n) = n {
            notices.push(n);
        }
        v
    };

    let enabled = g("REGISTRAR_ENABLED")
        .map(str::to_string)
        .or_else(|| owned("enabled"))
        .as_deref()
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let target_keys = num(
        "REGISTRAR_TARGET_KEYS",
        "targetKeys",
        DEFAULT_TARGET_KEYS.into(),
        "targetKeys",
    );
    let mint_batch = num(
        "REGISTRAR_MINT_BATCH",
        "mintBatch",
        DEFAULT_MINT_BATCH.into(),
        "mintBatch",
    );
    let tend_interval_ms = num(
        "REGISTRAR_TEND_INTERVAL_MS",
        "tendIntervalMs",
        DEFAULT_TEND_INTERVAL_MS,
        "tendIntervalMs",
    );
    let code_timeout_ms = num(
        "REGISTRAR_CODE_TIMEOUT_MS",
        "codeTimeoutMs",
        DEFAULT_CODE_TIMEOUT_MS,
        "codeTimeoutMs",
    );
    let mint_delay_min_ms = num(
        "REGISTRAR_MINT_DELAY_MIN_MS",
        "mintDelayMinMs",
        DEFAULT_MINT_DELAY_MIN_MS,
        "mintDelayMinMs",
    );
    let mint_delay_max_ms = num(
        "REGISTRAR_MINT_DELAY_MAX_MS",
        "mintDelayMaxMs",
        DEFAULT_MINT_DELAY_MAX_MS,
        "mintDelayMaxMs",
    );
    let max_domain_attempts = num(
        "REGISTRAR_MAX_DOMAIN_ATTEMPTS",
        "maxDomainAttempts",
        DEFAULT_MAX_DOMAIN_ATTEMPTS.into(),
        "maxDomainAttempts",
    );

    // ⚠️ **这一条不受 `enabled` 门控**：一份 min > max 的配置在打开注册机的那一刻就会出问题，
    // 而运维恰恰是在「打开之前先把参数填好」的时候需要看见它。
    if mint_delay_min_ms > mint_delay_max_ms {
        blockers.push(Blocker {
            code: "registrar_delay_min_gt_max",
            // 报在上界那一格：下界与上界只能改一个，而改上界是更常见的那次。
            field: "registrar.mintDelayMaxMs".into(),
            msg: format!("铸号间隔的下界（{mint_delay_min_ms}）比上界（{mint_delay_max_ms}）还大 —— 本次不启动注册机。"),
        });
    }

    // 🔴 **打开了却没选通道 ⇒ 产一条 blocker，不替他挑一条。**
    //
    // 设计上两条通道**完全平级**。静默挑一条等于替运维做了一个他没做的决定，
    // 而后果要到「凭据缺失」那条报错**指着一条他从没选过的通道**时才浮上来
    // —— 那时他会去补那条通道的凭据，而他本来想用的是另一条。
    //
    // ⚠️ **与写错了那一支是 `else if` 的关系**：写错了已经有自己的话要说，
    // 两句同时说只会让运维以为有两处要改。`channel_chosen` 在写错那一支为真。
    if enabled && !channel_chosen {
        blockers.push(Blocker {
            code: "registrar_channel_required",
            field: "registrar.channel".into(),
            msg: "注册机打开了，但没有选邮箱通道 —— 两条通道完全平级，请挑一条。".into(),
        });
    }

    // 🔴 **两条通道都解析**，但只有选中那条缺凭据才产 blocker（理由见 `resolve_creds`）。
    // ⚠️ `if enabled` 这道门保留：关着的注册机不解析任何凭据。
    // **代价（关着时两张卡都显示「未配置」）是今天就有的，如实登记为已知缺口。**
    let (yyds, moemail) = if enabled {
        let (y, yb) = resolve_creds(
            Channel::Yyds,
            g("YYDS_BASE_URL")
                .map(str::to_string)
                .or_else(|| nested("yyds", "baseUrl"))
                .as_deref(),
            g("YYDS_API_KEY")
                .map(str::to_string)
                .or_else(|| nested("yyds", "apiKey"))
                .as_deref(),
            channel,
        );
        let (m, mb) = resolve_creds(
            Channel::Moemail,
            g("MOEMAIL_BASE_URL")
                .map(str::to_string)
                .or_else(|| nested("moemail", "baseUrl"))
                .as_deref(),
            g("MOEMAIL_API_KEY")
                .map(str::to_string)
                .or_else(|| nested("moemail", "apiKey"))
                .as_deref(),
            channel,
        );
        blockers.extend(yb);
        blockers.extend(mb);
        (y, m)
    } else {
        (None, None)
    };

    let config = RegistrarConfig {
        enabled,
        channel,
        target_keys: u32::try_from(target_keys).unwrap_or(DEFAULT_TARGET_KEYS),
        mint_batch: u32::try_from(mint_batch).unwrap_or(DEFAULT_MINT_BATCH),
        tend_interval_ms,
        code_timeout_ms,
        mint_delay_min_ms,
        mint_delay_max_ms,
        max_domain_attempts: u32::try_from(max_domain_attempts)
            .unwrap_or(DEFAULT_MAX_DOMAIN_ATTEMPTS),
        token_name: g("REGISTRAR_TOKEN_NAME")
            .map(str::to_string)
            .or_else(|| owned("tokenName"))
            .unwrap_or_else(|| DEFAULT_TOKEN_NAME.to_string()),
        agnes_platform_url: g("AGNES_PLATFORM_URL")
            .map(str::to_string)
            .or_else(|| owned("agnesPlatformUrl"))
            .unwrap_or_else(|| DEFAULT_AGNES_PLATFORM_URL.to_string()),
        yyds,
        moemail,
        // 🔴 **`blocked` 是装载的产物，而 `enabled` 一个字都不改。**
        blocked: !blockers.is_empty(),
    };
    let warnings = startup_warnings(&config);
    super::config::RegistrarLoad {
        config,
        blockers,
        notices,
        warnings,
    }
}

// ── 定时轮 ────────────────────────────────────────────────────────────────

/// 起一条定时补池的后台任务。
///
/// 🔴 **它绝不许把进程带走。** 注册机是可选子系统 ——
/// `config-degrade` 那条事故判据（真机 74% 请求 500）钉的正是
/// **注册机出问题时网关照常活着**。
/// 定时轮。**无条件起，与注册机开没开无关。**
///
/// 🔴 两条互相独立的理由，各自都够让它不许被 `runnable()` 门控：
///
/// ① **索引对账不挑注册机开没开。** 孤儿记录与幽灵索引项是 `add`/`delete`
///    半路失败留下的，转发路径天天在跑，而注册机可能一天都没开过。
///    把对账放在开关后面 = **关着的部署永远不对账**。
///
/// ② **定时器必须先存在，面板才有东西可触发。** 启动时关着就不起定时器，
///    等于「此后怎么从面板打开都没用」；而启动时开着又读死一份快照，
///    等于「从面板关也关不掉」。两个方向都得靠重启容器 —— 而面板正是配置的编辑器。
pub fn spawn_scheduler(rt: Arc<RegistrarRuntime>, repo: Arc<crate::pool::repo::KeyPoolRepo>) {
    tokio::spawn(async move {
        // 沿用上一次已知合法的间隔：读存储失败时不许把节奏也一起退化掉。
        let mut interval_ms = rt.config.tend_interval_ms.max(1_000) as u64;
        loop {
            // ⚠️ **第一轮也要等一个间隔**：进程刚起来时存储、上游都还没热，
            // 而且容器重建会让「每次重启都立刻打一轮上游」变成一个隐形的放大器。
            tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;

            // ① 对账 —— **必须在「注册机是否启用」的判断之前**，理由见上。
            // 失败只记一条：索引残留是 fail-safe 的（key 不被用，而不是坏 key 被用），
            // 不该顺带把补池也停掉。
            if let Err(e) = repo.reconcile().await {
                rt.logger.log(
                    LogEntry::new(
                        LogLevel::Warn,
                        EVENT_POOL_INDEX_RECONCILE_FAILED,
                        "key 池索引对账失败，本轮跳过对账（索引残留是 fail-safe 的）",
                    )
                    .with("err", e.to_string()),
                );
            }

            // ② 每一轮重读配置。拿不到就沿用上一轮那份，**别静默降级成 env-only**。
            let fresh = match rt.reload().await {
                Some(l) => l,
                None => {
                    rt.logger.log(LogEntry::new(
                        LogLevel::Warn,
                        EVENT_REGISTRAR_RELOAD_FAILED,
                        "重排时读配置失败，沿用上一次已知的合法间隔，本轮不补池",
                    ));
                    continue;
                }
            };
            interval_ms = fresh.config.tend_interval_ms.max(1_000) as u64;

            // ③ 开关判在最后。**先判 `enabled` 再判 `blocked`**（两者含义不同）。
            if !fresh.config.enabled || fresh.config.blocked {
                continue;
            }

            let bridge = PoolRepoBridge {
                repo: Arc::clone(&repo),
            };
            // ⚠️ 整轮的任何失败都只落进它自己的结果与事件里 —— 这里不 `unwrap`、不 `?`。
            // 锁被占着也一样：定时轮不关心「没跑成」与「没开始」的区别。
            let _ = run_round(RoundDeps {
                config: &fresh.config,
                storage: Arc::clone(&rt.storage),
                logger: Arc::clone(&rt.logger),
                fetcher: Arc::clone(&rt.fetcher),
                repo: &bridge,
                trigger: TendTrigger::Scheduled,
                // 🔴 **定时轮不传预算** —— 见 `types::SCHEDULED_ROUND_BUDGET_MS`：
                // 那个常量今天没有运行期消费者，只剩启动期交叉校验。
                round_budget_ms: None,
            })
            .await;
        }
    });
}

#[cfg(test)]
mod tests;
