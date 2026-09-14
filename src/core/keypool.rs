//! key 池调度的**全部判据与全部状态迁移**（`keypool.ts`，01 §2）。
//!
//! 这个文件回答四个问题，**每个问题全仓只有这里一份答案**：
//!
//! 1. 这把 key 被停用了吗（[`is_disabled`]）
//! 2. 现在能不能拿它去打上游（[`is_available`]）
//! 3. 它还算不算在 `target_keys` 名额里（[`counts_toward_target`]）—— **补池专用，与 2 不是一个问题**
//! 4. 这一轮该选哪一把（[`select`] / [`select_key`] / [`select_sticky`]）
//!
//! 以及五个状态迁移（`apply_*`）与一张四格计数（[`pool_health`]）。
//!
//! # 🔴 `apply_*` 一律返回新记录，绝不就地改
//!
//! `pool/repo.rs` 的写消除依赖这条性质：它要拿**改之前那一份**和**改之后那一份**
//! 逐字段比，就地改会让「之前那一份」在比较发生之前就已经变了 ⇒ 写消除判据恒成立
//! ⇒ 每一次调度写都被丢掉，**而测试全绿**。所以这里全部是 `&KeyRecord -> KeyRecord`。

use crate::core::types::{KeyRecord, PoolPolicy};

/// 负载均衡模式（裁定 F）。
///
/// # 两种模式各自解决什么，以及为什么两个都要
///
/// - [`LbMode::Balanced`]：TS 版的游标轮询，**逐字移植**。它是既有行为，
///   `dispatcher` 与 `keypool` 那 79 格判据钉的全是它。
/// - [`LbMode::Priority`]：**粘滞**。当前这把还能用就一直用它，坏了才换人；
///   换人时取**优先级最小**的可用 key（**不是**游标的下一个 —— 后者会让每次换人
///   都往后漂，长期看又退化成轮询）。
///
/// 🔴 **默认是 [`LbMode::Priority`]，而这是一次行为变更，不是「移植的顺手改进」**
/// （规格 §3.10，要写进 CHANGELOG 与五语言文档）。
/// 依据：粘滞解决的是「几百个身份在同一个出口 IP 上秒级交替出现」这个形状 ——
/// 真实客户端永远是「一台机器、一个账号、连续干活」。
/// agnes 的 key 全是注册机铸出来的新号，这个形状在 agnes 侧**比在 kiro 侧更成立**。
///
/// ⚠️ **`Priority` 不许复用 `Balanced` 的判据**，两边各有各的一组（规格 §3.10）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LbMode {
    /// 粘滞 + 优先级最小者。**默认**。
    #[default]
    Priority,
    /// 游标轮询（TS 版的既有行为，逐字移植）。
    Balanced,
}

impl LbMode {
    /// 配置里那个字符串 → 模式。**认不出来的值回 `None`**，由调用方决定怎么办
    /// （规格：配置字段的降级方向归 `config_provenance`，`core::keypool` 不替它裁）。
    ///
    /// ⚠️ 大小写不敏感，与 kiro2api 的 `eq_ignore_ascii_case` 一致：
    /// 运维在面板上敲 `Priority` 与敲 `priority` 得到同一个结果，
    /// 否则那会是一个「配了但没生效、还没有任何提示」的洞。
    pub fn parse(v: &str) -> Option<Self> {
        if v.eq_ignore_ascii_case("priority") {
            Some(Self::Priority)
        } else if v.eq_ignore_ascii_case("balanced") {
            Some(Self::Balanced)
        } else {
            None
        }
    }

    /// 对外展示用的名字（面板 / 配置回显）。**与 [`LbMode::parse`] 认的字面量同一套。**
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Priority => "priority",
            Self::Balanced => "balanced",
        }
    }
}

/// 选中的结果。
///
/// 🔴 **`next_cursor` 与 `current_id` 都是「请调用方存回去」的状态。**
/// `core` 是零 IO 岛，它不持有任何跨请求的可变状态；游标那份住在
/// `AppState` 的 `AtomicUsize`（裁定 L：**每进程一份**，与 TS 的模块级 `let cursor`
/// 逐字同语义），粘滞那份住在 `AppState` 的一格 `Mutex<Option<String>>`。
///
/// 🔴 连带约束（01 §2.4）：**面板不许展示「下一把 key / 轮换顺序」** ——
/// 多副本共卷时那个值与实际不符，展示了就是给一个假数字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// 选中的那一条在 `records` 里的下标。
    pub index: usize,
    /// 存回游标的值。**`Priority` 模式下原样带回传进来的那个**（它不推进游标）。
    pub next_cursor: usize,
    /// 存回「当前粘着谁」的值。**两种模式都要存**：
    /// 运维把模式从 `balanced` 切到 `priority` 时，粘滞才有一个合理的起点。
    pub current_id: String,
}

