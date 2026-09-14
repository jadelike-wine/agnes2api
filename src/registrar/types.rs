//! 注册机的时间常量，以及它们之间那几条**必须成立**的关系。
//!
//! 移植自 TS 版 `src/core/registrar/types.ts`（150 行），规则真源
//! `docs/porting/01-core-rules.md §10.3`。
//!
//! # 这个文件里的数不是「调参」，每一个都压着一条失败形态
//!
//! 下面每个常量的文档里写的是**它为什么是这个数**，以及**改小 / 改大各会坏成什么样**。
//! 🔴 **动它们之前先把那段读完。**

/// 注册链上**每一个** HTTP 请求的超时。
///
/// # 🔴 为什么必须有
///
/// 注册机的所有耗时预算都建立在「每个请求都会及时返回」这个**未言明的前提**上。
/// **一次挂起的请求就能把单轮拖过补池锁的 TTL** ⇒ 下一轮在上一轮还活着的时候开跑
/// ⇒ **两轮同时占邮箱名额**；容器这时被重启（`docker compose up -d` / OOM kill）
/// 则正在铸的那个邮箱的收尾**不会执行** ⇒ **邮箱泄漏 ⇒ 悄悄耗尽活跃邮箱配额**。
pub const REGISTRAR_REQUEST_TIMEOUT_MS: i64 = 15_000;

/// 定时轮的墙钟上限 = **补池锁的 TTL**。
///
/// - **上界**：必须**明显短于**补池间隔（默认 30 分钟）。锁是这么用的：
///   进程被硬杀时收尾一行都不跑，锁只能靠自然过期放开 —— TTL 取到间隔的一半，
///   **代价上界就是最多跳过一轮**。
/// - **下界**：必须盖得住一轮真实最坏耗时，否则**一把还活着的锁会被下一轮读成空的，
///   两轮同时开跑**。
pub const SCHEDULED_ROUND_WALL_CLOCK_MS: i64 = 900_000; // 15 分钟

/// 定时轮的预算 = [`SCHEDULED_ROUND_WALL_CLOCK_MS`] 的 87%，留约 120 秒余量。
///
/// # 🔴 今天**没有运行期消费者**，只剩启动期交叉校验这一个用途
///
/// `tend_once` 的 `round_budget_ms` 是可选参数，**只有手动那一轮传**，定时轮一直不传。
/// **别把它读成「运行期真的会截断」—— 今天不会。**
///
/// 那 120 秒余量是留给注册链上那几个 [`REGISTRAR_REQUEST_TIMEOUT_MS`]、
/// 以及准备阶段的（读域名台账 + 取域名列表）。
/// ⚠️ **把它们也算进判据是行不通的** —— 理论最坏本来就高于 900 秒，
/// 那样会变成**一次尝试都不敢开始**。
pub const SCHEDULED_ROUND_BUDGET_MS: i64 = 780_000; // 13 分钟

// ── 手动补池专用一族 ──────────────────────────────────────────────────────
//
// 🔴🔴 **与上面那族不是一回事，不许合并。**
//
// # 这一族的来历（Cloudflare 平台日志原话，不是推断）
//
// 面板「立即补池」从前回 202 之后把整轮交给 `ctx.waitUntil`，而平台在**响应结束后约 30 秒**
// 就把它取消掉，**实测 3/3 复现**。取消**不抛异常** ⇒ 两层 try/catch/finally 一个都不执行
// ⇒ 不写补池历史、不发事件、锁泄漏到自然过期 ⇒ **点一次按钮把注册机整个停摆一刻钟。**
// 处置是**换载体** —— 端点自己 `await` 这一轮再返回。
//
// ⚠️ **平台前提在 v0.4.0 随 Worker 形态一起没了，这一族仍然留着**：
// 换完载体之后这一轮的载体就是**这条 HTTP 请求本身**，
// 于是它的墙钟上限变成「浏览器 / 反向代理愿意等多久」——
// **那是一个比定时轮小一个量级的数。**
// ⇒ 手动一族**不是**定时轮那一族的一个副本，两者今天仍然不许合并。

/// 手动一轮最多铸几把。
pub const MANUAL_MINT_BATCH: u32 = 1;

