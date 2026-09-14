//! 「立即补池」那颗按钮的两道闸：**10 分钟冷却** + **每天 24 次**。
//!
//! # 它挡的不是「点得太快」，是**真实的上游代价**
//!
//! 一次手动补池最多消耗 `MINT_BATCH` 个临时邮箱，而两条通道的活跃邮箱名额
//! **是有限的**；每铸一把 key 还是一次真实的上游建号，同时撞注册风控。
//! ⇒ 连点的代价不是「多几次请求」，是**把补池能不能继续工作的前提烧掉**。
//!
//! # 🟡 `MANUAL_TENDS_PER_DAY = 24` 这个数**今天没有推导它的分母**，如实登记
//!
//! 当初的算式照着「1,000 次写/天」的存储配额算（720 + 416 = 1,136 > 1,000 ⇒ 直接打穿），
//! **那个分母随 Worker/KV 形态一起没了**。
//! **护栏本身留着**：「一次点击就是几遍全量重写」与它挡的连点是同一件事。
//! **要调这个数，得先定一个新的分母** —— 别凭感觉改。
//!
//! 一次手动补池的**非铸造**写侧成本：固定 3 次 put（护栏键 1 + 抢锁 1 + `tend:history` 1）
//! 加两笔条件写（域名台账、退避键各最多 1）⇒ **上界 5 次 put**，再加 1 次 delete（释放锁）；
//! 每铸出一把 key 再加 2 次 put（记录 + 索引），而铸造那部分**自限**。
//!
//! # ⚠️ 这把键**不传 TTL**，这是承重的
//!
//! 冷却 10 分钟，而日预算靠 `day` 字段判跨天。给它挂 TTL 意味着
//! 「冷却一过键就没了」⇒ 日预算跟着一起归零 ⇒ **24 次/天形同虚设**。

use serde_json::Value;

/// 存储键。**不传 TTL**，理由见模块头。
pub const MANUAL_GUARD_KEY: &str = "registrar_manual_guard";

/// 手动补池的冷却窗口。挡的是连点。
pub const MANUAL_TEND_COOLDOWN_MS: i64 = 600_000;

/// 每天允许的手动补池次数。🟡 **分母已不存在，见模块头。**
pub const MANUAL_TENDS_PER_DAY: u32 = 24;

const DAY_MS: i64 = 86_400_000;

/// ⚠️ 冷却必须**远短于**一天，否则「每天 24 次」根本用不完，
/// 两道闸里靠后那道就成了装饰。
const _COOLDOWN_LEAVES_ROOM_FOR_THE_DAILY_BUDGET: () = assert!(
    MANUAL_TEND_COOLDOWN_MS * (MANUAL_TENDS_PER_DAY as i64) <= DAY_MS,
    "冷却 × 日次数 超过了一天 —— 日预算永远撞不到，它就成了一条死规则"
);

/// 落盘的护栏状态。值形状 `{ day, used, cooldownUntil }`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualGuard {
    pub day: i64,
    pub used: u32,
    pub cooldown_until: i64,
}

impl ManualGuard {
    pub fn to_value(self) -> Value {
        serde_json::json!({
            "day": self.day,
            "used": self.used,
            "cooldownUntil": self.cooldown_until,
        })
    }
}

/// 今天是第几天（UTC 整日）。
pub fn day_index(now: i64) -> i64 {
    now.div_euclid(DAY_MS)
}

/// 今天什么时候结束 —— 429 里那个 `resetAt`。
pub fn day_ends_at(now: i64) -> i64 {
    (day_index(now) + 1).saturating_mul(DAY_MS)
}

/// 读护栏键。
///
/// 🔴 **任一字段读不得 ⇒ 整条 `None`（= 今天还没点过 ⇒ 放行一次）。**
/// 失败方向是刻意的：被外部写坏一把键的代价是**多放行一轮补池**
///（还有存储锁与冷却兜着）；而反过来（当成已耗尽）会
/// **把「立即补池」这颗按钮永久锁死，而运维在面板上看不出任何原因**。
pub fn narrow_manual_guard(raw: Option<&Value>) -> Option<ManualGuard> {
    let o = raw?.as_object()?;
    let day = o.get("day")?.as_i64()?;
    let used = u32::try_from(o.get("used")?.as_i64()?).ok()?;
    let cooldown_until = o.get("cooldownUntil")?.as_i64()?;
    Some(ManualGuard {
        day,
        used,
        cooldown_until,
    })
}

