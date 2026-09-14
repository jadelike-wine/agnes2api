//! 对外 API 密钥（我们**签发**、别人拿来向我们证明身份的那一族）的纯核
//! （`admin/api-keys.ts`，01 §8.1）。
//!
//! ⚠️⚠️ **本仓有两个都叫 key 的东西，方向相反，别读混**：
//! - **上游 key 池**（[`crate::core::keypool`]，存储 `key:<id>` + `pool:index`，
//!   端点 `/admin/api/keys`）是**我们持有的**、拿去向 Agnes 证明身份的凭据，
//!   它**必须存明文**（要拿去用）；
//! - **本文件这一族**（单个顶层存储键 `apikeys`，端点 `/admin/api/apikeys`）是
//!   **我们签发的**。
//!
//! 路径刻意不共用：`/admin/api/keys` 已经是上游池的**已发布公开契约**，改它等于破坏
//! 既有 API。代价是两个名字很像，**缓解手段是文案与文档，不是去改既有路径**。
//!
//! # 这个文件里没有什么
//!
//! **签发（铸币）不在这里** —— 那要 CSPRNG，而「同样的输入永远给同样的输出」正是
//! 零 IO 岛存在的理由。铸币住在 `apikey/`（那一层本来就允许 IO），
//! 本文件只提供它要用的纯函数：[`digest`] / [`hint_of`] / 三条校验。

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// 明文密钥的前缀。**它是我们自己发的常量，不是从明文里存下来的一段。**
///
/// ⚠️ 写清这一点是因为掩码（[`mask_of`]）看起来像是「前缀 + 圆点 + 末四位」，
/// 而那个前缀**并不来自记录** —— 记录里只有 `hint`。
/// 下一个人很容易顺手去把明文的前几位也存进记录里好让掩码「更真」，
/// 那是**多存一段可泄漏的东西，收益为零**。
///
/// ⚠️ 名字刻意不叫 `API_KEY_PREFIX`：TS 那边有一道「源码里每一个 `*_KEY` /
/// `*_KEY_PREFIX` 常量都要在存储键登记里逐把表态」的扫描，
/// 而这个常量**根本不是存储键**。Rust 侧若要落同一道门禁，同一条理由照旧成立。
pub const APIKEY_SECRET_PREFIX: &str = "sk-";

/// 明文密钥的随机部分有多少字节。**16 = 128 bit。**
///
/// 🟢 取 128 位而不是 256 是有论证的：它是**均匀随机**的（不是人选的口令），
/// 128 位在离线爆破面前已经远超任何可行算力，而每多 16 字节就是每一条
/// `Authorization` 头多 32 个字符。
/// **这个数同时决定了 [`digest`] 那条「无盐单轮 SHA-256 是正确选择」的论证前提，
/// 改小它之前先读那段。**
pub const API_KEY_SECRET_BYTES: usize = 16;

/// 记录里留下的明文末尾位数，掩码用。
pub const API_KEY_HINT_LENGTH: usize = 4;

/// 一张表最多几把。
///
/// 🟢 取 200：整张表是**一个**存储值，每条记录约 200 B ⇒ 200 把 ≈ 40 KB。
/// 再大就该考虑分片，而**分片会把「一次刷新 = 1 次 get」这条本设计最要紧的性质破坏掉**。
/// **超了就 400，不静默截断。**
pub const APIKEY_MAX: usize = 200;

/// 名称长度上限。它进每一条记录，而整张表在每一次写里被**整体重写**。
pub const API_KEY_NAME_MAX: usize = 64;

/// 一条对外 API 密钥。**七个字段，鉴权热路径零存储写。**
///
/// ⚠️ kiro2api 那边是十一个字段，砍掉的四个各有各的理由：
/// `spendingLimit` / `limitUnit` 做不出跨实例正确的上限、
/// `durationDays` / `activatedAt` 的惰性激活要**在鉴权热路径上写存储**、
/// `boundCredentialIds` 在本仓**没有对应概念**。
/// **它们不是「留了个恒为空的格」，是这个结构里根本没有那几格。**
/// 👉 P2 对标面板时，这四格是明确的「不做」清单，**别照抄 UI 把空格子画出来**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyRecord {
    /// 12 位十六进制，**独立随机、不由密钥派生**。
    ///
    /// 不用自增号：自增要在表里维护一个 `next_id`，而那个数一旦被覆写就会**退号**，
    /// 退号 = **新密钥继承前任的身份**（用量桶、事件日志里的那一行）。
    /// 随机 id 天然不复用、零状态。代价是没有 `#001` 这种好念的编号
    /// ⇒ 面板上的行号由 [`ApiKeyView::seq`] 现算，**而 `seq` 不是身份**。
    ///
    /// **不由密钥派生**是安全要求：id 会进 URL、进事件日志，
    /// 那些地方不该携带密钥的任何函数值。
    pub id: String,
    /// 运维起的名字。**允许重名**（「编号冲突检查」是自增 id 的遗产，随 id 一起去掉）。
    pub name: String,
    /// 签发时的明文，供面板「点击显示 / 复制」。
    ///
    /// 🔴🔴 **这是一次由用户拍板的、以安全性换便利性的取舍（2026-09-10）。**
    /// 本仓从第一天起的姿态是「我们签发、只用来验证别人的这一族**根本不需要**可逆存」，
    /// 那条论证**在技术上依然成立**。改成存明文之后：
    /// - **面板一旦被打穿，所有客户端密钥的明文一次性泄漏**（从前泄漏的只是摘要，
    ///   而 128 位均匀随机 + SHA-256 是不可反推的）；
    /// - 存储介质从「不含可直接使用的凭据」变成「含」，**备份与快照的处置级别要跟着升**
    ///   （这一条要写进部署文档）。
    ///
    /// ⚠️ **可选字段，别当成必然存在**：升级之前签发的记录**没有**这一格 ——
    /// 它们的明文在当初那次 201 响应之后就不存在于世界上了，任何代码都变不出来。
    /// 面板对这一档要照实说「此密钥签发于升级前，无明文可显示」，
    /// 🔴 **不许用空串或掩码冒充明文**。
    ///
    /// 🔴 **它永不进 [`ApiKeyView`]**（那是投影，投影里一个字节明文都没有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// 明文的 SHA-256，64 位十六进制小写。**验证依据。**
    pub hash: String,
    /// 明文末四位。签发那一刻算一次。
    pub hint: String,
    /// 缺席 = 启用。可选的理由与 [`KeyRecord::disabled`](crate::core::types::KeyRecord)
    /// 逐字相同。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    pub created_at: i64,
    /// `None` = 不过期。**必填可空**（序列化成 `null`，不许 skip）。
    pub expires_at: Option<i64>,
}

