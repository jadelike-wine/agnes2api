//! 用量板块的取值判定。**06 §5.3 归档表里最重要的一份**（TS 侧 `pure/usage.mjs`，782 行）。
//!
//! # 这个板块的全部难点是「几件长得一模一样的事」
//!
//! 「今天真的是 0 次请求」「Tier-2 没开」「读不出来」「开着但一条分片都还没落盘」
//! 「读到了分片但每一个都坏」——**五件事，在屏幕上可以长成同一个 `0`**。
//! 本模块把它们变成一个显式的枚举 [`UsageState`]，让渲染层**不可能**把它们揉在一起。
//!
//! 🔴 这一层是「绝不伪造 0」那条产品纪律的实现体。改这里之前先读完每个函数
//! 上方那段说明 —— 它们每一条都对应 TS 侧一次真实的缺陷修复。
//!
//! # 与后端 `note` 的分工，一次说清
//!
//! [`crate::admin::usage::UsageNote`] 是一张**运行期**的 code 表，而 `note` 那一格是
//! **摘要**：每一条 code 的判据全部来自同一份响应体里的其它字段
//! （`tier` / `range.clamped` / `days == null` / `shards` / `malformed`）。
//! ⇒ 本模块**两条路都走，而且刻意分开**：
//! · 顶部横幅照 `note` 走（后端已经定好优先级，前端再定一次就是第二份知识）；
//! · 「这份数字完不完整」照 `shards` / `malformed` 两个**字段**走（[`malformed_kind`]）。
//!
//! ⚠️ **两条路分开不是冗余，是必需的**：`GET /admin/api/usage/{date}` **根本不发**
//! `all_malformed` / `partial_malformed` 这两条 code（它常态恒是 `no_request_detail`），
//! 单日下钻要判「这一天的分解是不是缺了几块」**只能靠字段**。
//! 同一件事在两条端点上有两套判据，本模块用**同一个函数**回答它们两个。

// ── 1. 分片畸形到什么程度 ──────────────────────────────────────────────────

/// [`malformed_kind`] 的四种结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedKind {
    /// 分片计数本身读不出来（两个字段里有一个是 `None`）。
    Unknown,
    /// 读到的分片全是好的。
    Clean,
    /// **读到了分片，但每一个都坏。**
    All,
    /// **一部分坏 ⇒ 这份数字是不完整的。**
    Partial,
}

impl MalformedKind {
    /// 对外的字面量。**面板按它分支，改了就是改对外契约。**
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Clean => "none",
            Self::All => "all",
            Self::Partial => "partial",
        }
    }
}

/// 分片畸形到什么程度。**判据是 `shards` 与 `malformed` 两个字段，不是 `note`。**
///
/// | 返回值 | 判据 |
/// |---|---|
/// | [`MalformedKind::Unknown`] | 两个字段里有一个是 `None`（含整块读不出来） |
/// | [`MalformedKind::Clean`] | `malformed == 0` |
/// | [`MalformedKind::All`] | `malformed > 0 && shards == 0` |
/// | [`MalformedKind::Partial`] | `malformed > 0 && shards > 0` |
///
/// ⚠️ **这是本模块唯一回答「分片坏没坏」的地方，两条端点共用它**（见文件头）：
/// 汇总端点在 `note` 里也说了同一件事，单日下钻端点**只说字段**。
/// 让两条端点各写一份判据的后果是它们迟早对同一份响应给出不同的结论。
#[must_use]
pub fn malformed_kind(shards: Option<usize>, malformed: Option<usize>) -> MalformedKind {
    let (Some(shards), Some(malformed)) = (shards, malformed) else {
        return MalformedKind::Unknown;
    };
    if malformed == 0 {
        return MalformedKind::Clean;
    }
    if shards == 0 {
        MalformedKind::All
    } else {
        MalformedKind::Partial
    }
}

// ── 2. 整块状态（五态互不重叠）─────────────────────────────────────────────

/// 整块用量的状态。**五态互不重叠。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageState {
    /// Tier-2 没打开，这个部署**根本没在记账**。
    Off,
    /// 读不出来 —— 含「读到的分片全是坏的」。
    Unavailable,
    /// 开着，但这段区间**一条分片都还没落盘**。
    NoShards,
    /// 读成功了，这段区间**真的是 0 次请求**。
    Empty,
    /// 读成功了，而且有请求。
    Data,
}

impl UsageState {
    /// 对外的字面量。🔴 **`no-shards` 是连字符不是下划线**（TS 侧逐字如此，面板按它分支）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Unavailable => "unavailable",
            Self::NoShards => "no-shards",
            Self::Empty => "empty",
            Self::Data => "data",
        }
    }
}

/// [`usage_state`] 要看的那几格。**一律是响应体里已经有的字段**，不新增后端字段。
#[derive(Debug, Clone, Copy)]
pub struct UsageFacts<'a> {
    /// 响应里那个 `tier`。🔴 **收字符串而不是枚举**：认不出来的值有专门的处置
    /// （退回 [`UsageState::Empty`]），而枚举天然表达不了「认不出来」。
    pub tier: &'a str,
    /// `days` 这一格在不在（`Some(_)` ⇒ 在）。
    pub days_present: bool,
    /// `total.requests`。整块 `total` 读不出来时是 `None`。
    pub total_requests: Option<i64>,
    pub shards: Option<usize>,
    pub malformed: Option<usize>,
    /// 响应里那个 `note` 的 code（不是句子）。
    pub note: Option<&'a str>,
}

