//! `tender` 的判据。
//!
//! 这一轮里每一条分支都对应一次真实的上游请求或一次真实的落盘 ——
//! 所以夹具给的是**可精确控制的时钟、随机、上游回话**，断言打在**可观测量**上：
//! 发了几次请求、写了几次存储、结果那几个字段各是什么。

use super::*;
use crate::registrar::config::{Channel, ChannelCreds, RegistrarConfig};
use crate::registrar::domain_ledger::{DomainEntry, DomainState};
use std::sync::Mutex;

// ── 夹具 ──────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Spy {
    added: Mutex<Vec<String>>,
    saved_backoff: Mutex<Vec<Option<BackoffState>>>,
    saved_ledger: Mutex<Vec<DomainLedger>>,
    events: Mutex<Vec<(&'static str, &'static str)>>,
    slept: Mutex<Vec<i64>>,
    reconciled: Mutex<u32>,
    /// 让对账失败一次 —— 钉住「它不许把已经铸出 key 的一轮说成失败」。
    reconcile_fails: bool,
    listed_calls: Mutex<u32>,
}

struct FakeRepo<'a> {
    spy: &'a Spy,
    counts: u32,
}
#[async_trait]
impl TendRepo for FakeRepo<'_> {
    async fn counts_toward_target(&self) -> u32 {
        self.counts
    }
    async fn add(&self, key: &str) -> Result<(), String> {
        self.spy.added.lock().unwrap().push(key.to_string());
        Ok(())
    }
    async fn reconcile_after_mint(&self) -> Result<(), String> {
        *self.spy.reconciled.lock().unwrap() += 1;
        if self.spy.reconcile_fails {
            return Err("索引写不进去".into());
        }
        Ok(())
    }
}

struct FakeStore<'a> {
    spy: &'a Spy,
    backoff: Option<BackoffState>,
    ledger: DomainLedger,
}
#[async_trait]
impl TendStore for FakeStore<'_> {
    async fn load_backoff(&self) -> Option<BackoffState> {
        self.backoff
    }
    async fn save_backoff(&self, next: Option<BackoffState>) -> Result<(), String> {
        self.spy.saved_backoff.lock().unwrap().push(next);
        Ok(())
    }
    async fn load_domain_ledger(&self) -> DomainLedger {
        self.ledger.clone()
    }
    async fn save_domain_ledger(&self, next: &DomainLedger) -> Result<(), String> {
        self.spy.saved_ledger.lock().unwrap().push(next.clone());
        Ok(())
    }
}

struct FakeDomains<'a> {
    spy: &'a Spy,
    domains: Vec<String>,
    /// ⚠️ 这一列上游请求**自己会把时钟推一大截** —— 规格逐字写着
    /// 「列域名单请求就允许 15 秒」。判据要钉的正是「这段时间算不算进预算」，
    /// 所以慢必须由**这个调用**造成，不能靠「时钟每读一次跳几秒」去凑：
    /// 后者会让判据依赖 `now()` 的**调用次数**，而那是实现细节。
    slow_by_ms: i64,
    clock: &'a Mutex<i64>,
}
#[async_trait]
impl DomainSource for FakeDomains<'_> {
    async fn list_domains(&self) -> Result<Vec<String>, String> {
        *self.spy.listed_calls.lock().unwrap() += 1;
        *self.clock.lock().unwrap() += self.slow_by_ms;
        Ok(self.domains.clone())
    }
}

/// 脚本化的铸号器：一次尝试消费脚本里的一格。
struct ScriptedMinter {
    script: Mutex<std::collections::VecDeque<MintOutcome>>,
    /// 每次尝试拿到的候选，按序记下来。
    seen: Mutex<Vec<Vec<String>>>,
    /// 每格脚本顺带往观测本子里记的判定。
    verdicts: Mutex<std::collections::VecDeque<Vec<(String, DomainState)>>>,
}
impl ScriptedMinter {
    fn new(script: Vec<MintOutcome>) -> Self {
        Self {
            script: Mutex::new(script.into()),
            seen: Mutex::new(Vec::new()),
            verdicts: Mutex::new(std::collections::VecDeque::new()),
        }
    }
    fn with_verdicts(script: Vec<MintOutcome>, v: Vec<Vec<(String, DomainState)>>) -> Self {
        let s = Self::new(script);
        *s.verdicts.lock().unwrap() = v.into();
        s
    }
}
#[async_trait]
impl Minter for ScriptedMinter {
    async fn mint_one(&self, req: MintRequest, journal: &mut Vec<Observation>) -> MintOutcome {
        self.seen.lock().unwrap().push(req.candidates.clone());
        if let Some(vs) = self.verdicts.lock().unwrap().pop_front() {
            for (d, v) in vs {
                journal.push(Observation {
                    domain: d,
                    verdict: v,
                });
            }
        }
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(MintOutcome::Failed {
                reason: TendFailureReason::NetworkError,
            })
    }
}