/// 存储里那个 `apikeys` 键的整份内容。
///
/// `version` 每次写加一，供**乐观并发**用：写之前回读一次（那一次 get 本来就要付），
/// 调用方手上的版本与读回来的对不上 ⇒ **409**。
///
/// ⚠️ **它不是 CAS。** 存储没有 CAS，读到写之间仍有一个窗口；这条机制把窗口从
/// 「整个人类操作时长」压到「一次读写之间」，
/// **残余窗口确实存在，别在任何一处把它说成「不会丢更新」。**
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApiKeyTable {
    /// ⚠️ 取 `i64` 而不是 `u64`：TS 侧是裸 `number`，一份手工编辑过的存储里出现负数
    /// 是可能的。**宁可原样收下也不要把整张表判成 `Invalid`** ——
    /// 后者的代价是写侧一律 409，而这一格只用来做等值比较。
    pub version: i64,
    pub keys: Vec<ApiKeyRecord>,
}

impl ApiKeyTable {
    /// 空表。**新建时 `version` 从 0 起**，第一次写落地的就是 1。
    pub fn empty() -> Self {
        Self {
            version: 0,
            keys: Vec::new(),
        }
    }
}

/// 明文的摘要。🔴 **全仓唯一一份** —— 签发时算一次存进 `hash`，鉴权时算一次拿去查表；
/// 两处必须是同一个函数，否则「同一把密钥」在两条路径上会得出两个值。
///
/// # 为什么是**无盐、单轮 SHA-256**，而不是 bcrypt / argon2
///
/// **这不是偷懒，是这一族的正确选择，别把「密码要用慢哈希」这条常识套上来：**
/// 1. 密钥是 128 位均匀随机（我们生成的，不是人选的口令）⇒ **没有字典可爆破**，
///    慢哈希想抵抗的那件事在这里不存在；
/// 2. 慢哈希**不可查表**：每请求都要拿明文对全表逐条算一遍，200 把就是 200 次 KDF ——
///    它跑在**鉴权热路径**上，几十毫秒的同步 CPU 是不可接受的。
///    单轮摘要则是「算一次、查一次表」；
/// 3. 无盐是可查表的前提，而它在这里不损失什么：盐防的是「同一个口令在两处的摘要相同」，
///    而这一族里两把密钥相同的概率是 2⁻¹²⁸。
///
/// 🔴 **不许改用 argon2 / bcrypt，除非先推翻上面三条。**
pub fn digest(secret: &str) -> String {
    let out = Sha256::digest(secret.as_bytes());
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// 明文末四位。**签发那一刻算一次。**
///
/// ⚠️ 按 `char` 取，不按字节 —— 明文是我们自己生成的十六进制，理论上全是 ASCII，
/// 但这个函数**也被面板的「补一个 hint」路径调到**，而那一侧的输入不受控。
pub fn hint_of(secret: &str) -> String {
    let n = secret.chars().count();
    secret
        .chars()
        .skip(n.saturating_sub(API_KEY_HINT_LENGTH))
        .collect()
}

/// 掩码：`sk-••••••••` + 末四位。
///
/// ⚠️ **`hint` 读不出来时画一根破折号「—」，不画一串光秃秃的圆点**：
/// 一串圆点会被读成「配了一把很短的口令」，而真相是「**这条记录里那一格坏了**」。
///
/// ⚠️ **不复用 [`mask_key`](crate::core::keys::view::mask_key)**：
/// 那个要明文的前 5 位，而我们**不存前 5 位，也不打算存**。
pub fn mask_of(hint: Option<&str>) -> String {
    match hint {
        Some(h) if !h.is_empty() => format!("{APIKEY_SECRET_PREFIX}••••••••{h}"),
        _ => "—".to_string(),
    }
}

/// 停用与否。**全仓唯一一份判据。**
///
/// ⚠️ 与上游池那边的 [`is_disabled`](crate::core::keypool::is_disabled) **不同**：
/// 这里是 `== Some(true)` 而不是真值性。两边不对称是 TS 原样如此，
/// 而它在这里说得通：上游 key 的 `disabled` 是**安全开关**（读不懂就当停用是安全侧），
/// 这一族的 `disabled` 读不懂却当停用的话，**客户端会当场 401 而没人知道为什么**。
/// 窄化那一侧同样按这个方向：非布尔的 `disabled` 让**整条记录**判 `Invalid`
/// （见 [`narrow_record`]），而不是悄悄当成停用。
pub fn is_api_key_disabled(r: &ApiKeyRecord) -> bool {
    r.disabled == Some(true)
}

/// 过期与否。**边界取 `now >= expires_at`**：`expires_at` 是「有效期到此为止」的那一刻，
/// 那一毫秒本身**已经在期外**。`None` 恒不过期。
pub fn is_api_key_expired(r: &ApiKeyRecord, now: i64) -> bool {
    r.expires_at.is_some_and(|e| now >= e)
}

/// 这把此刻能不能用。**鉴权那条路径上唯一的判据**，面板的分档从它派生。
pub fn is_api_key_usable(r: &ApiKeyRecord, now: i64) -> bool {
    !is_api_key_disabled(r) && !is_api_key_expired(r, now)
}

/// 分档。**顺序即优先级：`disabled > expired > active`。**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyBucket {
    Disabled,
    Expired,
    Active,
}

