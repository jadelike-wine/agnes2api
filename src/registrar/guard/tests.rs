//! 两道闸的判据。
//!
//! 它们守的是**上游代价**，不是「点得太快不好看」：一次点击最多烧掉
//! `MINT_BATCH` 个临时邮箱名额，而那是补池能不能继续工作的前提。

use super::*;

const DAY: i64 = 86_400_000;

/// 头一次点：放行，并把冷却推到 10 分钟后。
#[test]
fn the_first_click_of_the_day_goes_through_and_starts_the_cooldown() {
    let ManualTendVerdict::Ok {
        next,
        remaining,
        cooldown_until,
        retry_after_ms,
        reset_at,
    } = check_manual_tend(None, 0)
    else {
        panic!("今天还没点过，必须放行");
    };
    assert_eq!(next.used, 1);
    assert_eq!(next.day, 0);
    assert_eq!(cooldown_until, MANUAL_TEND_COOLDOWN_MS);
    assert_eq!(retry_after_ms, MANUAL_TEND_COOLDOWN_MS);
    assert_eq!(reset_at, DAY);
    assert_eq!(
        remaining,
        MANUAL_TENDS_PER_DAY - 1,
        "🔴 Ok 那一支的口径是「点完这一次之后还剩几次」",
    );
}

/// 🔴 **冷却里再点 ⇒ 429 `manual_cooldown`，而且两个倒计时字段成对给。**
#[test]
fn clicking_again_inside_the_cooldown_is_refused_with_a_live_countdown() {
    let g = ManualGuard {
        day: 0,
        used: 1,
        cooldown_until: 600_000,
    };
    let ManualTendVerdict::Denied {
        reason,
        retry_after_ms,
        cooldown_until,
        remaining,
        ..
    } = check_manual_tend(Some(&g), 100_000)
    else {
        panic!("冷却里必须拒");
    };
    assert_eq!(reason, REASON_MANUAL_COOLDOWN);
    assert_eq!(retry_after_ms, 500_000);
    assert_eq!(cooldown_until, Some(600_000));
    assert_eq!(
        remaining,
        MANUAL_TENDS_PER_DAY - 1,
        "拒的时候也要说还剩几次"
    );
}

/// 冷却过了就放行，`used` 累加。
#[test]
fn once_the_cooldown_expires_the_next_click_goes_through() {
    let g = ManualGuard {
        day: 0,
        used: 1,
        cooldown_until: 600_000,
    };
    let ManualTendVerdict::Ok { next, .. } = check_manual_tend(Some(&g), 600_000) else {
        panic!("冷却到点了必须放行");
    };
    assert_eq!(next.used, 2);
}

/// 🔴 **写预算排在冷却前面 —— 顺序有意义。**
///
/// 反过来的话，当天最后一次点击之后的 10 分钟里运维看到的是
/// 「冷却中，10 分钟后再试」，**而 10 分钟后等着他的是「今天用完了」**。
/// 先说那个当天不会再变的理由，运维才不会白等一轮。
#[test]
fn an_exhausted_daily_budget_is_reported_before_the_cooldown_not_after() {
    // 既用尽了、又还在冷却里 —— 两条都成立，看它先说哪一条。
    let g = ManualGuard {
        day: 0,
        used: MANUAL_TENDS_PER_DAY,
        cooldown_until: 600_000,
    };
    let ManualTendVerdict::Denied {
        reason,
        retry_after_ms,
        remaining,
        ..
    } = check_manual_tend(Some(&g), 100_000)
    else {
        panic!("用尽了必须拒");
    };
    assert_eq!(
        reason, REASON_WRITE_BUDGET_EXHAUSTED,
        "🔴 先说了「冷却中」—— 运维等满 10 分钟，回来撞上一堵当天挪不开的墙",
    );
    assert_eq!(remaining, 0);
    assert_eq!(
        retry_after_ms,
        DAY - 100_000,
        "耗尽那一支的倒计时是到明天，不是到冷却结束",
    );
}

/// 跨天之后额度自己回来 —— **靠 `day` 字段判，不靠键过期**。
#[test]
fn the_daily_budget_comes_back_on_its_own_the_next_day() {
    let g = ManualGuard {
        day: 0,
        used: MANUAL_TENDS_PER_DAY,
        cooldown_until: 0,
    };
    let ManualTendVerdict::Ok {
        next, remaining, ..
    } = check_manual_tend(Some(&g), DAY)
    else {
        panic!("跨天了必须放行");
    };
    assert_eq!(next.day, 1);
    assert_eq!(next.used, 1, "🔴 跨天没重置 —— 按钮从此永久锁死");
    assert_eq!(remaining, MANUAL_TENDS_PER_DAY - 1);
}