/// 手动一次尝试最多试几个域名。
///
/// # 🔴 这**不是**「保守一点」
///
/// 它让 [`worst_attempt_ms`] **退化成常量**。不压它的话，运维把 `MAX_DOMAIN_ATTEMPTS` 调成 2，
/// `worst_attempt_ms` 就跳到 150 秒 > 预算 ⇒ 预算判据当场判「这一次尝试开不起来」
/// ⇒ **按钮变成永远铸不出 key 的诚实空转，而且空转得毫无征兆。**
pub const MANUAL_MAX_DOMAIN_ATTEMPTS: u32 = 1;

/// 手动一轮等验证码最多等多久。
pub const MANUAL_CODE_TIMEOUT_MS: i64 = 60_000;

/// 手动一轮的预算。
///
/// # 🔴 **不是耗时上界，别读成上界**
///
/// 在 [`MANUAL_MINT_BATCH`]` = 1` 之下它**只被判一次**，
/// 语义是「**这一次尝试开不开得起来**」。
/// 唯一的硬约束是**必须严格大于 [`worst_attempt_ms`]**（手动一族之下 = 60 秒），
/// 由本文件末尾那条**编译期**断言钉着。
///
/// # ⚠️⚠️ 原注释「多出来的 10 秒留给 elapsed」是**错的，而且错得很贵**
///
/// 判据里的 elapsed 从前是从**整轮开头**算起的，中间隔着「取域名列表」这类
/// **单请求就允许 15 秒**的准备动作 —— **10 秒的余量根本不够，
/// 上游邮箱服务慢一次这颗按钮就诚实空转**，还会打出一条指向 `CODE_TIMEOUT_MS` 的错误处置
///（而手动轮的等码超时已经被压到 60 秒，调它没有用）。
///
/// ⇒ **处置不是把这个数抬到 85 秒**（那只是把同一条赌注的赔率改一改），
/// 而是**把预算判据的起点挪到准备阶段之后**。
pub const MANUAL_ROUND_BUDGET_MS: i64 = 70_000;

/// **一次尝试**的最坏耗时：等满一次验证码超时，外加这次尝试里换域名要付的
/// `max_domain_attempts − 1` 段间隔。
///
/// # 🔴🔴 这个口径在 TS 那边**被抄成过四份，四份互相不一致**
///
/// 原注释逐字写着：「上一版这张表被写了四份、四份点名的集合互相不一致 ——
/// **照任一份走都会漏掉一个文件**。」于是那边只好用一段注释挨个点名
/// 「改这里就得同时改 tender / config / wire / 五语言文档」。
///
/// ⇒ **Rust 这一侧不照抄那个形态。** 它就是唯一的一份，
/// [`crate::registrar::config`] 的启动期告警与将来 `tender` 的运行期判据**都调它**。
/// 漂移的可能不是靠注释提醒，是**结构上不存在**。
///
/// ⚠️ **移植时我自己在这里写错过一次，留个路标**：我最初写成
/// `max_domain_attempts × code_timeout_ms`。它在手动那一族的取值下**碰巧也得 60 秒**，
/// 判据全绿；连「调成 2 会爆预算」那一格算出的 150 秒都对得上 ——
/// 但那个 150 秒是 `2 × 75_000`，而正典是 `60_000 + 1 × 90_000`。
/// **数字对，公式和输入都错。**
/// ⚠️ `const fn`：下面那条交叉校验要在**编译期**求它的值。
/// 用 `as i64` 而不是 `i64::from` —— 后者不是 `const`，而这里的转换是无损的。
pub const fn worst_attempt_ms(
    max_domain_attempts: u32,
    code_timeout_ms: i64,
    mint_delay_max_ms: i64,
) -> i64 {
    let extra = max_domain_attempts.saturating_sub(1) as i64;
    code_timeout_ms.saturating_add(extra.saturating_mul(mint_delay_max_ms))
}