impl ApiKeyBucket {
    /// 对外的字面量。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Expired => "expired",
            Self::Active => "active",
        }
    }
}

/// 三个档次的闭集，**顺序即优先级**。
pub const API_KEY_BUCKETS: [ApiKeyBucket; 3] = [
    ApiKeyBucket::Disabled,
    ApiKeyBucket::Expired,
    ApiKeyBucket::Active,
];

/// 见 [`ApiKeyBucket`]。
///
/// ⚠️ **停用排在过期前面**：停用是**人做过的一个决定**，而过期是时间到了。
/// 一把既被停用又已过期的密钥在面板上该说「已停用」—— 那是运维要找的那条线索；
/// 说「已过期」会让他以为**只要把日期往后挪就能恢复**。
pub fn api_key_bucket(r: &ApiKeyRecord, now: i64) -> ApiKeyBucket {
    if is_api_key_disabled(r) {
        ApiKeyBucket::Disabled
    } else if is_api_key_expired(r, now) {
        ApiKeyBucket::Expired
    } else {
        ApiKeyBucket::Active
    }
}

/// 定长常数时间比较。
///
/// ⚠️⚠️ **常数时间这个性质无法用单元测试证明，原因是它根本不在返回值里**：
/// 被测的性质是「**耗时**不随输入而变」，而测试只能断言返回值。
/// 把整个函数换成 `a == b`、或把循环体改成提前 return，**返回值逐点相同**
/// （TS 侧已实测：两条变异全套测试照样绿）。
/// 不写计时断言是因为在 CI 上测不准 —— **写出来只会是一条随机红绿的用例，那比没有更糟**。
/// ⇒ **这一条明确不由测试保证，而由评审保证。**
///
/// 🔴 **「长度不同立刻 false」这条与 TS 逐字一致，且必须显式写出来**
/// （01 §15.8 点名要求）：`subtle::ConstantTimeEq` 对 `[u8]` 切片的实现
/// **本身就要求等长** —— 不等长时它返回 `Choice(0)`，但**长度差异会经由迭代次数泄漏**。
/// 所以这里先做一次显式的长度比较再进 `ct_eq`，而不是指望它替我们处理。
/// 长度泄漏是**标准取舍**：被它比较的两侧都是随机串
/// （管理口令 ≥24 位、摘要恒 64 位十六进制）。
pub fn constant_time_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

/// 按摘要查表。🔴 **常数时间比较，且遍历全表不在命中处提前 return。**
///
/// 两条性质分别挡两维泄漏：
/// - 摘要本身已经抹平「与某把密钥前多少字节相同」那一维（攻击者控制的是明文，
///   而明文一位之差会让摘要整体变样）；
/// - **表内位置**那一维靠「不短路」挡：在命中处 return 会让耗时随「命中的是第几条」变化，
///   于是**一个手里已经有一把有效密钥的人能量出别人那把排在自己前面还是后面**。
///
/// ⚠️ 循环里出现任何 `break` / `return` / `?` / `&&` / `||`（作为控制流）都是回归。
/// 🔴 **不许用 `.iter().find()`** —— 它天然短路。
/// 下面那个 `if ... else` 是**赋值**不是短路控制流：两条分支都只取一个已经算好的值，
/// 没有任何一侧会跳过后面的循环轮次。
pub fn find_by_digest<'a>(keys: &'a [ApiKeyRecord], d: &str) -> Option<&'a ApiKeyRecord> {
    let mut found: Option<&ApiKeyRecord> = None;
    for r in keys {
        let hit = constant_time_equal(&r.hash, d);
        found = if hit { Some(r) } else { found };
    }
    found
}

/// 面板上一条密钥卡的形状。🔴 **这个结构永不含明文，也永不含摘要。**
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyView {
    pub id: String,
    pub name: String,
    /// 面板上的行号。按 `created_at` 升序、`id` 破平。
    /// ⚠️ **它随删除而变，不是身份**：要指认一条记录只能用 `id`。
    pub seq: usize,
    pub masked: String,
    /// 末四位。面板的搜索要匹配它，所以单独给一格，而不是让前端去拆 `masked`。
    pub hint: String,
    pub bucket: ApiKeyBucket,
    /// **恒是布尔、恒存在。**
    pub disabled: bool,
    pub created_at: i64,
    pub expires_at: Option<i64>,
}

/// 投影成面板要的形状，**并按 `seq` 的顺序返回**（理由同
/// [`to_key_views`](crate::core::keys::view::to_key_views)）。
pub fn to_api_key_views(records: &[ApiKeyRecord], now: i64) -> Vec<ApiKeyView> {
    let mut sorted: Vec<&ApiKeyRecord> = records.iter().collect();
    sorted.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    sorted
        .into_iter()
        .enumerate()
        .map(|(i, r)| ApiKeyView {
            id: r.id.clone(),
            name: r.name.clone(),
            seq: i + 1,
            masked: mask_of(Some(r.hint.as_str())),
            hint: r.hint.clone(),
            bucket: api_key_bucket(r, now),
            disabled: is_api_key_disabled(r),
            created_at: r.created_at,
            expires_at: r.expires_at,
        })
        .collect()
}

/// 统计卡的四个数。
///
/// ⚠️ **判据只看 `v.bucket`**，不再各自重算一遍。
/// ⚠️ **没有 pending 那一档**（kiro2api 那边有）：惰性激活本期不做，
/// **画一张恒为 0 的卡就是撒谎**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ApiKeyCounts {
    pub all: usize,
    pub active: usize,
    pub disabled: usize,
    pub expired: usize,
}

/// 见 [`ApiKeyCounts`]。
pub fn api_key_counts(views: &[ApiKeyView]) -> ApiKeyCounts {
    let mut c = ApiKeyCounts {
        all: views.len(),
        active: 0,
        disabled: 0,
        expired: 0,
    };
    for v in views {
        match v.bucket {
            ApiKeyBucket::Disabled => c.disabled += 1,
            ApiKeyBucket::Expired => c.expired += 1,
            ApiKeyBucket::Active => c.active += 1,
        }
    }
    c
}

