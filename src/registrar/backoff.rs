//! 跨轮退避：撞上上游限流之后，**跨轮**地、**指数**地闭嘴一段时间。
//!
//! 移植自 TS 版 `src/core/registrar/backoff.ts`（248 行），规则真源
//! `docs/porting/01-core-rules.md §10.4`。**零 IO**：时钟由调用方传进来。
//!
//! # 🔴 为什么非有它不可
//!
//! 上游那两层限流的惩罚窗口都比「一轮补池」长得多，
//! **而窗口里每打一次请求就把窗口续一次**。
//! 从前撞上限流之后只在一次尝试内换个域名接着打，于是一轮里的每一次请求都还在窗口里、
//! 每一次都把窗口续上 —— **打得越多，恢复得越晚**。
//!
//! 光靠「本轮立刻中止」还不够：定时轮是雷打不动地每 `TEND_INTERVAL_MS` 来一轮，
//! 窗口比补池间隔长的时候，**每一轮都会去续一次窗口，正是同一个缺陷换了个尺度重演**。
//! **⇒ 退避必须跨轮，而且必须指数。**

use serde::Serialize;

/// 边缘网关那一层的退避基数。
///
/// 🟢 **实测 + 余量**：那一层的惩罚窗口约 14 分钟才退净，且窗口内每打一次就续一次。
/// 向上取整再加 1 分钟。
/// ⚠️ **这个数是观测，不是承诺。**
pub const EDGE_BACKOFF_MS: i64 = 900_000; // 15 分钟

/// 应用层限流的退避基数。
///
/// 🔴 **没有任何实测支撑，是猜的，不许在别处被写成实测结论。**
/// 我们只知道「≥60 秒间隔下第 7 次会触发」，**窗口有多长我们没量过**。
/// 取一个补池周期，依据只有一条：**我们不知道它多长，所以至少跳过下一轮。**
///
/// ⚠️ **第一次撞时它很可能一轮都跳不过去**：它恰好等于 `TEND_INTERVAL_MS` 的内置取值，
/// 而窗口判据是 [`in_backoff`] 里那一处**严格**比较 ⇒ 下一轮准点到达时窗口刚好算过去了。
/// **压住请求量靠的是指数那一段（30min → 1h → 2h → 4h 封顶），不是第一格。**
pub const APP_BACKOFF_MS: i64 = 1_800_000; // 30 分钟

/// 「一轮的形状不对」那一档的退避基数。🔴 同样**没有实测支撑**。
pub const CLUSTER_BACKOFF_MS: i64 = 1_800_000; // 30 分钟

/// 退避窗口的封顶。
///
/// 🟡 **取舍**：超过这个长度基本可以确定是**出口 IP 被长期限了**，
/// 那该让运维在面板上看见并去换出口，**而不是让程序无限等下去**。
pub const BACKOFF_MAX_MS: i64 = 14_400_000; // 4 小时（≈8 轮）

/// 撞的是哪一档。
///
/// # 🔴🔴 三档的**证据类型不同**，别把它们读成「三层限流」
///
/// - [`Edge`](BackoffKind::Edge) / [`App`](BackoffKind::App) ——
///   上游**回话里的字眼**落进了词表。这两档说的是「撞的是哪一层限流」。
/// - [`Cluster`](BackoffKind::Cluster) —— **我们的词表一个字都没命中**，
///   证据是**这一轮的形状**（一轮里连着好几个域名全挂）。
///   **它存在的唯一理由是「分类器读错了的那一档」**：上游改了限流文案
///   ⇒ 真限流被逐条读成域名屏蔽 ⇒ 前两档一个都不会产生。
///
/// # 🔴🔴 别把 `Cluster` 读成「上游没说过限流」
///
/// **那句话在它自己要治的场景里就是假的**：上游改了限流文案的时候，
/// 上游**明明说了一句限流的话**（TS 判据夹具里逐字是 `Slow down, mate.`），
/// 只是**我们的词表没认出来**。这两句话不是一回事，**而混同它们的代价是实的**：
/// 面板横幅曾经据此逐字写着「上游一句限流的话都没说」，
/// **等于在最该提示「可能是上游改了限流文案」的那一刻反过来否掉这个方向**。
/// ⇒ 这一档成立时**两种可能都还开着**，面板文案与文档**必须把两个方向都摆出来**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackoffKind {
    Edge,
    App,
    Cluster,
}