fn cfg() -> RegistrarConfig {
    RegistrarConfig {
        enabled: true,
        channel: Channel::Yyds,
        target_keys: 5,
        mint_batch: 5,
        tend_interval_ms: 1_800_000,
        code_timeout_ms: 120_000,
        mint_delay_min_ms: 60_000,
        mint_delay_max_ms: 90_000,
        max_domain_attempts: 1,
        token_name: "auto".into(),
        agnes_platform_url: "https://p.test".into(),
        yyds: Some(ChannelCreds {
            base_url: "https://y.test".into(),
            api_key: "k".into(),
        }),
        moemail: None,
        blocked: false,
    }
}

fn doms(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

#[allow(clippy::too_many_arguments)]
async fn run(
    cfg: &RegistrarConfig,
    spy: &Spy,
    counts: u32,
    backoff: Option<BackoffState>,
    ledger: DomainLedger,
    domains: Vec<String>,
    minter: &ScriptedMinter,
    budget: Option<i64>,
    clock_step: i64,
    slow_list_ms: i64,
) -> TendResult {
    // 🔴 时钟是**共享**的：夹具里那几样「慢」的东西各自把它推一段，
    // 而不是靠 `now()` 每读一次跳多少。这一条是判据可信度的前提 ——
    // 用后者时，「把预算起点挪回整轮开头」这个变异会**同时少掉一次 `now()` 调用**，
    // 两个效应正好抵消，于是判据全绿而行为已经坏了。**我第一版就栽在这里。**
    let clock = Mutex::new(0i64);
    let step = clock_step;
    let now = || {
        let mut g = clock.lock().unwrap();
        *g += step;
        *g
    };
    let repo = FakeRepo { spy, counts };
    let store = FakeStore {
        spy,
        backoff,
        ledger,
    };
    let dsrc = FakeDomains {
        spy,
        domains,
        slow_by_ms: slow_list_ms,
        clock: &clock,
    };
    let sleep = |ms: i64| -> futures_core::future::BoxFuture<'static, ()> {
        Box::pin(async move {
            let _ = ms;
        })
    };
    let slept = &spy.slept;
    let sleep_rec = move |ms: i64| -> futures_core::future::BoxFuture<'static, ()> {
        slept.lock().unwrap().push(ms);
        sleep(ms)
    };
    let emit = |e: TendEvent| {
        spy.events.lock().unwrap().push((e.level, e.event));
    };
    let deps = TendDeps {
        config: cfg,
        repo: &repo,
        store: &store,
        domains: &dsrc,
        minter,
        now: &now,
        rand: &|| 0.0,
        sleep: &sleep_rec,
        emit: &emit,
        round_budget_ms: budget,
        is_importable_key: &|k: &str| !k.trim().is_empty() && k.chars().all(|c| !c.is_whitespace()),
    };
    tend_once(&deps).await
}

fn ok(n: usize) -> Vec<MintOutcome> {
    (0..n)
        .map(|i| MintOutcome::Ok {
            key: format!("sk-key-{i}"),
        })
        .collect()
}

fn events(spy: &Spy) -> Vec<&'static str> {
    spy.events.lock().unwrap().iter().map(|(_, e)| *e).collect()
}

// ── 1. 关着 ───────────────────────────────────────────────────────────────