/// 整块用量的状态。**五态互不重叠。**
///
/// ⚠️ **判据取的是顶层 `total.requests`，不是 `days` 里有没有非零的一天**，
/// 而这一条不是随手选的：`total` 数的是**读到的每一个分片**，而 `days` 按分片自报的
/// `day` 分键 ⇒ 一个自报 `day` 与它所在的键对不上的分片会进 `total` 而进不了任何一格
/// `days`，方向是「`total` 只多不少」。照 `days` 判会得到「横幅说『这段时间没有请求』，
/// 而总请求数那张卡写着 42」。**状态判据必须与卡片除以的那个数同源。**
///
/// ⚠️⚠️ **`all_malformed` 归 [`UsageState::Unavailable`]，这一条是裁定，写下理由**：
/// 那一档下 `days` 每天一格全是 0 桶、`total` 也是 0 桶，但**那些 0 不是知识**
/// —— 读到的每一个分片都是坏的，我们对这段时间的用量**一无所知**。
/// 把它渲染成 `0` 正是「伪造 0」，只是这一次伪造的不是「接口失败」那一档，
/// 而是「读到的全是垃圾」那一档。
/// ⇒ **`partial_malformed` 不归这里**：那一档确实读到了好分片，数字是真的，只是不全。
///
/// ⚠️ **那一句必须排在 `no-shards` 前面**：「分片都在、每一个都坏」的 `shards`
/// 同样是 0，掉进 `no-shards` 就会把「读到的全是垃圾」说成「还没写进去」。
///
/// # 第五档 `no-shards`：「开着，但一条分片都还没落盘」（线上实测逼出来的）
///
/// ⚠️⚠️ **这一档以前掉进 `empty`，而 `empty` 那句话对它是假的。**
/// 打开统计、打了几次请求（池空、全 503）之后打开这一页，后端回的是
/// `tier:"tier2"` / `shards:0` / `note:"no_shards"` / `pending.count > 0`，
/// 而同一屏上同时出现了两句话：横幅说「这个部署确实没有记下任何用量」，
/// 紧挨着的尾巴说「还有 N 条计数没有落盘」。**前一句是假的**——它确实记下了，
/// 只是还在内存里。
///
/// ⚠️ **判据只从已有字段推**：`tier == "tier2"` 且 `total.requests` 不是正数
/// 且（`shards == 0` 或 `note == "no_shards"`）。**两个判据都要**，是因为
/// `range_clamped` **压过** `no_shards` ⇒ 区间被夹过时同一个部署拿到的 `note` 是
/// `range_clamped`，**只认 `note` 会让这一档在那条路上整个消失**。
///
/// ⚠️ **`tier == "tier2"` 那一半同样是判据，不是随手写的防御**：这一档换上去的那句话
/// **逐字点名了 Tier-2 的落盘机制**（「攒够一个落盘间隔才写一次」）。`tier` 是认不出来
/// 的值时我们并不知道那个部署按什么节奏落盘，照样说这句话就是拿一条**我们没有的知识**
/// 去解释屏幕上的 0。⇒ 认不出来的 `tier` 退回 [`UsageState::Empty`]。
/// 🔴 **Rust 侧今天只发得出 `"off"` / `"tier2"` 两个值**（`admin::usage` 里那句
/// `if wiring.is_none() { "off" } else { "tier2" }`），所以这一条钉的是
/// **面板对没见过的 `tier` 的态度**，不是一条今天可达的分支。**别因为「走不到」就删。**
///
/// ⚠️⚠️ **`pending.count == 0` 不归 `empty`，它照样是这一档**：`pending` 是**内存**
/// 状态，只反映服务这一次请求的那个实例。⇒「pending 是 0」证明不了「真的没人用」。
/// **`pending` 不参与分档**，所以它压根不在 [`UsageFacts`] 里。
#[must_use]
pub fn usage_state(f: &UsageFacts<'_>) -> UsageState {
    if f.tier == "off" {
        return UsageState::Off;
    }
    if !f.days_present {
        return UsageState::Unavailable;
    }
    if malformed_kind(f.shards, f.malformed) == MalformedKind::All {
        return UsageState::Unavailable;
    }
    let Some(requests) = f.total_requests else {
        return UsageState::Unavailable;
    };
    if requests > 0 {
        return UsageState::Data;
    }
    if f.tier == "tier2" && (f.shards == Some(0) || f.note == Some("no_shards")) {
        UsageState::NoShards
    } else {
        UsageState::Empty
    }
}

/// 单日下钻那一份的整块状态。**判据与 [`usage_state`] 同源，只是字段不同。**
///
/// ⚠️ **没有 `empty` 这一档**：这条端点的三个 map 只含**有过流量**的那些键，
/// 所以「表是空的」在读成功时就是「这一天没有记录」，不需要再分一档。
///
/// ⚠️⚠️ **它存在的理由与 [`usage_state`] 的 `all_malformed` 早退完全相同**：
/// 那一档下什么都合不出来 ⇒ 三个 map 都是空的 ⇒ 三张分解表都会说
/// 「这一天没有可以分解的记录」——**而事实是那一天的分片全坏了，我们什么都不知道**。
/// `date_out_of_retention` 同样归 `unavailable`：那天的记录已经不在了，
/// **不是那天没有请求**，具体是哪一种由 `note` 那条横幅说。
///
/// `hours_present` = `hours` **整块**在不在。空 map `{}` **算在**——那是
/// 「读成功了，这一天没有流量」。
#[must_use]
pub fn detail_state(
    tier: &str,
    hours_present: bool,
    shards: Option<usize>,
    malformed: Option<usize>,
) -> UsageState {
    if tier == "off" {
        return UsageState::Off;
    }
    if !hours_present {
        return UsageState::Unavailable;
    }
    if malformed_kind(shards, malformed) == MalformedKind::All {
        return UsageState::Unavailable;
    }
    UsageState::Data
}

// ── 3. 一行 / 一格该按哪一种状态渲染 ───────────────────────────────────────

/// 表格里一行的状态。[`UsageState`] 的三值投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    Unavailable,
    NoShards,
    Data,
}

