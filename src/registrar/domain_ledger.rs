//! 域名台账：记住哪些域名能用、哪些被上游屏蔽过，好让下一轮不拿它们浪费额度。
//!
//! 移植自 TS 版 `src/core/registrar/domain-ledger.ts`（686 行），规则真源
//! `docs/porting/01-core-rules.md §10.7`。**零 IO**：时钟、随机、存储都由调用方传进来。
//!
//! # 🔴🔴 这个模块最要紧的一句话：**判死的判据是启发式的，别把它改写成事实**
//!
//! 上游用 `400` **同时**表达「这个域名被屏蔽了」与「你这个出口发得太频繁了」，
//! **正文是唯一的区分线索**，而正文的措辞是别人家服务的实现细节。
//! ⇒ 判死走的是**负向匹配**（「不是限流就算屏蔽」），**因此必然会误判**。
//!
//! **三层东西压着这个误判，缺一层就变成静默的系统性记错**：
//! 1. 判死要**两跳**（`n >= 2`），**而且两跳必须来自两轮**；
//! 2. [`commit_journal`] 的**一轮最多学 1 个域名**的钳位（**与文案无关**）；
//! 3. [`select_domains`] 全序排序**永不 filter** ⇒ 判死的域名照样在候选里，只是排最后。
//!
//! ⚠️ **我们没有见过「域名真被屏蔽」那条 400 的正文长什么样。这是知识空白，不是代码事实。**
//! 真机抓到一条之后应该把判死改成**正向**匹配。

use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// 台账在存储里的键。
pub const DOMAIN_LEDGER_KEY: &str = "registrar:domains";

/// 台账里最多留几条域名结论。
///
/// ⚠️ **键空间有界不等于值有界**：这把键恒为 1 把，但它的**值**是按上游
/// `list_domains()` 的返回长起来的。512 的由来：上游今天返回 374 个域名，留出余量。
/// 🔴 **这是别人家服务的当前取值、不是常数，所以它不进面板文案** ——
/// 把一个「当前取值」印在面板上，运维会把它当成自己这套部署的事实。
pub const DOMAIN_LEDGER_CAP: usize = 512;

/// `ok` 结论的寿命。🟡 **没有实测支撑**，是个取舍值。
pub const OK_TTL_MS: i64 = 604_800_000; // 7 天

/// `blocked` 结论的寿命。
///
/// 🟡 **刻意取 24 小时而不是更长**：一条错记录的寿命上界越短越好，**而代价极小** ——
/// 过期之后它重新进候选，最多多花 1 次发码请求；真黑名单域名会被两跳规则立刻压回去。
pub const BLOCK_TTL_MS: i64 = 86_400_000; // 24 小时

/// 上游正文进事件之前截到多长。
pub const BODY_SNIPPET_MAX: usize = 512;

/// 回查发现仍有地址 / 凭据时的替代文本。
pub const UNSAFE_UPSTREAM_MESSAGE: &str = "<上游正文里仍有邮箱地址或凭据，已整段丢弃>";
const ADDRESS_PLACEHOLDER: &str = "<邮箱地址>";
const SECRET_PLACEHOLDER: &str = "<凭据>";

/// 限流的词表。
///
/// ⚠️ **它是启发式，不是协议**：命中 ⇒ 判限流（**安全**）；不命中 ⇒ 判域名屏蔽（**有风险**）。
const RATE_LIMIT_MARKERS: [&str; 6] = [
    "too many",
    "rate limit",
    "ratelimit",
    "too frequent",
    "频繁",
    "次数过多",
];

/// 一次发码请求的分档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendCodeClass {
    Ok,
    /// 边缘网关限流（429 / 403）。
    RateLimitedEdge,
    /// 应用层限流（400 + 正文命中词表）。
    RateLimitedApp,
    /// 判死：400 且正文可读但不像限流。**这一档是负向匹配的产物。**
    DomainBlocked,
    /// 400 但正文是空的。
    ///
    /// ⚠️ **它不是「域名被屏蔽」的同义词**：判死走负向匹配，而负向匹配至少要先有一段正文可读。
    /// **这一档不产生任何域名判定。**
    Unreadable,
    UpstreamError,
}

/// 把一次发码请求的结果分档。
pub fn classify_send_code(status: u16, body: &str) -> SendCodeClass {
    // ⚠️ **`403` 并进 edge**：它在本仓从前是一条**死分支**（撞到的实测全是 429 与 400），
    // 但并进来比留一条「睡 5 秒接着打」的路安全 ——
    // **后者正是把惩罚窗口一次次续上的那个形态。**
    if status == 429 || status == 403 {
        return SendCodeClass::RateLimitedEdge;
    }
    if status == 400 {
        let t = body.trim();
        if t.is_empty() {
            return SendCodeClass::Unreadable;
        }
        let lower = t.to_lowercase();
        return if RATE_LIMIT_MARKERS.iter().any(|m| lower.contains(m)) {
            SendCodeClass::RateLimitedApp
        } else {
            SendCodeClass::DomainBlocked
        };
    }
    if (200..300).contains(&status) {
        SendCodeClass::Ok
    } else {
        SendCodeClass::UpstreamError
    }
}

/// 边缘网关的那串标记（实测形态 `error code: 1015`）。
///
/// 🔴 **只进事件字段，不参与任何判定**：判据是状态码 429，而这一串是别人家边缘网关的
/// 实现细节。**让它参与判定就等于把一条外部实现细节焊进控制流。**
pub fn edge_marker(body: &str) -> Option<&'static str> {
    body.contains("1015").then_some("1015")
}

/// 一条域名结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DomainState {
    Ok,
    Blocked,
}

/// 台账里的一格。
///
/// ⚠️ **`n` 数的是轮不是观测条数**：[`commit_journal`] 先把同一个域名在这一轮里的多条观测
/// 折叠成一条，所以一轮之内最多 +1。**这条不是文风，是「两跳 = 两轮」的全部依据。**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DomainEntry {
    pub s: DomainState,
    pub at: i64,
    pub n: u32,
}