/// 🔴 **护栏键被写坏 ⇒ 当成「今天还没点过」放行，绝不当成「已耗尽」。**
///
/// 放行的代价是多一轮补池（有存储锁与冷却兜着）；
/// 反过来是**把按钮永久锁死，而面板上看不出任何原因**。
#[test]
fn a_corrupt_guard_key_degrades_to_allow_never_to_locked_out() {
    for bad in [
        Value::Null,
        serde_json::json!("字符串"),
        serde_json::json!([1, 2]),
        serde_json::json!({ "day": 0, "used": "不是数", "cooldownUntil": 0 }),
        serde_json::json!({ "day": 0, "used": 1 }),
        serde_json::json!({ "used": 1, "cooldownUntil": 0 }),
        serde_json::json!({ "day": 0, "used": -1, "cooldownUntil": 0 }),
    ] {
        assert_eq!(narrow_manual_guard(Some(&bad)), None, "{bad}");
        assert!(
            matches!(
                check_manual_tend(narrow_manual_guard(Some(&bad)).as_ref(), 0),
                ManualTendVerdict::Ok { .. }
            ),
            "🔴 一把写坏的键把按钮锁死了，而面板上看不出原因：{bad}",
        );
    }
}

/// 读得回来的那一份要原样读回来（落盘形状是对外的）。
#[test]
fn a_well_formed_guard_round_trips_through_storage() {
    let g = ManualGuard {
        day: 7,
        used: 3,
        cooldown_until: 999,
    };
    assert_eq!(narrow_manual_guard(Some(&g.to_value())), Some(g));
}

/// 🔴 **面板那份配额一格名额都不许消费。**
///
/// 它与 [`check_manual_tend`] 共用 [`used_today`]：两份各写一遍的话，
/// **分歧只在跨天那一瞬间可观测** —— 面板显示「还剩 24 次」、点下去却说用完了。
#[test]
fn reading_the_quota_never_consumes_a_slot_and_agrees_with_the_checker() {
    let g = ManualGuard {
        day: 0,
        used: 5,
        cooldown_until: 0,
    };
    let q = manual_tend_quota(Some(&g), 1_000);
    assert_eq!(q.used, 5);
    assert_eq!(q.remaining, MANUAL_TENDS_PER_DAY - 5, "现在还剩几次");
    assert_eq!(
        q.per_day, MANUAL_TENDS_PER_DAY,
        "面板要说「还剩 19 次（共 24 次）」"
    );

    // 再读一次，数字一个都不许变。
    assert_eq!(manual_tend_quota(Some(&g), 1_000), q, "只看不动");

    // 与判定那一侧差且只差 1 —— 那是「点完之后」与「现在」的差别。
    let ManualTendVerdict::Ok { remaining, .. } = check_manual_tend(Some(&g), 1_000) else {
        panic!()
    };
    assert_eq!(
        remaining + 1,
        q.remaining,
        "🔴 两个口径的差不是 1，面板会少显示一次"
    );
}

/// ⚠️ 不在冷却中时，两个倒计时字段**都得是 `None`**。
///
/// 给一个「已经过去的时刻」会让面板渲染出一个**恒为 0 的倒计时**，
/// 而那与「现在就能点」长得不一样却是同一件事。
#[test]
fn the_countdown_fields_are_absent_entirely_when_not_cooling_down() {
    let g = ManualGuard {
        day: 0,
        used: 1,
        cooldown_until: 600_000,
    };
    let after = manual_tend_quota(Some(&g), 600_000);
    assert_eq!(
        after.cooldown_until, None,
        "🔴 冷却过了还给时刻 ⇒ 恒为 0 的倒计时"
    );
    assert_eq!(after.retry_after_ms, None);

    let during = manual_tend_quota(Some(&g), 1_000);
    assert!(
        during.cooldown_until.is_some() && during.retry_after_ms.is_some(),
        "冷却中两个都要给，而且成对",
    );
}

/// 跨天时 `used_today` 归零 —— 两个消费者靠它对齐。
#[test]
fn used_today_ignores_yesterdays_count() {
    let g = ManualGuard {
        day: 0,
        used: 9,
        cooldown_until: 0,
    };
    assert_eq!(used_today(Some(&g), 0), 9);
    assert_eq!(used_today(Some(&g), DAY), 0, "昨天的数不许算进今天");
    assert_eq!(used_today(None, 0), 0);
}