impl RowState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::NoShards => "no-shards",
            Self::Data => "data",
        }
    }
}

/// 表格里一行该按哪一种状态渲染。
///
/// ⚠️⚠️ **整块读不出来时每一行都读不出来。** TS 侧上一版两张表各自写了
/// 「这一行的桶是不是 `null`」（**完全不看整块状态**），于是 [`usage_state`] 判出来的
/// `unavailable` 一步都没往下传 ⇒ 顶部六张卡刚说完「我们不知道」，紧挨着的表就把
/// 同一段区间写成「请求 0 次」。⭐ **根因是「上一处修复」本身**：为了让卡片别写 0
/// 而加的早退，把那份 0 留给了下一个消费者。
///
/// 第二个参数是这一行自己的桶在不在：整块读成功了、而这一行的桶读不回来（形状不对），
/// 那也是「这一行我们不知道」，**不是这一行是 0**。
///
/// ⚠️⚠️ **`no-shards` 原样往下传**：TS 侧上一版把它压成 `data`，于是日表里没有数字可写
/// 的那一列拿到的是 `cell_kind(Data, None)` ⇒ [`CellKind::None`] ⇒ tooltip 又变回
/// 「这段时间没有可用的样本」——**与六张卡上刚修掉的是同一句假话，只是挪到了下一屏**。
/// ⭐ 记一条形状：**「两种状态在今天的渲染上等价」不等于「可以把其中一种擦掉」**
/// ——擦掉之后，下一个只对其中一种成立的判据就再也接不到它了。
#[must_use]
pub fn row_state(state: UsageState, bucket_present: bool) -> RowState {
    if matches!(state, UsageState::Off | UsageState::Unavailable) {
        return RowState::Unavailable;
    }
    if !bucket_present {
        return RowState::Unavailable;
    }
    if state == UsageState::NoShards {
        RowState::NoShards
    } else {
        RowState::Data
    }
}

/// 这一块**读成功了没有**。两张表拿它决定「空」的时候该说哪一句。
///
/// ⚠️⚠️ **判据必须是白名单（「只有确知读成功了才说『没有』」），不是黑名单**：
/// TS 侧上一版日表写的是「不是 `unavailable` 就当空」——那是黑名单，方向是反的。
/// [`UsageState`] 哪天多一档，那一档会**默认落到「这段区间里没有可以列出的日子」**，
/// 也就是**默认说假话**。
/// ⭐ 记一条形状：**「排除掉已知的坏情况」与「只放行已知的好情况」在今天的取值集合上
/// 等价，在明天多一档时相反**——而护栏要挡的正是明天那一档。
///
/// ⚠️ **`no-shards` 进白名单**：那一档**读成功了**，只是这段区间一条分片都没落盘
/// ⇒ 表里那句话该是「没有可以列出的日子」，不是「读不出来」。
#[must_use]
pub fn read_succeeded(state: UsageState) -> bool {
    matches!(
        state,
        UsageState::Data | UsageState::Empty | UsageState::NoShards
    )
}

/// 一个数字格的四种结局。**「三态不许长得一样」在单元格这一层的落点。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    /// 整块读成功了，而且这一格是个数字（**含 0**）⇒ 渲染那个数字。
    Value,
    /// 整块读成功了，但这一格没有可用的样本 / 分母 ⇒ **`–` EN DASH（U+2013）**。
    None,
    /// 同上，但整块是 `no-shards` —— 有没有样本这件事我们不知道。
    /// **同一根 `–`，换一句 tooltip。**
    NoneNoShards,
    /// 整块就没读出来 ⇒ **`—` EM DASH（U+2014）**。
    Unknown,
}

impl CellKind {
    /// 对外的字面量。🔴 `none-no-shards` 是连字符。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Value => "value",
            Self::None => "none",
            Self::NoneNoShards => "none-no-shards",
            Self::Unknown => "unknown",
        }
    }
}

/// 一个数字格该渲染成哪一档。
///
/// ⚠️⚠️ **计数类在 `empty` 态显示 `0` 是对的，别「修」成破折号**：这一次读成功了，
/// 我们**确实知道**答案是零。把真零画成破折号是**反向的撒谎**，与「接口失败伪造 0」
/// 同样严重、方向相反。**而同一个 `empty` 态下比率与平均延迟走 [`CellKind::None`]**
/// （分母是 0、样本是 0），两类卡因此在同一个状态下长得不一样 —— 那正是要的。
///
/// ⚠️⚠️ **第四档藏在 `title` 属性里，它是终检点名的第三处同一个谎。**
/// `no-shards` 上一版掉进 [`CellKind::None`] ⇒ 三张卡挂的是「这一次读成功了，只是这段
/// 时间没有可用的样本」——**而同一屏的横幅刚说完「还有 N 条计数没有落盘」**。
/// 样本是有的，只是没落盘；这一档下「有没有样本」正是我们分不出来的那件事。
///
/// ⚠️ **它照旧渲染成 EN DASH，字形一个像素都不动**：`no-shards` 下这一格
/// 「读成功了、只是没有数字可写」与 `empty` 下是同一件事，改字形就成了
/// 「已经落盘的就是 0 条」那条裁定的反面。
///
/// ⚠️ **两个破折号必须视觉可区分，而判据在这里、不在渲染层**：让渲染层自己目测
/// 「这一格该画哪一根」就等于把三态判定复制回了 DOM 拼装代码。
#[must_use]
pub fn cell_kind(state: UsageState, value: Option<i64>) -> CellKind {
    if matches!(state, UsageState::Off | UsageState::Unavailable) {
        return CellKind::Unknown;
    }
    if value.is_some() {
        return CellKind::Value;
    }
    if state == UsageState::NoShards {
        CellKind::NoneNoShards
    } else {
        CellKind::None
    }
}