/// 「这把 key 被管理员**手工**停用了吗」。🔴 **全仓唯一一份判据。**
///
/// 各写各的后果是「面板显示已停用、调度器照常用它」—— 本字段最难被发现的失败形态。
/// 所以**问这个问题的地方一处都不许自己判**（包括 `key_bucket`、`pool_health`、
/// 面板的删除前置条件、503 的 `Retry-After` 计算）。
///
/// ⚠️ **TS 那边用的是真值性（`!!r.disabled`）而不是 `=== true`**，理由是记录来自裸
/// `JSON.parse`：对一个安全开关，「读不懂就当停用」是安全的那一侧。
/// **Rust 侧那一半责任在 [`crate::core::narrow::narrow_key_record`]** ——
/// 它把存储里的 `"true"` / `1` / `{}` 一律窄化成 `Some(true)`。
/// 到了这里类型已经是 `Option<bool>`，所以这里只剩「缺席 = 没停用」这一条。
/// 🔴 **别把窄化那一半挪进来**：挪进来就要求这个函数收 `serde_json::Value`，
/// 而它在 [`select_key`] 的热路径循环里，逐条记录做一次 JSON 取值是白付的代价。
pub fn is_disabled(r: &KeyRecord) -> bool {
    r.disabled.unwrap_or(false)
}

/// 「现在能不能拿这把 key 去打上游」。**只回答这一个问题**，见 [`counts_toward_target`]。
pub fn is_available(r: &KeyRecord, now: i64) -> bool {
    !r.evicted && !is_disabled(r) && r.cooldown_until <= now
}

/// 「这把 key 还算不算在 `target_keys` 名额里」——🔴 **补池专用，与 [`is_available`] 是两个问题。**
///
/// 两者曾经是同一个函数，只在**被停用**的 key 上分歧，所以那次合并一直没出事；
/// `disabled` 落地的那一刻就出事了（两次实测复现，01 §2.2）：
/// 补池拿 `target_keys - available` 当缺口，**差多少就真的去注册多少个 Agnes 账号**
/// ⇒ **「在面板上停用一把 key」＝「自动注册一个新账号」**。
///
/// ⚠️ **判据是 `!r.evicted`，一个字都不多 —— `disabled` 与 `cooling` 都占名额。**
/// 写成 `!evicted && cooldown_until <= now`（只放过 `disabled`）还留着两条同族缺陷：
/// ① 停用一把**正在冷却**的 key 仍会触发一次补池（而运维去停用一把 key 的最常见起因，
///    恰恰是面板显示它「冷却中」），多铸出来那把冷却到期后**再也不会退掉**；
/// ② 冷却导致池子**永久膨胀，线性、不封顶**（实测 `target_keys=3`：一次全池限流 ⇒ 6，
///    下一次 ⇒ 9，再下一次 ⇒ 12，每一把都是一次真实建号 + 一个临时邮箱）。
///
/// 🔴 **这个函数与时间无关，不许加回 `now` 参数** —— 加回去就是把上面两条的入口重新打开。
///
/// **代价明写**：整池都在冷却时补池**不会**去铸替补，网关在冷却窗口内持续 503。
/// 这是刻意的：那种情况下上游要么在限流要么在故障，铸新号打的是同一个后端。
pub fn counts_toward_target(r: &KeyRecord) -> bool {
    !r.evicted
}

/// 游标轮询选 key（`keypool.ts:97-110`，[`LbMode::Balanced`]）。**逐字移植。**
///
/// 返回 `(选中的下标, 下一个游标)`。`next_cursor = idx + 1`，
/// ⚠️ **刻意不取模**：选中最后一把时 `idx + 1` 恰好等于 `records.len()`（越了界），
/// 取模那一步留到下一次进来时才做（`cursor % records.len()`）——
/// 因为**池子大小会变**，进来时才取模才是对的。
///
/// # 🔴 这里**没有**溢出，所以也没有护栏（一次裁定，2026-09-11）
///
/// 曾经写的是 `idx.wrapping_add(1)`，注释说「`next_cursor` 可能溢出 `usize`
/// （理论上要 2^64 次请求）」。**那句话是错的**，两道各自独立成立的理由：
///
/// 1. `idx < records.len()`，而 Rust 的切片长度恒 `<= isize::MAX`（分配上限），
///    所以 `idx + 1 <= isize::MAX` —— **没有任何输入能让它够到 `usize` 的上界**。
/// 2. 游标**根本不累积**：唯一的调用方 `pool::dispatcher` 做的是
///    `cursor.store(sel.next_cursor)` 而**不是** `fetch_add`（全仓没有任何一处用
///    `fetch_add` 推游标），于是存回去的值恒在 `1..=len` 里。打多少次请求都一样。
///
/// ⇒ `wrapping_add` 是一道**永远够不着的护栏**。删掉它而不是留着，因为它谎报了
/// 一个不存在的风险；而且万一将来真被够到，悄悄绕成 `0` 比在 debug 下 panic **更难查**。
///
/// 判据改钉让它够不着的那条不变量本身，见
/// [`tests::the_cursor_never_accumulates_so_overflow_is_unreachable`]。
pub fn select_key(records: &[KeyRecord], cursor: usize, now: i64) -> Option<(usize, usize)> {
    if records.is_empty() {
        return None;
    }
    let start = cursor % records.len();
    for i in 0..records.len() {
        let idx = (start + i) % records.len();
        if is_available(&records[idx], now) {
            return Some((idx, idx + 1));
        }
    }
    None
}