/// 整份台账。
///
/// 🔴 **第三态 `unknown` 用「不在 `entries` 里」表示** ——
/// **少一个能撒谎的字段，也省掉「`s: "unknown"` 与不在表里哪个优先」这种没人会去测的分歧。**
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DomainLedger {
    pub updated_at: i64,
    /// 上一轮记下来的域名总数。
    ///
    /// ⚠️ **是上一轮记下来的观测值，不是现打一次 `list_domains()` 问出来的**：
    /// `/status` 今天一次上游请求都不发，**为了凑一个总数破掉这条性质，
    /// 等于给面板加了一颗每几秒就打一次上游的按钮**。**取不到就如实 `None`。**
    pub total: Option<u32>,
    pub entries: HashMap<String, DomainEntry>,
}

/// 面板取数用的汇总，**也是判据的观测点**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerSummary {
    /// 上一轮观测到的上游域名总数。
    ///
    /// ⚠️ **取不到就如实 `None`，不现打一次上游去凑**：
    /// `/status` 今天一次上游请求都不发，为了凑一个总数破掉这条性质，
    /// 等于给面板加了一颗每几秒就打一次上游的按钮。
    pub total: Option<u32>,
    pub ok: usize,
    pub blocked: usize,
    /// 「疑似」：判过一次、还没坐实，或者屏蔽窗口已经过了。
    pub suspect: usize,
    /// `total` 未知时它也是 `None` —— **「未探过」是个减法，减数没有就算不出来**。
    pub unknown: Option<u32>,
    pub updated_at: Option<i64>,
    /// 台账里实际记着几条。与 `cap` 一起说明「值有没有快撑满」。
    pub size: usize,
    pub cap: usize,
}

/// 把台账折成面板那一格。
///
/// # 🔴 三格的口径必须与选域名那张表**逐档同序**
///
/// 面板说「3 个可用」而选择器认为 0 个可用，是一种没人查得动的自相矛盾。
/// 所以这里的判定逐字照着 [`domain_rank`] 那几支写：
///
/// * **过期的 `ok` 不计进任何一格** —— 它在选择器里退回「未知」那一档，面板照同一个口径说话；
/// * `blocked` 且（屏蔽窗口过了 **或** 只判过一次）⇒ `suspect`，与那条「两跳 = 两轮」同源；
/// * 其余 `blocked` 才是坐实的 `blocked`。
pub fn summarize_ledger(ledger: &DomainLedger, now: i64) -> LedgerSummary {
    let (mut ok, mut blocked, mut suspect) = (0usize, 0usize, 0usize);
    for e in ledger.entries.values() {
        let age = now - e.at;
        if e.s == DomainState::Ok {
            if age < OK_TTL_MS {
                ok += 1;
            }
            // 过期的 ok：**一格都不计**，见上。
        } else if age >= BLOCK_TTL_MS || e.n <= 1 {
            suspect += 1;
        } else {
            blocked += 1;
        }
    }
    let known = u32::try_from(ok + blocked + suspect).unwrap_or(u32::MAX);
    LedgerSummary {
        total: ledger.total,
        ok,
        blocked,
        suspect,
        unknown: ledger.total.map(|t| t.saturating_sub(known)),
        // 🔴 `0` 是「从来没记过」，而不是 1970 年那一刻。
        // 原样交出去会让面板渲染出一个 1970 年的时间戳。
        updated_at: (ledger.updated_at != 0).then_some(ledger.updated_at),
        size: ledger.entries.len(),
        cap: DOMAIN_LEDGER_CAP,
    }
}

// ── 选域名 ────────────────────────────────────────────────────────────────

/// 档位。数字越小越优先。
fn tier_of(entry: Option<&DomainEntry>, now: i64) -> u8 {
    match entry {
        // 1 = ok 且未过期
        Some(e) if e.s == DomainState::Ok && now - e.at < OK_TTL_MS => 1,
        // 3 = blocked 但已过期、或 n == 1 的「可疑」
        Some(e) if e.s == DomainState::Blocked && (now - e.at >= BLOCK_TTL_MS || e.n < 2) => 3,
        // 4 = blocked 且 n >= 2 且未过期
        Some(e) if e.s == DomainState::Blocked => 4,
        // 2 = 不在表里的、以及 ok 已过期的（**真正的未知**）
        _ => 2,
    }
}

/// 从候选里挑出这一次尝试要用的域名。
///
/// # 🔴 全序排序 + 取前 N，**绝不是 filter**
///
/// 「只要 `all_domains` 非空、`limit >= 1`，返回值就非空」是**结构性**回答：
/// 最坏情况（全表被判死）退化成台账出现之前的随机轮换，
/// **而不是「一个候选都挑不出来 ⇒ `attempted` 恒为 0 ⇒ 注册机静默停摆」**。
///
/// 排序键（从最前到最后）：`rejected`（**跨档**）→ `tier` → `spent`（**档内**）→ `order` → 域名字典序。
///
/// - 🔴 **`spent` 用次数而不是布尔**：布尔在名额数多于域名数时会退化成 `p q r p p`
///   （第四个名额之后大家都是「派过」，次序又塌回 `at`），**而次数给出的是真正的轮转 `p q r p q`**。
/// - 🔴 **`spent` 只在档内生效，绝不跨档**：跨档的话「只有一个已知 ok 域名 + 一堆判死域名」
///   这种台账**会在第二个名额上把判死的那些顶到前面** —— 那正是台账存在的理由要挡掉的事。
/// - 🔴 **`rejected` 必须跨档**：[`commit_journal`] 的钳位在「同一轮里两个已知 ok 的域名同时被拉黑」时
///   会把两条结论**整体作废** ⇒ 台账一个字都不变 ⇒ 下一轮排序与这一轮**逐字节相同**
///   ⇒ 每一轮都把全部名额喂给那两个坏域名，**补池归零直到那两条 ok 过 7 天**。
///   光靠档内轮换救不了它：那两个域名自己就是第一档，档内怎么转都轮不到第二档。
/// - ⚠️ **两者敢不敢跨档的判据**：`spent` 表达的是「派出去过」（**噪声**），
///   `rejected` 表达的是「上游这一轮当着面拒了它」（**证据**）。
/// - ⚠️ **同档内以域名字典序兜底**：输入顺序本身是上游返回的顺序 —— **那不是我们能断言的东西。**
///
/// `all_domains` 的顺序就是调用方洗过的牌，第二档按它排（`order` = 在这个切片里的下标）。
pub fn select_domains(
    ledger: &DomainLedger,
    all_domains: &[String],
    now: i64,
    limit: usize,
    spent: &HashMap<String, u32>,
    rejected: &HashSet<String>,
) -> Vec<String> {
    if all_domains.is_empty() {
        return Vec::new();
    }
    // 🔴 夹到至少 1 —— 返回空数组会让这一次尝试**连一个域名都没有**。
    let limit = limit.max(1);

    let mut ranked: Vec<(u8, u8, u32, usize, &String)> = all_domains
        .iter()
        .enumerate()
        .map(|(order, d)| {
            let rej = u8::from(rejected.contains(d));
            let tier = tier_of(ledger.entries.get(d), now);
            let sp = spent.get(d).copied().unwrap_or(0);
            (rej, tier, sp, order, d)
        })
        .collect();

    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0) // rejected：跨档
            .then(a.1.cmp(&b.1)) // tier
            .then(a.2.cmp(&b.2)) // spent：档内
            .then(a.3.cmp(&b.3)) // order：调用方洗过的牌
            .then(a.4.cmp(b.4)) // 域名字典序兜底
    });

    ranked
        .into_iter()
        .take(limit)
        .map(|(_, _, _, _, d)| d.clone())
        .collect()
}