/// 搜索。🔴 **只匹配名称、掩码的可见部分与 id，绝不匹配摘要。**
///
/// 匹配摘要等于把「没有 reveal 端点」这条保证降级成一个慢速预言机 ——
/// 虽然摘要本身不可逆，但**一个能按摘要前缀筛选的接口让「这把密钥在不在表里」
/// 变得可批量试探**。
pub fn matches_api_key_query(v: &ApiKeyView, q: &str) -> bool {
    let s = q.trim().to_lowercase();
    if s.is_empty() {
        return true;
    }
    v.name.to_lowercase().contains(&s)
        || v.masked.to_lowercase().contains(&s)
        || v.id.to_lowercase().contains(&s)
}

/// 排序档。**闭集是这个数组，类型从它派生。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeySort {
    /// 新 → 旧。
    New,
    /// 旧 → 新。
    Old,
    /// 按名称。
    Name,
}

/// 三个排序档的字面量（面板的查询参数值）。
pub const API_KEY_SORTS: [&str; 3] = ["new", "old", "name"];

impl ApiKeySort {
    /// 查询参数 → 档位。**认不出来回 `None`**（调用方决定是 400 还是回落默认）。
    pub fn parse(v: &str) -> Option<Self> {
        match v {
            "new" => Some(Self::New),
            "old" => Some(Self::Old),
            "name" => Some(Self::Name),
            _ => None,
        }
    }
}

/// 排序。
///
/// ⚠️ **`new` / `old` 一律拿 `seq` 破平**，不拿别的东西 ——
/// 同一毫秒签发的两把在两次刷新之间必须落在同一个位置，否则列表会自己跳。
///
/// ⚠️ **一处对 TS 的刻意偏离，登记在此**：TS 用 `localeCompare`（区域感知的排序规则），
/// Rust 标准库**没有**对应物，引 ICU 是一整条依赖。这里用 `str::cmp`（码位序）。
/// 可观察差异：中文名 / 带重音的名字**排序顺序会与 TS 版不同**。
/// 它只影响面板列表的呈现顺序，**不影响任何判据与任何身份**；
/// 真要对齐得先决定用哪套 collation。
pub fn sort_api_key_views(views: &[ApiKeyView], sort: ApiKeySort) -> Vec<ApiKeyView> {
    let mut out = views.to_vec();
    match sort {
        ApiKeySort::Name => out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.seq.cmp(&b.seq))),
        ApiKeySort::Old => out.sort_by_key(|v| v.seq),
        ApiKeySort::New => out.sort_by_key(|v| std::cmp::Reverse(v.seq)),
    }
    out
}

// ── 校验 ────────────────────────────────────────────────────────────────────

/// 名称的校验结果。**闭集**；这里不产生任何一句用户文案（文案住在五语言字典里）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyNameProblem {
    NameNotAString,
    NameEmpty,
    NameTooLong,
}

impl ApiKeyNameProblem {
    /// 与管理接口错误码一一对应的那个码。
    pub fn code(self) -> &'static str {
        match self {
            Self::NameNotAString => "name_not_a_string",
            Self::NameEmpty => "name_empty",
            Self::NameTooLong => "name_too_long",
        }
    }
}

/// 名称校验。
///
/// ⚠️ **先判空再判长**：一个空名字同时不满足「非空」，
/// 报「太长」会把人指向**反方向**。
pub fn check_api_key_name(v: Option<&Value>) -> Option<ApiKeyNameProblem> {
    let Some(Value::String(s)) = v else {
        return Some(ApiKeyNameProblem::NameNotAString);
    };
    if s.trim().is_empty() {
        return Some(ApiKeyNameProblem::NameEmpty);
    }
    // ⚠️ 用**字符数**而不是字节数：64 字节的上限对中文名意味着 21 个字 ——
    // 而运维看到的是「最多 64 个字符」。TS 的 `String.length` 是 UTF-16 码元数，
    // 对 BMP 内的中文与 `chars().count()` 一致（差异只在 emoji 这类代理对上）。
    if s.chars().count() > API_KEY_NAME_MAX {
        return Some(ApiKeyNameProblem::NameTooLong);
    }
    None
}

/// 到期时刻的校验结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyExpiryProblem {
    ExpiresNotANumber,
    ExpiresInThePast,
}

impl ApiKeyExpiryProblem {
    /// 与管理接口错误码一一对应的那个码。
    pub fn code(self) -> &'static str {
        match self {
            Self::ExpiresNotANumber => "expires_not_a_number",
            Self::ExpiresInThePast => "expires_in_the_past",
        }
    }
}

/// 到期时刻校验。`null` 是**合法输入**（不过期），不是「没填」。
///
/// **过去的时刻当场拒**：签发一把生下来就过期的密钥不是一个有意义的形态，
/// 而它在面板上长得与「签发成功」一模一样（列表里多一行「已过期」），
/// **运维会以为自己按错了什么**。
///
/// ⚠️ 判据是 `<= now` 而不是 `< now`：等于 `now` 的那一刻按 [`is_api_key_expired`]
/// 已经在期外，**两处必须用同一条边界**，否则会造出一个
/// 「创建时合法、下一行就过期」的洞。
pub fn check_api_key_expires_at(v: Option<&Value>, now: i64) -> Option<ApiKeyExpiryProblem> {
    match v {
        None | Some(Value::Null) => None,
        Some(x) => match x.as_f64() {
            Some(n) if n.is_finite() && n.fract() == 0.0 => {
                if (n as i64) <= now {
                    Some(ApiKeyExpiryProblem::ExpiresInThePast)
                } else {
                    None
                }
            }
            _ => Some(ApiKeyExpiryProblem::ExpiresNotANumber),
        },
    }
}

// ── 窄化（裁定 B） ──────────────────────────────────────────────────────────