/// 比率格的结局。**分母 `<= 0` 就是「没有比率可算」，不是「比率是 0%」。**
///
/// ⚠️ **少了这一层，`empty` 与 `unavailable` 在比率卡上会长得一模一样**：
/// 「0 ÷ 0」与「null ÷ null」格式化出来**都是破折号**，而前者的意思是
/// 「这段时间没有请求」、后者是「读取失败」。
///
/// ⚠️ **它转调 [`cell_kind`]，所以第四档在这里是白拿的** —— `no-shards` 下分母是 0
/// ⇒ 传进去的是 `None` ⇒ 落第四档。**不必在这里再写一次判据。**
#[must_use]
pub fn ratio_kind(state: UsageState, den: Option<i64>) -> CellKind {
    cell_kind(state, den.filter(|d| *d > 0))
}

/// 平均延迟。`latency_count == 0` 时返回 `None` —— **没有样本不等于零延迟。**
///
/// 调用方拿 [`cell_kind`] 去决定它渲染成哪一种破折号，本模块不决定它长什么样。
///
/// ⚠️ **负的 `latency_count` 同样归 `None`**：那只可能是存储被写坏，
/// 除以它会得到一个负的「平均延迟」，而那比一根破折号更难被发现。
#[must_use]
pub fn avg_latency(latency_sum: i64, latency_count: i64) -> Option<i64> {
    if latency_count <= 0 {
        return None;
    }
    // 四舍五入，与 TS 侧 `Math.round` 同语义（两边都是非负数，所以 `+ half` 足够）。
    Some((latency_sum + latency_count / 2) / latency_count)
}

// ── 4. 横幅分档 ────────────────────────────────────────────────────────────

/// 一条 `note` 该用哪一档横幅。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteSeverity {
    /// **要人去查。**
    Error,
    /// 只是一句提示。
    Warn,
    /// 「本来就该是这样」的常态说明。
    Info,
}

impl NoteSeverity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
        }
    }
}

/// 这条 `note` 该用哪一档横幅。`None` = 不出横幅（`note` 本来就没有）。
///
/// ⚠️ **分档判据是「谁需要人去查」，与后端 `note` 的优先级同源**
/// （畸形要人去查存储，被夹只是一句提示）。
///
/// ⚠️ **表外的 code 归 [`NoteSeverity::Warn`]，刻意不归 `Info` 也不归 `Error`**：
/// 归 `Info` 等于把一条读不懂的信号当成常态，归 `Error` 又会在后端加一条纯提示型
/// code 的那天把面板变成一片红。
/// 🔴 **这一条在 Rust 侧不是形式主义**：`note` 是从
/// [`crate::admin::usage::UsageNote::as_str`] 过来的字符串，后端加第十条 code 时
/// 这里不会有任何东西红 —— 除了 `admin::usage` 那边那格「每一条 code 都被这张表认得」
/// 的对表判据。
#[must_use]
pub fn note_severity(note: Option<&str>) -> Option<NoteSeverity> {
    let note = note?;
    Some(match note {
        "read_failed" | "clock_unavailable" | "all_malformed" | "partial_malformed" => {
            NoteSeverity::Error
        }
        "range_clamped" => NoteSeverity::Warn,
        "tier2_off" | "no_shards" | "date_out_of_retention" | "no_request_detail" => {
            NoteSeverity::Info
        }
        _ => NoteSeverity::Warn,
    })
}

// ── 5. 「API 密钥」那一屏要的那一格 ────────────────────────────────────────

/// 「这把密钥这段时间被用了几次」的结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyUsageKind {
    /// Tier-2 关着 ⇒ 这个部署**根本没在记账**。
    Off,
    /// 读不出来。
    Unknown,
    /// 开着，但一条分片都还没落盘。
    NoShards,
    /// 有数可查（具体几次去 `byApiKey` 里取，缺席就是真的 0 次）。
    Value,
}

impl ApiKeyUsageKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Unknown => "unknown",
            Self::NoShards => "no-shards",
            Self::Value => "value",
        }
    }
}

/// 「API 密钥」板块每张卡上那个用量数该怎么读。
///
/// ⚠️⚠️ **`off` 是一档独立的结局，不许并进 `value` 的 0**：Tier-2 关着时这个部署
/// **根本没在记账**，画一个 `0` 就是把「没开」说成「没人用」。
/// 判据走的是**同一个** [`usage_state`]，不另写一套 —— 两套判据迟早会对同一份响应
/// 给出不同的结论，而分叉的那一边正好是「关着」时。
///
/// ⚠️⚠️ **`no-shards` 同样是一档独立的结局**：TS 侧这个函数上一版是**黑名单**
/// （只挡 `off` / `unavailable`，其余一律往 `value` 倒），于是 [`usage_state`] 多分出来
/// 的那一档**默认落进「就是 0」**——而「API 密钥」那一屏上没有任何横幅替它说话，
/// 每张卡照旧写「近 24 小时 ≈ 0 次请求」。
/// ⚠️ 它**排在「`byApiKey` 在不在」之前**：这一档下那份 map 本来就是空的，
/// 走下去只会拿到 `value` + `0`，也就是那句假话本身。
///
/// ⚠️ **「这把密钥这段时间一次都没被用过」确实是 `value` + 缺席**，那不是伪造：
/// 读成功了、区间里**有分片**、这一格就是没有。**「有分片」这半句是这条论证的支点**，
/// `no-shards` 那一档正好没有它。
#[must_use]
pub fn api_key_usage_kind(state: UsageState, by_api_key_present: bool) -> ApiKeyUsageKind {
    match state {
        UsageState::Off => ApiKeyUsageKind::Off,
        UsageState::Unavailable => ApiKeyUsageKind::Unknown,
        UsageState::NoShards => ApiKeyUsageKind::NoShards,
        UsageState::Empty | UsageState::Data => {
            // `byApiKey` 整块拿不到 ⇒ 「我们不知道」，**不是 0**。
            if by_api_key_present {
                ApiKeyUsageKind::Value
            } else {
                ApiKeyUsageKind::Unknown
            }
        }
    }
}