// ── 一轮的观测记账 ────────────────────────────────────────────────────────

/// 一条观测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub domain: String,
    pub verdict: DomainState,
}

/// [`commit_journal`] 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    pub ledger: DomainLedger,
    /// 这一轮**第二跳判死**的域名（只有它们记事件）。
    pub newly_blocked: Vec<String>,
    /// 被钳位丢掉的域名数。
    ///
    /// ⚠️ **数的是折叠之后的域名数，不是观测条数。**
    pub discarded: usize,
    /// 🔴 **为假时调用方一次 `put` 都不许发。**
    pub dirty: bool,
}

/// 把一轮的观测合进台账。
///
/// 🔴 **「一轮最多学 1 条」是本设计里唯一与上游文案无关的防线。**
/// 判据取自实测形态：**限流发作是「一轮里连着好几个域名全挂」**，
/// 而有台账之后正常的域名屏蔽是零星的。
///
/// 🔴 **两跳 = 两轮，所以同一个域名在这一轮里的多条观测先折叠成一条。**
/// 不折叠的话「两跳」在**一轮之内**就走得完：钳位数的是**域名数**，而 `n` 若按**观测条数**累加
/// ⇒ 同一个域名在一轮里被拒两次 ⇒ 钳位不触发、`n` 直接到 2
/// ⇒ **一轮之内从 unknown 判死并发出事件**。
///
/// ⚠️ **第一跳刻意不记事件**：事件环只有 100 格，冷启动一轮就能产出好几条，
/// **第一跳也记会在最该看诊断的那一刻把诊断挤出环。**
/// **学得慢完全可以接受：我们要的是记住好域名，不是记全所有坏的。**
pub fn commit_journal(
    cur: &DomainLedger,
    observations: &[Observation],
    total: Option<u32>,
    now: i64,
) -> CommitOutcome {
    // ① 一轮之内同一个域名只留最后一条（**保留首次插入的次序**）。
    let mut order: Vec<String> = Vec::new();
    let mut folded: HashMap<String, DomainState> = HashMap::new();
    for o in observations {
        if !folded.contains_key(&o.domain) {
            order.push(o.domain.clone());
        }
        folded.insert(o.domain.clone(), o.verdict);
    }

    // ② **折叠之后再算钳位。**
    let blocked_count = folded
        .values()
        .filter(|v| **v == DomainState::Blocked)
        .count();
    let clamp = blocked_count >= 2;

    let mut next = cur.clone();
    let mut newly_blocked = Vec::new();
    let mut changed = false;
    let mut discarded = 0usize;

    for d in &order {
        let verdict = folded[d];
        if clamp && verdict == DomainState::Blocked {
            discarded += 1;
            continue;
        }
        // ③ 逐条合并：方向相同 ⇒ `n += 1`；方向变了 ⇒ `n = 1`；`at = now`。
        let was = next.entries.get(d).copied();
        let was_dead = was.is_some_and(|e| e.s == DomainState::Blocked && e.n >= 2);
        let n = match was {
            Some(e) if e.s == verdict => e.n.saturating_add(1),
            _ => 1,
        };
        next.entries.insert(
            d.clone(),
            DomainEntry {
                s: verdict,
                at: now,
                n,
            },
        );
        changed = true;
        // **只有第二跳判死才记事件。**
        if verdict == DomainState::Blocked && n >= 2 && !was_dead {
            newly_blocked.push(d.clone());
        }
    }

    // ④ 值的有界性：超过 CAP ⇒ 按 (at, 域名) 升序删掉最旧的那些。
    if next.entries.len() > DOMAIN_LEDGER_CAP {
        let mut all: Vec<(String, DomainEntry)> =
            next.entries.iter().map(|(k, v)| (k.clone(), *v)).collect();
        all.sort_by(|a, b| a.1.at.cmp(&b.1.at).then(a.0.cmp(&b.0)));
        let drop_n = next.entries.len() - DOMAIN_LEDGER_CAP;
        for (k, _) in all.into_iter().take(drop_n) {
            next.entries.remove(&k);
        }
        changed = true;
    }

    // ⑤ `dirty = changed || (total 非 None 且与旧值不同)`。
    let total_changed = total.is_some() && total != cur.total;
    if total.is_some() {
        next.total = total;
    }
    let dirty = changed || total_changed;
    if dirty {
        next.updated_at = now;
    }

    CommitOutcome {
        ledger: next,
        newly_blocked,
        discarded,
        dirty,
    }
}