impl BackoffKind {
    /// 这一档的退避基数。
    ///
    /// ⚠️ **写成穷尽的 `match` 而不是三元链**：再加一档时这里会在**编译期**报错，
    /// 而三元链只会安静地把新档落进 `else` 那一支。
    fn base_ms(self) -> i64 {
        match self {
            BackoffKind::Edge => EDGE_BACKOFF_MS,
            BackoffKind::App => APP_BACKOFF_MS,
            BackoffKind::Cluster => CLUSTER_BACKOFF_MS,
        }
    }
}

/// 退避键的值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BackoffState {
    /// 窗口到什么时候。
    pub until: i64,
    pub kind: BackoffKind,
    /// **这一串**是什么时候起的头。
    ///
    /// ⚠️ 它同时是「这是哪一串」的**身份**：只在**重新起一串**时推到当前时刻，
    /// [`merge_backoff`] 靠比它大小分辨串。
    pub since: i64,
    /// 「这一串」里撞了几次。
    ///
    /// ⚠️ **「连续」指的是「连续几轮零产出」，不是「连续几次请求」**：
    /// 一轮里既铸出了 key 又撞了限流时**重新起一串**（`hits` 回到 1），
    /// 因为那一轮上游明明还在给我们发号 —— **它不是「越限越死」的那种形态**。
    pub hits: u32,
}

/// 还在退避窗口里吗。
///
/// ⚠️ 这里是**严格**比较：`until == now` 时算窗口已过。
/// `APP_BACKOFF_MS` 那条注释里「第一次撞时很可能一轮都跳不过去」说的就是它。
pub fn in_backoff(s: Option<&BackoffState>, now: i64) -> bool {
    s.is_some_and(|s| s.until > now)
}

/// 还要等多久。**不在窗口里时是 `None`**（口径与 `manual_tend_quota` 逐字一致）。
pub fn retry_after_ms(s: Option<&BackoffState>, now: i64) -> Option<i64> {
    s.filter(|s| s.until > now).map(|s| s.until - now)
}

/// 上一串断了没有。
///
/// ⚠️ **刻意复用 [`BACKOFF_MAX_MS`] 当阈值，不新增第四个常量**：
/// 合法的一串里，两次撞之间最多隔一个「上一段窗口 + 一个补池间隔」，而窗口本身封顶就是它。
///
/// 🔴 **它治的是一条真实的驻留路径**：池子填满之后 `tend_once` 在 `need <= 0` 那里就 return 了，
/// **根本走不到收尾** ⇒ 一把 `hits` 很高的退避键会一直留在存储里；
/// 等池子再耗干时**第一次撞限流就直接跳到封顶那一档**。
pub fn streak_broken(prev: Option<&BackoffState>, now: i64) -> bool {
    match prev {
        None => true,
        Some(p) => now - p.until > BACKOFF_MAX_MS,
    }
}

/// 算出这一次该退避到什么时候。
pub fn next_backoff(prev: Option<&BackoffState>, kind: BackoffKind, now: i64) -> BackoffState {
    let streak = if streak_broken(prev, now) { None } else { prev };
    let hits = streak.map_or(0, |s| s.hits).saturating_add(1);
    // ⚠️ **先夹后乘**：`hits` 很大时 `2^(hits-1)` 会溢出。
    let factor = 2i64.saturating_pow(hits.saturating_sub(1).min(30));
    let span = kind.base_ms().saturating_mul(factor).min(BACKOFF_MAX_MS);
    BackoffState {
        until: now.saturating_add(span),
        kind,
        since: streak.map_or(now, |s| s.since),
        hits,
    }
}