// ── 6. 保留伪 id ───────────────────────────────────────────────────────────

/// 后端那三个**保留伪 id**。它们不是密钥 id，是桶名。
pub const USAGE_MASTER_BUCKET: &str = "master";
/// 见 [`USAGE_MASTER_BUCKET`]。
pub const USAGE_UNATTRIBUTED_BUCKET: &str = "unattributed";
/// 见 [`USAGE_MASTER_BUCKET`]。
pub const USAGE_OTHER_BUCKET: &str = "__other__";

/// 按密钥那一维：一个桶键该显示成哪一条 i18n key。**认不出来返回 `None`
/// ⇒ 渲染层照实画那个原始 id。**
///
/// ⚠️⚠️ **只有这一维做 id → 名的映射，`byModel` 那一维刻意不做**：这一维的键是
/// **我们自己发的**（id 或那三个保留串），客户端选不了自己落进哪一格；而 `byModel`
/// 的 `__other__` **客户端可以直接在请求体里填** —— 在那一维做映射等于让任何一个
/// 客户端把自己的流量显示成「其它」。**两维差的是「谁能控制这个键」。**
///
/// ⚠️ **认不出来的 id 一律原样画，不猜、也不标「已删除」**：一个已经被删掉的密钥
/// 与一个「这一刻那张表读不出来」的密钥在面板手上**完全同形**。
#[must_use]
pub fn api_key_row_label_key(id: &str) -> Option<&'static str> {
    match id {
        USAGE_MASTER_BUCKET => Some("usage.key.master"),
        USAGE_UNATTRIBUTED_BUCKET => Some("usage.key.unattributed"),
        USAGE_OTHER_BUCKET => Some("usage.key.other"),
        _ => None,
    }
}

// ── 7. 小时表的行序 ────────────────────────────────────────────────────────