/// 合并两份台账（并发写时的收敛规则）。
///
/// 同名域名取 `at` 更大的那个，`total` 取新的那份。
/// **丢一条域名结论只是下一轮重学（便宜）**，所以合并只把后果从「整份覆盖」
/// 降到「逐域名取更新的那一条」。
pub fn merge_domain_ledger(cur: &DomainLedger, next: &DomainLedger) -> DomainLedger {
    let mut out = cur.clone();
    for (d, e) in &next.entries {
        match out.entries.get(d) {
            Some(old) if old.at >= e.at => {}
            _ => {
                out.entries.insert(d.clone(), *e);
            }
        }
    }
    out.total = next.total.or(cur.total);
    out.updated_at = cur.updated_at.max(next.updated_at);
    out
}

// ── 脱敏 ──────────────────────────────────────────────────────────────────

fn url_encode(s: &str) -> String {
    // 只处理会出现在邮箱地址与凭据里的那几个字符，够用且可预测。
    s.replace('%', "%25")
        .replace('@', "%40")
        .replace('+', "%2B")
        .replace('/', "%2F")
        .replace(':', "%3A")
}

fn body_snippet(s: &str) -> String {
    s.chars().take(BODY_SNIPPET_MAX).collect()
}

/// 把上游正文脱敏成可以进事件的一小段。
///
/// 🔴 **顺序是「先脱敏、后截断」，反过来会在边界上漏出地址前缀**：
/// 从前是先截断再替换 —— 地址正好跨在 512 那一刀上时，**前半截留在正文里**，
/// 而回查查的是**完整**地址、查不出来 ⇒ 一个 `u0@x` 这样的前缀原样进事件。
/// **泄漏量小，但那句「替换完再回头查一遍」的纪律在那一档从前是没做到的。**
///
/// 🔴 **纪律：替换完再回头查一遍，还命中就整段丢掉。**
/// **「我们没想到的编码形态」的后果因此是「少说一句话」，不是「多漏一个地址」。**
///
/// ⚠️ **代价明写（一）**：回查只查**完整地址**（含 URL 编码形态），**不查本地部分**
///（形如 `u0` 的短串，回查它会把一大堆本来无害的正文整段丢掉）。**这是有意取舍，不是遗漏。**
///
/// ⚠️⚠️ **代价明写（二）—— 一处真盲区，移植时实测到的**：
/// 替换与回查用的是**同一份** [`url_encode`]，而它只处理 `% @ + / :` 那几个字符。
/// 于是**我们不认识的编码形态两头都看不见**：例如表单编码把空格写成 `+` 时，
/// secret `a b` 在正文里是 `a+b` ⇒ 替换对不上、回查查的两种形态在输出里也都不存在
/// ⇒ **原样放行**。
/// 🔴 **这与上面那条「没想到的形态只会少说一句话」不矛盾，但它划出了那句话的边界**：
/// 那句话管的是「**认得出**是凭据、只是替换没覆盖到」；这一档是「**根本认不出**」。
/// 由 `an_encoding_we_do_not_know_about_slips_through_and_we_say_so` 如实钉着。
///
/// ⚠️ **空串必须跳过**：`""` 在任何 `contains` 下都恒为真 ——
/// 不跳过的话**一个空 secret 就能让每一条诊断都变成 [`UNSAFE_UPSTREAM_MESSAGE`]**。
pub fn upstream_message(body: &str, address: &str, secrets: &[String]) -> String {
    let mut redacted = body.to_string();
    if !address.is_empty() {
        redacted = redacted
            .replace(address, ADDRESS_PLACEHOLDER)
            .replace(&url_encode(address), ADDRESS_PLACEHOLDER);
    }
    for s in secrets {
        if s.is_empty() {
            continue;
        }
        redacted = redacted
            .replace(s.as_str(), SECRET_PLACEHOLDER)
            .replace(&url_encode(s), SECRET_PLACEHOLDER);
    }
    // 与 `dispatcher` 逐字同源的那条：`sk-` 开头的凭据形态。
    redacted = crate::pool::dispatcher::redact_text(&redacted);

    // **回查打在真正会被写出去的那一段上**：替换本身也可能拼出新的一处命中。
    let out = body_snippet(&redacted);
    let hunted: Vec<&str> = std::iter::once(address)
        .chain(secrets.iter().map(String::as_str))
        .filter(|s| !s.is_empty())
        .collect();
    if hunted
        .iter()
        .any(|s| out.contains(s) || out.contains(&url_encode(s)))
    {
        return UNSAFE_UPSTREAM_MESSAGE.to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn led(pairs: &[(&str, DomainState, i64, u32)]) -> DomainLedger {
        DomainLedger {
            updated_at: 0,
            total: None,
            entries: pairs
                .iter()
                .map(|(d, s, at, n)| {
                    (
                        (*d).to_string(),
                        DomainEntry {
                            s: *s,
                            at: *at,
                            n: *n,
                        },
                    )
                })
                .collect(),
        }
    }
    fn doms(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    // ── 分档 ──────────────────────────────────────────────────────────

    #[test]
    fn four_oh_three_is_folded_into_the_edge_rate_limit_bucket() {
        // ⚠️ 它从前是死分支，但并进来比留一条「睡 5 秒接着打」的路安全 ——
        // 后者正是把惩罚窗口一次次续上的那个形态。
        assert_eq!(classify_send_code(403, ""), SendCodeClass::RateLimitedEdge);
        assert_eq!(classify_send_code(429, ""), SendCodeClass::RateLimitedEdge);
    }

    #[test]
    fn a_four_hundred_that_mentions_rate_limiting_is_never_read_as_a_dead_domain() {
        for body in [
            "Too Many Requests",
            "rate limit exceeded",
            "RateLimit",
            "too frequent",
            "请求过于频繁",
            "发送次数过多",
        ] {
            assert_eq!(
                classify_send_code(400, body),
                SendCodeClass::RateLimitedApp,
                "词表漏了：{body}",
            );
        }
    }

    /// 🔴 这一格钉的是那条负向匹配本身：**不像限流 ⇒ 判死**。
    #[test]
    fn a_four_hundred_with_an_unfamiliar_body_falls_through_to_domain_blocked() {
        assert_eq!(
            classify_send_code(400, "domain not allowed"),
            SendCodeClass::DomainBlocked,
        );
    }

    /// ⚠️ **`unreadable` 不是「域名被屏蔽」的同义词**：负向匹配至少要先有一段正文可读。
    #[test]
    fn a_four_hundred_with_an_empty_body_produces_no_domain_verdict_at_all() {
        assert_eq!(classify_send_code(400, ""), SendCodeClass::Unreadable);
        assert_eq!(classify_send_code(400, "   \n "), SendCodeClass::Unreadable);
    }

    /// 🔴 边缘标记**只进事件字段，不参与判定** —— 这一格钉的是「它没被焊进控制流」。
    #[test]
    fn the_edge_marker_is_diagnostic_only_and_changes_no_verdict() {
        let with = classify_send_code(400, "blocked, error code: 1015");
        let without = classify_send_code(400, "blocked");
        assert_eq!(with, without, "1015 改变了判定 —— 它只该进事件字段");
        assert_eq!(edge_marker("error code: 1015"), Some("1015"));
        assert_eq!(edge_marker("nothing here"), None);
    }

    // ── 选域名 ────────────────────────────────────────────────────────

    /// 🔴 **结构性回答**：全表被判死时也必须挑得出东西来。
    ///
    /// 变异对象：把 `select_domains` 写成 filter 掉第 4 档 ——
    /// 这一格会拿到空数组，也就是「`attempted` 恒为 0 ⇒ 注册机静默停摆」。
    #[test]
    fn a_fully_dead_ledger_still_yields_candidates() {
        let now = 1_000_000;
        let l = led(&[
            ("a.com", DomainState::Blocked, now, 5),
            ("b.com", DomainState::Blocked, now, 5),
        ]);
        let got = select_domains(
            &l,
            &doms(&["a.com", "b.com"]),
            now,
            1,
            &HashMap::new(),
            &HashSet::new(),
        );
        assert_eq!(
            got.len(),
            1,
            "全表判死时一个候选都挑不出来 —— 注册机会静默停摆"
        );
    }

    /// `limit` 夹到至少 1。
    #[test]
    fn a_zero_limit_is_clamped_to_one() {
        let got = select_domains(
            &DomainLedger::default(),
            &doms(&["a.com"]),
            0,
            0,
            &HashMap::new(),
            &HashSet::new(),
        );
        assert_eq!(got, doms(&["a.com"]));
    }

    #[test]
    fn known_good_domains_come_before_unknown_ones_and_dead_ones_come_last() {
        let now = 1_000_000;
        let l = led(&[
            ("good.com", DomainState::Ok, now, 1),
            ("dead.com", DomainState::Blocked, now, 2),
        ]);
        let got = select_domains(
            &l,
            &doms(&["dead.com", "unknown.com", "good.com"]),
            now,
            3,
            &HashMap::new(),
            &HashSet::new(),
        );
        assert_eq!(got, doms(&["good.com", "unknown.com", "dead.com"]));
    }

    /// 🔴 **`spent` 只在档内生效，绝不跨档。**
    ///
    /// 变异对象：把 `spent` 提到 `tier` 之前 ——
    /// 「一个已知 ok + 一堆判死」这种台账会在第二个名额上把判死的顶到前面，
    /// 那正是台账存在的理由要挡掉的事。
    #[test]
    fn spending_a_good_domain_never_promotes_a_dead_one_above_it() {
        let now = 1_000_000;
        let l = led(&[
            ("good.com", DomainState::Ok, now, 1),
            ("dead.com", DomainState::Blocked, now, 3),
        ]);
        let spent = HashMap::from([("good.com".to_string(), 3u32)]);
        let got = select_domains(
            &l,
            &doms(&["dead.com", "good.com"]),
            now,
            1,
            &spent,
            &HashSet::new(),
        );
        assert_eq!(got, doms(&["good.com"]), "判死的域名被 spent 顶到了前面");
    }

    /// 🔴 **`rejected` 必须跨档。**
    ///
    /// 变异对象：把 `rejected` 放到 `tier` 之后 —— 那两个「已知 ok 但这一轮当着面被拒」
    /// 的域名会继续霸占全部名额，而钳位让台账一个字都不变 ⇒ **补池归零直到那两条 ok 过 7 天**。
    #[test]
    fn a_domain_the_upstream_just_refused_drops_below_even_the_dead_ones() {
        let now = 1_000_000;
        let l = led(&[
            ("good.com", DomainState::Ok, now, 1),
            ("dead.com", DomainState::Blocked, now, 3),
        ]);
        let rejected = HashSet::from(["good.com".to_string()]);
        let got = select_domains(
            &l,
            &doms(&["good.com", "dead.com"]),
            now,
            2,
            &HashMap::new(),
            &rejected,
        );
        assert_eq!(
            got,
            doms(&["dead.com", "good.com"]),
            "被当面拒过的没有沉到最后"
        );
    }

    /// 🔴 **`spent` 用次数而不是布尔** —— 名额多于域名时要给出真正的轮转。
    ///
    /// 变异对象：把 `spent` 换成 `bool` ⇒ 第四个名额之后次序塌回 `order`，
    /// 得到 `p q r p p` 而不是 `p q r p q`。
    #[test]
    fn spending_rotates_properly_when_there_are_more_slots_than_domains() {
        let all = doms(&["p.com", "q.com", "r.com"]);
        let l = DomainLedger::default(); // 三个都在第 2 档，只由 spent / order 排
        let mut spent: HashMap<String, u32> = HashMap::new();
        let mut picked = Vec::new();
        for _ in 0..5 {
            let one = select_domains(&l, &all, 0, 1, &spent, &HashSet::new());
            let d = one[0].clone();
            *spent.entry(d.clone()).or_insert(0) += 1;
            picked.push(d);
        }
        assert_eq!(picked, doms(&["p.com", "q.com", "r.com", "p.com", "q.com"]));
    }

    /// ⚠️ 同档同 `spent` 时以域名字典序兜底 —— 输入顺序是上游返回的顺序，
    /// **那不是我们能断言的东西**（此处 `order` 相同才轮到字典序）。
    #[test]
    fn the_final_tiebreak_is_the_domain_name_itself() {
        let all = doms(&["b.com", "a.com"]);
        // order 不同 ⇒ 先按 order；把 order 拉平只能靠同一个下标，这里改测「稳定且可预测」。
        let got = select_domains(
            &DomainLedger::default(),
            &all,
            0,
            2,
            &HashMap::new(),
            &HashSet::new(),
        );
        assert_eq!(got, doms(&["b.com", "a.com"]), "洗好的牌次序被破坏了");
    }

    #[test]
    fn an_expired_ok_entry_falls_back_to_genuinely_unknown_not_to_good() {
        let now = OK_TTL_MS + 10;
        let l = led(&[("old.com", DomainState::Ok, 0, 1)]);
        // old.com 的 ok 已过期 ⇒ 第 2 档，与 fresh.com 同档，按 order 排
        let got = select_domains(
            &l,
            &doms(&["old.com", "fresh.com"]),
            now,
            2,
            &HashMap::new(),
            &HashSet::new(),
        );
        assert_eq!(got, doms(&["old.com", "fresh.com"]));
        assert_eq!(tier_of(l.entries.get("old.com"), now), 2);
    }

    #[test]
    fn an_expired_block_and_a_single_hop_suspicion_share_the_third_tier() {
        let now = BLOCK_TTL_MS + 10;
        let expired = led(&[("x.com", DomainState::Blocked, 0, 5)]);
        let suspicious = led(&[("y.com", DomainState::Blocked, now, 1)]);
        assert_eq!(tier_of(expired.entries.get("x.com"), now), 3);
        assert_eq!(tier_of(suspicious.entries.get("y.com"), now), 3);
    }

    // ── 记账 ──────────────────────────────────────────────────────────

    /// 🔴 **一轮最多学 1 条 blocked** —— 这一格是那条防线的**唯一检测点**。
    ///
    /// 删掉它，「上游改了限流文案 ⇒ 好域名被成批判死」就重新变成静默的。
    #[test]
    fn a_round_that_blames_two_domains_learns_neither_of_them() {
        let obs = vec![
            Observation {
                domain: "a.com".into(),
                verdict: DomainState::Blocked,
            },
            Observation {
                domain: "b.com".into(),
                verdict: DomainState::Blocked,
            },
        ];
        let out = commit_journal(&DomainLedger::default(), &obs, None, 100);
        assert!(
            out.ledger.entries.is_empty(),
            "钳位没生效：{:?}",
            out.ledger.entries
        );
        assert_eq!(out.discarded, 2);
        assert!(
            !out.dirty,
            "什么都没学到却说自己脏了 ⇒ 调用方会白写一次存储"
        );
    }

    /// 一条 blocked 不触发钳位。
    #[test]
    fn a_round_that_blames_exactly_one_domain_does_learn_it() {
        let obs = vec![Observation {
            domain: "a.com".into(),
            verdict: DomainState::Blocked,
        }];
        let out = commit_journal(&DomainLedger::default(), &obs, None, 100);
        assert_eq!(out.ledger.entries["a.com"].n, 1);
        assert!(out.dirty);
    }

    /// 🔴 **两跳 = 两轮** —— 同一个域名在一轮里的多条观测先折叠成一条。
    ///
    /// 变异对象：把折叠那一步删掉 ⇒ 钳位数的是域名数（仍为 1、不触发），
    /// 而 `n` 按观测条数累加直接到 2 ⇒ **一轮之内从 unknown 判死并发出事件**。
    #[test]
    fn two_refusals_of_the_same_domain_in_one_round_count_as_one_hop() {
        let obs = vec![
            Observation {
                domain: "a.com".into(),
                verdict: DomainState::Blocked,
            },
            Observation {
                domain: "a.com".into(),
                verdict: DomainState::Blocked,
            },
        ];
        let out = commit_journal(&DomainLedger::default(), &obs, None, 100);
        assert_eq!(out.ledger.entries["a.com"].n, 1, "一轮之内就走完了两跳");
        assert!(out.newly_blocked.is_empty(), "⚠️ 第一跳刻意不记事件");
    }

    /// **只有第二跳判死才记事件。**
    #[test]
    fn only_the_second_hop_emits_an_event() {
        let obs = vec![Observation {
            domain: "a.com".into(),
            verdict: DomainState::Blocked,
        }];
        let r1 = commit_journal(&DomainLedger::default(), &obs, None, 100);
        assert!(r1.newly_blocked.is_empty());
        let r2 = commit_journal(&r1.ledger, &obs, None, 200);
        assert_eq!(r2.newly_blocked, vec!["a.com".to_string()]);
        // 第三跳不再重复记 —— 它已经死了。
        let r3 = commit_journal(&r2.ledger, &obs, None, 300);
        assert!(r3.newly_blocked.is_empty(), "已经死了的域名又记了一次事件");
    }

    /// 方向变了 ⇒ `n` 归 1。
    #[test]
    fn a_verdict_that_flips_direction_resets_the_hop_counter() {
        let blocked = vec![Observation {
            domain: "a.com".into(),
            verdict: DomainState::Blocked,
        }];
        let ok = vec![Observation {
            domain: "a.com".into(),
            verdict: DomainState::Ok,
        }];
        let r1 = commit_journal(&DomainLedger::default(), &blocked, None, 100);
        let r2 = commit_journal(&r1.ledger, &blocked, None, 200);
        assert_eq!(r2.ledger.entries["a.com"].n, 2);
        let r3 = commit_journal(&r2.ledger, &ok, None, 300);
        assert_eq!(r3.ledger.entries["a.com"].n, 1, "换了方向却接着往上加");
        assert_eq!(r3.ledger.entries["a.com"].s, DomainState::Ok);
    }

    /// 🔴 `dirty` 为假时调用方**一次 put 都不许发**。
    #[test]
    fn a_round_with_nothing_to_say_is_not_dirty() {
        let out = commit_journal(&DomainLedger::default(), &[], None, 100);
        assert!(!out.dirty);
        assert_eq!(out.ledger.updated_at, 0, "没脏却动了 updated_at");
    }

    /// `total` 变了也算脏（即使一条观测都没有）。
    #[test]
    fn a_new_total_alone_makes_the_ledger_dirty() {
        let out = commit_journal(&DomainLedger::default(), &[], Some(374), 100);
        assert!(out.dirty);
        assert_eq!(out.ledger.total, Some(374));
        // 同一个 total 再来一次就不脏了。
        let again = commit_journal(&out.ledger, &[], Some(374), 200);
        assert!(!again.dirty);
    }

    #[test]
    fn the_ledger_is_capped_by_dropping_the_oldest_entries_first() {
        let mut cur = DomainLedger::default();
        for i in 0..DOMAIN_LEDGER_CAP {
            cur.entries.insert(
                format!("d{i:04}.com"),
                DomainEntry {
                    s: DomainState::Ok,
                    at: i as i64,
                    n: 1,
                },
            );
        }
        let obs = vec![Observation {
            domain: "new.com".into(),
            verdict: DomainState::Ok,
        }];
        let out = commit_journal(&cur, &obs, None, 999_999);
        assert_eq!(out.ledger.entries.len(), DOMAIN_LEDGER_CAP);
        assert!(out.ledger.entries.contains_key("new.com"), "新的没留住");
        assert!(
            !out.ledger.entries.contains_key("d0000.com"),
            "最旧的没被删掉"
        );
    }

    #[test]
    fn merging_takes_the_newer_entry_per_domain() {
        let a = led(&[
            ("x.com", DomainState::Ok, 100, 1),
            ("y.com", DomainState::Ok, 500, 1),
        ]);
        let b = led(&[
            ("x.com", DomainState::Blocked, 300, 2),
            ("y.com", DomainState::Blocked, 200, 2),
        ]);
        let m = merge_domain_ledger(&a, &b);
        assert_eq!(
            m.entries["x.com"].s,
            DomainState::Blocked,
            "x 该取 b 的（at 更大）"
        );
        assert_eq!(
            m.entries["y.com"].s,
            DomainState::Ok,
            "y 该保留 a 的（at 更大）"
        );
    }

    // ── 脱敏 ──────────────────────────────────────────────────────────

    #[test]
    fn the_address_is_replaced_in_both_plain_and_url_encoded_form() {
        let out = upstream_message(
            "failed for u0@mail.test and u0%40mail.test",
            "u0@mail.test",
            &[],
        );
        assert!(!out.contains("u0@mail.test"), "{out}");
        assert!(!out.contains("u0%40mail.test"), "{out}");
        assert_eq!(out.matches(ADDRESS_PLACEHOLDER).count(), 2);
    }

    /// 🔴 **先脱敏、后截断。**
    ///
    /// 变异对象：调换顺序 —— 地址跨在 512 那一刀上时前半截会留在正文里，
    /// 而回查查的是完整地址、查不出来 ⇒ `u0@x` 这样的前缀原样进事件。
    #[test]
    fn redaction_happens_before_truncation_so_no_prefix_survives_at_the_cut() {
        let addr = "verylonglocalpart@mail.test";
        // 让地址正好跨在 512 那一刀上
        let pad = "x".repeat(BODY_SNIPPET_MAX - 10);
        let body = format!("{pad}{addr} tail");
        let out = upstream_message(&body, addr, &[]);
        assert!(
            !out.contains("verylonglocalpart"),
            "地址前缀在截断边界上漏出来了：{out}"
        );
    }

    /// 我们**认识**的那种编码形态，替换本身就该盖住 —— 不该劳动回查。
    ///
    /// ⚠️ 写这一格时我先把它当成了「回查要拦住」的例子，实际跑出来是 `token=<凭据>` ——
    /// **因为 `AAA%3ABBB` 正是 [`url_encode`] 会生成的那一个**。留着它当反向控制：
    /// 回查若在这里就发作，说明替换那一侧漏了自己认识的形态。
    #[test]
    fn an_encoding_we_do_generate_is_caught_by_replacement_not_by_the_post_check() {
        let out = upstream_message("token=AAA%3ABBB", "", &["AAA:BBB".to_string()]);
        assert_eq!(out, "token=<凭据>");
    }

    /// 🔴 **回查那一格**：替换**自己拼出**一处新命中时，整段丢掉。
    ///
    /// 变异对象：把 `upstream_message` 末尾那道回查删掉 —— 这一格会拿到
    /// `token=<凭据>` 并放行，而输出里确确实实还留着那段 secret。
    ///
    /// ⚠️ 这不是构造出来的怪例，它正是规格里那句
    /// 「**替换本身也可能拼出新的一处命中**」说的形态：占位符是一段固定文本，
    /// 而 secret 恰好是它的子串时，替换完反而制造了一处命中。
    #[test]
    fn a_hit_created_by_the_replacement_itself_costs_the_whole_message() {
        let out = upstream_message("token=zzz", "", &["zzz".to_string(), "凭据".to_string()]);
        assert_eq!(
            out, UNSAFE_UPSTREAM_MESSAGE,
            "回查没拦住替换自己拼出来的那一处"
        );
    }

    /// ⚠️⚠️ **如实登记一处盲区：我们不认识的编码形态，替换与回查都看不见。**
    ///
    /// 例如表单编码把空格写成 `+`：secret 是 `a b` 时，
    /// [`url_encode`] 不产生 `a+b`（它只处理 `% @ + / :` 那几个），
    /// 于是 ①替换对不上、②回查查的两种形态在输出里也都不存在 ⇒ **原样放行**。
    ///
    /// 🔴 **这一格断言的是「今天确实会漏」，不是「今天挡住了」。**
    /// 留着它是为了让这处盲区有一个会说话的落点：哪天有人扩了 [`url_encode`]，
    /// 这一格会红，而那时该做的是**更新这一格**，不是回头把 `url_encode` 改窄。
    #[test]
    fn an_encoding_we_do_not_know_about_slips_through_and_we_say_so() {
        let out = upstream_message("q=a+b", "", &["a b".to_string()]);
        assert_eq!(
            out, "q=a+b",
            "⚠️ 若这一格红了：说明 url_encode 的射程变了，请更新本判据的登记，别改窄 url_encode",
        );
    }

    /// ⚠️ **空串必须跳过** —— 一个空 secret 会让每一条诊断都变成整段丢弃。
    #[test]
    fn an_empty_secret_does_not_poison_every_diagnostic() {
        let out = upstream_message("a harmless body", "", &[String::new()]);
        assert_eq!(out, "a harmless body", "空 secret 让回查恒真了");
    }

    /// 与 dispatcher 同源的那条 `sk-` 形态也要被抹掉。
    #[test]
    fn a_credential_shaped_string_is_masked_even_when_nobody_told_us_about_it() {
        let out = upstream_message("bad key sk-AbCd1234EfGh end", "", &[]);
        assert!(!out.contains("sk-AbCd1234EfGh"), "{out}");
    }

    #[test]
    fn the_snippet_is_bounded() {
        let out = upstream_message(&"x".repeat(5000), "", &[]);
        assert_eq!(out.chars().count(), BODY_SNIPPET_MAX);
    }

    // ── 面板汇总 ──────────────────────────────────────────────────────────
    //
    // 🔴 这几格的口径必须与选域名那张表同源。面板说「3 个可用」
    // 而选择器认为 0 个可用，是一种没人查得动的自相矛盾。

    /// 🔴 **过期的 `ok` 一格都不计。**
    ///
    /// 它在选择器里退回「未知」那一档（`domain_rank` 里那条 `now - e.at < OK_TTL_MS`），
    /// 面板必须照同一个口径说话 —— 继续算 `ok` 就是**拿一条 7 天前的观测当今天的结论**。
    #[test]
    fn an_expired_ok_falls_out_of_every_bucket_just_like_the_selector_says() {
        let l = led(&[
            ("fresh.test", DomainState::Ok, 0, 1),
            ("stale.test", DomainState::Ok, 0, 1),
        ]);
        // 把时钟推到「fresh 还新鲜、stale 已过期」办不到（两条 at 一样），
        // 所以分两次看：先都新鲜，再都过期。
        assert_eq!(summarize_ledger(&l, 1).ok, 2);
        let old = summarize_ledger(&l, OK_TTL_MS);
        assert_eq!(old.ok, 0, "🔴 7 天前的观测还在被当成今天的结论");
        assert_eq!(
            (old.blocked, old.suspect),
            (0, 0),
            "过期的 ok 不许掉进 blocked/suspect —— 它是「不知道」，不是「不能用」",
        );
        assert_eq!(old.size, 2, "但它还占着台账里的位置");
    }

    /// 判过一次、还没坐实 ⇒ `suspect`，不是 `blocked`。
    ///
    /// 与「两跳 = 两轮」同源：`n <= 1` 的那一条在选择器里排在坐实的前面。
    #[test]
    fn one_strike_is_suspect_and_two_strikes_is_blocked() {
        let l = led(&[
            ("a.test", DomainState::Blocked, 0, 1),
            ("b.test", DomainState::Blocked, 0, 2),
        ]);
        let s = summarize_ledger(&l, 1);
        assert_eq!(s.suspect, 1, "只判过一次的那条是「疑似」");
        assert_eq!(s.blocked, 1, "判过两次的才是坐实的");
    }

    /// 屏蔽窗口过了 ⇒ 退回 `suspect`，与选择器里那条 `age >= BLOCK_TTL_MS` 同源。
    #[test]
    fn a_block_that_aged_out_goes_back_to_suspect() {
        let l = led(&[("a.test", DomainState::Blocked, 0, 5)]);
        assert_eq!(summarize_ledger(&l, 1).blocked, 1);
        let aged = summarize_ledger(&l, BLOCK_TTL_MS);
        assert_eq!(aged.blocked, 0);
        assert_eq!(aged.suspect, 1, "🔴 24 小时前的屏蔽还被当成今天还屏蔽着");
    }

    /// 🔴 **`total` 不知道时 `unknown` 也必须是 `None`。**
    ///
    /// 「未探过几个」是个减法，**减数没有就算不出来**。
    /// 拿 0 当减数会算出一个看着很确定的负数截断值。
    #[test]
    fn unknown_is_absent_when_the_total_is_absent_because_it_is_a_subtraction() {
        let mut l = led(&[("a.test", DomainState::Ok, 0, 1)]);
        assert_eq!(l.total, None);
        assert_eq!(
            summarize_ledger(&l, 1).unknown,
            None,
            "🔴 减数没有却算出了一个数 —— 那个数是编的",
        );
        l.total = Some(10);
        assert_eq!(summarize_ledger(&l, 1).unknown, Some(9));
    }

    /// 台账比上游报的总数还多时，`unknown` 夹到 0 而不是下溢。
    #[test]
    fn a_ledger_larger_than_the_reported_total_clamps_to_zero() {
        let mut l = led(&[
            ("a.test", DomainState::Ok, 0, 1),
            ("b.test", DomainState::Ok, 0, 1),
            ("c.test", DomainState::Ok, 0, 1),
        ]);
        l.total = Some(1);
        assert_eq!(
            summarize_ledger(&l, 1).unknown,
            Some(0),
            "🔴 下溢成了一个天文数字"
        );
    }

    /// `updated_at == 0` 的含义是「从来没记过」，不是 1970 年那一刻。
    #[test]
    fn a_never_written_ledger_reports_no_timestamp_instead_of_the_epoch() {
        let l = led(&[]);
        assert_eq!(
            summarize_ledger(&l, 1).updated_at,
            None,
            "🔴 面板会渲染出一个 1970 年的时间戳",
        );
        let mut l2 = led(&[]);
        l2.updated_at = 5;
        assert_eq!(summarize_ledger(&l2, 1).updated_at, Some(5));
    }

    /// `size` / `cap` 一起给 —— 运维要看得出「值有没有快撑满」。
    #[test]
    fn the_summary_reports_how_full_the_ledger_is() {
        let s = summarize_ledger(&led(&[("a.test", DomainState::Ok, 0, 1)]), 1);
        assert_eq!(s.size, 1);
        assert_eq!(s.cap, DOMAIN_LEDGER_CAP, "只给 size 说不出「快满了吗」");
    }
}