/// 粘滞选 key（[`LbMode::Priority`]）。
///
/// `current` 是「上一次选中的那把的 id」。
/// - 它还在池子里、还可用 ⇒ **继续用它，不换**；
/// - 否则 ⇒ 取**优先级最小**的可用 key；同优先级按**池内顺序**破平。
///
/// 🔴 **换人时取优先级最小者，而不是「当前这把的下一个」**：后者会让每次换人都往后漂，
/// 长期看又退化成轮询，于是上游看到的仍然是几百个身份在交替 —— 粘滞就白做了。
///
/// ⚠️ **按 `(priority, 下标)` 破平，不是按 id 破平**：下标来自 `pool:index` 的顺序，
/// 而索引顺序**无语义**（对账会整体重排它）。这意味着一次索引对账之后，
/// 同优先级的那一组里「换人先换到谁」可能变 —— 这是可接受的：它只影响换人的目标，
/// 不影响粘滞本身（粘住的那把是按 id 找的）。**照 kiro2api 逐字同形。**
pub fn select_sticky(records: &[KeyRecord], current: Option<&str>, now: i64) -> Option<usize> {
    if records.is_empty() {
        return None;
    }
    // 粘滞：当前那把还能用就继续用它。
    if let Some(id) = current
        && let Some(i) = records.iter().position(|r| r.id == id)
        && is_available(&records[i], now)
    {
        return Some(i);
    }
    records
        .iter()
        .enumerate()
        .filter(|(_, r)| is_available(r, now))
        .min_by_key(|(i, r)| (r.priority, *i))
        .map(|(i, _)| i)
}

/// 两种模式的统一入口。**转发热路径只调这一个。**
///
/// 把「哪种模式」留在这里而不是散在调用方，是因为两种模式要存回去的状态不一样
/// （游标 / 粘滞 id），散出去就会出现「切了模式但有一半状态没跟着切」的洞。
pub fn select(
    mode: LbMode,
    records: &[KeyRecord],
    cursor: usize,
    current: Option<&str>,
    now: i64,
) -> Option<Selection> {
    match mode {
        LbMode::Balanced => {
            select_key(records, cursor, now).map(|(index, next_cursor)| Selection {
                index,
                next_cursor,
                current_id: records[index].id.clone(),
            })
        }
        LbMode::Priority => select_sticky(records, current, now).map(|index| Selection {
            index,
            // 🔴 粘滞模式**不推进游标**：推进了会让「从 priority 切回 balanced」
            //    的那一刻起点漂到一个与两种模式都无关的位置。原样带回。
            next_cursor: cursor,
            current_id: records[index].id.clone(),
        }),
    }
}

/// 一次成功之后（`keypool.ts:112-115`）。
///
/// `strikes` 清零、记 `last_used_at`、**清掉 `cooldown_reason`**
/// —— 最后那一格容易漏：不清的话面板上会一直挂着一条早就过去了的冷却理由。
pub fn apply_success(r: &KeyRecord, now: i64) -> KeyRecord {
    KeyRecord {
        strikes: 0,
        last_used_at: Some(now),
        cooldown_reason: None,
        ..r.clone()
    }
}

/// 进冷却（`keypool.ts:117-119`）。
pub fn apply_cooldown(r: &KeyRecord, now: i64, ms: i64, reason: &str) -> KeyRecord {
    KeyRecord {
        cooldown_until: now.saturating_add(ms),
        cooldown_reason: Some(reason.to_string()),
        ..r.clone()
    }
}