/// 小时分解表的**行序**。
///
/// ⚠️⚠️ **`hours` 的键是 `"0"`…`"23"`，而响应体里它躺在一个按字典序排的 map 里**
/// ⇒ `"10"` 会排在 `"2"` 前面。那不是「顺手排一下好看」：小时表按字典序读出来的
/// 是 0,1,10,11,…,19,2,20,… —— 一张**乱序的时间轴**。
///
/// ⚠️ **另外两维（模型 / 协议）刻意留在字典序**：那两维的键里会出现客户端填进来的
/// 任意串，而按值排序会让同一天的两次刷新在数字相等时给出不同的行序
/// （读的人以为数据变了）。**两种序都由后端给，渲染层不自己排。**
///
/// 解析不出数字的键（存储被写坏、或者将来多了一种键）**排在最后、内部保持字典序**，
/// 而不是被丢掉 —— 丢掉就是把「这里有一条我们读不懂的记录」这件事藏起来。
#[must_use]
pub fn hour_order<'a, I: IntoIterator<Item = &'a str>>(keys: I) -> Vec<String> {
    let mut out: Vec<&str> = keys.into_iter().collect();
    out.sort_unstable_by(|a, b| match (a.parse::<i64>(), b.parse::<i64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y).then_with(|| a.cmp(b)),
        // 认得的排在认不得的前面；两个都认不得时退回字典序。
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(_), Err(_)) => a.cmp(b),
    });
    out.into_iter().map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(tier: &'a str, shards: Option<usize>, malformed: Option<usize>) -> UsageFacts<'a> {
        UsageFacts {
            tier,
            days_present: true,
            total_requests: Some(0),
            shards,
            malformed,
            note: None,
        }
    }

    // ── malformed_kind ─────────────────────────────────────────────────────

    /// 两个字段里有一个读不出来 ⇒ **分片计数本身是未知的**，不许当成「干净」。
    ///
    /// 回归对象：把 `None` 当 0 兜底，于是「读失败」在这一层被洗成「全是好分片」。
    #[test]
    fn malformed_kind_is_unknown_when_either_count_is_missing() {
        assert_eq!(malformed_kind(None, Some(0)), MalformedKind::Unknown);
        assert_eq!(malformed_kind(Some(3), None), MalformedKind::Unknown);
        assert_eq!(malformed_kind(None, None), MalformedKind::Unknown);
    }

    /// 四档的判据逐条。🔴 **`all` 与 `partial` 的分界是 `shards == 0`**，不是别的；
    /// **「干净」与「有畸形」的分界是 `malformed == 0`**，同样不是别的。
    ///
    /// ⚠️ **`malformed == 1` 与 `shards == 1` 两行是变异复检补进来的**：
    /// 原表只有 0 / 2 / 5，于是把两处 `== 0` 分别写成 `<= 1`（很自然的「差一」笔误）
    /// 各有一次**全绿逃逸**。边界值要自己在表里出现，
    /// 不能指望「0 和 2 之间的都一样」。
    #[test]
    fn malformed_kind_splits_all_from_partial_on_the_shard_count() {
        assert_eq!(malformed_kind(Some(0), Some(0)), MalformedKind::Clean);
        assert_eq!(malformed_kind(Some(5), Some(0)), MalformedKind::Clean);
        assert_eq!(malformed_kind(Some(0), Some(2)), MalformedKind::All);
        assert_eq!(malformed_kind(Some(1), Some(2)), MalformedKind::Partial);
        // ⭐ 边界：只有一个畸形分片，照样是「有畸形」。
        assert_eq!(malformed_kind(Some(0), Some(1)), MalformedKind::All);
        assert_eq!(malformed_kind(Some(2), Some(1)), MalformedKind::Partial);
        // ⭐ 边界：只有一个好分片，那也是 partial 而不是 all。
        assert_eq!(malformed_kind(Some(1), Some(1)), MalformedKind::Partial);
    }

    /// 四个字面量互不相同 —— 两档共用一个串就等于面板分不出它们。
    #[test]
    fn malformed_kind_wire_values_are_pairwise_distinct() {
        let all = [
            MalformedKind::Unknown.as_str(),
            MalformedKind::Clean.as_str(),
            MalformedKind::All.as_str(),
            MalformedKind::Partial.as_str(),
        ];
        let mut seen: Vec<&str> = all.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), all.len(), "四个字面量必须互不相同：{all:?}");
    }

    // ── usage_state ────────────────────────────────────────────────────────

    /// Tier-2 关着**压过一切**：`days` 在不在、分片坏没坏都不改变这一档。
    #[test]
    fn usage_state_off_wins_over_everything_else() {
        assert_eq!(
            usage_state(&facts("off", Some(0), Some(9))),
            UsageState::Off
        );
        let mut f = facts("off", None, None);
        f.days_present = false;
        assert_eq!(usage_state(&f), UsageState::Off);
    }

    /// `days` 整块读不出来 ⇒ `unavailable`。**不是「这段时间没有请求」。**
    #[test]
    fn usage_state_without_days_is_unavailable() {
        let mut f = facts("tier2", None, None);
        f.days_present = false;
        f.total_requests = None;
        assert_eq!(usage_state(&f), UsageState::Unavailable);
    }

    /// 🔴 **读到了分片、每一个都坏 ⇒ `unavailable`，不是 `no-shards`。**
    ///
    /// 那一档下 `shards` 同样是 0，顺序反了就会把「读到的全是垃圾」说成
    /// 「还没写进去」——两句话指向完全相反的排查方向。
    #[test]
    fn usage_state_all_malformed_beats_no_shards() {
        let f = facts("tier2", Some(0), Some(4));
        assert_eq!(usage_state(&f), UsageState::Unavailable);
    }

    /// 一部分分片坏 ⇒ **数字是真的，只是不全** ⇒ 照常分档，不早退。
    #[test]
    fn usage_state_partial_malformed_does_not_early_return() {
        let mut f = facts("tier2", Some(3), Some(1));
        f.total_requests = Some(42);
        assert_eq!(usage_state(&f), UsageState::Data);
    }

    /// `total` 整块读不出来 ⇒ `unavailable`（即使 `days` 在）。
    #[test]
    fn usage_state_without_total_requests_is_unavailable() {
        let mut f = facts("tier2", Some(2), Some(0));
        f.total_requests = None;
        assert_eq!(usage_state(&f), UsageState::Unavailable);
    }

    /// 🔴 **`no_shards` 与 `empty` 的分界**：有分片 ⇒ 真的是 0；没分片 ⇒ 还没落盘。
    ///
    /// 回归对象：两者压成同一个「今日请求数 0」。
    #[test]
    fn usage_state_splits_not_landed_yet_from_a_real_zero() {
        assert_eq!(
            usage_state(&facts("tier2", Some(0), Some(0))),
            UsageState::NoShards
        );
        assert_eq!(
            usage_state(&facts("tier2", Some(7), Some(0))),
            UsageState::Empty
        );
    }

    /// ⚠️ **`range_clamped` 压过 `no_shards`**，所以区间被夹过时 `note` 不是
    /// `no_shards` —— 这一档只认 `note` 就会整个消失。`shards == 0` 那一半接住它。
    #[test]
    fn usage_state_no_shards_survives_a_clamped_range() {
        let mut f = facts("tier2", Some(0), Some(0));
        f.note = Some("range_clamped");
        assert_eq!(usage_state(&f), UsageState::NoShards);
    }

    /// 反过来：`shards` 读不出来但 `note` 明说 `no_shards` ⇒ 照样是这一档。
    #[test]
    fn usage_state_no_shards_also_reads_the_note() {
        let mut f = UsageFacts {
            tier: "tier2",
            days_present: true,
            total_requests: Some(0),
            shards: Some(3),
            malformed: Some(0),
            note: Some("no_shards"),
        };
        assert_eq!(usage_state(&f), UsageState::NoShards);
        f.note = None;
        assert_eq!(usage_state(&f), UsageState::Empty);
    }

    /// 🔴 **认不出来的 `tier` 退回 `empty`，不许归 `no-shards`。**
    ///
    /// 那一档换上去的那句话逐字点名了 Tier-2 的落盘机制；`tier` 认不出来时我们
    /// 并不知道那个部署按什么节奏落盘，照样说这句话就是拿一条我们没有的知识去
    /// 解释屏幕上的 0。
    /// ⚠️ Rust 侧今天只发得出 `off` / `tier2`，这一格钉的是**面板的态度**。
    #[test]
    fn usage_state_an_unrecognised_tier_never_claims_not_landed_yet() {
        assert_eq!(
            usage_state(&facts("tier3", Some(0), Some(0))),
            UsageState::Empty
        );
    }

    /// 有请求 ⇒ `data`，**排在 `no-shards` 之前**。
    #[test]
    fn usage_state_positive_requests_is_data() {
        let mut f = facts("tier2", Some(0), Some(0));
        f.total_requests = Some(1);
        assert_eq!(usage_state(&f), UsageState::Data);
    }

    /// 五个字面量互不相同。🔴 `no-shards` 是**连字符**（面板按它分支）。
    #[test]
    fn usage_state_wire_values_are_pairwise_distinct_and_use_hyphens() {
        let all = [
            UsageState::Off,
            UsageState::Unavailable,
            UsageState::NoShards,
            UsageState::Empty,
            UsageState::Data,
        ];
        let mut seen: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
        assert_eq!(UsageState::NoShards.as_str(), "no-shards");
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), all.len());
    }

    // ── detail_state ───────────────────────────────────────────────────────

    /// 单日下钻**没有 `empty` 这一档**：空 map 是「这一天没有记录」，照常是 `data`。
    #[test]
    fn detail_state_has_no_empty_tier() {
        assert_eq!(
            detail_state("tier2", true, Some(0), Some(0)),
            UsageState::Data
        );
    }

    /// `hours` 整块读不出来 ⇒ `unavailable`；全坏的分片同样 ⇒ `unavailable`。
    ///
    /// 回归对象：三张分解表说「这一天没有可以分解的记录」，而事实是那天的分片全坏了。
    #[test]
    fn detail_state_treats_all_malformed_as_unreadable() {
        assert_eq!(
            detail_state("tier2", false, Some(1), Some(0)),
            UsageState::Unavailable
        );
        assert_eq!(
            detail_state("tier2", true, Some(0), Some(5)),
            UsageState::Unavailable
        );
    }

    /// Tier-2 关着照样先说「关着」。
    #[test]
    fn detail_state_off_wins() {
        assert_eq!(detail_state("off", false, None, None), UsageState::Off);
    }

    // ── row_state / read_succeeded ─────────────────────────────────────────

    /// 🔴 **整块读不出来时每一行都读不出来**，即使这一行自己的桶是好的。
    ///
    /// 回归对象：顶部六张卡说「我们不知道」，紧挨着的日表把同一段区间写成「0 次」。
    #[test]
    fn row_state_propagates_the_block_level_verdict() {
        assert_eq!(
            row_state(UsageState::Unavailable, true),
            RowState::Unavailable
        );
        assert_eq!(row_state(UsageState::Off, true), RowState::Unavailable);
    }

    /// 整块好、这一行的桶读不回来 ⇒ **这一行我们不知道，不是这一行是 0**。
    #[test]
    fn row_state_a_missing_bucket_is_unknown_not_zero() {
        assert_eq!(row_state(UsageState::Data, false), RowState::Unavailable);
    }

    /// 🔴 **`no-shards` 原样往下传，不许压成 `data`。**
    ///
    /// 压成 `data` 不改变有数字那几格的结局，唯一的差别在那一列破折号的 tooltip 上
    /// ——而那句 tooltip 正是上一屏刚修掉的假话。
    #[test]
    fn row_state_keeps_no_shards_distinct_from_data() {
        assert_eq!(row_state(UsageState::NoShards, true), RowState::NoShards);
        assert_eq!(row_state(UsageState::Data, true), RowState::Data);
        assert_eq!(row_state(UsageState::Empty, true), RowState::Data);
    }

    /// 🔴 **白名单，不是黑名单。** 明天多一档时它默认倒向「我们不知道」。
    #[test]
    fn read_succeeded_is_a_whitelist() {
        assert!(read_succeeded(UsageState::Data));
        assert!(read_succeeded(UsageState::Empty));
        assert!(read_succeeded(UsageState::NoShards));
        assert!(!read_succeeded(UsageState::Off));
        assert!(!read_succeeded(UsageState::Unavailable));
    }

    // ── cell_kind / ratio_kind ─────────────────────────────────────────────

    /// 🔴 **`empty` 态下计数格写 `0`，不是破折号。** 把真零画成破折号是反向的撒谎。
    #[test]
    fn cell_kind_renders_a_real_zero_as_a_value() {
        assert_eq!(cell_kind(UsageState::Empty, Some(0)), CellKind::Value);
        assert_eq!(cell_kind(UsageState::NoShards, Some(0)), CellKind::Value);
    }

    /// 整块读不出来 ⇒ EM DASH 那一档，**即使这一格恰好有个数字**。
    #[test]
    fn cell_kind_unknown_wins_over_a_present_number() {
        assert_eq!(
            cell_kind(UsageState::Unavailable, Some(7)),
            CellKind::Unknown
        );
        assert_eq!(cell_kind(UsageState::Off, Some(7)), CellKind::Unknown);
    }

    /// 🔴 **第四档**：`no-shards` 下没有数字可写的那几格换一句 tooltip，字形不变。
    #[test]
    fn cell_kind_splits_the_no_shards_tooltip_from_the_plain_one() {
        assert_eq!(
            cell_kind(UsageState::NoShards, None),
            CellKind::NoneNoShards
        );
        assert_eq!(cell_kind(UsageState::Empty, None), CellKind::None);
        assert_eq!(cell_kind(UsageState::Data, None), CellKind::None);
    }

    /// 分母 `<= 0` 是「没有比率可算」，不是「比率是 0%」。
    #[test]
    fn ratio_kind_treats_a_zero_denominator_as_no_sample() {
        assert_eq!(ratio_kind(UsageState::Empty, Some(0)), CellKind::None);
        assert_eq!(ratio_kind(UsageState::Data, Some(-1)), CellKind::None);
        assert_eq!(ratio_kind(UsageState::Data, Some(1)), CellKind::Value);
    }

    /// `no-shards` 下比率卡白拿第四档 —— **不必在 `ratio_kind` 里再写一次判据。**
    #[test]
    fn ratio_kind_inherits_the_fourth_tier_for_free() {
        assert_eq!(
            ratio_kind(UsageState::NoShards, Some(0)),
            CellKind::NoneNoShards
        );
    }

    /// 四个字面量互不相同。
    #[test]
    fn cell_kind_wire_values_are_pairwise_distinct() {
        let all = [
            CellKind::Value,
            CellKind::None,
            CellKind::NoneNoShards,
            CellKind::Unknown,
        ];
        let mut seen: Vec<&str> = all.iter().map(|k| k.as_str()).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), all.len());
    }

    // ── avg_latency ────────────────────────────────────────────────────────

    /// **没有样本不等于零延迟。**
    #[test]
    fn avg_latency_without_samples_is_none() {
        assert_eq!(avg_latency(0, 0), None);
        assert_eq!(avg_latency(500, 0), None);
    }

    /// 坏数据（负的样本数）同样归 `None` —— 除以它会得到一个负的「平均延迟」。
    #[test]
    fn avg_latency_rejects_a_negative_sample_count() {
        assert_eq!(avg_latency(500, -3), None);
    }

    /// 四舍五入，与 TS 侧 `Math.round(sum / count)` 同语义。
    #[test]
    fn avg_latency_rounds_like_the_typescript_original() {
        assert_eq!(avg_latency(10, 4), Some(3)); // 2.5 → 3
        assert_eq!(avg_latency(9, 4), Some(2)); // 2.25 → 2
        assert_eq!(avg_latency(300, 3), Some(100));
    }

    // ── note_severity ──────────────────────────────────────────────────────

    /// 没有 `note` ⇒ 不出横幅。
    #[test]
    fn note_severity_without_a_note_shows_no_banner() {
        assert_eq!(note_severity(None), None);
    }

    /// 三档逐条。判据是「谁需要人去查」。
    #[test]
    fn note_severity_splits_go_look_at_storage_from_a_mere_hint() {
        for code in [
            "read_failed",
            "clock_unavailable",
            "all_malformed",
            "partial_malformed",
        ] {
            assert_eq!(
                note_severity(Some(code)),
                Some(NoteSeverity::Error),
                "{code}"
            );
        }
        assert_eq!(
            note_severity(Some("range_clamped")),
            Some(NoteSeverity::Warn)
        );
        for code in [
            "tier2_off",
            "no_shards",
            "date_out_of_retention",
            "no_request_detail",
        ] {
            assert_eq!(
                note_severity(Some(code)),
                Some(NoteSeverity::Info),
                "{code}"
            );
        }
    }

    /// 🔴 **表外的 code 归 `warn`** —— 不当成常态（`info`），也不把面板变成一片红（`error`）。
    #[test]
    fn note_severity_of_an_unknown_code_is_warn() {
        assert_eq!(
            note_severity(Some("some_code_the_backend_added_later")),
            Some(NoteSeverity::Warn)
        );
    }

    // ── api_key_usage_kind ─────────────────────────────────────────────────

    /// 🔴 **`off` 不许并进 `value` 的 0**：那是把「没开」说成「没人用」。
    #[test]
    fn api_key_usage_kind_keeps_tier2_off_separate_from_a_real_zero() {
        assert_eq!(
            api_key_usage_kind(UsageState::Off, true),
            ApiKeyUsageKind::Off
        );
        assert_eq!(
            api_key_usage_kind(UsageState::Empty, true),
            ApiKeyUsageKind::Value
        );
    }

    /// 🔴 **`no-shards` 也不许并进 `value` 的 0**：这一屏上没有横幅替它说话。
    ///
    /// 回归对象：TS 侧这个函数曾经是黑名单，于是新分出来的那一档默认落进「就是 0」。
    #[test]
    fn api_key_usage_kind_is_a_whitelist_not_a_blacklist() {
        assert_eq!(
            api_key_usage_kind(UsageState::NoShards, true),
            ApiKeyUsageKind::NoShards
        );
        assert_eq!(
            api_key_usage_kind(UsageState::Unavailable, true),
            ApiKeyUsageKind::Unknown
        );
    }

    /// `byApiKey` 整块拿不到 ⇒ **「我们不知道」，不是 0。**
    #[test]
    fn api_key_usage_kind_without_the_map_is_unknown() {
        assert_eq!(
            api_key_usage_kind(UsageState::Data, false),
            ApiKeyUsageKind::Unknown
        );
    }

    // ── 保留伪 id ──────────────────────────────────────────────────────────

    /// 三个保留桶各有一条文案；**别的 id 一律 `None`（照实画那个原始 id）。**
    #[test]
    fn api_key_row_label_key_never_guesses_an_unknown_id() {
        assert_eq!(
            api_key_row_label_key(USAGE_MASTER_BUCKET),
            Some("usage.key.master")
        );
        assert_eq!(
            api_key_row_label_key(USAGE_UNATTRIBUTED_BUCKET),
            Some("usage.key.unattributed")
        );
        assert_eq!(
            api_key_row_label_key(USAGE_OTHER_BUCKET),
            Some("usage.key.other")
        );
        assert_eq!(api_key_row_label_key("a1b2c3d4e5f6"), None);
        assert_eq!(api_key_row_label_key("other"), None, "少两个下划线就不是它");
    }

    // ── hour_order ─────────────────────────────────────────────────────────

    /// 🔴 **小时表走数值序**：字典序会把 `"10"` 排在 `"2"` 前面 = 一张乱序的时间轴。
    #[test]
    fn hour_order_sorts_numerically_not_lexicographically() {
        let keys = ["0", "1", "2", "10", "11", "20", "23", "9"];
        let got = hour_order(keys);
        assert_eq!(
            got,
            vec!["0", "1", "2", "9", "10", "11", "20", "23"],
            "字典序会给出 0,1,10,11,2,20,23,9"
        );
    }

    /// 解析不出数字的键**排在最后并保留下来**，不是被悄悄丢掉。
    ///
    /// 丢掉就是把「这里有一条我们读不懂的记录」这件事藏起来。
    #[test]
    fn hour_order_keeps_unparseable_keys_at_the_tail() {
        let got = hour_order(["5", "zz", "3", "aa"]);
        assert_eq!(got, vec!["3", "5", "aa", "zz"]);
    }

    /// 空输入给空结果（不是 24 格补零 —— 那是渲染层的事）。
    #[test]
    fn hour_order_of_nothing_is_nothing() {
        let got = hour_order(std::iter::empty::<&str>());
        assert!(got.is_empty());
    }
}