/// 读回来的 `apikeys` 到底是什么。**三档，🔴 不许合并成两档。**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyTableRead {
    /// 存储里压根没有这个键。**这是一份合法快照**（还没签发过任何密钥），
    /// holder 要按 TTL 缓存它 —— 那条是「1000 次错口令只许产生 1 次存储读」的前提。
    Absent,
    /// 认得出来的一张表。
    Ok(ApiKeyTable),
    /// 读得到、但结构不认。🔴 **绝不当空表**：
    /// 当空表的后果是**全部客户 401**，而**下一次面板写会把幸存记录整份覆掉，不可逆**。
    Invalid,
}

/// 一条记录的窄化。**坏一条 = 整张表不认**（见 [`parse_api_key_table`]）。
///
/// **不做逐条抢救**（kiro2api 那边有 `salvage_api_keys`）：本仓这张 blob 只有面板
/// 一个写者、没有「运维手工编辑」这条正当来源，而坏字节本身留在存储里**没有被任何
/// 一条路径覆盖过**（读路径零 put，写路径读到 `Invalid` 一律 409 拒绝落盘）
/// ⇒ **人工捞回随时可做**。
/// 抢救逻辑的代价是一整条「部分正确」的语义，**收益在本仓为零**。
pub fn narrow_record(v: &Value) -> Option<ApiKeyRecord> {
    let o = v.as_object()?;
    let s = |k: &str| o.get(k).and_then(Value::as_str);
    let id = s("id").filter(|x| !x.is_empty())?;
    let name = s("name")?;
    let hash = s("hash").filter(|x| !x.is_empty())?;
    let hint = s("hint")?;
    let disabled = match o.get("disabled") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        _ => return None,
    };
    // `secret` 是升级后才有的可选字段：**缺席合法**，但出现时必须是非空字符串 ——
    // 空串会被前端当成「有明文但是空的」渲染出去。
    let secret = match o.get("secret") {
        None | Some(Value::Null) => None,
        Some(Value::String(x)) if !x.is_empty() => Some(x.clone()),
        _ => return None,
    };
    let created_at = finite(o.get("createdAt")?)?;
    // ⚠️ `expiresAt` **必须在场**：`null` = 不过期，缺席 = 这条记录不完整。
    //    TS 侧逐字如此（`r.expiresAt !== null && typeof !== "number"` ⇒ 不认）。
    let expires_at = match o.get("expiresAt") {
        Some(Value::Null) => None,
        Some(x) => Some(finite(x)?),
        None => return None,
    };
    Some(ApiKeyRecord {
        id: id.to_string(),
        name: name.to_string(),
        secret,
        hash: hash.to_string(),
        hint: hint.to_string(),
        disabled,
        created_at,
        expires_at,
    })
}

/// 把读回来的裸值窄化成一张表。见 [`ApiKeyTableRead`] 那三档。
pub fn parse_api_key_table(raw: Option<&Value>) -> ApiKeyTableRead {
    let Some(raw) = raw else {
        return ApiKeyTableRead::Absent;
    };
    if raw.is_null() {
        return ApiKeyTableRead::Absent;
    }
    let Some(o) = raw.as_object() else {
        return ApiKeyTableRead::Invalid;
    };
    let Some(version) = o.get("version").and_then(finite) else {
        return ApiKeyTableRead::Invalid;
    };
    let Some(Value::Array(items)) = o.get("keys") else {
        return ApiKeyTableRead::Invalid;
    };
    let mut keys = Vec::with_capacity(items.len());
    for it in items {
        match narrow_record(it) {
            Some(r) => keys.push(r),
            // 🔴 坏一条 = 整张表不认。**绝不当空表、也绝不逐条抢救。**
            None => return ApiKeyTableRead::Invalid,
        }
    }
    ApiKeyTableRead::Ok(ApiKeyTable { version, keys })
}