/// 两份退避键的收敛规则（并发写时用）。
///
/// # 🔴 丢更新在这把键上最疼
///
/// 丢掉 `until` 等于**退避窗口凭空消失** ⇒ 继续打 ⇒ 每打一次续一次窗口
/// ⇒ **正好回到本模块要修的那个缺陷**。
/// 所以**窗口本身一律取更保守的那个**：`until` 取大的，`kind` 跟着胜出的 `until` 走。
///
/// # ⚠️⚠️ `hits` **不能**照着 `until` 那条「取大的」办 —— 这是一条实测出来的坑
///
/// [`next_backoff`] 重新起一串时写回来的 `hits` 是 1，而存储里那份旧的可能是 5 ——
/// **无脑取大就把「重新起一串」在落盘这一层原地撤销掉，
/// 而 `tend_once` 那一侧的判据（注入的假 save）照样全绿：
/// 一份行为在两个地方各说各话。**
///
/// ⇒ **判据是 [`BackoffState::since`]**：它只在重新起一串时才前进，
/// 所以 `since` 更大的那一份说的就是**更新的那一串**，`hits` / `since` **整对**跟着它走；
/// 两份 `since` 相同才是同一串，那时才取 `hits` 更大的。
pub fn merge_backoff(cur: &BackoffState, next: &BackoffState) -> BackoffState {
    // 窗口：取更保守的一个，`kind` 跟着它。
    let (until, kind) = if next.until >= cur.until {
        (next.until, next.kind)
    } else {
        (cur.until, cur.kind)
    };
    // 串：`since` 更大的那一份是更新的一串，`hits` 与 `since` **整对**跟着走。
    let (since, hits) = match next.since.cmp(&cur.since) {
        std::cmp::Ordering::Greater => (next.since, next.hits),
        std::cmp::Ordering::Less => (cur.since, cur.hits),
        std::cmp::Ordering::Equal => (cur.since, cur.hits.max(next.hits)),
    };
    BackoffState {
        until,
        kind,
        since,
        hits,
    }
}