/// 累计一次瞬时故障（上游 5xx / 超时 / 网络错误，`keypool.ts:133-143`）。
///
/// 🔴 **攒满 `max_strikes` 时是「进长冷却」，不是「永久剔除」。**
/// 原实现在这里直接置 `evicted`，后果是上游一次抖动就能永久摧毁整个 key 池：
/// 三把 key 的池子在上游持续 503 时**只需五个请求即全部报废**，
/// 而当时没有任何 un-evict 路径，**上游恢复后网关也永远起不来**。
/// 永久剔除只保留给凭据失效（401/403，见 [`apply_evict`]）。
///
/// ⚠️ `strikes` 在**进入**冷却时即清零，不是等冷却到期再清：冷却期内这把 key 根本
/// 不会被 [`select_key`] 选中，两种写法对外行为完全一致，
/// 但前者不需要额外的惰性归一化步骤。
pub fn apply_strike(r: &KeyRecord, now: i64, policy: &PoolPolicy, reason: &str) -> KeyRecord {
    let strikes = r.strikes.saturating_add(1);
    if strikes >= policy.max_strikes {
        KeyRecord {
            strikes: 0,
            cooldown_until: now.saturating_add(policy.cooldown_strike_ms),
            cooldown_reason: Some(reason.to_string()),
            ..r.clone()
        }
    } else {
        KeyRecord {
            strikes,
            ..r.clone()
        }
    }
}

/// 永久剔除（`keypool.ts:145-148`）。**只用于 401/403** —— 凭据失效是确定性的，重试无意义。
pub fn apply_evict(r: &KeyRecord, reason: &str) -> KeyRecord {
    KeyRecord {
        evicted: true,
        evicted_reason: Some(reason.to_string()),
        ..r.clone()
    }
}

/// 四格计数（`keypool.ts:150-170`）。
///
/// ⚠️ **判定顺序即优先级：`disabled > evicted > cooling > fresh`，
/// 且必须与 [`crate::core::keys::view::key_bucket`] 逐档同序** ——
/// 两处不同序就会出现「概览卡说 3 把冷却中、Key 池列表一把冷却中的都没有」
/// 这种自相矛盾。
///
/// ⚠️ **被停用的 key 既不算 `fresh` 也不算 `evicted`**：
/// 算进 `fresh` 会让 503 退回 `upstream_error`（池子看起来还有得用）；
/// 算进 `evicted` 会让 503 告诉运维「去换 key」，而其实是他自己在面板上关的。
///
/// 四格互斥且穷尽 ⇒ `total == fresh + cooling + evicted + disabled` **恒成立**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct PoolHealth {
    pub total: usize,
    pub fresh: usize,
    pub cooling: usize,
    pub evicted: usize,
    pub disabled: usize,
}