/// 🔴 `skipped` **有且只有一个含义**：`enabled == false`。
#[tokio::test]
async fn a_disabled_registrar_is_the_only_thing_that_sets_skipped() {
    let mut c = cfg();
    c.enabled = false;
    let spy = Spy::default();
    let m = ScriptedMinter::new(vec![]);
    let r = run(
        &c,
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert!(r.skipped);
    assert_eq!(
        r.primary_channel, "",
        "skipped 时通道记空串（历史格式如此）"
    );
    assert_eq!(
        *spy.listed_calls.lock().unwrap(),
        0,
        "关着的注册机发了一次上游请求",
    );
    assert!(spy.saved_backoff.lock().unwrap().is_empty());
    assert!(spy.saved_ledger.lock().unwrap().is_empty());
}

// ── 2. 没有缺口 ───────────────────────────────────────────────────────────

/// 健康轮：`{}` 的五个产出者之一 —— 靠 `!skipped && attempted == 0 && failures 空` 认出来。
#[tokio::test]
async fn a_full_pool_returns_without_touching_the_upstream_or_the_store() {
    let spy = Spy::default();
    let m = ScriptedMinter::new(vec![]);
    let r = run(
        &cfg(),
        &spy,
        5, // target_keys 也是 5 ⇒ need = 0
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert!(!r.skipped);
    assert_eq!((r.attempted, r.minted), (0, 0));
    assert!(r.failures.is_empty(), "健康轮不许有 failures");
    assert_eq!(r.available, 5);
    assert_eq!(*spy.listed_calls.lock().unwrap(), 0);
    // 🔴 **这一支走不到收尾** —— 那正是 `backoff::streak_broken` 要治的驻留路径。
    assert!(spy.saved_ledger.lock().unwrap().is_empty());
    assert!(spy.saved_backoff.lock().unwrap().is_empty());
}

// ── 3. 退避闸 ─────────────────────────────────────────────────────────────

/// 🔴 退避窗口里**一次请求都不发**，而且这一行要**自己说清**发生了什么。
#[tokio::test]
async fn a_round_inside_the_backoff_window_never_starts_and_says_so_in_its_own_row() {
    let spy = Spy::default();
    let m = ScriptedMinter::new(ok(5));
    let b = BackoffState {
        until: 999_999,
        kind: BackoffKind::Edge,
        since: 0,
        hits: 1,
    };
    let r = run(
        &cfg(),
        &spy,
        0,
        Some(b),
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert!(
        !r.skipped,
        "🔴 绝不许改用 skipped 表示它 —— skipped 只有一个含义"
    );
    assert_eq!(r.attempted, 0);
    assert_eq!(
        r.failures,
        vec![Failure {
            reason: TendFailureReason::UpstreamBackoff,
            channel: "yyds".into(),
        }],
        "补池历史那一行必须自己说清「这一轮根本没跑」",
    );
    assert_eq!(
        *spy.listed_calls.lock().unwrap(),
        0,
        "退避窗口里还去问了上游"
    );
    assert!(m.seen.lock().unwrap().is_empty());
}

// ── 4. 预算判据 ───────────────────────────────────────────────────────────

/// 🔴 **这一格钉的是「起点挪到准备阶段之后」那条修复。**
///
/// 变异对象：把 `round_started_at` 挪回整轮开头 ——
/// 准备阶段（读池 / 读退避 / 读台账 / 列域名）一慢，i=0 那次判定就不通过，
/// 于是整颗「立即补池」按钮**诚实空转**，还打出一条
/// 「这是配置问题，请调小 CODE_TIMEOUT_MS」的**确切而错误**的处置。
#[tokio::test]
async fn a_slow_preparation_phase_does_not_eat_the_round_budget() {
    let mut c = cfg();
    // 手动那一族的形状：一次尝试恒 60 秒，预算 70 秒 ⇒ 留给 elapsed 的只有 10 秒。
    c.mint_batch = 1;
    c.max_domain_attempts = 1;
    c.code_timeout_ms = 60_000;
    let spy = Spy::default();
    let m = ScriptedMinter::new(ok(1));
    // 🔴 **慢由「列域名」这一次上游调用造成** —— 15 秒，规格里写的那个上限。
    // 时钟步长保持 1 毫秒，所以这一格**不依赖 `now()` 被调了几次**。
    let r = run(
        &c,
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        Some(70_000),
        1,
        15_000,
    )
    .await;
    assert_eq!(
        r.attempted, 1,
        "准备阶段的 15 秒被算进了预算（10 秒余量装不下）—— \
         那颗「立即补池」按钮会诚实空转，而且打出一条指向 CODE_TIMEOUT_MS 的**错误**处置。\
         人恰恰是在池子出问题时才会去点它，也就是最可能撞上这一格的时刻。",
    );
    assert_eq!(r.minted, 1);
    assert!(
        !events(&spy).contains(&"registrar.round_budget_impossible"),
        "把一次正常的手动补池报成了「这个配置永远铸不出 key」",
    );
}

/// 一次尝试装不进预算 ⇒ `error` 级，而且**措辞必须与「提前收尾」分开**。
///
/// 🔴 这一档是唯一能说破它的地方：`failures` 是空的、`minted < attempted` 是 `0 < 0` 为假
/// ⇒ 两个入口的归因日志一条都不会打。
#[tokio::test]
async fn a_configuration_that_can_never_mint_says_so_at_error_level() {
    let mut c = cfg();
    c.mint_batch = 1;
    c.code_timeout_ms = 600_000; // 一次尝试 600 秒 > 70 秒预算
    let spy = Spy::default();
    let m = ScriptedMinter::new(ok(1));
    let r = run(
        &c,
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        Some(70_000),
        1,
        0,
    )
    .await;
    assert_eq!(r.attempted, 0);
    assert!(r.failures.is_empty(), "这一档 failures 本来就是空的");
    let evs = spy.events.lock().unwrap();
    let one = evs
        .iter()
        .find(|(_, e)| *e == "registrar.round_budget_impossible")
        .expect("这个配置永远铸不出 key，而没有任何人说破");
    assert_eq!(one.0, "error", "它走的必须是 error，不是 warn");
}

/// 跑过一次之后预算不够了 ⇒ `warn` 级「提前收尾」。
///
/// 🔴 **关键在于「不开始」而不是「跑到一半停下」**：被从中间砍断时铸号的收尾不会执行，
/// **邮箱就漏了**；主动提前收尾则每一次尝试都完整走完。
#[tokio::test]
async fn running_out_of_budget_midway_finishes_early_instead_of_cutting_an_attempt() {
    let mut c = cfg();
    c.mint_batch = 5;
    c.code_timeout_ms = 20_000;
    c.mint_delay_min_ms = 0;
    c.mint_delay_max_ms = 0;
    let spy = Spy::default();
    let m = ScriptedMinter::new(ok(5));
    // 预算 45 秒：第一次尝试开得起来（20 秒），之后时钟跳到超预算。
    let r = run(
        &c,
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        Some(45_000),
        9_000,
        0,
    )
    .await;
    assert!(
        r.attempted >= 1 && r.attempted < 5,
        "attempted = {}",
        r.attempted
    );
    assert!(events(&spy).contains(&"registrar.round_budget_exhausted"));
    // 每一次开始了的尝试都完整走完 —— 铸出来几把就该有几把落盘。
    assert_eq!(spy.added.lock().unwrap().len(), r.minted as usize);
}

// ── 5. 铸出来了 ───────────────────────────────────────────────────────────

/// 🔴 **照存不误**：可疑的 key 也照样进池子，只是另记一条。
///
/// 在这里拒收 = **销毁凭据 = 数据丢失**。
#[tokio::test]
async fn a_suspicious_key_is_still_saved_and_reported_without_its_plaintext() {
    let mut c = cfg();
    c.mint_batch = 1;
    let spy = Spy::default();
    let m = ScriptedMinter::new(vec![MintOutcome::Ok {
        key: "sk-with space".into(),
    }]);
    let r = run(
        &c,
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert_eq!(
        spy.added.lock().unwrap().len(),
        1,
        "🔴 可疑就没存 = 销毁凭据"
    );
    assert_eq!(
        r.minted, 1,
        "🔴 minted 照常 +1 —— 上游那边账号是真的建出来了，说 0 是伪造"
    );
    assert!(
        r.failures
            .iter()
            .any(|f| f.reason == TendFailureReason::KeySuspicious),
        "运维必须在补池历史那一行上看得见它",
    );
    assert!(events(&spy).contains(&"registrar.minted_key_suspicious"));
}

/// 铸出来了 ⇒ 再对一次账（把「孤儿窗口」从一整轮压到一轮之内）。
#[tokio::test]
async fn minting_anything_triggers_one_reconcile() {
    let mut c = cfg();
    c.mint_batch = 1;
    let spy = Spy::default();
    let m = ScriptedMinter::new(ok(1));
    run(
        &c,
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert_eq!(*spy.reconciled.lock().unwrap(), 1);
}

/// 🔴 **对账失败不许把一轮「已经铸出了 key」的补池说成失败。**
///
/// key 已经落盘了，剩下的只是索引残留 —— 而索引残留是 fail-safe 的：
/// 读路径的空池兜底 `list()` 照样找得回来。让它翻成异常，
/// 只会让入口和补池历史一起打出误导性的「补池失败」，
/// **运维据此去查一个根本不存在的铸造故障。**
#[tokio::test]
async fn a_failed_reconcile_only_warns_and_never_unsays_the_keys_that_landed() {
    let mut c = cfg();
    c.mint_batch = 1;
    let spy = Spy {
        reconcile_fails: true,
        ..Spy::default()
    };
    let m = ScriptedMinter::new(ok(1));
    let r = run(
        &c,
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;

    assert_eq!(r.minted, 1, "🔴 对账失败把已经落盘的 key 从战报里抹掉了");
    assert!(
        r.failures.is_empty(),
        "🔴 对账失败被记成了一条铸造失败 —— 运维会去查一个不存在的故障：{:?}",
        r.failures,
    );
    assert_eq!(spy.added.lock().unwrap().len(), 1, "key 本来就已经落盘了");
    assert!(
        events(&spy).contains(&"registrar.post_mint_reconcile_failed"),
        "但它必须留下痕迹，否则索引残留就成了无声的：{:?}",
        events(&spy),
    );
}

/// 一把都没铸出来就不对账。
#[tokio::test]
async fn a_barren_round_does_not_reconcile() {
    let mut c = cfg();
    c.mint_batch = 1;
    let spy = Spy::default();
    let m = ScriptedMinter::new(vec![MintOutcome::Failed {
        reason: TendFailureReason::CodeTimeout,
    }]);
    run(
        &c,
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert_eq!(*spy.reconciled.lock().unwrap(), 0);
}

// ── 6. 失败处置 ───────────────────────────────────────────────────────────

/// `upstream_error` ⇒ **立刻中止整轮**（后端整体故障，继续只会制造更多注定失败的请求）。
#[tokio::test]
async fn an_upstream_error_aborts_the_whole_round() {
    let spy = Spy::default();
    let m = ScriptedMinter::new(vec![
        MintOutcome::Failed {
            reason: TendFailureReason::UpstreamError,
        },
        MintOutcome::Ok { key: "sk-x".into() },
    ]);
    let r = run(
        &cfg(),
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert_eq!(r.attempted, 1, "整轮没有中止 —— 第二个名额不该开始");
    assert_eq!(r.minted, 0);
}

/// 其余几支 ⇒ 本次名额作废，**下一个名额照常开始**。
#[tokio::test]
async fn an_ordinary_failure_only_wastes_one_slot() {
    let mut c = cfg();
    c.mint_delay_min_ms = 0;
    c.mint_delay_max_ms = 0;
    let spy = Spy::default();
    let m = ScriptedMinter::new(vec![
        MintOutcome::Failed {
            reason: TendFailureReason::CodeTimeout,
        },
        MintOutcome::Ok { key: "sk-x".into() },
    ]);
    let r = run(
        &c,
        &spy,
        3, // need = 2
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert_eq!((r.attempted, r.minted), (2, 1));
}

// ── 7. 限流与退避 ─────────────────────────────────────────────────────────

/// 🔴 **这一格钉的是「这一轮铸出过 key 就重新起一串」。**
///
/// 变异对象：把 `if minted > 0 { None } else { backoff }` 换回无条件传旧键 ——
/// 「前 4 把成功、第 5 把撞限流」这种**默认参数下最常见的一轮**会把 `hits` 一路推上去：
/// 连着几轮都长这样 ⇒ 15min → 30min → 1h → 2h → 4h(封顶)，
/// **而每一轮其实都在正常出 key**。
#[tokio::test]
async fn a_round_that_minted_before_hitting_the_limit_starts_a_fresh_streak() {
    let mut c = cfg();
    c.mint_delay_min_ms = 0;
    c.mint_delay_max_ms = 0;
    let spy = Spy::default();
    let mut script = ok(4);
    script.push(MintOutcome::RateLimited {
        limit_kind: BackoffKind::Edge,
        marker: None,
    });
    let m = ScriptedMinter::new(script);
    // 存储里已经攒到 hits = 4 的一串。
    let stale = BackoffState {
        until: 1,
        kind: BackoffKind::Edge,
        since: 0,
        hits: 4,
    };
    let r = run(
        &c,
        &spy,
        0,
        Some(stale),
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert_eq!(r.minted, 4);
    let saved = spy.saved_backoff.lock().unwrap();
    let s = saved[0].expect("撞了限流却没记退避");
    assert_eq!(
        s.hits, 1,
        "这一轮明明铸出了 4 把却接着旧串数下去 —— \
         指数退避要治的是「越打越死」，不是「出着 key 顺带撞了一次上限」",
    );
}

/// 一把都没铸出来 ⇒ 接着旧串数。
#[tokio::test]
async fn a_barren_round_that_hits_the_limit_continues_the_streak() {
    let mut c = cfg();
    c.mint_batch = 1;
    let spy = Spy::default();
    let m = ScriptedMinter::new(vec![MintOutcome::RateLimited {
        limit_kind: BackoffKind::App,
        marker: Some("1015".into()),
    }]);
    let stale = BackoffState {
        until: 1,
        kind: BackoffKind::App,
        since: 0,
        hits: 3,
    };
    run(
        &c,
        &spy,
        0,
        Some(stale),
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    let saved = spy.saved_backoff.lock().unwrap();
    assert_eq!(saved[0].unwrap().hits, 4);
}

/// 撞限流 ⇒ 立刻中止整轮。
#[tokio::test]
async fn hitting_the_rate_limit_aborts_the_round_immediately() {
    let spy = Spy::default();
    let mut script = vec![MintOutcome::RateLimited {
        limit_kind: BackoffKind::Edge,
        marker: None,
    }];
    script.extend(ok(4));
    let m = ScriptedMinter::new(script);
    let r = run(
        &cfg(),
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert_eq!(r.attempted, 1);
    assert_eq!(r.minted, 0);
}

/// 铸出来了、而且存储里真有一把陈旧退避键 ⇒ 清掉它。
/// ⚠️ **只有键真的存在时才写**，否则每一轮成功补池都要白付一次写。
#[tokio::test]
async fn a_successful_round_clears_a_stale_backoff_key_but_only_if_there_was_one() {
    let mut c = cfg();
    c.mint_batch = 1;
    // 有旧键
    let spy = Spy::default();
    let m = ScriptedMinter::new(ok(1));
    let stale = BackoffState {
        until: 1,
        kind: BackoffKind::Edge,
        since: 0,
        hits: 2,
    };
    run(
        &c,
        &spy,
        0,
        Some(stale),
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert_eq!(
        *spy.saved_backoff.lock().unwrap(),
        vec![None],
        "没把陈旧退避键清掉"
    );

    // 没有旧键 ⇒ 一次写都不许发
    let spy2 = Spy::default();
    let m2 = ScriptedMinter::new(ok(1));
    run(
        &c,
        &spy2,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m2,
        None,
        1,
        0,
    )
    .await;
    assert!(
        spy2.saved_backoff.lock().unwrap().is_empty(),
        "每一轮成功补池都白付了一次写",
    );
}

// ── 8. cluster 那一档 ─────────────────────────────────────────────────────

/// 🔴🔴 **这一格钉的是那条实测出来的、对整类部署完全失效的漏洞。**
///
/// **只配了一个邮箱域名的部署永远凑不满钳位要的那 2 个** ——
/// 上游一改限流文案，那种部署每一轮都打满 `mint_batch` 次注定失败的发码请求、
/// **一个退避键都不写**（稳态 240 次/天）。
///
/// 变异对象：把 `round_all_rejected` 那一半删掉，只留 `discarded > 0`。
#[tokio::test]
async fn a_single_domain_deployment_whose_only_domain_is_refused_still_backs_off() {
    let mut c = cfg();
    c.mint_batch = 1;
    let spy = Spy::default();
    let m = ScriptedMinter::with_verdicts(
        vec![MintOutcome::Failed {
            reason: TendFailureReason::DomainBlockedAll,
        }],
        vec![vec![("only.test".to_string(), DomainState::Blocked)]],
    );
    run(
        &c,
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["only.test"]), // **上游只给出一个域名**
        &m,
        None,
        1,
        0,
    )
    .await;
    let saved = spy.saved_backoff.lock().unwrap();
    let s = saved
        .first()
        .and_then(|x| *x)
        .expect("单域名部署一个退避键都没写 —— 每一轮都会打满注定失败的请求");
    assert_eq!(s.kind, BackoffKind::Cluster);
    assert!(events(&spy).contains(&"registrar.round_all_domains_rejected"));
}

/// ⚠️ **「一个不落全试过」这半句是承重的，不许省。**
///
/// 变异对象：把 `probed.len() == listed.len()` 删掉 ——
/// 「池子快满、这一轮只开了 1 个名额、而它恰好撞上一个真被拉黑的域名」也会记退避。
#[tokio::test]
async fn refusing_one_domain_out_of_many_is_not_enough_to_back_off() {
    let mut c = cfg();
    c.mint_batch = 1; // 只开一个名额
    let spy = Spy::default();
    let m = ScriptedMinter::with_verdicts(
        vec![MintOutcome::Failed {
            reason: TendFailureReason::DomainBlockedAll,
        }],
        vec![vec![("a.test".to_string(), DomainState::Blocked)]],
    );
    run(
        &c,
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test", "b.test", "c.test"]), // 上游给了三个，这一轮只试了一个
        &m,
        None,
        1,
        0,
    )
    .await;
    assert!(
        spy.saved_backoff.lock().unwrap().is_empty(),
        "只试了三分之一就记退避了 —— 那会把一次普通的域名拉黑误判成上游限流",
    );
}

/// ⚠️ **`minted == 0` 这个前提是承重的，不是保险起见。**
///
/// 变异对象：把 `minted == 0` 删掉 ——
/// 「上游真的成批拉黑了好几个域名」时这一轮**照样铸得出 key**，
/// **那时上游明明还在给我们发号，记退避等于自己把补池按停。**
#[tokio::test]
async fn a_round_that_still_minted_never_records_a_cluster_backoff() {
    let mut c = cfg();
    c.mint_batch = 2;
    c.mint_delay_min_ms = 0;
    c.mint_delay_max_ms = 0;
    let spy = Spy::default();
    let m = ScriptedMinter::with_verdicts(
        vec![
            MintOutcome::Failed {
                reason: TendFailureReason::DomainBlockedAll,
            },
            MintOutcome::Ok { key: "sk-x".into() },
        ],
        vec![
            vec![
                ("a.test".to_string(), DomainState::Blocked),
                ("b.test".to_string(), DomainState::Blocked),
            ],
            vec![],
        ],
    );
    let r = run(
        &c,
        &spy,
        3,
        None,
        DomainLedger::default(),
        doms(&["a.test", "b.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert_eq!(r.minted, 1);
    assert!(
        spy.saved_backoff.lock().unwrap().is_empty(),
        "上游明明还在给我们发号，却把补池按停了",
    );
}

/// 钳位生效那一支：**拿不到全部上游原话** —— 这条代价如实钉着。
#[tokio::test]
async fn the_clamped_branch_emits_its_own_event_and_no_domain_blocked_at_all() {
    let mut c = cfg();
    c.mint_batch = 1;
    let spy = Spy::default();
    let m = ScriptedMinter::with_verdicts(
        vec![MintOutcome::Failed {
            reason: TendFailureReason::DomainBlockedAll,
        }],
        vec![vec![
            ("a.test".to_string(), DomainState::Blocked),
            ("b.test".to_string(), DomainState::Blocked),
        ]],
    );
    run(
        &c,
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test", "b.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    let evs = events(&spy);
    assert!(evs.contains(&"registrar.domain_verdicts_discarded"));
    assert!(
        !evs.contains(&"registrar.domain_blocked"),
        "🔴 钳位生效时 newly_blocked 可证恒为空 —— \
         面板横幅与文档不许说成「去翻 domain_blocked 就能分辨」",
    );
}

// ── 9. 写次数 ─────────────────────────────────────────────────────────────

/// 🔴 **「结论没变就一次写都不发」是写账的一根轴。**
///
/// 稳态下台账的结论不再变动 ⇒ 这把键的写次数是 **0** 而不是 48 次/天。
#[tokio::test]
async fn a_healthy_round_writes_the_ledger_at_most_once_and_the_backoff_key_zero_times() {
    let mut c = cfg();
    c.mint_delay_min_ms = 0;
    c.mint_delay_max_ms = 0;
    let spy = Spy::default();
    let m = ScriptedMinter::new(ok(5));
    run(
        &c,
        &spy,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert!(
        spy.saved_ledger.lock().unwrap().len() <= 1,
        "一轮铸 5 把，台账写了 {} 次",
        spy.saved_ledger.lock().unwrap().len(),
    );
    assert!(
        spy.saved_backoff.lock().unwrap().is_empty(),
        "退避键不该被写"
    );
}

/// 台账**一条观测都没有**时，一次写都不发。
#[tokio::test]
async fn a_round_with_nothing_learned_writes_the_ledger_zero_times() {
    let mut c = cfg();
    c.mint_batch = 1;
    let spy = Spy::default();
    let m = ScriptedMinter::new(ok(1));
    run(
        &c,
        &spy,
        0,
        None,
        // ⚠️ 给一个**已经记过 total** 的台账：默认台账的 total 是 None，
        // 而这一轮会观测到 1 个域名 ⇒ total 变了 ⇒ 照样算脏、照样写一次。
        // 要测「结论一个字都没变」就必须让 total 也不变。
        DomainLedger {
            updated_at: 0,
            total: Some(1),
            entries: std::collections::HashMap::new(),
        },
        doms(&["a.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    assert!(
        spy.saved_ledger.lock().unwrap().is_empty(),
        "结论一个字都没变却写了一次",
    );
}

// ── 10. 轮内域名轮换 ──────────────────────────────────────────────────────

/// 🔴 **这一格钉的是 `used_this_round` 的理由。**
///
/// 台账**轮内不更新**，而选域名对「已知 ok」那一档按 `at` 全序排序 —— `at` 轮内不变
/// ⇒ 不标记的话**一轮 5 个名额确定性地全落在同一个域名上**
///（6 分钟内 5 个账号挂在同一个域名下），而那一档的说明逐字写着
/// 「LRU 轮换，别把一个好域名打成上游风控的焦点」——
/// **而「打成焦点」发生的正是轮内那几次连续注册。**
#[tokio::test]
async fn one_round_rotates_across_domains_instead_of_hammering_the_same_one() {
    let mut c = cfg();
    c.mint_delay_min_ms = 0;
    c.mint_delay_max_ms = 0;
    let spy = Spy::default();
    let m = ScriptedMinter::new(ok(5));
    // 三个都是「已知 ok」且 `at` 相同 —— 不标记的话它们的排序轮内恒定。
    let mut entries = std::collections::HashMap::new();
    for d in ["a.test", "b.test", "c.test"] {
        entries.insert(
            d.to_string(),
            DomainEntry {
                s: DomainState::Ok,
                at: 1,
                n: 1,
            },
        );
    }
    run(
        &c,
        &spy,
        0,
        None,
        DomainLedger {
            updated_at: 0,
            total: Some(3),
            entries,
        },
        doms(&["a.test", "b.test", "c.test"]),
        &m,
        None,
        1,
        0,
    )
    .await;
    let seen = m.seen.lock().unwrap();
    let picked: Vec<String> = seen.iter().map(|c| c[0].clone()).collect();
    let distinct: std::collections::HashSet<&String> = picked.iter().collect();
    assert!(
        distinct.len() >= 3,
        "一轮 5 个名额全落在 {} 个域名上：{picked:?} —— \
         那正是「把一个好域名打成上游风控焦点」的形态",
        distinct.len(),
    );
}

// ── 11. 五个空表产出者分得开 ──────────────────────────────────────────────

/// 🔴 **`{}` 有五个产出者，消费方必须靠 `skipped` + `attempted` + `failures` 三个字段合读。**
///
/// **别拿 `minted_by_channel` 的空表去推断** —— 这一格把五种形态摆在一起，
/// 断言它们**两两可分**。
#[tokio::test]
async fn all_five_producers_of_an_empty_mint_table_are_distinguishable() {
    let mut disabled = cfg();
    disabled.enabled = false;
    let mut one = cfg();
    one.mint_batch = 1;

    let s1 = Spy::default();
    let r_disabled = run(
        &disabled,
        &s1,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &ScriptedMinter::new(vec![]),
        None,
        1,
        0,
    )
    .await;

    let s2 = Spy::default();
    let r_full = run(
        &cfg(),
        &s2,
        5,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &ScriptedMinter::new(vec![]),
        None,
        1,
        0,
    )
    .await;

    let s3 = Spy::default();
    let b = BackoffState {
        until: 999_999,
        kind: BackoffKind::Edge,
        since: 0,
        hits: 1,
    };
    let r_backoff = run(
        &cfg(),
        &s3,
        0,
        Some(b),
        DomainLedger::default(),
        doms(&["a.test"]),
        &ScriptedMinter::new(vec![]),
        None,
        1,
        0,
    )
    .await;

    let s4 = Spy::default();
    let r_failed = run(
        &one,
        &s4,
        0,
        None,
        DomainLedger::default(),
        doms(&["a.test"]),
        &ScriptedMinter::new(vec![MintOutcome::Failed {
            reason: TendFailureReason::CodeTimeout,
        }]),
        None,
        1,
        0,
    )
    .await;

    // ① 关着
    assert!(r_disabled.skipped);
    // ② 健康轮（缺口 <= 0）
    assert!(!r_full.skipped && r_full.attempted == 0 && r_full.failures.is_empty());
    // ③ 退避窗口内整轮跳过
    assert!(!r_backoff.skipped && r_backoff.attempted == 0);
    assert_eq!(
        r_backoff.failures[0].reason,
        TendFailureReason::UpstreamBackoff
    );
    // ④ 尝试了但全失败
    assert!(r_failed.attempted > 0 && !r_failed.failures.is_empty());

    // 两两可分：这四种的 `minted_by_channel` **全是空表**。
    for r in [&r_disabled, &r_full, &r_backoff, &r_failed] {
        assert!(
            r.minted_by_channel.is_empty(),
            "🔴 这一格的前提就是它们的 minted_by_channel 都为空 —— \
             靠它推断产出者是不可能的",
        );
    }
}

// ── 12. 聚合口径 ──────────────────────────────────────────────────────────

/// **两个入口的收尾日志必须给出同一份口径。**
#[test]
fn failures_summarize_into_one_stable_shape() {
    let f = |r: TendFailureReason, c: &str| Failure {
        reason: r,
        channel: c.into(),
    };
    let out = summarize_failures(&[
        f(TendFailureReason::RegisterFailed, "yyds"),
        f(TendFailureReason::RegisterFailed, "yyds"),
        f(TendFailureReason::CodeTimeout, "moemail"),
        f(TendFailureReason::RegisterFailed, "yyds"),
    ]);
    assert_eq!(out, "yyds:register_failed×3 moemail:code_timeout×1");
    assert_eq!(summarize_failures(&[]), "");
}

/// 本轮已被拒过的域名从观测本子现算，**同一个域名以最后一条为准**。
#[test]
fn a_domain_that_was_refused_then_accepted_is_not_held_back() {
    let j = vec![
        Observation {
            domain: "a.test".into(),
            verdict: DomainState::Blocked,
        },
        Observation {
            domain: "a.test".into(),
            verdict: DomainState::Ok,
        },
        Observation {
            domain: "b.test".into(),
            verdict: DomainState::Blocked,
        },
    ];
    let r = rejected_this_round(&j);
    assert!(
        !r.contains("a.test"),
        "先被拒、后来又成功过的，这一轮不该再被让到后面"
    );
    assert!(r.contains("b.test"));
}