/// 今天已经用掉几次。
///
/// ⚠️ **提成具名函数是为了让两个消费者用同一处判据，不是为了好看。**
/// [`check_manual_tend`]（消费名额）与 [`manual_tend_quota`]（只看不动）
/// 曾经各写一遍，**而两份的分歧只在跨天那一瞬间可观测** ——
/// 面板上显示「今天还剩 24 次」、点下去却拿到「今天的额度用完了」。
/// **分歧点越窄，越不会有人想到去测它。**
pub fn used_today(cur: Option<&ManualGuard>, now: i64) -> u32 {
    cur.filter(|g| g.day == day_index(now))
        .map_or(0, |g| g.used)
}

/// 面板读的那份配额。**一次写都不产生、一格名额都不消费。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualTendQuota {
    /// ⚠️ 口径是「**现在**还剩几次」，与 [`ManualTendVerdict::Ok`] 的 `remaining`
    /// （「点完这一次之后还剩几次」）**差 1，而且是刻意的**。
    /// 两个数字都对，但读错就会在面板上少显示一次。
    pub remaining: u32,
    pub used: u32,
    pub per_day: u32,
    pub reset_at: i64,
    /// ⚠️ 与 `retry_after_ms` **成对给，且只在真的冷却中才非空**。
    /// 给一个「已经过去的时刻」会让面板渲染出一个**恒为 0 的倒计时**，
    /// 而那与「现在就能点」长得不一样却是同一件事。
    pub cooldown_until: Option<i64>,
    pub retry_after_ms: Option<i64>,
}

pub fn manual_tend_quota(cur: Option<&ManualGuard>, now: i64) -> ManualTendQuota {
    let used = used_today(cur, now);
    let cooling = cur.filter(|g| now < g.cooldown_until);
    ManualTendQuota {
        remaining: MANUAL_TENDS_PER_DAY.saturating_sub(used),
        used,
        per_day: MANUAL_TENDS_PER_DAY,
        reset_at: day_ends_at(now),
        cooldown_until: cooling.map(|g| g.cooldown_until),
        retry_after_ms: cooling.map(|g| g.cooldown_until - now),
    }
}

/// 判这一次点击放不放行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualTendVerdict {
    Ok {
        /// 放行就要把它落盘 —— **在真跑之前**，否则崩一次等于白送一格名额。
        next: ManualGuard,
        remaining: u32,
        reset_at: i64,
        cooldown_until: i64,
        retry_after_ms: i64,
    },
    Denied {
        reason: &'static str,
        remaining: u32,
        reset_at: i64,
        retry_after_ms: i64,
        /// 只有 `manual_cooldown` 那一支给。
        cooldown_until: Option<i64>,
    },
}

/// 今日次数用尽。
pub const REASON_WRITE_BUDGET_EXHAUSTED: &str = "write_budget_exhausted";
/// 还在冷却里。
pub const REASON_MANUAL_COOLDOWN: &str = "manual_cooldown";

/// 🔴 **写预算排在冷却前面，顺序有意义。**
///
/// 反过来的话，当天最后一次点击之后的 10 分钟里，运维看到的是
/// 「冷却中，10 分钟后再试」——**而 10 分钟后等着他的是「今天用完了」**。
/// 先说那个**当天不会再变**的理由，运维才不会白等一轮。
pub fn check_manual_tend(cur: Option<&ManualGuard>, now: i64) -> ManualTendVerdict {
    let today = day_index(now);
    let reset_at = day_ends_at(now);
    let used = used_today(cur, now);
    let remaining = MANUAL_TENDS_PER_DAY.saturating_sub(used);

    if used >= MANUAL_TENDS_PER_DAY {
        return ManualTendVerdict::Denied {
            reason: REASON_WRITE_BUDGET_EXHAUSTED,
            remaining: 0,
            reset_at,
            retry_after_ms: reset_at - now,
            cooldown_until: None,
        };
    }
    if let Some(g) = cur
        && now < g.cooldown_until
    {
        return ManualTendVerdict::Denied {
            reason: REASON_MANUAL_COOLDOWN,
            remaining,
            reset_at,
            retry_after_ms: g.cooldown_until - now,
            cooldown_until: Some(g.cooldown_until),
        };
    }
    let cooldown_until = now + MANUAL_TEND_COOLDOWN_MS;
    ManualTendVerdict::Ok {
        next: ManualGuard {
            day: today,
            used: used + 1,
            cooldown_until,
        },
        remaining: remaining - 1,
        reset_at,
        cooldown_until,
        retry_after_ms: MANUAL_TEND_COOLDOWN_MS,
    }
}

#[cfg(test)]
mod tests;