/// 见 [`PoolHealth`]。
pub fn pool_health(records: &[KeyRecord], now: i64) -> PoolHealth {
    let mut h = PoolHealth {
        total: records.len(),
        fresh: 0,
        cooling: 0,
        evicted: 0,
        disabled: 0,
    };
    for r in records {
        if is_disabled(r) {
            h.disabled += 1;
        } else if r.evicted {
            h.evicted += 1;
        } else if r.cooldown_until > now {
            h.cooling += 1;
        } else {
            h.fresh += 1;
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    fn policy() -> PoolPolicy {
        PoolPolicy {
            max_strikes: 3,
            cooldown_rate_limit_ms: 60_000,
            cooldown_payment_ms: 3_600_000,
            cooldown_strike_ms: 1_800_000,
        }
    }

    fn rec(id: &str) -> KeyRecord {
        KeyRecord::fresh(id, format!("plain-{id}"), NOW - 1_000)
    }

    // ── 三件套判据 ─────────────────────────────────────────────────────────

    /// 缺席的 `disabled` = 没停用。**不许当成「读不出来所以停用」** ——
    /// 存量记录全都没有这一格，那会让升级之后整个池子当场消失。
    #[test]
    fn absent_disabled_means_not_disabled() {
        assert!(!is_disabled(&rec("a")));
    }

    #[test]
    fn explicit_disabled_flag_is_honoured_in_both_directions() {
        let mut r = rec("a");
        r.disabled = Some(true);
        assert!(is_disabled(&r));
        r.disabled = Some(false);
        assert!(!is_disabled(&r));
    }

    /// `is_available` 的三条否决各自独立成立。
    #[test]
    fn availability_is_vetoed_by_each_of_the_three_conditions() {
        let mut ok = rec("a");
        ok.cooldown_until = NOW; // 边界：cooldown_until == now 算可用
        assert!(is_available(&ok, NOW), "冷却到期的那一刻就该可用");

        let mut evicted = rec("b");
        evicted.evicted = true;
        assert!(!is_available(&evicted, NOW));

        let mut disabled = rec("c");
        disabled.disabled = Some(true);
        assert!(!is_available(&disabled, NOW));

        let mut cooling = rec("d");
        cooling.cooldown_until = NOW + 1;
        assert!(!is_available(&cooling, NOW), "差 1 毫秒也还在冷却里");
    }

    /// 🔴 **`counts_toward_target` 与 `is_available` 在被停用的 key 上必须分歧。**
    ///
    /// 回归对象（两次实测复现）：合并之后「在面板上停用一把 key」
    /// ＝「自动注册一个新 Agnes 账号」。
    #[test]
    fn disabled_key_still_counts_toward_target_but_is_not_available() {
        let mut r = rec("a");
        r.disabled = Some(true);
        assert!(!is_available(&r, NOW));
        assert!(
            counts_toward_target(&r),
            "停用的 key 仍占名额，否则停用 = 触发一次真实建号"
        );
    }

    /// 🔴 **正在冷却的 key 也占名额。**
    ///
    /// 回归对象：只放过 `disabled` 的那一版会让「停用一把正在冷却的 key」照样补池，
    /// 而且冷却导致池子永久膨胀、线性、不封顶。
    #[test]
    fn cooling_key_still_counts_toward_target() {
        let mut r = rec("a");
        r.cooldown_until = NOW + 10_000;
        assert!(!is_available(&r, NOW));
        assert!(counts_toward_target(&r), "冷却是会自己回来的状态，不是缺口");
    }

    /// 只有 `evicted` 让出名额 —— 它是唯一一个「这把死了，去换一把」的状态。
    #[test]
    fn only_evicted_frees_a_slot() {
        let mut r = rec("a");
        r.evicted = true;
        assert!(!counts_toward_target(&r));
    }

    // ── 游标轮询（LbMode::Balanced，逐字移植） ─────────────────────────────

    #[test]
    fn empty_pool_selects_nothing() {
        assert_eq!(select_key(&[], 0, NOW), None);
        assert_eq!(select_sticky(&[], None, NOW), None);
    }

    /// 轮询从 `cursor % len` 起，**逐个往后绕一圈**。
    #[test]
    fn round_robin_starts_at_cursor_and_wraps_once() {
        let rs = vec![rec("a"), rec("b"), rec("c")];
        assert_eq!(select_key(&rs, 0, NOW), Some((0, 1)));
        assert_eq!(select_key(&rs, 1, NOW), Some((1, 2)));
        assert_eq!(select_key(&rs, 2, NOW), Some((2, 3)));
        // 游标不取模地一直增，进来时才取模 —— 池子大小会变。
        assert_eq!(select_key(&rs, 3, NOW), Some((0, 1)));
        assert_eq!(select_key(&rs, 7, NOW), Some((1, 2)));
    }

    /// 不可用的被跳过，**而游标落在真正选中的那一条上**（不是落在起点上）。
    #[test]
    fn round_robin_skips_unavailable_and_cursor_follows_the_winner() {
        let mut rs = vec![rec("a"), rec("b"), rec("c")];
        rs[0].evicted = true;
        rs[1].disabled = Some(true);
        assert_eq!(select_key(&rs, 0, NOW), Some((2, 3)));
    }

    /// 一圈都不可用 ⇒ `None`（调用方据此回 503）。**不许退化成「随便挑一把」。**
    #[test]
    fn round_robin_returns_none_when_nothing_is_available() {
        let mut rs = vec![rec("a"), rec("b")];
        rs[0].evicted = true;
        rs[1].cooldown_until = NOW + 1;
        assert_eq!(select_key(&rs, 0, NOW), None);
    }

    /// 连续选三次覆盖三把 key —— 轮询的**那个**性质，不是「选中了一把」而已。
    #[test]
    fn round_robin_actually_rotates_over_the_whole_pool() {
        let rs = vec![rec("a"), rec("b"), rec("c")];
        let mut cursor = 0usize;
        let mut seen = Vec::new();
        for _ in 0..3 {
            let (idx, next) = select_key(&rs, cursor, NOW).unwrap();
            seen.push(rs[idx].id.clone());
            cursor = next;
        }
        assert_eq!(seen, vec!["a", "b", "c"]);
    }

    /// 🔴 **游标不累积**：`next_cursor` 恒落在 `1..=records.len()` 里，
    /// 无论传进来的 `cursor` 有多大。
    ///
    /// # 这一格替换掉了什么，以及为什么（裁定，2026-09-11）
    ///
    /// 原来这里是一格叫 `cursor_overflow_wraps_instead_of_panicking` 的**假判据**。
    /// 它拿 `select_key(&[一把 key], usize::MAX, NOW)` 去「测溢出」，而那条路径上
    /// `start = usize::MAX % 1 = 0`、`idx = 0` ⇒ `next = 0 + 1 = 1` ——
    /// **根本不经过溢出**。收尾那句 `assert_eq!(usize::MAX.wrapping_add(1), 0)`
    /// 断言的是标准库，不是被测代码。
    /// 实测：把实现里的 `idx.wrapping_add(1)` 换成 `idx + 1`，那一格照样绿
    /// （而且全仓 573 格判据**没有一格**变红）—— 它一个字节的实现都没钉住。
    ///
    /// # 裁定：那道护栏够不着，于是删护栏、判据改钉「够不着」本身
    ///
    /// `idx < records.len()`，而切片长度恒 `<= isize::MAX`，所以 `idx + 1` 够不到
    /// `usize` 的上界；且唯一的调用方做的是 `cursor.store(next_cursor)` 而非
    /// `fetch_add`，存回去的值也恒在 `1..=len` 里。**没有任何输入能让它溢出**
    /// （详见 [`select_key`] 的文档）。
    ///
    /// 所以这里**不再声称「溢出被守住了」**（本仓的规矩：宁可如实写「今天没人守」，
    /// 也不许留假探针），改钉那条让溢出够不着的不变量 —— 而它是钉得住东西的：
    /// 把实现写成 `cursor.wrapping_add(1)`（「推进传进来的那个游标」这个很自然的
    /// 误写，也正是唯一能让游标真的一路累积到溢出的写法）当场红。
    #[test]
    fn the_cursor_never_accumulates_so_overflow_is_unreachable() {
        let rs = vec![rec("a"), rec("b"), rec("c")];
        for cursor in [0, 1, 2, 3, 7, usize::MAX - 1, usize::MAX] {
            let (idx, next) = select_key(&rs, cursor, NOW).unwrap();
            assert!(idx < rs.len(), "cursor={cursor}: 下标 {idx} 越界");
            assert!(
                (1..=rs.len()).contains(&next),
                "cursor={cursor}: next_cursor={next} 跑出了 1..={}，游标在累积 —— \
                 那才是溢出唯一可能的来源",
                rs.len()
            );
        }
    }

    // ── 粘滞（LbMode::Priority，新行为，独立的一组判据） ───────────────────

    /// 🔴 **粘滞的那个性质**：当前这把还能用就一直选它，选十次都是它。
    /// （这一格是 `Priority` 与 `Balanced` 行为上的分水岭。）
    #[test]
    fn priority_sticks_to_the_current_key_while_it_is_usable() {
        let rs = vec![rec("a"), rec("b"), rec("c")];
        let mut current = Some("b".to_string());
        for _ in 0..10 {
            let i = select_sticky(&rs, current.as_deref(), NOW).unwrap();
            assert_eq!(rs[i].id, "b");
            current = Some(rs[i].id.clone());
        }
    }

    /// 没有粘滞对象（冷启动）时取优先级最小者。
    #[test]
    fn priority_picks_the_lowest_priority_number_when_there_is_nobody_to_stick_to() {
        let mut rs = vec![rec("a"), rec("b"), rec("c")];
        rs[0].priority = 10;
        rs[1].priority = -5;
        rs[2].priority = 0;
        let i = select_sticky(&rs, None, NOW).unwrap();
        assert_eq!(rs[i].id, "b", "越小越先用");
    }

    /// 🔴 **换人时取优先级最小者，不是「当前这把的下一个」、也不是「池内第一个」。**
    ///
    /// 回归对象：取下一个会让每次换人都往后漂，长期退化成轮询 —— 粘滞就白做了。
    ///
    /// ⚠️ 夹具刻意把**优先级最小的那把放在最后一个下标**：
    /// 这样「取下一个」（会选 `c`）与「取池内第一个」（会选 `b`，它就是当前那把）
    /// 两种退化写法都会当场红。把它摆在下标 0 的夹具**测不出这件事**
    /// （已实测：那种夹具下「按下标取最小」照样绿）。
    #[test]
    fn priority_switches_to_the_lowest_priority_not_to_the_next_one() {
        let mut rs = vec![rec("b"), rec("c"), rec("a")];
        rs[0].priority = 9; // 下标 0：当前这把，马上要坏
        rs[1].priority = 5; // 下标 1：「下一个」就是它 —— 不许选它
        rs[2].priority = 1; // 下标 2：优先级最小的那把
        rs[0].evicted = true;
        let i = select_sticky(&rs, Some("b"), NOW).unwrap();
        assert_eq!(rs[i].id, "a", "换人取优先级最小者，不是池内的下一个");
    }

    /// 同优先级按**池内顺序**破平 —— 否则同一批导入的 key 每次换人都随机跳。
    #[test]
    fn priority_breaks_ties_by_pool_order() {
        let rs = vec![rec("a"), rec("b"), rec("c")];
        let i = select_sticky(&rs, None, NOW).unwrap();
        assert_eq!(rs[i].id, "a");
    }

    /// 粘着的那把**被删掉了**（id 已经不在池子里）⇒ 照常换人，不许回 `None`。
    #[test]
    fn priority_recovers_when_the_sticky_key_is_gone_from_the_pool() {
        let rs = vec![rec("a"), rec("b")];
        let i = select_sticky(&rs, Some("deleted-id"), NOW).unwrap();
        assert_eq!(rs[i].id, "a");
    }

    /// 粘着的那把进了冷却 ⇒ 换人；冷却到期之后**不会自动粘回去**
    /// （它只是重新变成候选之一）。这一条写下来免得后人以为粘滞是「绑定」。
    #[test]
    fn priority_leaves_a_cooling_key_and_does_not_automatically_come_back() {
        let mut rs = vec![rec("a"), rec("b")];
        rs[1].cooldown_until = NOW + 10_000;
        rs[1].priority = -1; // 即使它优先级更高
        let i = select_sticky(&rs, Some("b"), NOW).unwrap();
        assert_eq!(rs[i].id, "a", "冷却中的粘滞对象要让位");
    }

    /// 全池不可用 ⇒ `None`（与轮询同）。
    #[test]
    fn priority_returns_none_when_nothing_is_available() {
        let mut rs = vec![rec("a"), rec("b")];
        rs[0].disabled = Some(true);
        rs[1].evicted = true;
        assert_eq!(select_sticky(&rs, Some("a"), NOW), None);
    }

    // ── 统一入口：两种模式要存回去的状态不一样 ───────────────────────────

    /// 🔴 `Balanced` 推进游标；`Priority` **不推进**（原样带回）。
    ///
    /// 回归对象：粘滞模式顺手推进游标的话，从 priority 切回 balanced 的那一刻
    /// 起点会漂到一个与两种模式都无关的位置。
    #[test]
    fn only_balanced_advances_the_cursor() {
        let rs = vec![rec("a"), rec("b"), rec("c")];
        let b = select(LbMode::Balanced, &rs, 1, None, NOW).unwrap();
        assert_eq!((b.index, b.next_cursor, b.current_id.as_str()), (1, 2, "b"));

        let p = select(LbMode::Priority, &rs, 1, Some("c"), NOW).unwrap();
        assert_eq!(p.index, 2);
        assert_eq!(p.next_cursor, 1, "粘滞模式原样带回游标");
        assert_eq!(p.current_id, "c");
    }

    /// **两种模式都要回 `current_id`**：切到 priority 的那一刻才有合理的粘滞起点。
    #[test]
    fn both_modes_report_who_was_chosen_so_stickiness_has_a_starting_point() {
        let rs = vec![rec("a"), rec("b")];
        let b = select(LbMode::Balanced, &rs, 1, None, NOW).unwrap();
        assert_eq!(b.current_id, "b");
    }

    /// 模式名的解析：大小写不敏感，认不出来回 `None`（降级方向由调用方裁）。
    #[test]
    fn mode_parsing_is_case_insensitive_and_honest_about_unknown_values() {
        assert_eq!(LbMode::parse("priority"), Some(LbMode::Priority));
        assert_eq!(LbMode::parse("Priority"), Some(LbMode::Priority));
        assert_eq!(LbMode::parse("BALANCED"), Some(LbMode::Balanced));
        assert_eq!(LbMode::parse("round-robin"), None);
        assert_eq!(LbMode::parse(""), None);
    }

    /// 🔴 **默认是 priority**（规格 §3.10，登记在案的一次行为变更）。
    #[test]
    fn default_mode_is_priority() {
        assert_eq!(LbMode::default(), LbMode::Priority);
        assert_eq!(LbMode::default().as_str(), "priority");
    }

    // ── 状态迁移 ───────────────────────────────────────────────────────────

    /// `apply_*` **一律返回新记录**，入参一个字节都不许被改。
    /// 回归对象：就地改会让 `pool/repo.rs` 的写消除比较恒成立 ⇒ 每一次调度写都被丢掉，
    /// **而测试全绿**。
    #[test]
    fn transitions_never_mutate_the_input() {
        let r = rec("a");
        let before = r.clone();
        let _ = apply_success(&r, NOW);
        let _ = apply_cooldown(&r, NOW, 1_000, "why");
        let _ = apply_strike(&r, NOW, &policy(), "why");
        let _ = apply_evict(&r, "why");
        assert_eq!(r, before);
    }

    /// 成功：清 strikes、记时间、**清掉冷却理由**。
    #[test]
    fn success_clears_strikes_and_the_stale_cooldown_reason() {
        let mut r = rec("a");
        r.strikes = 2;
        r.cooldown_reason = Some("rate limited".into());
        let next = apply_success(&r, NOW);
        assert_eq!(next.strikes, 0);
        assert_eq!(next.last_used_at, Some(NOW));
        assert_eq!(next.cooldown_reason, None, "过去了的冷却理由不许挂在面板上");
        assert_eq!(next.cooldown_until, r.cooldown_until, "成功不解除冷却");
    }

    #[test]
    fn cooldown_sets_the_deadline_and_the_reason() {
        let r = rec("a");
        let next = apply_cooldown(&r, NOW, 60_000, "rate limited");
        assert_eq!(next.cooldown_until, NOW + 60_000);
        assert_eq!(next.cooldown_reason.as_deref(), Some("rate limited"));
        assert!(!next.evicted, "冷却绝不顺手置 evicted");
    }

    /// 🔴 **攒满 strike 是长冷却，不是永久剔除。**
    ///
    /// 回归对象：置 evicted 的那一版让三把 key 的池子在持续 503 下五个请求全报废，
    /// 且没有 un-evict 路径 —— 上游恢复后网关永远起不来。
    #[test]
    fn strikes_escalate_into_a_long_cooldown_never_into_eviction() {
        let mut r = rec("a");
        for expected in 1..3 {
            r = apply_strike(&r, NOW, &policy(), "upstream 503");
            assert_eq!(r.strikes, expected);
            assert_eq!(r.cooldown_until, 0, "还没攒满不许进冷却");
            assert!(!r.evicted);
        }
        r = apply_strike(&r, NOW, &policy(), "upstream 503");
        assert!(!r.evicted, "🔴 攒满也绝不 evict");
        assert_eq!(r.cooldown_until, NOW + 1_800_000);
        assert_eq!(r.cooldown_reason.as_deref(), Some("upstream 503"));
        assert_eq!(r.strikes, 0, "进冷却时即清零，不留给惰性归一化");
    }

    /// `max_strikes = 1` 时第一次 strike 就进冷却（`>=` 的边界）。
    #[test]
    fn a_single_strike_is_enough_when_max_strikes_is_one() {
        let p = PoolPolicy {
            max_strikes: 1,
            ..policy()
        };
        let next = apply_strike(&rec("a"), NOW, &p, "timeout");
        assert_eq!(next.cooldown_until, NOW + p.cooldown_strike_ms);
    }

    #[test]
    fn evict_is_permanent_and_records_why() {
        let next = apply_evict(&rec("a"), "upstream 401");
        assert!(next.evicted);
        assert_eq!(next.evicted_reason.as_deref(), Some("upstream 401"));
        assert!(!is_available(&next, NOW));
        assert!(!counts_toward_target(&next));
    }

    // ── 四格计数 ───────────────────────────────────────────────────────────

    /// 🔴 **顺序即优先级**：一把「既停用、又剔除、又在冷却」的 key 只能算 `disabled`。
    #[test]
    fn health_buckets_follow_the_priority_order() {
        let mut r = rec("a");
        r.disabled = Some(true);
        r.evicted = true;
        r.cooldown_until = NOW + 1;
        let h = pool_health(std::slice::from_ref(&r), NOW);
        assert_eq!(h.disabled, 1);
        assert_eq!(h.evicted, 0);
        assert_eq!(h.cooling, 0);
        assert_eq!(h.fresh, 0);
    }

    /// 四格互斥且穷尽：`total == fresh + cooling + evicted + disabled` **恒成立**。
    #[test]
    fn health_buckets_are_mutually_exclusive_and_exhaustive() {
        let mut rs = vec![rec("a"), rec("b"), rec("c"), rec("d"), rec("e")];
        rs[1].disabled = Some(true);
        rs[2].evicted = true;
        rs[3].cooldown_until = NOW + 5;
        let h = pool_health(&rs, NOW);
        assert_eq!(h.total, 5);
        assert_eq!(h.fresh, 2);
        assert_eq!(h.cooling, 1);
        assert_eq!(h.evicted, 1);
        assert_eq!(h.disabled, 1);
        assert_eq!(h.total, h.fresh + h.cooling + h.evicted + h.disabled);
    }

    /// 🔴 **`fresh` 那一格当且仅当 `is_available`。**
    ///
    /// 这条等价关系是 503 四条 reason 的依据（`fresh > 0` 却选不出 key，
    /// 或者反过来，都会让 503 把运维指向错误的方向）。
    #[test]
    fn fresh_count_agrees_with_is_available_on_every_shape() {
        let mut shapes = Vec::new();
        for disabled in [None, Some(false), Some(true)] {
            for evicted in [false, true] {
                for cd in [0, NOW, NOW + 1] {
                    let mut r = rec("x");
                    r.disabled = disabled;
                    r.evicted = evicted;
                    r.cooldown_until = cd;
                    shapes.push(r);
                }
            }
        }
        let expected = shapes.iter().filter(|r| is_available(r, NOW)).count();
        assert_eq!(pool_health(&shapes, NOW).fresh, expected);
        assert!(expected > 0 && expected < shapes.len(), "夹具必须两侧都有");
    }

    /// 空池的四格全零，`total` 也是零 —— 别让「没有池子」看起来像「池子健康」。
    #[test]
    fn empty_pool_reports_zeros() {
        let h = pool_health(&[], NOW);
        assert_eq!(h.total, 0);
        assert_eq!(h.fresh + h.cooling + h.evicted + h.disabled, 0);
    }
}