/// **一轮**的最坏耗时：每个名额等满一次验证码超时，外加名额之间的间隔。
///
/// 与 [`worst_attempt_ms`] 同源同形，见那里的说明。
///
/// ⚠️ **名额之间那一项是后来补上的**：默认间隔从几秒改成几十秒之后，
/// 它不再是可以忽略的尾巴（默认配置下占 360 秒，而整轮最坏是 960 秒）。
/// **漏掉它，那条 `interval_shorter_than_worst_round` 的告警
/// 就会在一份真的会重叠的配置上保持沉默。**
pub const fn worst_round_ms(mint_batch: u32, code_timeout_ms: i64, mint_delay_max_ms: i64) -> i64 {
    let slots = mint_batch as i64;
    let gaps = mint_batch.saturating_sub(1) as i64;
    slots
        .saturating_mul(code_timeout_ms)
        .saturating_add(gaps.saturating_mul(mint_delay_max_ms))
}

// ── 三条交叉校验：**编译期**断言 ─────────────────────────────────────────
//
// 🔴 **刻意做成编译期，而不是启动期或 `#[test]`。**
// 这三条原先是三格 `#[test]`，clippy 报了 `this assertion has a constant value` ——
// **那句话本身就是结论**：它们全由常量决定，运行期不可能失败。
// 一条运行期不可能失败的断言放在测试里，只是让人**以为**它在守着什么。
// 挪到这里之后，配错一个常量**编译都过不去**。
//
// ⚠️ 代价：它们不再出现在 `cargo test` 的用例计数里。**那不是判别力下降** ——
// 编译期断言比测试更早、更硬，而且没有「忘了跑那一格」这种可能。
//
// ⚠️⚠️ **移植时我在这里栽过一次，留个路标**：我第一次做这个改动时，
// 删掉了那三格 `#[test]`、而新的 `const _` **因为字符串没匹配上压根没写进来**。
// 于是 clippy 变绿（触发告警的那三格没了）、`cargo test` 也绿（少了三格没人注意），
// **三条护栏凭空消失而四道门禁全是绿的**。
// 是「故意改坏一个常量，看它编译过不过」这一步把它抓出来的。
// **改这一段之后一定要再跑一次那个变异。**

/// 手动一族的预算必须**严格大于**它自己的最坏尝试。
///
/// 🔴 这是 [`MANUAL_ROUND_BUDGET_MS`] 那条唯一硬约束的落点。
/// 不成立时「立即补池」会变成**永远铸不出 key 的诚实空转**，而且空转得毫无征兆。
///
/// 手动一族之下 `max_domain_attempts = 1` ⇒ 换域名那一项恒为 0，
/// 于是最坏尝试退化成常量 60 秒（正是 [`MANUAL_MAX_DOMAIN_ATTEMPTS`] 那条裁定要的效果）。
/// 间隔上限传 0：这一族压根不走名额之间的间隔。
const _MANUAL_BUDGET_COVERS_WORST_ATTEMPT: () = assert!(
    MANUAL_ROUND_BUDGET_MS
        > worst_attempt_ms(MANUAL_MAX_DOMAIN_ATTEMPTS, MANUAL_CODE_TIMEOUT_MS, 0),
    "手动预算盖不住它自己的最坏尝试 —— 「立即补池」会变成永远铸不出 key 的诚实空转",
);

/// 定时轮的预算必须**明显短于**它的墙钟上限
///（那 120 秒余量留给请求超时与准备阶段，见 [`SCHEDULED_ROUND_BUDGET_MS`]）。
const _SCHEDULED_BUDGET_LEAVES_HEADROOM: () = assert!(
    SCHEDULED_ROUND_BUDGET_MS < SCHEDULED_ROUND_WALL_CLOCK_MS,
    "定时轮预算没给墙钟上限留余量",
);

/// 🔴🔴 **两族不许合并。**
///
/// 手动轮的载体是**那条 HTTP 请求本身**，墙钟上限是「浏览器 / 反向代理愿意等多久」，
/// 比定时轮小一个量级。这一条钉的就是「它们真的不一样」。
const _THE_TWO_FAMILIES_STAY_AN_ORDER_OF_MAGNITUDE_APART: () = assert!(
    MANUAL_ROUND_BUDGET_MS * 10 < SCHEDULED_ROUND_BUDGET_MS,
    "两族靠得太近了 —— 合并会让「立即补池」挂在一条 HTTP 请求上等十几分钟",
);