/// 有限数字 → `i64`（截断）。非有限 / 非数字 ⇒ `None`。
fn finite(v: &Value) -> Option<i64> {
    match v.as_f64() {
        Some(n) if n.is_finite() => Some(n.trunc() as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_700_000_000_000;

    fn rec(id: &str, secret: &str) -> ApiKeyRecord {
        ApiKeyRecord {
            id: id.to_string(),
            name: format!("key-{id}"),
            secret: Some(secret.to_string()),
            hash: digest(secret),
            hint: hint_of(secret),
            disabled: None,
            created_at: NOW - 1_000,
            expires_at: None,
        }
    }

    // ── digest ─────────────────────────────────────────────────────────────

    /// 摘要是 64 位十六进制小写，且**同一个明文恒定**（签发与鉴权两条路必须对得上）。
    #[test]
    fn digest_is_stable_lowercase_hex() {
        let d = digest("sk-0123456789abcdef");
        assert_eq!(d.len(), 64);
        assert!(
            d.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
        assert_eq!(d, digest("sk-0123456789abcdef"));
        assert_ne!(d, digest("sk-0123456789abcdee"), "一位之差整体变样");
    }

    /// 对着一个已知向量钉住算法本身 —— 换成别的哈希会当场红
    /// （而「所有既有密钥一次性全部失效」是这个函数唯一的失败形态）。
    #[test]
    fn digest_is_plain_sha256_not_something_else() {
        assert_eq!(
            digest("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    // ── hint / mask ────────────────────────────────────────────────────────

    #[test]
    fn hint_takes_the_last_four_characters() {
        assert_eq!(hint_of("sk-0123456789ab"), "89ab");
        assert_eq!(hint_of("ab"), "ab", "比四位还短就全给（本来也没别的可给）");
        assert_eq!(hint_of(""), "");
    }

    /// ⚠️ **hint 读不出来时画破折号，不画光秃秃的圆点。**
    ///
    /// 一串圆点会被读成「配了一把很短的口令」，而真相是「这条记录里那一格坏了」。
    #[test]
    fn a_missing_hint_shows_a_dash_not_a_row_of_dots() {
        assert_eq!(mask_of(None), "—");
        assert_eq!(mask_of(Some("")), "—");
        assert_eq!(mask_of(Some("89ab")), "sk-••••••••89ab");
    }

    // ── 判据三件套 ─────────────────────────────────────────────────────────

    /// ⚠️ 这一族用 `== Some(true)`（与上游池那边的真值性**刻意不同**）。
    #[test]
    fn disabled_requires_an_explicit_true() {
        let mut r = rec("a", "sk-aaaa");
        assert!(!is_api_key_disabled(&r));
        r.disabled = Some(false);
        assert!(!is_api_key_disabled(&r));
        r.disabled = Some(true);
        assert!(is_api_key_disabled(&r));
    }

    /// 🔴 **过期的边界是 `now >= expires_at`**，且与
    /// [`check_api_key_expires_at`] 的 `<= now` 是**同一条线**。
    #[test]
    fn expiry_boundary_is_inclusive_on_both_sides() {
        let mut r = rec("a", "sk-aaaa");
        r.expires_at = Some(NOW);
        assert!(is_api_key_expired(&r, NOW), "到点那一刻已经在期外");
        assert!(!is_api_key_expired(&r, NOW - 1));
        // 同一条线：`now` 这个时刻在创建侧也是被拒的 ⇒ 不存在
        // 「创建时合法、下一行就过期」的洞。
        assert_eq!(
            check_api_key_expires_at(Some(&json!(NOW)), NOW),
            Some(ApiKeyExpiryProblem::ExpiresInThePast)
        );
        assert_eq!(check_api_key_expires_at(Some(&json!(NOW + 1)), NOW), None);
    }

    #[test]
    fn no_expiry_means_never_expires() {
        let r = rec("a", "sk-aaaa");
        assert!(!is_api_key_expired(&r, i64::MAX));
        assert!(is_api_key_usable(&r, i64::MAX));
    }

    /// ⚠️ **停用排在过期前面**：既停用又过期的那一把在面板上必须说「已停用」，
    /// 否则运维会以为把日期往后挪就能恢复。
    #[test]
    fn disabled_outranks_expired_in_the_bucket_order() {
        let mut r = rec("a", "sk-aaaa");
        r.disabled = Some(true);
        r.expires_at = Some(NOW - 1);
        assert_eq!(api_key_bucket(&r, NOW), ApiKeyBucket::Disabled);
        r.disabled = None;
        assert_eq!(api_key_bucket(&r, NOW), ApiKeyBucket::Expired);
        r.expires_at = None;
        assert_eq!(api_key_bucket(&r, NOW), ApiKeyBucket::Active);
    }

    #[test]
    fn bucket_literals_are_a_contract() {
        assert_eq!(
            API_KEY_BUCKETS.map(ApiKeyBucket::as_str),
            ["disabled", "expired", "active"]
        );
    }

    // ── constant_time_equal / find_by_digest ───────────────────────────────

    /// 返回值层面的正确性（**常数时间那一半由评审保证，见函数文档**）。
    #[test]
    fn constant_time_equality_agrees_with_plain_equality_on_values() {
        assert!(constant_time_equal("", ""));
        assert!(constant_time_equal("abc", "abc"));
        assert!(!constant_time_equal("abc", "abd"));
        assert!(!constant_time_equal("abc", "abcd"), "长度不同立刻 false");
        assert!(!constant_time_equal("abcd", "abc"));
    }

    /// 摘要命中就能查到，不命中就是 `None`。
    #[test]
    fn lookup_finds_the_matching_record() {
        let keys = vec![
            rec("a", "sk-aaaa"),
            rec("b", "sk-bbbb"),
            rec("c", "sk-cccc"),
        ];
        assert_eq!(
            find_by_digest(&keys, &digest("sk-bbbb")).map(|r| r.id.as_str()),
            Some("b")
        );
        assert!(find_by_digest(&keys, &digest("sk-zzzz")).is_none());
        assert!(find_by_digest(&[], &digest("sk-aaaa")).is_none());
    }

    /// 命中**表尾**与命中**表首**都要查得到 —— 这一格顺带钉住「循环真的走完了」
    /// （短路实现在表尾这一格上也会绿，所以它挡不住短路；
    /// 短路那一条由评审 + 函数文档里的禁令守着，这里不假装测到了）。
    #[test]
    fn lookup_works_at_both_ends_of_the_table() {
        let keys = vec![
            rec("a", "sk-aaaa"),
            rec("b", "sk-bbbb"),
            rec("c", "sk-cccc"),
        ];
        assert_eq!(
            find_by_digest(&keys, &digest("sk-aaaa")).map(|r| r.id.as_str()),
            Some("a")
        );
        assert_eq!(
            find_by_digest(&keys, &digest("sk-cccc")).map(|r| r.id.as_str()),
            Some("c")
        );
    }

    // ── 投影 ───────────────────────────────────────────────────────────────

    /// 🔴 **投影里既没有明文也没有摘要。**
    #[test]
    fn a_view_carries_neither_the_plaintext_nor_the_hash() {
        let keys = vec![rec("a", "sk-super-secret")];
        let json = serde_json::to_string(&to_api_key_views(&keys, NOW)).unwrap();
        assert!(!json.contains("super-secret"), "明文泄漏进面板响应");
        assert!(
            !json.contains(&digest("sk-super-secret")),
            "摘要泄漏进面板响应"
        );
        assert!(json.contains("sk-••••••••cret"));
    }

    /// 返回顺序与 `seq` 一致；`created_at` 相同时按 `id` 破平。
    #[test]
    fn views_are_returned_in_seq_order_with_id_as_tiebreak() {
        let mut a = rec("zz", "sk-aaaa");
        let mut b = rec("aa", "sk-bbbb");
        let mut c = rec("mm", "sk-cccc");
        a.created_at = 100;
        b.created_at = 100;
        c.created_at = 50;
        let views = to_api_key_views(&[a, b, c], NOW);
        assert_eq!(
            views.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
            ["mm", "aa", "zz"]
        );
        assert_eq!(views.iter().map(|v| v.seq).collect::<Vec<_>>(), [1, 2, 3]);
    }

    /// `disabled` 在投影里**恒是布尔、恒存在**。
    #[test]
    fn view_disabled_is_always_a_real_boolean() {
        let keys = vec![rec("a", "sk-aaaa")];
        assert_eq!(keys[0].disabled, None);
        let v = serde_json::to_value(&to_api_key_views(&keys, NOW)[0]).unwrap();
        assert_eq!(v.get("disabled"), Some(&json!(false)));
        assert_eq!(
            v.get("expiresAt"),
            Some(&json!(null)),
            "必填可空，不许 skip"
        );
    }

    /// 统计卡只看 bucket，且**没有 pending 那一档**。
    #[test]
    fn counts_have_exactly_three_buckets_and_no_pending() {
        let mut a = rec("a", "sk-aaaa");
        let mut b = rec("b", "sk-bbbb");
        let c = rec("c", "sk-cccc");
        a.disabled = Some(true);
        b.expires_at = Some(NOW - 1);
        let counts = api_key_counts(&to_api_key_views(&[a, b, c], NOW));
        assert_eq!(
            counts,
            ApiKeyCounts {
                all: 3,
                active: 1,
                disabled: 1,
                expired: 1
            }
        );
        let json = serde_json::to_value(counts).unwrap();
        assert!(json.get("pending").is_none(), "画一张恒为 0 的卡就是撒谎");
    }

    /// 🔴 **搜索绝不匹配摘要**（否则「这把在不在表里」就可批量试探）。
    #[test]
    fn search_never_matches_the_hash() {
        let keys = vec![rec("abc123", "sk-super-secret")];
        let v = &to_api_key_views(&keys, NOW)[0];
        let h = digest("sk-super-secret");
        assert!(!matches_api_key_query(v, &h[..8]), "🔴 摘要前缀不许可搜");
        assert!(!matches_api_key_query(v, "super-secret"), "明文同理");
        assert!(matches_api_key_query(v, "abc123"));
        assert!(matches_api_key_query(v, "key-abc"), "名称可搜");
        assert!(matches_api_key_query(v, "cret"), "掩码露出来的那一段可搜");
        assert!(matches_api_key_query(v, ""), "空查询放行全部");
    }

    /// `new` / `old` 用 `seq` 破平 —— 同一毫秒签发的两把不许在两次刷新之间换位置。
    #[test]
    fn sorting_is_stable_through_seq() {
        let mut a = rec("aa", "sk-aaaa");
        let mut b = rec("bb", "sk-bbbb");
        a.created_at = 100;
        b.created_at = 100;
        let views = to_api_key_views(&[a, b], NOW);
        let new = sort_api_key_views(&views, ApiKeySort::New);
        assert_eq!(new.iter().map(|v| v.seq).collect::<Vec<_>>(), [2, 1]);
        let old = sort_api_key_views(&views, ApiKeySort::Old);
        assert_eq!(old.iter().map(|v| v.seq).collect::<Vec<_>>(), [1, 2]);
        let by_name = sort_api_key_views(&views, ApiKeySort::Name);
        assert_eq!(
            by_name.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
            ["key-aa", "key-bb"]
        );
    }

    #[test]
    fn sort_parsing_is_a_closed_set() {
        assert_eq!(ApiKeySort::parse("new"), Some(ApiKeySort::New));
        assert_eq!(ApiKeySort::parse("old"), Some(ApiKeySort::Old));
        assert_eq!(ApiKeySort::parse("name"), Some(ApiKeySort::Name));
        assert_eq!(
            ApiKeySort::parse("New"),
            None,
            "闭集是字面量，不做大小写宽容"
        );
        assert_eq!(ApiKeySort::parse("newest"), None);
        assert_eq!(API_KEY_SORTS, ["new", "old", "name"]);
    }

    // ── 校验 ───────────────────────────────────────────────────────────────

    /// ⚠️ **先判空再判长**：空名字报「太长」会把人指向反方向。
    #[test]
    fn name_validation_reports_empty_before_too_long() {
        assert_eq!(check_api_key_name(Some(&json!("ok"))), None);
        assert_eq!(
            check_api_key_name(Some(&json!("   "))),
            Some(ApiKeyNameProblem::NameEmpty)
        );
        assert_eq!(
            check_api_key_name(Some(&json!(""))),
            Some(ApiKeyNameProblem::NameEmpty)
        );
        assert_eq!(
            check_api_key_name(Some(&json!(42))),
            Some(ApiKeyNameProblem::NameNotAString)
        );
        assert_eq!(
            check_api_key_name(None),
            Some(ApiKeyNameProblem::NameNotAString)
        );
        assert_eq!(
            check_api_key_name(Some(&json!("x".repeat(API_KEY_NAME_MAX)))),
            None,
            "恰好 64 是合法的"
        );
        assert_eq!(
            check_api_key_name(Some(&json!("x".repeat(API_KEY_NAME_MAX + 1)))),
            Some(ApiKeyNameProblem::NameTooLong)
        );
    }

    /// `null` 是合法输入（不过期），非整数 / 非数字是 `expires_not_a_number`。
    #[test]
    fn expiry_validation_accepts_null_and_rejects_non_integers() {
        assert_eq!(check_api_key_expires_at(Some(&json!(null)), NOW), None);
        assert_eq!(check_api_key_expires_at(None, NOW), None);
        assert_eq!(
            check_api_key_expires_at(Some(&json!("tomorrow")), NOW),
            Some(ApiKeyExpiryProblem::ExpiresNotANumber)
        );
        assert_eq!(
            check_api_key_expires_at(Some(&json!(1.5)), NOW),
            Some(ApiKeyExpiryProblem::ExpiresNotANumber)
        );
    }

    /// 错误码是**码**（文案住在五语言字典里），这一格钉住码本身。
    #[test]
    fn validation_problems_carry_codes_not_prose() {
        assert_eq!(ApiKeyNameProblem::NameEmpty.code(), "name_empty");
        assert_eq!(ApiKeyNameProblem::NameTooLong.code(), "name_too_long");
        assert_eq!(
            ApiKeyNameProblem::NameNotAString.code(),
            "name_not_a_string"
        );
        assert_eq!(
            ApiKeyExpiryProblem::ExpiresInThePast.code(),
            "expires_in_the_past"
        );
        assert_eq!(
            ApiKeyExpiryProblem::ExpiresNotANumber.code(),
            "expires_not_a_number"
        );
    }

    // ── 窄化（裁定 B） ─────────────────────────────────────────────────────

    /// 🔴 **`Absent` 与 `Invalid` 是两档，不许合并。**
    ///
    /// `Absent`（还没签发过）是一份**合法快照**，要按 TTL 缓存 ——
    /// 那是「1000 次错口令只许产生 1 次存储读」的前提。
    #[test]
    fn an_absent_table_is_a_valid_snapshot_not_an_error() {
        assert_eq!(parse_api_key_table(None), ApiKeyTableRead::Absent);
        assert_eq!(
            parse_api_key_table(Some(&json!(null))),
            ApiKeyTableRead::Absent
        );
    }

    /// 🔴 **结构不认的一律 `Invalid`，绝不当空表。**
    ///
    /// 回归对象：当空表 ⇒ **全部客户当场 401**，
    /// 而下一次面板写会把幸存记录**整份覆掉，不可逆**。
    #[test]
    fn a_broken_table_is_invalid_and_never_silently_empty() {
        for raw in [
            json!(7),
            json!("x"),
            json!([]),
            json!({}),                             // 没有 version
            json!({ "version": 1 }),               // 没有 keys
            json!({ "version": "1", "keys": [] }), // version 不是数
            json!({ "version": 1, "keys": {} }),   // keys 不是数组
        ] {
            assert_eq!(
                parse_api_key_table(Some(&raw)),
                ApiKeyTableRead::Invalid,
                "{raw}"
            );
        }
    }

    /// 一张正常的表读得回来，`secret` / `disabled` 的缺席都是合法的。
    #[test]
    fn a_healthy_table_narrows_cleanly() {
        let raw = json!({
            "version": 3,
            "keys": [
                { "id": "a1", "name": "n", "hash": "h", "hint": "89ab",
                  "createdAt": 100, "expiresAt": null },
                { "id": "a2", "name": "", "hash": "h2", "hint": "", "secret": "sk-x",
                  "disabled": true, "createdAt": 200, "expiresAt": 300 },
            ],
        });
        let ApiKeyTableRead::Ok(t) = parse_api_key_table(Some(&raw)) else {
            panic!("这张表是好的");
        };
        assert_eq!(t.version, 3);
        assert_eq!(t.keys.len(), 2);
        assert_eq!(t.keys[0].expires_at, None);
        assert_eq!(t.keys[0].secret, None, "升级前签发的记录没有明文");
        assert_eq!(t.keys[1].secret.as_deref(), Some("sk-x"));
        assert_eq!(t.keys[1].disabled, Some(true));
        assert_eq!(t.keys[1].expires_at, Some(300));
    }

    /// 🔴 **坏一条 = 整张表不认**（不做逐条抢救）。
    #[test]
    fn one_broken_record_invalidates_the_whole_table() {
        let bad_records = [
            json!({ "id": "", "name": "n", "hash": "h", "hint": "", "createdAt": 1, "expiresAt": null }),
            json!({ "id": "a", "name": 7, "hash": "h", "hint": "", "createdAt": 1, "expiresAt": null }),
            json!({ "id": "a", "name": "n", "hash": "", "hint": "", "createdAt": 1, "expiresAt": null }),
            json!({ "id": "a", "name": "n", "hash": "h", "hint": null, "createdAt": 1, "expiresAt": null }),
            json!({ "id": "a", "name": "n", "hash": "h", "hint": "", "createdAt": "x", "expiresAt": null }),
            // `expiresAt` 缺席 = 记录不完整（`null` 才是「不过期」）。
            json!({ "id": "a", "name": "n", "hash": "h", "hint": "", "createdAt": 1 }),
            // `disabled` 出现时必须是布尔 —— 这一族**不做**「读不懂就当停用」。
            json!({ "id": "a", "name": "n", "hash": "h", "hint": "", "disabled": "true",
                    "createdAt": 1, "expiresAt": null }),
            // `secret` 出现时必须是非空字符串（空串会被前端渲染成「有明文但是空的」）。
            json!({ "id": "a", "name": "n", "hash": "h", "hint": "", "secret": "",
                    "createdAt": 1, "expiresAt": null }),
        ];
        for bad in bad_records {
            assert!(narrow_record(&bad).is_none(), "{bad} 该被判坏");
            assert_eq!(
                parse_api_key_table(Some(&json!({ "version": 1, "keys": [bad] }))),
                ApiKeyTableRead::Invalid
            );
        }
    }

    /// `keys: []` 是**合法**的空表（签发过又全删了），与 `Absent` 都不是 `Invalid`。
    #[test]
    fn an_explicitly_empty_key_list_is_valid() {
        let ApiKeyTableRead::Ok(t) =
            parse_api_key_table(Some(&json!({ "version": 9, "keys": [] })))
        else {
            panic!("空列表是合法的");
        };
        assert_eq!(t.version, 9);
        assert!(t.keys.is_empty());
    }

    /// 记录写出去的形状，窄化必须读得回来（存储往返闭合）。
    #[test]
    fn records_round_trip_through_serialize_and_narrow() {
        let r = rec("a1", "sk-0123456789ab");
        let raw = serde_json::to_value(&r).unwrap();
        assert_eq!(
            narrow_record(&raw).as_ref(),
            Some(&r),
            "写出去的必须读得回来"
        );
    }

    #[test]
    fn an_empty_table_starts_at_version_zero() {
        let t = ApiKeyTable::empty();
        assert_eq!(t.version, 0, "第一次写落地的就是 1");
        assert!(t.keys.is_empty());
    }
}