/// 把存储里读出来的那一份窄化成 [`BackoffState`]。
///
/// # 🔴 失败方向与域名台账**相反，这是刻意的**
///
/// 台账读坏了当成「什么都没记住」是安全的（多探几次）；
/// **而退避读坏了当成「还在退避中」会让注册机静默停摆。**
/// 这里 fail-safe 的方向是**放行**，代价明写：**一把被写坏的退避键 = 多打一轮。**
pub fn narrow_backoff(v: Option<&serde_json::Value>) -> Option<BackoffState> {
    let o = v?.as_object()?;
    let until = o.get("until")?.as_i64()?;
    let since = o.get("since")?.as_i64()?;
    let hits = u32::try_from(o.get("hits")?.as_u64()?).ok()?;
    let kind = match o.get("kind")?.as_str()? {
        "edge" => BackoffKind::Edge,
        "app" => BackoffKind::App,
        "cluster" => BackoffKind::Cluster,
        // 认不出的档位 ⇒ 整条不认 ⇒ 放行。**不许挑一个档位当兜底** ——
        // 那等于凭空替上游做了一次分类。
        _ => return None,
    };
    Some(BackoffState {
        until,
        kind,
        since,
        hits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn st(until: i64, kind: BackoffKind, since: i64, hits: u32) -> BackoffState {
        BackoffState {
            until,
            kind,
            since,
            hits,
        }
    }

    // ── 窗口判定 ──────────────────────────────────────────────────────

    /// ⚠️ **严格**比较：`until == now` 时窗口已过。
    /// `APP_BACKOFF_MS` 那条「第一次撞很可能一轮都跳不过去」说的就是它。
    #[test]
    fn the_window_boundary_is_strict_so_a_punctual_next_round_slips_through() {
        let s = st(1000, BackoffKind::App, 0, 1);
        assert!(in_backoff(Some(&s), 999));
        assert!(!in_backoff(Some(&s), 1000), "边界上该算过去了");
        assert!(!in_backoff(None, 999));
    }

    #[test]
    fn retry_after_is_none_when_the_window_is_over() {
        let s = st(1000, BackoffKind::App, 0, 1);
        assert_eq!(retry_after_ms(Some(&s), 400), Some(600));
        assert_eq!(
            retry_after_ms(Some(&s), 1000),
            None,
            "窗口过了却还报一个等待时长"
        );
        assert_eq!(retry_after_ms(None, 0), None);
    }

    // ── 指数 ──────────────────────────────────────────────────────────

    /// 🔴 压住请求量靠的是**指数那一段**，不是第一格。
    #[test]
    fn the_window_doubles_each_hit_until_it_is_capped() {
        let mut cur: Option<BackoffState> = None;
        let mut now = 0i64;
        let mut spans = Vec::new();
        for _ in 0..8 {
            let next = next_backoff(cur.as_ref(), BackoffKind::App, now);
            spans.push(next.until - now);
            // 下一次撞在窗口刚过的那一刻（同一串里）
            now = next.until + 1;
            cur = Some(next);
        }
        assert_eq!(
            spans,
            vec![
                1_800_000,  // 30min
                3_600_000,  // 1h
                7_200_000,  // 2h
                14_400_000, // 4h —— 封顶
                14_400_000, 14_400_000, 14_400_000, 14_400_000,
            ],
        );
    }

    /// ⚠️ **先夹后乘**：不夹的话 `2^(hits-1)` 会溢出。
    ///
    /// 变异对象：把 `.min(30)` 挪到 `pow` 之后 —— `hits` 大时会 panic 或绕回。
    #[test]
    fn a_huge_hit_count_neither_panics_nor_wraps_around() {
        let prev = st(0, BackoffKind::Edge, 0, u32::MAX - 1);
        let next = next_backoff(Some(&prev), BackoffKind::Edge, 0);
        assert_eq!(next.until, BACKOFF_MAX_MS, "封顶没兜住：{}", next.until);
        assert!(next.until > 0, "溢出绕回成了负数/零");
    }

    /// 三档各自的基数走的是穷尽查表。
    #[test]
    fn each_kind_uses_its_own_base() {
        for (kind, base) in [
            (BackoffKind::Edge, EDGE_BACKOFF_MS),
            (BackoffKind::App, APP_BACKOFF_MS),
            (BackoffKind::Cluster, CLUSTER_BACKOFF_MS),
        ] {
            let s = next_backoff(None, kind, 0);
            assert_eq!(s.until, base, "{kind:?} 的基数取错了");
            assert_eq!(s.hits, 1);
        }
    }

    // ── 串 ────────────────────────────────────────────────────────────

    /// 🔴 这一格钉的是那条**真实的驻留路径**：
    /// 池子填满之后 `tend_once` 在 `need <= 0` 就 return，走不到收尾 ⇒
    /// 一把 `hits` 很高的退避键长期留在存储里；等池子再耗干时，
    /// 若不断串，**第一次撞限流就直接跳到封顶那一档**。
    #[test]
    fn a_long_idle_gap_starts_a_fresh_streak_instead_of_resuming_a_stale_one() {
        let stale = st(1000, BackoffKind::App, 0, 5);
        // 距上次窗口结束超过 BACKOFF_MAX_MS
        let now = 1000 + BACKOFF_MAX_MS + 1;
        assert!(streak_broken(Some(&stale), now));
        let next = next_backoff(Some(&stale), BackoffKind::App, now);
        assert_eq!(next.hits, 1, "接着旧串数下去了 —— 第一次撞就会跳到封顶");
        assert_eq!(next.since, now, "重新起一串时 since 要推到当前时刻");
    }

    /// 同一串里 `since` 不动 —— 它是这一串的身份。
    #[test]
    fn since_stays_put_within_one_streak() {
        let first = next_backoff(None, BackoffKind::App, 100);
        let second = next_backoff(Some(&first), BackoffKind::App, first.until + 1);
        assert_eq!(second.since, 100, "同一串里 since 动了，串的身份就没了");
        assert_eq!(second.hits, 2);
    }

    // ── 合并 ──────────────────────────────────────────────────────────

    /// 🔴 窗口取更保守的一个 —— 丢掉 `until` 等于退避凭空消失。
    #[test]
    fn merging_always_keeps_the_longer_window() {
        let a = st(5000, BackoffKind::Edge, 0, 1);
        let b = st(9000, BackoffKind::App, 0, 1);
        assert_eq!(merge_backoff(&a, &b).until, 9000);
        assert_eq!(merge_backoff(&b, &a).until, 9000);
    }

    /// `kind` 跟着胜出的 `until` 走，不是各取各的。
    #[test]
    fn the_kind_follows_the_window_that_won() {
        let a = st(5000, BackoffKind::Edge, 0, 1);
        let b = st(9000, BackoffKind::Cluster, 0, 1);
        assert_eq!(merge_backoff(&a, &b).kind, BackoffKind::Cluster);
    }

    /// 🔴🔴 **这一格钉的是那条实测出来的坑。**
    ///
    /// 变异对象：把 `hits` 也写成 `cur.hits.max(next.hits)` ——
    /// 「重新起一串」在落盘这一层被原地撤销，而 `tend_once` 那一侧的判据照样全绿，
    /// **一份行为在两个地方各说各话**。
    #[test]
    fn a_fresh_streak_is_not_undone_by_a_stale_higher_hit_count() {
        // 存储里那份：旧串，hits 已经攒到 5
        let stored = st(1_000, BackoffKind::App, 0, 5);
        // 刚算出来的：重新起的一串，hits = 1，since 更新
        let fresh = st(2_000, BackoffKind::App, 1_500, 1);
        let m = merge_backoff(&stored, &fresh);
        assert_eq!(
            m.hits, 1,
            "旧串的 hits 把新串顶掉了 —— 重新起一串被原地撤销"
        );
        assert_eq!(m.since, 1_500, "hits 与 since 必须整对跟着走");
        assert_eq!(m.until, 2_000, "窗口仍取更保守的那个");
    }

    /// 反方向也要成立：存储里那份才是更新的一串时，别被刚算的那份顶掉。
    #[test]
    fn the_newer_streak_wins_regardless_of_which_side_it_is_on() {
        let newer = st(1_000, BackoffKind::App, 9_000, 1);
        let older = st(2_000, BackoffKind::App, 0, 7);
        let m = merge_backoff(&older, &newer);
        assert_eq!((m.since, m.hits), (9_000, 1));
        assert_eq!(m.until, 2_000, "串新不新与窗口取哪个是两件事");
    }

    /// `since` 相同 = 同一串，这时才轮到「取大的」。
    #[test]
    fn within_the_same_streak_the_higher_hit_count_wins() {
        let a = st(1_000, BackoffKind::App, 500, 2);
        let b = st(1_000, BackoffKind::App, 500, 4);
        assert_eq!(merge_backoff(&a, &b).hits, 4);
    }

    // ── 窄化 ──────────────────────────────────────────────────────────

    #[test]
    fn a_well_formed_value_round_trips() {
        let v = json!({"until": 123, "kind": "edge", "since": 45, "hits": 3});
        assert_eq!(
            narrow_backoff(Some(&v)),
            Some(st(123, BackoffKind::Edge, 45, 3)),
        );
    }

    /// 🔴 **失败方向是放行**，与域名台账相反。
    ///
    /// 变异对象：把任一读不出来的分支改成「返回一个兜底的 `BackoffState`」——
    /// 一把被写坏的键会让注册机**静默停摆**，而面板上看不出任何原因。
    #[test]
    fn anything_unreadable_lets_the_registrar_through_rather_than_wedging_it() {
        for bad in [
            json!(null),
            json!("a string"),
            json!([1, 2, 3]),
            json!({}),
            json!({"until": "not a number", "kind": "edge", "since": 0, "hits": 1}),
            json!({"until": 1, "kind": "edge", "since": 0}), // 少一格
            json!({"until": 1, "kind": "brand-new-kind", "since": 0, "hits": 1}),
            json!({"until": 1, "kind": "edge", "since": 0, "hits": -1}), // 负数
        ] {
            let got = narrow_backoff(Some(&bad));
            assert!(got.is_none(), "读坏的键被当成了有效退避：{bad}");
            assert!(!in_backoff(got.as_ref(), 0), "读坏 ⇒ 必须放行");
        }
        assert!(narrow_backoff(None).is_none());
    }

    /// ⚠️ 认不出的档位 ⇒ **整条不认**，不许挑一个当兜底。
    ///
    /// 挑一个兜底等于凭空替上游做了一次分类，而那正是 `Cluster` 那一档
    /// 花了一整段注释在防的读法。
    #[test]
    fn an_unknown_kind_is_not_silently_coerced_into_one_of_the_three() {
        let v = json!({"until": 999_999, "kind": "someday-a-fourth", "since": 0, "hits": 9});
        assert_eq!(narrow_backoff(Some(&v)), None);
    }
}