#[cfg(test)]
mod tests {
    use super::*;

    /// 🔴 这一格钉的是 [`MANUAL_MAX_DOMAIN_ATTEMPTS`]` = 1` 那条裁定的**理由**，
    /// 不是那个数本身。
    ///
    /// 它让最坏尝试退化成常量；调成 2 就跳到 150 秒 > 预算。
    #[test]
    fn raising_the_manual_domain_attempts_would_blow_the_budget() {
        // attempts = 1 ⇒ 换域名那一项为 0 ⇒ 退化成常量。
        assert_eq!(worst_attempt_ms(1, MANUAL_CODE_TIMEOUT_MS, 90_000), 60_000);
        // attempts = 2 ⇒ 多付一段 `mint_delay_max_ms`（默认 90 秒）⇒ 150 秒 > 70 秒预算。
        assert_eq!(worst_attempt_ms(2, MANUAL_CODE_TIMEOUT_MS, 90_000), 150_000);
        assert!(
            worst_attempt_ms(2, MANUAL_CODE_TIMEOUT_MS, 90_000) > MANUAL_ROUND_BUDGET_MS,
            "把 max_domain_attempts 调成 2 之后仍然盖得住 —— \
             那 MANUAL_MAX_DOMAIN_ATTEMPTS = 1 这条裁定的理由就不成立了，该重写它",
        );
    }

    /// 🔴 **这一格钉的是公式本身，不是某个取值。**
    ///
    /// 我移植时把它写成过 `attempts × code_timeout`，而那个写法在手动一族的取值下
    /// **与正典同值** —— 上面那两格都不会红。这一格挑一组**能把两个公式分开**的输入。
    #[test]
    fn the_worst_attempt_formula_is_timeout_plus_gaps_not_timeout_times_attempts() {
        // 正典：60 + (3-1)×10 = 80；错误写法：3 × 60 = 180。
        assert_eq!(worst_attempt_ms(3, 60, 10), 80);
        // 间隔为 0 时两个公式**仍然**分得开（正典 60，错误写法 180）——
        // 所以这一格不依赖 mint_delay_max 非零。
        assert_eq!(worst_attempt_ms(3, 60, 0), 60);
    }

    /// ⚠️ 一轮的公式里「名额之间那一项」是后来补上的，漏掉它那条告警会在
    /// **真的会重叠**的配置上保持沉默。
    #[test]
    fn the_worst_round_formula_counts_the_gaps_between_slots() {
        // 默认配置：batch 5 × 等码 120 秒 + 4 段 × 90 秒 = 600 + 360 = 960 秒。
        assert_eq!(worst_round_ms(5, 120_000, 90_000), 960_000);
        // 漏掉间隔那一项就只剩 600 秒 —— 那正是这条告警会闭嘴的那个数。
        assert_ne!(worst_round_ms(5, 120_000, 90_000), 600_000);
        // 单个名额没有「之间」。
        assert_eq!(worst_round_ms(1, 120_000, 90_000), 120_000);
        assert_eq!(worst_round_ms(0, 120_000, 90_000), 0);
    }

    /// `worst_attempt_ms` 的 `max(1)`：传 0 不许让上界塌成 0。
    ///
    /// 上界塌成 0 会让「这一次尝试开不开得起来」恒为真 ——
    /// 那道判据就**恒绿**，而它本来是要拦住「开起来也跑不完」的。
    #[test]
    fn a_zero_attempt_count_does_not_collapse_the_upper_bound() {
        // `saturating_sub(1)` 让 0 与 1 同义：一次尝试至少要等满一次验证码超时。
        assert_eq!(worst_attempt_ms(0, 60_000, 90_000), 60_000);
    }

    /// 极端输入不许绕回。
    #[test]
    fn absurd_inputs_saturate_instead_of_wrapping() {
        for v in [
            worst_attempt_ms(u32::MAX, i64::MAX, i64::MAX),
            worst_round_ms(u32::MAX, i64::MAX, i64::MAX),
        ] {
            assert!(v > 0, "算术绕回成了负数/零：{v}");
        }
    }
}
