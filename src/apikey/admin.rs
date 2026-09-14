//! `/admin/api/apikeys*` —— **我们签发**的那一族 `sk-*` 的六条端点（02 §9.25 ~ §9.30）。
//!
//! ⚠️⚠️ **`/admin/api/keys` 与 `/admin/api/apikeys` 是两回事，别读混**：
//! 前者是**我们持有的上游凭据**（[`crate::admin::keys`]），后者是**我们签发的、
//! 别人拿来向我们证明身份的凭据**。刻意不共用 `/admin/api/keys` 前缀，
//! 因为那条已经是发布过的公开契约。两个名字很像是**已知代价**，
//! 缓解手段是文案与文档，不是去改既有路径。
//!
//! # 四条贯穿全文件的纪律，每一条都对应一次已经发生过的失效
//!
//! ① 🔴 **明文绝不进列表响应。** 列表一律走 [`ApiKeyView`]（那个结构里既没有明文
//!    也没有摘要），明文只走 [`reveal`] 这一条专门端点、且**每次必留痕**。
//!    列表是**高频、无意识**被调用的：塞进去等于每一次面板轮询、每一条被记下的
//!    响应体、每一个中间层缓存里都带着全部客户端密钥的明文。
//!
//! ② 🔴 **写操作看当前真值**：六条端点一律走 [`read_for_write`]（**直接读存储、
//!    绕过 [`ApiKeyHolder`] 的进程内快照**）。读快照的话，一次「停用」可能建立在
//!    一份最多一个 `APIKEY_CACHE_TTL_MS` 之前的视图上，把这期间别的副本
//!    （或别的运维）写下的记录整份覆盖回去。
//!    连带的一条：乐观并发的 `version` 必须是**刚读回来的那个**，
//!    读快照会让「拿旧版本写」这道护栏建立在一个更旧的版本上。
//!
//! ③ 🔴 **读到 `Invalid` 一律拒绝落盘**（`409 apikeys_unreadable`），
//!    不是「当空表然后覆盖」。当空表的后果是把幸存记录整份覆掉，**不可逆**；
//!    拒绝之后原字节原地不动，人工捞回随时可做。
//!
//! ④ 🔴 **明文落盘是用户 2026-09-10 拍板的取舍，不是技术结论。**
//!    完整代价登记在 [`ApiKeyRecord::secret`] 的文档里（面板被打穿 = 全部明文
//!    一次性泄漏；备份与快照的处置级别要跟着升）。本文件只负责照移植，
//!    **不在这里重新论证，也不许「顺手改回去」**。
//!
//! # 🔴 reveal 的**四态**，以及为什么端点直接给 `kind`
//!
//! TS 版把这四态混成过一态，代价是「对着一把永远取不回的密钥反复点」。四态是：
//!
//! | `kind` | 载体 | 含义 | 运维该做什么 |
//! |---|---|---|---|
//! | `ok` | **200 体** | 明文在这儿 | 复制走 |
//! | `unavailable` | **200 体** | 这把签发于明文落盘之前，服务端**从来就没有过**它的明文 | **永久**如此，重发一把 |
//! | `gone` | **404 状态**（`apikey_not_found`） | 这条记录已经不在表里了（别处删过） | 刷新列表 |
//! | `failed` | **任何其它非 2xx / 传输失败** | 这一次没拿到 | 重试可能有用 |
//!
//! 🔴 **`unavailable` 与 `failed` 不许并档**：前者是一个**永远不会变**的事实，
//! 后者重试可能就好了。并成一句「取不到」，运维会对着一把永远取不回的密钥反复点。
//! ⚠️ **`gone` 与 `failed` 也不许并档**：前者的处置是「刷新一下列表」。
//!
//! ⚠️ **后两态结构上不可能是 2xx 响应体**（它们是状态码与传输层的事实），
//! 所以本端点只可能吐出前两个 `kind`；这一条由
//! [`tests::only_the_two_two_hundred_kinds_can_ever_be_a_body`] 钉着。
//! 06 §5.3 要求「端点直接给 `kind`」，`kind` 因此是**加法**：
//! 02 §9.27 逐字写着的 `secret` / `reason` 两格**一格不动**。
//!
//! # 与 [`crate::admin::keys`] 刻意不同的两处，别「统一」掉
//!
//! 1. **没有「必须先停用才能删」那道前置**：上游 key 删掉之后那把凭据本身就没了；
//!    而这一族删掉的只是我们自己签发的一条验证记录，重发一把是完全正常的操作。
//!    给它加前置只会让「吊销一把已泄漏的密钥」变成两步 —— 而那一步恰恰要越快越好。
//! 2. **写侧收 `version` 而不是 `expect`（条数）**：版本号能识别「删一把又加一把」
//!    这种**条数没变**的改动。
//!
//! # 配额账（写进五份 DEPLOY.md 的那一份）
//!
//! | 动作 | get | put | delete | list |
//! |---|---:|---:|---:|---:|
//! | `GET /apikeys` | 1 | 0 | 0 | 0 |
//! | 任一条写端点 | **2** | 1 | 0 | 0 |
//!
//! ⚠️ **写侧那个 2 是对 TS 版（1 次 get）的一处刻意偏离，如实登记在此**：
//! [`crate::apikey::save`] 在落盘之前**自己再回读一次**并复查版本，
//! 于是「读到写之间」的残余窗口被压到一次读写之间（TS 那份的窗口从
//! `readForWrite` 就开始了）。代价是一次 get，收益是一道真的护栏。
//! 🔴 **`put` 恒是 1、`delete` 恒是 0** —— 单 blob 的直接好处，别改成逐条存。

use crate::core::admin::view::reveal::RevealKind;
use crate::core::keys::api_keys::{
    API_KEY_NAME_MAX, APIKEY_MAX, ApiKeyCounts, ApiKeyRecord, ApiKeyTable, ApiKeyTableRead,
    ApiKeyView, api_key_counts, check_api_key_expires_at, check_api_key_name, is_api_key_usable,
    to_api_key_views,
};
use crate::ports::logger::{LogEntry, LogLevel};
use crate::server::errors::{AdminError, AdminErrorCode, GatewayError};
use crate::server::holders::apikey_holder::ApiKeyHolder;
use crate::server::state::AppState;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Map, Value};
use std::sync::Arc;

// ── 1. 常量（对外契约的真源）─────────────────────────────────────────────────

/// `POST /admin/api/apikeys/purge` 的**完整**对外路径。
///
/// 🔴 **它是单一真源**（02 §9.30 点名：「路径来自真源常量 `APIKEYS_PURGE_PATH`」）：
/// 五语言 ADMIN.md 的危险区一节逐份写着它。注册用的子树内路径由
/// [`ApiKeysEndpoint::Purge`] 给出，两者的关系由
/// [`tests::the_purge_path_constant_is_the_single_source`] 钉住。
pub const APIKEYS_PURGE_PATH: &str = "/admin/api/apikeys/purge";

/// `/admin/api` 这个挂载前缀。路由注册在子树内，对外路径 = 它 + 子树内路径。
pub const ADMIN_API_PREFIX: &str = "/admin/api";

/// 「这个部署没接上对外 API 密钥的存储」的机器可读判别串。
///
/// ⚠️ 它走**顶层 `reason` + 不带 `code` 的网关信封**（02 §4.4 登记的混合体），
/// 与 `config` / `registrar` 那两族逐字同源：这一档是「装配没走 wire.rs」，
/// 面板对它只有一句固定文案（按 `reason` 选），不需要一条要五语言维护的码。
pub const REASON_NOT_WIRED: &str = "not_wired";

/// 「这把签发于明文落盘之前」的机器可读判别串。
pub const REASON_ISSUED_BEFORE_PLAINTEXT: &str = "issued_before_plaintext";

/// reveal 的四态。**闭集，顺序即文件头那张表的顺序。**
///
/// 🔴 **四个串一律由 [`crate::core::admin::view::reveal::RevealKind`] 派生**
/// （06 §5.3 的 A 档）：那一层是两族 reveal 共用的状态机
/// （上游 Agnes key 与我们签发的 `sk-`），**这里再抄一份字面量就是第二份真源**，
/// 而漂的那一格恰好是「永远取不回」与「这一次没取到」的分界 ——
/// 两者并档的后果是运维**对着一把永远取不回的密钥反复点**。
pub const REVEAL_KINDS: [&str; 4] = [
    REVEAL_KIND_OK,
    REVEAL_KIND_UNAVAILABLE,
    REVEAL_KIND_GONE,
    REVEAL_KIND_FAILED,
];

/// 明文在这儿。
pub const REVEAL_KIND_OK: &str = RevealKind::Ok.as_str();
/// 签发于明文落盘之前，**永久**取不回。
pub const REVEAL_KIND_UNAVAILABLE: &str = RevealKind::Unavailable.as_str();
/// 这条记录已经不在表里了（404）。🔴 **结构上不可能是 2xx 响应体。**
pub const REVEAL_KIND_GONE: &str = RevealKind::Gone.as_str();
/// 这一次没拿到，重试可能有用。🔴 **结构上不可能是 2xx 响应体。**
///
/// ⚠️ **「结构上不可能」这句话今天由两道闸支撑，别以为只有一道**：
/// ① [`crate::core::keys::api_keys::narrow_record`] 把 `secret` 是**空串**的记录
///    整条判成读不出来（⇒ 整张表 `Invalid` ⇒ 这条端点回 **409**，见
///    [`tests::an_empty_plaintext_is_rejected_one_layer_earlier_than_reveal`]）；
/// ② [`reveal`] 的判定走共用状态机
///    （[`crate::core::admin::view::reveal::reveal_kind`]），那一层**也**不认空串。
/// **别因为「闸①已经挡住了」就把闸②改回 `match &rec.secret`** ——
/// 窄化那一层的职责随时可能被放宽（它管的是「这条记录能不能用」，
/// 而不是「这个串能不能交给剪贴板」），而放宽的那天这里就会把空串当明文发出去：
/// 屏幕上一句「已复制」，剪贴板里什么都没有。
pub const REVEAL_KIND_FAILED: &str = RevealKind::Failed.as_str();

/// `PATCH /admin/api/apikeys/:id` 能改的那几件事。**顺序即文档顺序。**
///
/// `version` 不在其中 —— 它是**并发凭证**，不是一个可改的字段
/// （但它出现在请求体里是合法的，见 [`patch`] 的白名单）。
pub const APIKEY_PATCH_FIELDS: [&str; 3] = ["name", "disabled", "expiresAt"];

/// `DELETE` 那条把版本号放在**查询参数**里，参数名是这个。
///
/// ⚠️ **刻意不走请求体**：`DELETE` 带请求体在规范上合法，但中间代理与部分 HTTP
/// 客户端会把它丢掉，而一条**被静默丢掉的并发凭证**的后果是
/// 「运维以为自己带了那道保险，其实没带」。查询参数在每一层都活得下来。
pub const VERSION_QUERY_PARAM: &str = "version";

// ── 事件名（运维的 grep 锚点，一个字都不许改）────────────────────────────────

pub const EVENT_APIKEY_ISSUED: &str = "apikey.issued";
pub const EVENT_APIKEY_REVEALED: &str = "apikey.revealed";
pub const EVENT_APIKEY_DISABLED: &str = "apikey.disabled";
pub const EVENT_APIKEY_ENABLED: &str = "apikey.enabled";
pub const EVENT_APIKEY_DELETED: &str = "apikey.deleted";

/// 管理树读存储失败。
///
/// ⚠️ **与 [`crate::admin::keys::EVENT_ADMIN_STORAGE_FAILED`] 是同一个锚点**，
/// 这里直接引它而不是再写一遍字面量：两份字面量迟早会漂，而漂开的现象是
/// 运维按事件名 grep 只捞到一半的 500。
pub use crate::admin::keys::EVENT_ADMIN_STORAGE_FAILED;

// ── 2. 六条端点（注册表的真源）──────────────────────────────────────────────

/// 本组的六条端点。
///
/// 🔴 存在的理由与 [`crate::admin::keys::KeysEndpoint`] 相同：`app.rs` 的
/// `.route()` 从这里取路径，**不许手抄字面量** —— 手抄的那一份与
/// [`APIKEYS_PURGE_PATH`] 迟早会漂，而漂开的现象是「文档里那条危险区路径 404」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeysEndpoint {
    List,
    Issue,
    Purge,
    Reveal,
    Delete,
    Patch,
}

impl ApiKeysEndpoint {
    /// 🔴 **唯一真源**：判据遍历它。
    pub const ALL: [ApiKeysEndpoint; 6] = [
        Self::List,
        Self::Issue,
        Self::Purge,
        Self::Reveal,
        Self::Delete,
        Self::Patch,
    ];

    /// 注册用的**子树内**路径（子树挂在 [`ADMIN_API_PREFIX`] 下）。
    ///
    /// ⚠️ 02 §3 的注册顺序铁律要求 `/apikeys/purge` 排在 `/apikeys/:id` **之前**。
    /// axum 0.8 用 matchit，**静态段天然优先于 `{param}`** ⇒ 这条风险在 Rust 侧
    /// 本身会消失，但**必须写测试把它钉住**（[`tests::the_purge_path_is_not_swallowed_by_the_id_route`]），
    /// 否则将来换匹配器会复发。
    pub const fn subpath(self) -> &'static str {
        match self {
            Self::List | Self::Issue => "/apikeys",
            Self::Purge => "/apikeys/purge",
            Self::Reveal => "/apikeys/{id}/reveal",
            Self::Delete | Self::Patch => "/apikeys/{id}",
        }
    }

    /// HTTP 方法。
    pub const fn method(self) -> &'static str {
        match self {
            Self::List | Self::Reveal => "GET",
            Self::Issue | Self::Purge => "POST",
            Self::Delete => "DELETE",
            Self::Patch => "PATCH",
        }
    }

    /// 这条端点收不收 `version`（乐观并发凭证），收的话走哪儿。
    ///
    /// 🔴 **签发那条刻意不收**：它是**追加**，落在「刚刚回读出来的那一份」之上，
    /// 不会覆盖任何别人写下的记录。收一个版本号只会让
    /// 「两个人同时各签一把」这件完全正当的事失败一次。
    pub const fn version_source(self) -> VersionSource {
        match self {
            Self::List | Self::Issue | Self::Reveal => VersionSource::None,
            Self::Delete => VersionSource::Query,
            Self::Purge | Self::Patch => VersionSource::Body,
        }
    }
}

/// 并发凭证从哪儿来。见 [`ApiKeysEndpoint::version_source`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionSource {
    /// 这条端点不收版本号。
    None,
    /// 请求体里的 `version`。
    Body,
    /// 查询参数 `?version=`。
    Query,
}

// ── 3. 拒绝与降级（每一条都只有一处实现）────────────────────────────────────

/// 没接线时的 503。
///
/// **与 `registrar` / `config` 那两族逐字同源**：端点照常注册、照常鉴权，
/// 但**不假装读到了一张空表** ——「这个部署没接」与「一把密钥都没签发过」
/// 在面板上会长得一模一样，而后者是一句假话。
fn not_wired() -> Response {
    #[derive(Serialize)]
    struct Inner {
        r#type: &'static str,
        message: &'static str,
    }
    #[derive(Serialize)]
    struct Body {
        error: Inner,
        reason: &'static str,
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(Body {
            error: Inner {
                r#type: "internal_error",
                message: "这个部署没有接上对外 API 密钥的存储（装配没走 wire.rs 的 buildApp）",
            },
            reason: REASON_NOT_WIRED,
        }),
    )
        .into_response()
}

/// 接线了没有。`Err` 那一支已经是一个成品响应。
///
/// 🔴 **六条端点每一条的第一句都是它**：漏一条就是一条在没接线的部署上
/// 500（而不是 503）的端点，而 500 在面板上读作「后端坏了」。
fn wiring(st: &AppState) -> Result<&Arc<ApiKeyHolder>, Response> {
    st.api_keys.as_ref().ok_or_else(not_wired)
}

/// 「没有这把对外 API 密钥」。**404 + 管理信封（带 `code`）。**
///
/// ⚠️ 与 [`crate::admin::keys`] 那边的 `Usage` / `Verify` 不同，本组**六条全带
/// `code`** —— 02 §4.4 登记的那处「一半带码一半不带」的刻意不一致**不覆盖本组**。
fn apikey_not_found() -> AdminError {
    AdminError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        AdminErrorCode::ApikeyNotFound,
        "没有这把对外 API 密钥",
    )
}

/// 读存储失败 ⇒ 打一条事件 + 500。
///
/// 🔴 **500 不许静默**：管理面上一次读失败若不留痕，运维只看得到一个白屏。
/// ⚠️ **`err` 只进事件，不进响应体**：`StorageError` 的 `Display` 里可能带路径。
fn storage_failed(st: &AppState, err: &crate::ports::storage::StorageError) -> Response {
    st.logger.log(
        LogEntry::new(
            LogLevel::Error,
            EVENT_ADMIN_STORAGE_FAILED,
            "管理面读对外 API 密钥表失败",
        )
        .with("err", err.to_string()),
    );
    GatewayError::internal().into_response()
}

// ── 4. 请求体窄化（🔴 一律 `Value` + 逐字段窄化，不做 `Deserialize` 整体绑定）──

/// 读一个 JSON **对象**请求体，带 `code`。
///
/// ⚠️ **空体也是 400 `bad_json`**：TS 的 `c.req.json()` 对空体就是抛，
/// 而收请求体的三条端点（签发 / PATCH / 清理）的请求体本来就是必填的。
fn read_admin_object(body: &Bytes) -> Result<Map<String, Value>, AdminError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| AdminError::bad_json())?;
    match v {
        Value::Object(o) => Ok(o),
        // ⚠️ 「请求体」这三个字**刻意不进 `params`**：它是一句中文，
        //    插进 ja/ko 的字典串里等于把中文又漏回屏幕上。
        _ => Err(AdminError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            AdminErrorCode::BodyNotAnObject,
            "请求体必须是一个 JSON 对象",
        )),
    }
}

/// 不认识的字段一律 400。
///
/// **不是洁癖**：`{ disbaled: true }` 这种拼错在宽松实现下是一次
/// 「保存成功、什么都没发生」——而面板会如实地显示保存成功。
fn reject_unknown(o: &Map<String, Value>, allowed: &[&str]) -> Result<(), AdminError> {
    let extra: Vec<&str> = o
        .keys()
        .map(String::as_str)
        .filter(|k| !allowed.contains(k))
        .collect();
    if extra.is_empty() {
        return Ok(());
    }
    let joined = extra.join(", ");
    Err(AdminError::new(
        StatusCode::BAD_REQUEST,
        "invalid_request_error",
        AdminErrorCode::UnknownField,
        format!("不认识的字段：{joined}"),
    )
    .param("fields", joined))
}

/// 名称校验 ⇒ **trim 之后**的名字。判据本身在
/// [`check_api_key_name`]（零 IO 岛那一份），这里只负责挑信封。
fn checked_name(v: Option<&Value>) -> Result<String, AdminError> {
    use crate::core::keys::api_keys::ApiKeyNameProblem;
    let Some(p) = check_api_key_name(v) else {
        return Ok(v
            .and_then(Value::as_str)
            // 结构上到不了：`check_api_key_name` 放行的一定是字符串。
            .unwrap_or_default()
            .trim()
            .to_string());
    };
    let e = AdminError::new(
        StatusCode::BAD_REQUEST,
        "invalid_request_error",
        // 🔴 码从 `core` 那份**闭集**现取，不在这里手写第二份映射。
        code_of(p.code()),
        match p {
            ApiKeyNameProblem::NameNotAString => "name 必须是字符串".to_string(),
            ApiKeyNameProblem::NameEmpty => "name 不能为空".to_string(),
            ApiKeyNameProblem::NameTooLong => format!("name 最长 {API_KEY_NAME_MAX} 个字符"),
        },
    );
    Err(if p == ApiKeyNameProblem::NameTooLong {
        e.param("max", u64::try_from(API_KEY_NAME_MAX).unwrap_or(u64::MAX))
    } else {
        e
    })
}

/// 到期时刻校验。`null` 与**缺席**同义（都是「不过期」）。
///
/// ⚠️ **缺席 = `null` 是签发那条的语义**：面板那个「不过期」选项送的就是 `null`，
/// 而 cURL 用户不写这一格是同一个意思，让两者分道扬镳只会造出一条要解释的差异。
/// （PATCH 那条**不一样**：缺席 = 这次不动它，见 [`patch`]。）
fn checked_expires_at(v: Option<&Value>, now: i64) -> Result<Option<i64>, AdminError> {
    use crate::core::keys::api_keys::ApiKeyExpiryProblem;
    match check_api_key_expires_at(v, now) {
        None => Ok(v.and_then(Value::as_f64).map(|n| n.trunc() as i64)),
        Some(p) => Err(AdminError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            code_of(p.code()),
            match p {
                ApiKeyExpiryProblem::ExpiresNotANumber => {
                    "expiresAt 必须是整数毫秒时间戳，或者 null（不过期）"
                }
                ApiKeyExpiryProblem::ExpiresInThePast => {
                    "expiresAt 已经过去了：签发一把生下来就过期的密钥不是一个有意义的动作"
                }
            },
        )),
    }
}

/// `core` 那两个 `problem.code()` 吐出来的字面量 ⇒ [`AdminErrorCode`]。
///
/// 🔴 **两侧都是闭集，这里只做一次对接**。表外一律 `panic` 而不是挑一个凑合的码：
/// 挑一个凑合的码会让面板查出**另一条**五语言文案，把「name 太长」画成别的东西。
/// 这条路径在 `core` 那两个枚举不增长时不可达，
/// 由 [`tests::every_core_problem_code_maps_to_an_admin_code`] 逐条守着。
fn code_of(code: &str) -> AdminErrorCode {
    AdminErrorCode::ALL
        .iter()
        .copied()
        .find(|c| c.as_str() == code)
        .unwrap_or_else(|| panic!("core 吐出了一个管理接口不认的错误码：{code}"))
}

/// 调用方手上那份快照的版本号。
///
/// ⚠️ **必填，而且必须是调用方屏幕上那个数。** 面板上那张列表是运维**读过一眼**的，
/// 中间被别人签发 / 删掉一把时，一次「清理失效」会连带删掉他从来没见过的那一条。
///
/// JSON 里 `3` 与 `3.0` 都算（`JSON.parse("3.0")` 在 TS 侧就是 `3`）。
fn checked_version(v: Option<&Value>) -> Result<i64, AdminError> {
    let ok = match v {
        // ⚠️ 先挡布尔：`Value::as_f64` 对 `true` 回 `None`，但显式写出来才拦得住
        //    将来有人把它换成一个「真值性」的读法。
        Some(Value::Number(_)) => v.and_then(Value::as_f64).filter(|n| {
            n.is_finite() && n.fract() == 0.0 && *n >= 0.0 && *n < 9_007_199_254_740_992.0
        }),
        _ => None,
    };
    ok.map(|n| n as i64).ok_or_else(version_not_a_number)
}

fn version_not_a_number() -> AdminError {
    AdminError::new(
        StatusCode::BAD_REQUEST,
        "invalid_request_error",
        AdminErrorCode::VersionNotANumber,
        "version 必须是一个非负整数：它是你屏幕上那份列表的版本号",
    )
}

/// `?version=N`。**重复键取第一个**（与 `URLSearchParams.get` 同语义）。
///
/// ⚠️ **一处对 TS 的刻意偏离，登记在此**：TS 走 `Number(raw)`，而
/// `Number("")` **是 0** ⇒ 一个 `?version=`（空值）会被读成「第 0 版」并可能通过
/// 新建表的版本检查。本仓全仓的约定是「**空串 == 没设**」
/// （`APIKEY_CACHE_TTL_MS` 那条逐字如此），
/// 而一道**静默变弱的**并发护栏比没有护栏更坏 ⇒ 这里空值一律 400。
fn version_from_query(raw: Option<&str>) -> Result<i64, AdminError> {
    let raw = raw.unwrap_or("");
    let found = url::form_urlencoded::parse(raw.as_bytes())
        .find(|(k, _)| k == VERSION_QUERY_PARAM)
        .map(|(_, v)| v.into_owned())
        .ok_or_else(version_not_a_number)?;
    let n: f64 = found.trim().parse().map_err(|_| version_not_a_number())?;
    checked_version(Some(&Value::from(n)))
}

// ── 5. 存储侧（读一次 / 版本闸 / 写回）──────────────────────────────────────

/// 写之前回读一次。**那一次 get 本来就要付**（整表覆写必须建立在最新的一份上）。
///
/// 🔴 读到 `Invalid` 一律**拒绝写**：覆盖它是不可逆的，而坏掉的原字节此刻还完整地
/// 留在存储里（读路径一个 put 都不做）。运维捞回它的办法写在五份 ADMIN.md 的排障一节。
async fn read_for_write(st: &AppState) -> Result<ApiKeyTable, Response> {
    match crate::apikey::load(st.storage.as_ref()).await {
        Err(e) => Err(storage_failed(st, &e)),
        Ok(ApiKeyTableRead::Invalid) => Err(AdminError::new(
            StatusCode::CONFLICT,
            "conflict",
            AdminErrorCode::ApikeysUnreadable,
            "存储里的对外 API 密钥表结构不认，本次写入已拒绝，以免覆盖掉里面还留着的内容",
        )
        .into_response()),
        Ok(ApiKeyTableRead::Absent) => Ok(ApiKeyTable::empty()),
        Ok(ApiKeyTableRead::Ok(t)) => Ok(t),
    }
}

/// 乐观并发。
///
/// ⚠️ **它不是 CAS**：读到写之间仍有一个窗口。这条机制把窗口压到一次读写之间，
/// **残余窗口确实存在** —— 任何一处都不许把它说成「不会丢更新」。
fn require_fresh(table: &ApiKeyTable, given: i64) -> Result<(), AdminError> {
    if table.version == given {
        return Ok(());
    }
    Err(stale_write(given, table.version))
}

/// 「你手上那一版已经不是当前版了」。**这一族只有这一处实现**：
/// 版本闸在读侧（[`require_fresh`]）与写侧（[`crate::apikey::save`] 的回读）各查一次，
/// 两处必须吐出**同一条码、同一组 `params`**，否则面板会把同一件事画成两种。
fn stale_write(expected: i64, actual: i64) -> AdminError {
    AdminError::new(
        StatusCode::CONFLICT,
        "conflict",
        AdminErrorCode::StaleWrite,
        format!(
            "这份列表已经被改过了：你看到的是第 {expected} 版，现在是第 {actual} 版。什么都没有改，请刷新后重来"
        ),
    )
    .param("expected", expected)
    .param("actual", actual)
}

/// 整表写回（版本 +1）+ 让持有者作废。
///
/// 🔴 **`invalidate()` 必须与写在同一处**：漏掉它的现象是「面板上停用了，
/// 而本实例最多还要一个 TTL 才真的拒绝那把密钥」——一条**安全**降级，
/// 而面板显示的是「已生效」。
async fn save_table(
    st: &AppState,
    holder: &Arc<ApiKeyHolder>,
    base: &ApiKeyTable,
    keys: Vec<ApiKeyRecord>,
) -> Result<ApiKeyTable, Response> {
    match crate::apikey::save(st.storage.as_ref(), keys, base.version).await {
        Ok(t) => {
            holder.invalidate();
            Ok(t)
        }
        // 回读时发现别人抢先写过 ⇒ 与上面那道闸同一条码、同一组 params。
        Err(crate::apikey::ApiKeyWriteError::Stale { expected, actual }) => {
            Err(stale_write(expected, actual).into_response())
        }
        Err(crate::apikey::ApiKeyWriteError::Unreadable) => Err(AdminError::new(
            StatusCode::CONFLICT,
            "conflict",
            AdminErrorCode::ApikeysUnreadable,
            "存储里的对外 API 密钥表结构不认，本次写入已拒绝，以免覆盖掉里面还留着的内容",
        )
        .into_response()),
        Err(crate::apikey::ApiKeyWriteError::Storage(e)) => Err(storage_failed(st, &e)),
    }
}

// ── 6. 响应体 ───────────────────────────────────────────────────────────────

/// `GET /admin/api/apikeys` 的响应体。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListBody {
    /// 🔴 **它不等于「一把都没有」**，面板必须按这一格分支渲染。
    unreadable: bool,
    /// `unreadable` 时是 `null`。三条写端点都要把它原样带回来。
    version: Option<i64>,
    keys: Vec<ApiKeyView>,
    counts: ApiKeyCounts,
    max: usize,
    cache_ttl_ms: i64,
    /// 🔴 **已经判好的视图**（06 §5.3 的 A 档）。见
    /// [`crate::core::admin::view::apikeys`]。
    ///
    /// ⚠️ **`rows` 与 `keys` 同序同长**，逐条带 `id` 以便面板自查对齐。
    view: ApiKeysViewWire,
}

/// 列表那份响应的视图 DTO。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiKeysViewWire {
    /// `unreadable` / `ok`。🔴 **「表读不出来」不等于「一把都没有」**
    /// —— 拿 `keys.length == 0` 去判就会把「表坏了、全部客户端正在 401」
    /// 画成一句「你还没签发过密钥」。
    state: &'static str,
    rows: Vec<ApiKeyRowViewWire>,
    /// 「清理失效（N）」那颗按钮上的 N。**数的是整张表，不是当前搜索结果。**
    purge_count: usize,
    /// N = 0 时那颗按钮**整个不画**：一颗点了什么都不会发生的按钮比没有更糟。
    purge_visible: bool,
    /// 整张表空时那句话。`null` = 表非空（「搜索没命中」那一句由渲染层在自己
    /// 筛完之后决定 —— 搜索是客户端做的，服务端答不了那一半）。
    empty_key: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiKeyRowViewWire {
    id: String,
    badge_class: &'static str,
    bucket_label_key: &'static str,
    toggle_label_key: &'static str,
}

/// 拼列表那份视图。**判定一律转调 [`crate::core::admin::view::apikeys`]。**
fn apikeys_view(unreadable: bool, views: &[ApiKeyView]) -> ApiKeysViewWire {
    use crate::core::admin::view::apikeys as v;

    let purge_count = v::ak_purge_count(views.iter().map(|k| k.bucket.as_str()));
    let state = v::ak_list_state(unreadable);
    ApiKeysViewWire {
        state: state.as_str(),
        rows: views
            .iter()
            .map(|k| ApiKeyRowViewWire {
                id: k.id.clone(),
                badge_class: v::ak_badge_class(k.bucket.as_str()),
                bucket_label_key: v::ak_bucket_label_key(k.bucket.as_str()),
                toggle_label_key: v::ak_toggle_label_key(k.disabled),
            })
            .collect(),
        purge_count,
        purge_visible: v::ak_purge_visible(purge_count),
        empty_key: v::ak_empty_key(state, views.len()),
    }
}

/// 签发那一次的 201。**明文只在这里。**
#[derive(Serialize)]
struct IssueBody {
    /// 🔴 字段名刻意叫 `secret` 而不是 `key`：这棵树上 `key` 已经是上游池那一族的词，
    /// 两族在同一份响应里撞名会让人读错。
    secret: String,
    record: ApiKeyView,
    version: i64,
}

/// PATCH 的 200。
#[derive(Serialize)]
struct PatchBody {
    ok: bool,
    record: ApiKeyView,
    version: i64,
}

/// 清理失效的 200。
#[derive(Serialize)]
struct PurgeBody {
    deleted: usize,
    remaining: usize,
    version: i64,
}

/// reveal 的 200。见文件头那张四态表。
#[derive(Serialize)]
struct RevealBody {
    /// 06 §5.3 要的那一格：**端点直接给档位**，面板不必自己从字段组合里反推。
    kind: &'static str,
    /// 🔴 取不回时是 `null`，**不许用空串或掩码冒充明文**。
    secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

// ── 7. 六条 handler ─────────────────────────────────────────────────────────

/// `GET /admin/api/apikeys` —— 列表 + 统计 + 版本号。**1 次 get，0 次写。**
///
/// 🔴 **它刻意不走 [`ApiKeyHolder`] 的快照**，两条理由：
/// ① 乐观并发的 `version` 必须是**刚读回来的那个**；
/// ② 只有直接读得出「表坏了」这一档，而那一档必须如实报给运维
///    （在快照里它与「还没签发过」长得一模一样）。
pub async fn list(State(st): State<AppState>) -> Response {
    let holder = match wiring(&st) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let ttl = holder.ttl_ms();
    let read = match crate::apikey::load(st.storage.as_ref()).await {
        Ok(v) => v,
        Err(e) => return storage_failed(&st, &e),
    };
    let table = match read {
        // 🔴 **读不出来 ≠ 一把都没有。** 两者在面板上必须分得开。
        ApiKeyTableRead::Invalid => {
            return Json(ListBody {
                unreadable: true,
                version: None,
                keys: Vec::new(),
                counts: api_key_counts(&[]),
                max: APIKEY_MAX,
                cache_ttl_ms: ttl,
                // ⚠️ **`emptyKey` 在这一档下必须是 `null`**：表读不出来时说
                //    「你还没签发过密钥」正是这一格要挡的那句假话。
                //    那一档由 `state: "unreadable"` 独占。
                view: apikeys_view(true, &[]),
            })
            .into_response();
        }
        ApiKeyTableRead::Absent => ApiKeyTable::empty(),
        ApiKeyTableRead::Ok(t) => t,
    };
    let views = to_api_key_views(&table.keys, crate::adapters::now_ms());
    // ⚠️ 必须在 `views` 被 move 进响应体**之前**算完。
    let view = apikeys_view(false, &views);
    Json(ListBody {
        unreadable: false,
        version: Some(table.version),
        counts: api_key_counts(&views),
        keys: views,
        max: APIKEY_MAX,
        cache_ttl_ms: ttl,
        view,
    })
    .into_response()
}

/// `POST /admin/api/apikeys` —— 签发，**201 带一次明文**。
///
/// ⚠️⚠️ **这是全仓唯一一处在响应体里交出我们签发的凭据明文的地方。**
/// 明文同时落盘（2026-09-10 用户拍板），所以它此后还能经 [`reveal`] 取回 ——
/// 但**列表、PATCH、事件里一个字节都没有**。
pub async fn issue(State(st): State<AppState>, body: Bytes) -> Response {
    let holder = match wiring(&st) {
        Ok(h) => h.clone(),
        Err(r) => return r,
    };
    let o = match read_admin_object(&body) {
        Ok(o) => o,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = reject_unknown(&o, &["name", "expiresAt"]) {
        return e.into_response();
    }
    let now = crate::adapters::now_ms();
    let name = match checked_name(o.get("name")) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let expires_at = match checked_expires_at(o.get("expiresAt"), now) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    let table = match read_for_write(&st).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    if table.keys.len() >= APIKEY_MAX {
        // **超了就 400，不静默截断**：截断会让运维以为签发成功了。
        return AdminError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            AdminErrorCode::TooManyApikeys,
            format!("最多只能有 {APIKEY_MAX} 把对外 API 密钥"),
        )
        .param("max", u64::try_from(APIKEY_MAX).unwrap_or(u64::MAX))
        .into_response();
    }

    let (secret, record) = crate::apikey::issue(name, now, expires_at);
    let mut keys = table.keys.clone();
    keys.push(record.clone());
    let next = match save_table(&st, &holder, &table, keys).await {
        Ok(t) => t,
        Err(r) => return r,
    };

    st.logger.log(
        LogEntry::new(
            LogLevel::Warn,
            EVENT_APIKEY_ISSUED,
            "面板签发了一把对外 API 密钥",
        )
        // 🔴 **只记 id 与到期，绝不记明文、绝不记摘要**：日志常被转发到第三方。
        //    这一条在明文落盘之后**更要紧**了，不是更不要紧。
        .with("id", record.id.clone())
        .with("expiresAt", record.expires_at),
    );

    let view = to_api_key_views(std::slice::from_ref(&record), now)
        .into_iter()
        .next()
        // 结构上到不了：投影一条进去必然出来一条。
        .expect("一条记录必然投影出一条视图");
    (
        StatusCode::CREATED,
        Json(IssueBody {
            secret,
            record: view,
            version: next.version,
        }),
    )
        .into_response()
}

/// `GET /admin/api/apikeys/{id}/reveal` —— 取回明文。见文件头那张四态表。
pub async fn reveal(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    if let Err(r) = wiring(&st) {
        return r;
    }
    // ⚠️ 走 `read_for_write`：表坏了时**如实 409**，而不是报一句「没有这把密钥」——
    //    后者会让运维去重发一把，而真正该做的是把那张表捞回来。
    let table = match read_for_write(&st).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    let Some(rec) = table.keys.iter().find(|r| r.id == id) else {
        return apikey_not_found().into_response();
    };

    // **取明文是一次凭据访问，必须留痕**，与签发同级别（warn）。
    st.logger.log(
        LogEntry::new(
            LogLevel::Warn,
            EVENT_APIKEY_REVEALED,
            "面板取回了一把对外 API 密钥的明文",
        )
        // 🔴 只记 id 与「有没有明文」，**绝不记明文本身**。
        .with("id", id)
        .with("available", rec.secret.is_some()),
    );

    // 🔴 **档位走共用状态机，别在这里写 `match &rec.secret`**：
    //    那个 match 会把一条被外部写坏成**空串**的明文当成可用明文交出去
    //    （屏幕上一句「已复制」，剪贴板里什么都没有）。
    //    第二个参数恒为 `true`：这一族的 `secret == None` 在后端就只有一种成因
    //    —— 签发于明文落盘之前。
    //
    // ⚠️⚠️ **如实登记：这一行今天没有观测量。** `narrow_record` 已经把空串明文
    //    整条判成读不出来（见 `an_empty_plaintext_is_rejected_one_layer_earlier_than_reveal`），
    //    所以把这里换回 `if rec.secret.is_some()` **变异复检是全绿逃逸的**
    //    （实测：82 格全过）。它是一道**刻意的纵深防御**，不是一条被判据守着的行为
    //    —— 别读成「有判据钉着」，也别因为「反正测不出来」就删：
    //    上游那道闸管的是「这条记录能不能用」，放宽它是个随时可能发生的合理改动。

    let kind = crate::core::admin::view::reveal::reveal_kind(rec.secret.as_deref(), true);
    match kind {
        RevealKind::Ok => Json(RevealBody {
            kind: kind.as_str(),
            // `Ok` 那一档必然有非空明文（状态机的定义），所以这里取得到。
            secret: rec.secret.clone(),
            reason: None,
        })
        .into_response(),
        RevealKind::Unavailable => Json(RevealBody {
            kind: kind.as_str(),
            secret: None,
            reason: Some(REASON_ISSUED_BEFORE_PLAINTEXT),
        })
        .into_response(),
        // 只剩「明文是空串」这一条路走得到（`Gone` 那一档在上面已经 404 掉了）。
        // ⚠️ **不带 `reason`**：我们并不知道那条记录是怎么被写成空串的，
        //    编一个理由比不给理由更糟。
        RevealKind::Gone | RevealKind::Failed => Json(RevealBody {
            kind: RevealKind::Failed.as_str(),
            secret: None,
            reason: None,
        })
        .into_response(),
    }
}

/// `PATCH /admin/api/apikeys/{id}` —— 改名 / 停用启用 / 改到期。
///
/// ⚠️ **停用不是即时的**：本实例立刻生效（`invalidate()`），别的副本最多还要一个
/// `APIKEY_CACHE_TTL_MS` 才看得见。这是安全相关的，面板的成功提示里要写这个数字。
pub async fn patch(State(st): State<AppState>, Path(id): Path<String>, body: Bytes) -> Response {
    let holder = match wiring(&st) {
        Ok(h) => h.clone(),
        Err(r) => return r,
    };
    let o = match read_admin_object(&body) {
        Ok(o) => o,
        Err(e) => return e.into_response(),
    };
    let mut allowed = APIKEY_PATCH_FIELDS.to_vec();
    allowed.push("version");
    if let Err(e) = reject_unknown(&o, &allowed) {
        return e.into_response();
    }
    let version = match checked_version(o.get("version")) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let now = crate::adapters::now_ms();

    let name = if o.contains_key("name") {
        match checked_name(o.get("name")) {
            Ok(v) => Some(v),
            Err(e) => return e.into_response(),
        }
    } else {
        None
    };
    let disabled = match o.get("disabled") {
        None => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => {
            return AdminError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                AdminErrorCode::NotABoolean,
                "disabled 必须是布尔值",
            )
            .param("field", "disabled")
            .into_response();
        }
    };
    // 🔴 **显式传 `null` 是「改成不过期」，缺席才是「这次不动它」** —— 两者绝不能合并。
    let expires_at = if o.contains_key("expiresAt") {
        match checked_expires_at(o.get("expiresAt"), now) {
            Ok(v) => Some(v),
            Err(e) => return e.into_response(),
        }
    } else {
        None
    };
    if name.is_none() && disabled.is_none() && expires_at.is_none() {
        // 空 patch 返回 200 就是一次「保存成功」而什么都没做。
        return AdminError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            AdminErrorCode::EmptyPatch,
            "至少要改一个字段",
        )
        .into_response();
    }

    let table = match read_for_write(&st).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    if let Err(e) = require_fresh(&table, version) {
        return e.into_response();
    }
    let Some(at) = table.keys.iter().position(|r| r.id == id) else {
        return apikey_not_found().into_response();
    };

    let mut keys = table.keys.clone();
    let rec = &mut keys[at];
    if let Some(n) = name {
        rec.name = n;
    }
    if let Some(d) = disabled {
        rec.disabled = Some(d);
    }
    if let Some(e) = expires_at {
        rec.expires_at = e;
    }
    let next_record = rec.clone();
    let next = match save_table(&st, &holder, &table, keys).await {
        Ok(t) => t,
        Err(r) => return r,
    };

    // 一次 PATCH 最多打一条事件，**停用优先**：它是唯一一个让密钥停止服役的动作。
    // 只改名不打事件 —— 名字不改变任何鉴权行为。
    if disabled == Some(true) {
        st.logger.log(
            LogEntry::new(
                LogLevel::Warn,
                EVENT_APIKEY_DISABLED,
                "面板停用了一把对外 API 密钥（别的实例最多还要一个缓存周期才看得见）",
            )
            .with("id", next_record.id.clone()),
        );
    } else if disabled == Some(false) {
        st.logger.log(
            LogEntry::new(
                LogLevel::Info,
                EVENT_APIKEY_ENABLED,
                "面板重新启用了一把对外 API 密钥",
            )
            .with("id", next_record.id.clone()),
        );
    }

    let view = to_api_key_views(std::slice::from_ref(&next_record), now)
        .into_iter()
        .next()
        .expect("一条记录必然投影出一条视图");
    Json(PatchBody {
        ok: true,
        record: view,
        version: next.version,
    })
    .into_response()
}

/// `DELETE /admin/api/apikeys/{id}?version=N` —— `204` / `404` / `409`。
pub async fn delete(
    State(st): State<AppState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let holder = match wiring(&st) {
        Ok(h) => h.clone(),
        Err(r) => return r,
    };
    let version = match version_from_query(raw.as_deref()) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let table = match read_for_write(&st).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    if let Err(e) = require_fresh(&table, version) {
        return e.into_response();
    }
    let Some(at) = table.keys.iter().position(|r| r.id == id) else {
        return apikey_not_found().into_response();
    };
    let mut keys = table.keys.clone();
    keys.remove(at);
    if let Err(r) = save_table(&st, &holder, &table, keys).await {
        return r;
    }
    st.logger.log(
        LogEntry::new(
            LogLevel::Warn,
            EVENT_APIKEY_DELETED,
            "面板删除了一把对外 API 密钥（拿着它的客户端最多再过一个缓存周期就会开始 401）",
        )
        .with("id", id),
    );
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /admin/api/apikeys/purge` —— 一次清掉全部**此刻用不了**的（已停用或已过期）。
///
/// ⚠️ **判据是 [`is_api_key_usable`] 的反面**，与面板上那两张统计卡（已停用 / 已过期）
/// 之和逐条相同。🔴 **不许在这里另写一条判据**：面板上写着「清理失效（N）」，
/// 而 N 是按分档数出来的 —— 两份判据一漂，那颗按钮删掉的条数就与它自己写的数字对不上。
pub async fn purge(State(st): State<AppState>, body: Bytes) -> Response {
    let holder = match wiring(&st) {
        Ok(h) => h.clone(),
        Err(r) => return r,
    };
    let o = match read_admin_object(&body) {
        Ok(o) => o,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = reject_unknown(&o, &["version"]) {
        return e.into_response();
    }
    let version = match checked_version(o.get("version")) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let table = match read_for_write(&st).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    if let Err(e) = require_fresh(&table, version) {
        return e.into_response();
    }
    let now = crate::adapters::now_ms();
    let kept: Vec<ApiKeyRecord> = table
        .keys
        .iter()
        .filter(|r| is_api_key_usable(r, now))
        .cloned()
        .collect();
    let deleted = table.keys.len() - kept.len();
    let remaining = kept.len();
    let next = match save_table(&st, &holder, &table, kept).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    if deleted > 0 {
        st.logger.log(
            LogEntry::new(
                LogLevel::Warn,
                EVENT_APIKEY_DELETED,
                "面板清理了全部已失效的对外 API 密钥",
            )
            // **只记数**：一次清理的 id 可能有上百个，而 `fields` 的值是标量。
            .with("count", deleted as u64)
            .with("remaining", remaining as u64),
        );
    }
    Json(PurgeBody {
        deleted,
        remaining,
        version: next.version,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::core::keys::api_keys::{
        ApiKeyExpiryProblem, ApiKeyNameProblem, digest, hint_of, mask_of,
    };
    use crate::ports::storage::Storage;
    use crate::server::app::build_router;
    use crate::server::holders::apikey_holder::APIKEYS_KEY;
    use crate::server::test_support::{RecordingLogger, TestApp, state_with_storage};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use serde_json::json;
    use tower::ServiceExt;

    const ADMIN: &str = "admin-token-0123456789abcdef";

    struct Harness {
        app: Router,
        recorded: Arc<RecordingLogger>,
    }

    async fn harness_with(storage: Arc<dyn Storage>, wired: bool) -> Harness {
        let TestApp {
            mut state,
            recorded,
            ..
        } = state_with_storage(storage).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        if !wired {
            state.api_keys = None;
        }
        Harness {
            app: build_router(state),
            recorded,
        }
    }

    async fn harness() -> Harness {
        harness_with(Arc::new(MemoryStorage::new()), true).await
    }

    async fn call(
        h: &Harness,
        method: Method,
        uri: &str,
        body: Option<&str>,
    ) -> (StatusCode, String) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-admin-key", ADMIN);
        if body.is_some() {
            b = b.header("content-type", "application/json");
        }
        let req = b
            .body(body.map_or_else(Body::empty, |s| Body::from(s.to_string())))
            .unwrap();
        let res = h.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    async fn json_call(
        h: &Harness,
        method: Method,
        uri: &str,
        body: Option<&str>,
    ) -> (StatusCode, Value) {
        let (s, b) = call(h, method, uri, body).await;
        (s, serde_json::from_str(&b).unwrap_or(Value::Null))
    }

    async fn get(h: &Harness, uri: &str) -> (StatusCode, Value) {
        json_call(h, Method::GET, uri, None).await
    }

    async fn post(h: &Harness, uri: &str, body: &str) -> (StatusCode, Value) {
        json_call(h, Method::POST, uri, Some(body)).await
    }

    async fn patch_req(h: &Harness, uri: &str, body: &str) -> (StatusCode, Value) {
        json_call(h, Method::PATCH, uri, Some(body)).await
    }

    /// 签发一把，回 `(id, secret, version)`。
    async fn issue_one(h: &Harness, name: &str) -> (String, String, i64) {
        let (s, v) = post(
            h,
            "/admin/api/apikeys",
            &json!({ "name": name }).to_string(),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "夹具自己的签发必须成功：{v}");
        (
            v["record"]["id"].as_str().unwrap().to_string(),
            v["secret"].as_str().unwrap().to_string(),
            v["version"].as_i64().unwrap(),
        )
    }

    /// 直接往存储里放一张表 —— 「已过期」「签发于明文落盘之前」这两种记录
    /// **走不了签发端点**（前者被 `expires_in_the_past` 挡，后者根本铸不出来）。
    async fn seed_table(storage: &Arc<dyn Storage>, version: i64, keys: Vec<Value>) {
        storage
            .put(
                APIKEYS_KEY,
                json!({ "version": version, "keys": keys }),
                None,
            )
            .await
            .unwrap();
    }

    fn raw_record(id: &str, secret: Option<&str>, disabled: Option<bool>, expires: Value) -> Value {
        let plain = secret.unwrap_or("sk-fixture-00112233445566778899aabbccddeeff");
        let mut o = json!({
            "id": id,
            "name": format!("k-{id}"),
            "hash": digest(plain),
            "hint": hint_of(plain),
            "createdAt": 1_000,
            "expiresAt": expires,
        });
        if let Some(s) = secret {
            o["secret"] = json!(s);
        }
        if let Some(d) = disabled {
            o["disabled"] = json!(d);
        }
        o
    }

    // ── 注册表与路径 ───────────────────────────────────────────────────────

    /// 🔴 **危险区那条路径是单一真源**：对外路径 = 挂载前缀 + 子树内路径。
    ///
    /// 回归对象：`app.rs` 手抄一个字面量、或有人只改了其中一处 ⇒
    /// 文档里那条危险区路径 404，而一条 404 的清理端点在面板上长得像「没这个功能」。
    #[test]
    fn the_purge_path_constant_is_the_single_source() {
        assert_eq!(
            APIKEYS_PURGE_PATH,
            format!("{ADMIN_API_PREFIX}{}", ApiKeysEndpoint::Purge.subpath())
        );
        // 期望值**手写字面量**，不从被测常量现算 —— 后者是同义反复。
        assert_eq!(APIKEYS_PURGE_PATH, "/admin/api/apikeys/purge");
    }

    /// ⚠️⚠️ **`/apikeys` 与 `/keys` 是两族，路径不许撞。**
    ///
    /// 回归对象：有人「顺手统一」成 `/admin/api/keys/...`，
    /// 于是我们签发的那一族把上游池那条**已发布的公开契约**盖掉。
    #[test]
    fn the_two_key_families_never_share_a_path() {
        for ep in ApiKeysEndpoint::ALL {
            assert!(
                ep.subpath().starts_with("/apikeys"),
                "{} 不在 /apikeys 子树下",
                ep.subpath()
            );
            for other in crate::admin::keys::KeysEndpoint::ALL {
                assert_ne!(
                    (ep.subpath(), ep.method()),
                    (other.subpath(), other.method()),
                    "两族撞了一条 (路径, 方法)"
                );
            }
        }
    }

    /// 六条端点**全部注册**，且每一条都在 `admin_auth` 之后。
    ///
    /// 🔴 挪到静态兜底之后的端点**恒 404，拿着正确口令也是 404，没有任何报错**。
    /// ⚠️ 「这条路由不存在」与「这把密钥不存在」**都是 404**，靠信封分：
    /// 前者是管理树的兜底（网关信封，**不带 `code`**），后者带 `apikey_not_found`。
    #[tokio::test]
    async fn all_six_endpoints_are_registered_behind_the_gate() {
        let h = harness().await;
        for ep in ApiKeysEndpoint::ALL {
            let uri = format!(
                "{ADMIN_API_PREFIX}{}",
                ep.subpath().replace("{id}", "deadbeef0000")
            );
            let method = Method::from_bytes(ep.method().as_bytes()).unwrap();
            let (status, body) = call(&h, method, &uri, Some("{}")).await;
            assert_ne!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{uri} 注册的方法不是 {}",
                ep.method()
            );
            if status == StatusCode::NOT_FOUND {
                let v: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(
                    v["error"]["code"],
                    "apikey_not_found",
                    "{uri} {} 落到了管理树的兜底 ⇒ 这条路由根本没注册",
                    ep.method()
                );
            }
        }
    }

    /// ⚠️ **「更窄排在更宽之前」**：`/apikeys/purge` 不许被 `/apikeys/{id}` 吞掉。
    ///
    /// axum 0.8 的 matchit 让静态段天然优先，所以这一格今天必绿 ——
    /// **写它是为了将来换匹配器时当场红**（02 §3 实测过 Hono 上的这条事故）。
    #[tokio::test]
    async fn the_purge_path_is_not_swallowed_by_the_id_route() {
        let h = harness().await;
        // 打到 purge ⇒ 它要 `version`，缺了就是 400 version_not_a_number；
        // 若被 `{id}` 吞掉，POST /apikeys/{id} 根本没注册 ⇒ 405。
        let (s, v) = post(&h, APIKEYS_PURGE_PATH, "{}").await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], "version_not_a_number");
    }

    /// `version_source()` 这张表与 handler 的真实行为一致。
    ///
    /// 🔴 **签发那条刻意不收 `version`**：收了会让「两个人同时各签一把」失败一次。
    #[tokio::test]
    async fn the_version_source_table_matches_the_handlers() {
        let h = harness().await;
        // ① 签发：不带 version 也成功。
        let (s, _) = post(&h, "/admin/api/apikeys", r#"{"name":"a"}"#).await;
        assert_eq!(s, StatusCode::CREATED);
        assert_eq!(ApiKeysEndpoint::Issue.version_source(), VersionSource::None);
        // ② 带了 version ⇒ 未知字段。
        let (s, v) = post(&h, "/admin/api/apikeys", r#"{"name":"b","version":1}"#).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], "unknown_field");
        // ③ 删除走查询参数，不走请求体。
        assert_eq!(
            ApiKeysEndpoint::Delete.version_source(),
            VersionSource::Query
        );
        assert_eq!(ApiKeysEndpoint::Purge.version_source(), VersionSource::Body);
    }

    // ── 没接线 ─────────────────────────────────────────────────────────────

    /// 🔴 **没接线的部署：六条端点逐条 `503 not_wired`，且是混合信封。**
    ///
    /// 回归对象：漏一条 ⇒ 那条端点在没接线的部署上 500，
    /// 而 500 在面板上读作「后端坏了」（真相是「这个部署没有这个功能」）。
    /// ⚠️ 同时钉住**不带 `code`**：这一档面板按顶层 `reason` 选文案。
    #[tokio::test]
    async fn every_endpoint_reports_not_wired_with_the_mixed_envelope() {
        let h = harness_with(Arc::new(MemoryStorage::new()), false).await;
        for ep in ApiKeysEndpoint::ALL {
            let uri = format!(
                "{ADMIN_API_PREFIX}{}",
                ep.subpath().replace("{id}", "deadbeef0000")
            );
            let method = Method::from_bytes(ep.method().as_bytes()).unwrap();
            let (status, body) = call(&h, method, &uri, Some(r#"{"version":0}"#)).await;
            let v: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}");
            assert_eq!(v["reason"], "not_wired", "{uri} 的顶层 reason");
            assert_eq!(v["error"]["type"], "internal_error", "{uri}");
            assert!(
                v["error"]["code"].is_null(),
                "{uri} 这一档刻意不带 code（面板按 reason 选文案）"
            );
        }
    }

    /// 没接线时**一次存储调用都不产生** —— 它是「装配不全」，不是「读一读看看」。
    ///
    /// ⚠️ 判据看的是**装配之后的增量**：装配本身（配置 prime 等）要付的那几次
    /// 与本组无关，把它们算进来会让这一格钉的不是自己那一段。
    #[tokio::test]
    async fn a_not_wired_deployment_touches_storage_zero_times() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = harness_with(counting, false).await;
        let before = counts.snapshot();
        get(&h, "/admin/api/apikeys").await;
        post(&h, "/admin/api/apikeys", r#"{"name":"a"}"#).await;
        assert_eq!(counts.snapshot(), before);
    }

    // ── 列表 ───────────────────────────────────────────────────────────────

    /// 空部署：`unreadable:false` + 版本 0 + 四个计数全 0 + 上限与 TTL 如实报。
    ///
    /// ⚠️ `max` / `cacheTtlMs` 由后端给，🔴 **面板不许在前端写死**。
    #[tokio::test]
    async fn an_empty_deployment_lists_version_zero_not_unreadable() {
        let h = harness().await;
        let (s, v) = get(&h, "/admin/api/apikeys").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["unreadable"], false);
        assert_eq!(v["version"], 0);
        assert_eq!(v["keys"].as_array().unwrap().len(), 0);
        assert_eq!(
            v["counts"],
            json!({"all":0,"active":0,"disabled":0,"expired":0})
        );
        assert_eq!(v["max"], 200);
        assert_eq!(v["cacheTtlMs"], 300_000);
    }

    /// 🔴🔴 **「读不出来」与「一把都没有」在响应里必须分得开。**
    ///
    /// 回归对象：把 `Invalid` 折叠成空表 ⇒ 面板把一张**还完整躺在存储里**的表
    /// 画成「你还没签发过任何密钥」，而运维的下一步就是签一把新的、然后覆盖掉它。
    #[tokio::test]
    async fn an_unreadable_table_is_reported_as_such_not_as_an_empty_one() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        mem.put(APIKEYS_KEY, json!("这不是一张表"), None)
            .await
            .unwrap();
        let h = harness_with(mem, true).await;
        let (s, v) = get(&h, "/admin/api/apikeys").await;
        assert_eq!(s, StatusCode::OK, "它不是一个错误，是一档要如实呈现的状态");
        assert_eq!(v["unreadable"], true);
        assert!(v["version"].is_null(), "读不出来时没有版本号可言");
        assert_eq!(v["keys"].as_array().unwrap().len(), 0);
        assert_eq!(v["counts"]["all"], 0);
    }

    /// 🔴🔴 **「表读不出来」那一档，`view` 里一个字都不许说「你还没签发过密钥」。**
    ///
    /// 那一档同样发 `keys: []` ⇒ 光看条数的判据会说出一句方向完全相反的话，
    /// 而运维的下一步就是签一把新的、然后覆盖掉那张还完整躺在存储里的表。
    ///
    /// ⚠️ 顺带钉住「清理失效」那颗按钮在这一档下**整个不画**：
    /// 它此刻不知道要清几条，画一颗写着「(0)」的按钮就是另一句假话。
    #[tokio::test]
    async fn the_unreadable_view_never_claims_the_table_is_merely_empty() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        mem.put(APIKEYS_KEY, json!("这不是一张表"), None)
            .await
            .unwrap();
        let h = harness_with(mem, true).await;
        let (_, v) = get(&h, "/admin/api/apikeys").await;
        assert_eq!(v["view"]["state"], "unreadable");
        assert_eq!(
            v["view"]["emptyKey"],
            json!(null),
            "🔴 这一档由 state 独占，emptyKey 一个字都不许替它说"
        );
        assert_eq!(v["view"]["purgeVisible"], json!(false));
        assert_eq!(v["view"]["purgeCount"], json!(0));
        assert!(v["view"]["rows"].as_array().expect("rows").is_empty());
    }

    /// 真的一把都没有时才说「还没签发过」。
    #[tokio::test]
    async fn an_actually_empty_table_does_say_so() {
        let h = harness().await;
        let (_, v) = get(&h, "/admin/api/apikeys").await;
        assert_eq!(v["view"]["state"], "ok");
        assert_eq!(v["view"]["emptyKey"], json!("ak.empty"));
    }

    /// 🔴 **「清理失效（N）」那个 N 数的是「停用」+「过期」两档，与
    /// `is_api_key_usable` 同一条判据** —— 另写一条的话，那颗按钮删掉的条数
    /// 会与它自己写的数字对不上。
    ///
    /// ⚠️ 这一格拿真的 purge 端点对表，不是只断言一个数。
    #[tokio::test]
    async fn the_purge_count_matches_what_purge_actually_deletes() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        seed_table(
            &mem,
            1,
            vec![
                raw_record("active000001", None, None, Value::Null),
                raw_record("disabled0001", None, Some(true), Value::Null),
                raw_record("expired00001", None, None, json!(1_500)),
                raw_record("active000002", None, None, Value::Null),
            ],
        )
        .await;
        let h = harness_with(mem, true).await;
        let (_, v) = get(&h, "/admin/api/apikeys").await;
        assert_eq!(v["view"]["state"], "ok");
        assert_eq!(v["view"]["emptyKey"], json!(null));
        let n = v["view"]["purgeCount"].as_u64().expect("purgeCount");
        assert_eq!(n, 2, "停用 1 + 过期 1");
        assert_eq!(v["view"]["purgeVisible"], json!(true));

        let (s, p) = post(
            &h,
            APIKEYS_PURGE_PATH,
            &json!({ "version": v["version"] }).to_string(),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{p}");
        assert_eq!(
            p["deleted"].as_u64(),
            Some(n),
            "🔴 按钮上写的数字与它真删掉的条数必须是同一个"
        );
    }

    /// 一把都不失效时那颗按钮**整个不画**：点了什么都不会发生的按钮比没有更糟。
    #[tokio::test]
    async fn the_purge_button_disappears_when_nothing_is_unusable() {
        let h = harness().await;
        issue_one(&h, "a").await;
        let (_, v) = get(&h, "/admin/api/apikeys").await;
        assert_eq!(v["view"]["purgeCount"], json!(0));
        assert_eq!(v["view"]["purgeVisible"], json!(false));
    }

    /// 三档徽章互不相同，且「已停用」是中性灰（它不是故障，是运维按下的开关）；
    /// `view.rows` 与 `keys` **同序同长**。
    #[tokio::test]
    async fn the_row_view_gives_three_distinguishable_badges_aligned_with_keys() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        seed_table(
            &mem,
            1,
            vec![
                raw_record("active000001", None, None, Value::Null),
                raw_record("disabled0001", None, Some(true), Value::Null),
                raw_record("expired00001", None, None, json!(1_500)),
            ],
        )
        .await;
        let h = harness_with(mem, true).await;
        let (_, v) = get(&h, "/admin/api/apikeys").await;
        let keys = v["keys"].as_array().expect("keys");
        let rows = v["view"]["rows"].as_array().expect("rows");
        assert_eq!(keys.len(), rows.len());
        let mut badges = Vec::new();
        for (k, row) in keys.iter().zip(rows) {
            assert_eq!(k["id"], row["id"], "同序");
            badges.push(row["badgeClass"].as_str().unwrap());
        }
        let mut uniq = badges.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), 3, "三档必须互不相同：{badges:?}");
        let disabled_row = rows
            .iter()
            .find(|r| r["id"] == json!("disabled0001"))
            .expect("那一行在");
        assert_eq!(disabled_row["badgeClass"], json!("badge"), "中性灰");
        assert_eq!(disabled_row["toggleLabelKey"], json!("ak.action.enable"));
        assert_eq!(disabled_row["bucketLabelKey"], json!("ak.bucket.disabled"));
    }

    /// 列表给的是 `seq` 顺序的投影 + 分档 + 掩码。
    #[tokio::test]
    async fn the_list_projects_seq_bucket_and_mask() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let now = crate::adapters::now_ms();
        seed_table(
            &mem,
            5,
            vec![
                raw_record(
                    "bbb",
                    Some("sk-fixture-aaaabbbbccccdddd1111222233334444"),
                    None,
                    Value::Null,
                ),
                raw_record("aaa", None, Some(true), Value::Null),
                raw_record("ccc", None, None, json!(now - 1)),
            ],
        )
        .await;
        let h = harness_with(mem, true).await;
        let (s, v) = get(&h, "/admin/api/apikeys").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["version"], 5);
        let keys = v["keys"].as_array().unwrap();
        assert_eq!(keys.len(), 3);
        // 三条 createdAt 相同 ⇒ 按 id 破平 ⇒ aaa/bbb/ccc。
        assert_eq!(keys[0]["id"], "aaa");
        assert_eq!(keys[0]["seq"], 1);
        assert_eq!(keys[0]["bucket"], "disabled");
        assert_eq!(keys[1]["bucket"], "active");
        assert_eq!(keys[1]["masked"], mask_of(Some("4444")));
        assert_eq!(keys[2]["bucket"], "expired");
        assert_eq!(
            v["counts"],
            json!({"all":3,"active":1,"disabled":1,"expired":1})
        );
    }

    /// 🔴🔴 **明文绝不进列表响应，摘要同样不进。**
    ///
    /// 判据扫的是**整个响应体文本**，不是查某个字段名 ——
    /// 查字段名的话，哪天有人把明文塞进 `debug` 或错误信息里它照样绿。
    #[tokio::test]
    async fn neither_the_secret_nor_its_digest_ever_reaches_the_list() {
        let h = harness().await;
        let (_, secret, _) = issue_one(&h, "我的密钥").await;
        let (_, body) = call(&h, Method::GET, "/admin/api/apikeys", None).await;
        assert!(!body.contains(&secret), "列表里出现了明文：{body}");
        assert!(!body.contains(&digest(&secret)), "列表里出现了摘要：{body}");
        // 掩码那一格照常在（末四位不是凭据）。
        assert!(body.contains(&mask_of(Some(&hint_of(&secret)))));
    }

    /// 列表**读一次存储、一个字节都不写**。
    ///
    /// 🔴 写一次的实现会让「面板开着那个板块」变成一根持续撬动存储的杠杆。
    #[tokio::test]
    async fn the_list_costs_exactly_one_get_and_never_writes() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = harness_with(counting, true).await;
        let (g0, p0, d0, l0) = counts.snapshot();
        get(&h, "/admin/api/apikeys").await;
        let (g1, p1, d1, l1) = counts.snapshot();
        assert_eq!((g1 - g0, p1 - p0, d1 - d0, l1 - l0), (1, 0, 0, 0));
    }

    // ── 签发 ───────────────────────────────────────────────────────────────

    /// 签发的 201：明文 + 视图 + 下一版版本号，且明文形状是 `sk-` + 32 位十六进制。
    #[tokio::test]
    async fn issuing_returns_the_secret_once_with_the_next_version() {
        let h = harness().await;
        let (s, v) = post(&h, "/admin/api/apikeys", r#"{"name":"  生产  "}"#).await;
        assert_eq!(s, StatusCode::CREATED);
        let secret = v["secret"].as_str().unwrap();
        assert!(secret.starts_with("sk-"));
        assert_eq!(secret.len(), 35);
        assert_eq!(v["version"], 1, "空表是第 0 版，签完是第 1 版");
        // **名字 trim 之后落盘**（前后空格不是名字的一部分）。
        assert_eq!(v["record"]["name"], "生产");
        assert_eq!(v["record"]["bucket"], "active");
        assert!(v["record"]["expiresAt"].is_null());
        // 🔴 视图里没有明文、没有摘要。
        assert!(v["record"].get("secret").is_none());
        assert!(v["record"].get("hash").is_none());
    }

    /// 🔴 **明文真的落进存储** —— 这是用户拍板的取舍，不是「漏存了摘要」。
    ///
    /// 同时钉住摘要也在（鉴权靠它）：只存明文不存摘要会让鉴权当场全挂。
    #[tokio::test]
    async fn the_plaintext_is_persisted_next_to_the_digest() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let h = harness_with(mem.clone(), true).await;
        let (_, secret, _) = issue_one(&h, "a").await;
        let raw = mem.get(APIKEYS_KEY).await.unwrap().unwrap();
        let rec = &raw["keys"][0];
        assert_eq!(rec["secret"], secret);
        assert_eq!(rec["hash"], digest(&secret));
        assert_eq!(rec["hint"], hint_of(&secret));
    }

    /// 签发那次写完必须让持有者作废 —— 否则新密钥要等满一个 TTL 才能用。
    ///
    /// 判据是**可观测的**：签发之后拿这把密钥直接打 `/v1/models`，必须当场通过鉴权
    /// （持有者没作废的话它会拿着签发前那份「还没签过任何密钥」的快照 401）。
    #[tokio::test]
    async fn a_freshly_issued_key_authenticates_immediately() {
        let h = harness().await;
        // 先让持有者装载一次「空表」快照：拿一把不存在的密钥打一发。
        let pre = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .header("authorization", "Bearer sk-not-a-real-key")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(pre.status(), StatusCode::UNAUTHORIZED);

        let (_, secret, _) = issue_one(&h, "a").await;
        let res = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .header("authorization", format!("Bearer {secret}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "签发之后没让持有者作废，新密钥要等满一个 TTL 才认"
        );
    }

    /// 签发打一条 `apikey.issued`（warn），**只记 id 与到期，不记明文也不记摘要**。
    #[tokio::test]
    async fn issuing_logs_an_event_without_any_credential_material() {
        let h = harness().await;
        let now = crate::adapters::now_ms();
        let (_, v) = post(
            &h,
            "/admin/api/apikeys",
            &json!({"name":"a","expiresAt": now + 600_000}).to_string(),
        )
        .await;
        let secret = v["secret"].as_str().unwrap().to_string();
        let entries = h.recorded.entries();
        let e = entries
            .iter()
            .find(|e| e.event == EVENT_APIKEY_ISSUED)
            .expect("没打 apikey.issued");
        assert_eq!(e.level, LogLevel::Warn);
        assert_eq!(e.fields.get("id").unwrap(), &json!(v["record"]["id"]));
        assert_eq!(e.fields.get("expiresAt").unwrap(), &json!(now + 600_000));
        let text = serde_json::to_string(&e.fields).unwrap() + &e.msg;
        assert!(!text.contains(&secret), "事件里出现了明文");
        assert!(!text.contains(&digest(&secret)), "事件里出现了摘要");
    }

    /// 🔴 **两次签发的明文与 id 都不同** —— CSPRNG 的可观测后果。
    #[tokio::test]
    async fn two_issues_never_collide() {
        let h = harness().await;
        let (id1, s1, v1) = issue_one(&h, "a").await;
        let (id2, s2, v2) = issue_one(&h, "a").await;
        assert_ne!(s1, s2);
        assert_ne!(id1, id2);
        assert_eq!((v1, v2), (1, 2), "每次写版本号加一");
    }

    /// 名称三档拒绝，逐条对上码与 `params`。
    ///
    /// ⚠️ **先判空再判长**：一个空名字报「太长」会把人指向反方向。
    #[tokio::test]
    async fn the_three_name_rejections_carry_their_own_codes() {
        let h = harness().await;
        let long = "x".repeat(65);
        let cases = [
            (json!({}), "name_not_a_string"),
            (json!({"name": 5}), "name_not_a_string"),
            (json!({"name": "   "}), "name_empty"),
            (json!({"name": long}), "name_too_long"),
        ];
        for (body, code) in cases {
            let (s, v) = post(&h, "/admin/api/apikeys", &body.to_string()).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(v["error"]["code"], code, "{body}");
        }
        let (_, v) = post(
            &h,
            "/admin/api/apikeys",
            &json!({"name": "x".repeat(65)}).to_string(),
        )
        .await;
        assert_eq!(v["error"]["params"]["max"], 64);
        // 64 个字符正好放行（上界是闭的）。
        let (s, _) = post(
            &h,
            "/admin/api/apikeys",
            &json!({"name": "x".repeat(64)}).to_string(),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
    }

    /// 名字按**字符数**算，不按字节数：64 个汉字必须放得进去。
    ///
    /// 回归对象：按字节算的话运维看到的「最多 64 个字符」在第 22 个汉字上就拒了。
    #[tokio::test]
    async fn the_name_limit_counts_characters_not_bytes() {
        let h = harness().await;
        let (s, _) = post(
            &h,
            "/admin/api/apikeys",
            &json!({"name": "密".repeat(64)}).to_string(),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
    }

    /// 到期两档拒绝 + 三种放行（缺席 / `null` / 未来的整数毫秒）。
    ///
    /// 🔴 **已经过去的时刻当场拒**：一把生下来就过期的密钥在面板上长得与
    /// 「签发成功」一模一样（列表里多一行「已过期」），运维会以为自己按错了什么。
    #[tokio::test]
    async fn expiry_is_validated_against_the_same_boundary_as_the_bucket() {
        let h = harness().await;
        let now = crate::adapters::now_ms();
        for (body, code) in [
            (
                json!({"name":"a","expiresAt":"soon"}),
                "expires_not_a_number",
            ),
            (json!({"name":"a","expiresAt":1.5}), "expires_not_a_number"),
            (
                json!({"name":"a","expiresAt": now - 1}),
                "expires_in_the_past",
            ),
            (json!({"name":"a","expiresAt": now}), "expires_in_the_past"),
        ] {
            let (s, v) = post(&h, "/admin/api/apikeys", &body.to_string()).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(v["error"]["code"], code, "{body}");
        }
        // 缺席 == null == 不过期。
        for body in [json!({"name":"a"}), json!({"name":"a","expiresAt":null})] {
            let (s, v) = post(&h, "/admin/api/apikeys", &body.to_string()).await;
            assert_eq!(s, StatusCode::CREATED, "{body}");
            assert!(v["record"]["expiresAt"].is_null(), "{body}");
        }
        // ⚠️ 未来那一档留 10 分钟余量而不是 `now + 1`：handler 读的是**它自己那一刻**的
        //    墙钟，比夹具这一行晚若干毫秒 ⇒ `now + 1` 会变成一条按机器快慢随机红绿的
        //    用例，而那比没有更糟。边界闭合（`== now` 也算过期）由上面那一档钉住。
        let (s, v) = post(
            &h,
            "/admin/api/apikeys",
            &json!({"name":"a","expiresAt": now + 600_000}).to_string(),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
        assert_eq!(v["record"]["expiresAt"], now + 600_000);
        assert_eq!(v["record"]["bucket"], "active");
    }

    /// 🔴 **表满了就 400，不静默截断。**
    #[tokio::test]
    async fn the_table_refuses_to_grow_past_the_max() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let full: Vec<Value> = (0..APIKEY_MAX)
            .map(|i| raw_record(&format!("id{i:04}"), None, None, Value::Null))
            .collect();
        seed_table(&mem, 3, full).await;
        let h = harness_with(mem.clone(), true).await;
        let (s, v) = post(&h, "/admin/api/apikeys", r#"{"name":"再来一把"}"#).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], "too_many_apikeys");
        assert_eq!(v["error"]["params"]["max"], 200);
        // 一个字节都没写。
        let raw = mem.get(APIKEYS_KEY).await.unwrap().unwrap();
        assert_eq!(raw["version"], 3);
        assert_eq!(raw["keys"].as_array().unwrap().len(), APIKEY_MAX);
    }

    /// 写端点的配额形态：**2 次 get + 1 次 put，0 次 delete、0 次 list。**
    ///
    /// ⚠️ 那个 2 是刻意的（见文件头配额账），钉住它是为了**改动时必须来这里表态** ——
    /// 退化成逐条存的实现会让这一格当场红。
    #[tokio::test]
    async fn a_write_costs_two_gets_and_exactly_one_put() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = harness_with(counting, true).await;
        let (g0, p0, d0, l0) = counts.snapshot();
        issue_one(&h, "a").await;
        let (g1, p1, d1, l1) = counts.snapshot();
        assert_eq!((g1 - g0, p1 - p0, d1 - d0, l1 - l0), (2, 1, 0, 0));
    }

    // ── reveal 四态 ────────────────────────────────────────────────────────

    /// `ok`：明文原样取回，且 `kind` 由端点直接给。
    #[tokio::test]
    async fn reveal_returns_the_plaintext_with_kind_ok() {
        let h = harness().await;
        let (id, secret, _) = issue_one(&h, "a").await;
        let (s, v) = get(&h, &format!("/admin/api/apikeys/{id}/reveal")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["kind"], "ok");
        assert_eq!(v["secret"], secret);
        assert!(v.get("reason").is_none(), "ok 这一档没有 reason 可言");
    }

    /// 🔴🔴 **`unavailable` 是永久事实，不许被当成一次失败。**
    ///
    /// 回归对象：签发于明文落盘之前的记录**没有**那一格，服务端从来就没有过它的明文。
    /// 与 `failed` 并档会让运维对着一把永远取不回的密钥反复点。
    /// ⚠️ 同时钉住 **`secret` 是 `null`，不是空串、不是掩码**。
    #[tokio::test]
    async fn a_key_issued_before_plaintext_reveals_as_permanently_unavailable() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        seed_table(
            &mem,
            1,
            vec![raw_record("oldkey000001", None, None, Value::Null)],
        )
        .await;
        let h = harness_with(mem, true).await;
        let (s, v) = get(&h, "/admin/api/apikeys/oldkey000001/reveal").await;
        assert_eq!(s, StatusCode::OK, "它不是一次失败，是一个如实的回答");
        assert_eq!(v["kind"], "unavailable");
        assert!(v["secret"].is_null());
        assert_eq!(v["reason"], "issued_before_plaintext");
        assert_ne!(v["secret"], json!(""), "空串冒充明文会被复制进剪贴板");
    }

    /// `gone`：这条记录已经不在表里了 ⇒ **404 + `apikey_not_found`**。
    ///
    /// ⚠️ 面板据**状态码**分档（404 ⇒ 刷新列表），不去解析 `code`。
    #[tokio::test]
    async fn revealing_a_vanished_key_is_a_404_not_a_failure() {
        let h = harness().await;
        issue_one(&h, "a").await;
        let (s, v) = get(&h, "/admin/api/apikeys/nosuchid00000/reveal").await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        assert_eq!(v["error"]["code"], "apikey_not_found");
        assert_eq!(v["error"]["type"], "not_found");
    }

    /// 🔴 **后两态结构上不可能是 2xx 响应体。**
    ///
    /// 回归对象：有人「把四态补齐」，在某条 2xx 上吐 `kind:"gone"` ——
    /// 那会让面板把一次成功的响应渲染成一条错误提示。
    #[tokio::test]
    async fn only_the_two_two_hundred_kinds_can_ever_be_a_body() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        seed_table(
            &mem,
            1,
            vec![
                raw_record(
                    "withplain001",
                    Some("sk-fixture-1111222233334444aaaabbbbccccdddd"),
                    None,
                    Value::Null,
                ),
                raw_record("noplain00001", None, None, Value::Null),
            ],
        )
        .await;
        let h = harness_with(mem, true).await;
        for id in ["withplain001", "noplain00001"] {
            let (s, v) = get(&h, &format!("/admin/api/apikeys/{id}/reveal")).await;
            assert_eq!(s, StatusCode::OK);
            let kind = v["kind"].as_str().unwrap();
            assert!(REVEAL_KINDS.contains(&kind), "{kind} 不在闭集里");
            assert_ne!(kind, REVEAL_KIND_GONE);
            assert_ne!(kind, REVEAL_KIND_FAILED);
        }
    }

    /// 🔴 **一条被写坏成空串的明文，在 reveal 之前一层就被拒了。**
    ///
    /// 这一格钉的是 [`REVEAL_KIND_FAILED`] 那句「结构上不可能是 2xx 响应体」
    /// **今天为什么成立**：[`crate::core::keys::api_keys::narrow_record`] 把
    /// `secret` 是空串的记录整条判成读不出来 ⇒ 整张表 `Invalid` ⇒ 这条端点回 409。
    ///
    /// ⚠️ **它不能替代 [`reveal`] 里那一层判定**：窄化那一层管的是
    /// 「这条记录能不能用」，而不是「这个串能不能交给剪贴板」。两者的职责不同，
    /// 放宽前者是个随时可能发生的合理改动 —— 而那天起，把空串当明文发出去的
    /// 就只剩共用状态机在挡。
    #[tokio::test]
    async fn an_empty_plaintext_is_rejected_one_layer_earlier_than_reveal() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let mut rec = raw_record("emptysecret1", None, None, Value::Null);
        // ⚠️ `raw_record` 在 `secret: None` 时**不写** `secret` 字段，
        //    所以这里手工塞一个空串进去 —— 那正是「被外部写坏」的形状。
        rec["secret"] = json!("");
        seed_table(&mem, 1, vec![rec]).await;
        let h = harness_with(mem, true).await;
        let (s, v) = get(&h, "/admin/api/apikeys/emptysecret1/reveal").await;
        assert_eq!(
            s,
            StatusCode::CONFLICT,
            "空串明文 ⇒ 整条记录读不出来 ⇒ 如实 409，而不是「没有这把密钥」"
        );
        assert_eq!(v["error"]["code"], "apikeys_unreadable");

        // 而共用状态机**独立地**也不认空串 —— 两道闸，别删其中任何一道。
        use crate::core::admin::view::reveal::{RevealKind, reveal_kind};
        assert_eq!(reveal_kind(Some(""), true), RevealKind::Unavailable);
        assert_eq!(reveal_kind(Some(""), false), RevealKind::Failed);
    }

    /// 取明文**必留痕**，事件里只有 id 与「有没有明文」，🔴 **没有明文本身**。
    #[tokio::test]
    async fn revealing_is_always_audited_but_never_logs_the_secret() {
        let h = harness().await;
        let (id, secret, _) = issue_one(&h, "a").await;
        get(&h, &format!("/admin/api/apikeys/{id}/reveal")).await;
        let entries = h.recorded.entries();
        let e = entries
            .iter()
            .find(|e| e.event == EVENT_APIKEY_REVEALED)
            .expect("取明文没留痕");
        assert_eq!(e.level, LogLevel::Warn);
        assert_eq!(e.fields.get("id").unwrap(), &json!(id));
        assert_eq!(e.fields.get("available").unwrap(), &json!(true));
        assert!(!serde_json::to_string(&e.fields).unwrap().contains(&secret));
    }

    /// 表坏了时 reveal 回 **409 `apikeys_unreadable`**，不是 404。
    ///
    /// 🔴 两者的处置完全不同：404 让运维去重发一把（于是覆盖掉那张表），
    /// 409 才是「把那张表捞回来」。
    #[tokio::test]
    async fn revealing_against_a_broken_table_is_a_conflict_not_a_not_found() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        mem.put(APIKEYS_KEY, json!([1, 2, 3]), None).await.unwrap();
        let h = harness_with(mem, true).await;
        let (s, v) = get(&h, "/admin/api/apikeys/whatever0001/reveal").await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(v["error"]["code"], "apikeys_unreadable");
    }

    // ── PATCH ──────────────────────────────────────────────────────────────

    /// 改名：200 + 新视图 + 下一版版本号。
    #[tokio::test]
    async fn patching_a_name_bumps_the_version_and_returns_the_new_view() {
        let h = harness().await;
        let (id, _, v0) = issue_one(&h, "旧名").await;
        let (s, v) = patch_req(
            &h,
            &format!("/admin/api/apikeys/{id}"),
            &json!({"version": v0, "name": "新名"}).to_string(),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["ok"], true);
        assert_eq!(v["record"]["name"], "新名");
        assert_eq!(v["version"], v0 + 1);
    }

    /// 停用 ⇒ 分档变 `disabled`，并打一条 **warn** 的 `apikey.disabled`；
    /// 重新启用 ⇒ `apikey.enabled`（info）。
    ///
    /// ⚠️ 一次 PATCH 最多打一条事件，**停用优先**。
    #[tokio::test]
    async fn disabling_and_enabling_each_log_their_own_event() {
        let h = harness().await;
        let (id, _, v0) = issue_one(&h, "a").await;
        let (s, v) = patch_req(
            &h,
            &format!("/admin/api/apikeys/{id}"),
            &json!({"version": v0, "disabled": true, "name": "顺手改个名"}).to_string(),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["record"]["bucket"], "disabled");
        assert_eq!(v["record"]["disabled"], true);

        let (s, v2) = patch_req(
            &h,
            &format!("/admin/api/apikeys/{id}"),
            &json!({"version": v["version"], "disabled": false}).to_string(),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v2["record"]["bucket"], "active");

        let events = h.recorded.events();
        assert_eq!(
            events
                .iter()
                .filter(|e| **e == EVENT_APIKEY_DISABLED)
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| **e == EVENT_APIKEY_ENABLED)
                .count(),
            1
        );
    }

    /// **只改名不打事件** —— 名字不改变任何鉴权行为，而事件板块说的是
    /// 「它为什么突然不能用了」。
    #[tokio::test]
    async fn renaming_alone_logs_nothing() {
        let h = harness().await;
        let (id, _, v0) = issue_one(&h, "a").await;
        patch_req(
            &h,
            &format!("/admin/api/apikeys/{id}"),
            &json!({"version": v0, "name": "b"}).to_string(),
        )
        .await;
        let events = h.recorded.events();
        assert!(!events.contains(&EVENT_APIKEY_DISABLED));
        assert!(!events.contains(&EVENT_APIKEY_ENABLED));
    }

    /// 🔴🔴 **显式 `null` 是「改成不过期」，缺席才是「这次不动它」。**
    ///
    /// 回归对象：两者合并之后，一次「只改个名」会把到期时刻悄悄清成「永不过期」
    /// —— 一把本该在周五失效的密钥就此长生。
    #[tokio::test]
    async fn an_absent_expires_at_is_not_the_same_as_an_explicit_null() {
        let h = harness().await;
        let now = crate::adapters::now_ms();
        let (_, v) = post(
            &h,
            "/admin/api/apikeys",
            &json!({"name":"a","expiresAt": now + 600_000}).to_string(),
        )
        .await;
        let id = v["record"]["id"].as_str().unwrap().to_string();
        let mut ver = v["version"].as_i64().unwrap();

        // ① 缺席 ⇒ 不动它。
        let (_, v1) = patch_req(
            &h,
            &format!("/admin/api/apikeys/{id}"),
            &json!({"version": ver, "name":"b"}).to_string(),
        )
        .await;
        assert_eq!(v1["record"]["expiresAt"], now + 600_000);
        ver = v1["version"].as_i64().unwrap();

        // ② 显式 null ⇒ 改成不过期。
        let (_, v2) = patch_req(
            &h,
            &format!("/admin/api/apikeys/{id}"),
            &json!({"version": ver, "expiresAt": null}).to_string(),
        )
        .await;
        assert!(v2["record"]["expiresAt"].is_null());
    }

    /// PATCH 的五档拒绝，逐条对上码。
    ///
    /// ⚠️ **顺序也被钉住**：未知字段 → 版本号 → 字段各自的校验 → 空 patch。
    /// 一个 `{}` 报的是 `version_not_a_number` 而不是 `empty_patch`
    /// —— 版本号是必填的并发凭证，它缺席时后面的校验没有意义。
    #[tokio::test]
    async fn patch_rejections_carry_their_codes_in_a_pinned_order() {
        let h = harness().await;
        let (id, _, v0) = issue_one(&h, "a").await;
        let uri = format!("/admin/api/apikeys/{id}");
        let cases = [
            (json!({"version": v0, "nope": 1}), "unknown_field"),
            (json!({}), "version_not_a_number"),
            (json!({"version": "7"}), "version_not_a_number"),
            (json!({"version": -1}), "version_not_a_number"),
            (json!({"version": 1.5}), "version_not_a_number"),
            (json!({"version": true}), "version_not_a_number"),
            (json!({"version": v0}), "empty_patch"),
            (json!({"version": v0, "disabled": "true"}), "not_a_boolean"),
            (json!({"version": v0, "name": 5}), "name_not_a_string"),
            (
                json!({"version": v0, "expiresAt": 1}),
                "expires_in_the_past",
            ),
        ];
        for (body, code) in cases {
            let (s, v) = patch_req(&h, &uri, &body.to_string()).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(v["error"]["code"], code, "{body}");
        }
        let (_, v) = patch_req(&h, &uri, &json!({"version": v0, "disabled": 1}).to_string()).await;
        assert_eq!(v["error"]["params"]["field"], "disabled");
    }

    /// 畸形 JSON / 非对象请求体，各自有码。
    #[tokio::test]
    async fn a_malformed_body_has_its_own_code() {
        let h = harness().await;
        for (body, code) in [
            ("{", "bad_json"),
            ("", "bad_json"),
            ("[1,2]", "body_not_an_object"),
            ("\"x\"", "body_not_an_object"),
        ] {
            let (s, v) = post(&h, "/admin/api/apikeys", body).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(v["error"]["code"], code, "{body}");
        }
    }

    /// 🔴 **拿旧 version 写 ⇒ 409 `stale_write`，而且一个字节都没写。**
    #[tokio::test]
    async fn a_stale_version_is_refused_without_writing_anything() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let h = harness_with(mem.clone(), true).await;
        let (id, _, _) = issue_one(&h, "a").await;
        issue_one(&h, "b").await; // 版本推到 2
        let before = mem.get(APIKEYS_KEY).await.unwrap().unwrap();

        let (s, v) = patch_req(
            &h,
            &format!("/admin/api/apikeys/{id}"),
            &json!({"version": 1, "name": "抢写"}).to_string(),
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(v["error"]["code"], "stale_write");
        assert_eq!(v["error"]["params"]["expected"], 1);
        assert_eq!(v["error"]["params"]["actual"], 2);
        assert_eq!(mem.get(APIKEYS_KEY).await.unwrap().unwrap(), before);
    }

    /// 版本对得上但 id 不存在 ⇒ **404**，且不落盘。
    #[tokio::test]
    async fn patching_an_unknown_id_is_a_not_found() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let h = harness_with(mem.clone(), true).await;
        let (_, _, v0) = issue_one(&h, "a").await;
        let before = mem.get(APIKEYS_KEY).await.unwrap().unwrap();
        let (s, v) = patch_req(
            &h,
            "/admin/api/apikeys/nosuchid00000",
            &json!({"version": v0, "name": "x"}).to_string(),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        assert_eq!(v["error"]["code"], "apikey_not_found");
        assert_eq!(mem.get(APIKEYS_KEY).await.unwrap().unwrap(), before);
    }

    /// 🔴 **表结构不认时写路径一律拒绝落盘，原字节原地不动。**
    #[tokio::test]
    async fn a_broken_table_is_never_overwritten_by_any_write_endpoint() {
        let junk = json!({"version": "七", "keys": []});
        for (method, uri, body) in [
            (Method::POST, "/admin/api/apikeys", Some(r#"{"name":"a"}"#)),
            (
                Method::PATCH,
                "/admin/api/apikeys/abc000000001",
                Some(r#"{"version":0,"name":"a"}"#),
            ),
            (
                Method::DELETE,
                "/admin/api/apikeys/abc000000001?version=0",
                None,
            ),
            (Method::POST, APIKEYS_PURGE_PATH, Some(r#"{"version":0}"#)),
        ] {
            let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
            mem.put(APIKEYS_KEY, junk.clone(), None).await.unwrap();
            let h = harness_with(mem.clone(), true).await;
            let (s, b) = call(&h, method.clone(), uri, body).await;
            assert_eq!(s, StatusCode::CONFLICT, "{method} {uri}");
            let v: Value = serde_json::from_str(&b).unwrap();
            assert_eq!(v["error"]["code"], "apikeys_unreadable", "{method} {uri}");
            assert_eq!(
                mem.get(APIKEYS_KEY).await.unwrap().unwrap(),
                junk,
                "{method} {uri} 动了原字节"
            );
        }
    }

    // ── DELETE ─────────────────────────────────────────────────────────────

    /// 删除成功 ⇒ **204 空体**，记录真的没了，并打一条 warn。
    #[tokio::test]
    async fn deleting_returns_204_and_really_removes_the_record() {
        let h = harness().await;
        let (id, _, v0) = issue_one(&h, "a").await;
        let (s, body) = call(
            &h,
            Method::DELETE,
            &format!("/admin/api/apikeys/{id}?version={v0}"),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        assert_eq!(body, "", "204 必须是空体");
        let (_, v) = get(&h, "/admin/api/apikeys").await;
        assert_eq!(v["keys"].as_array().unwrap().len(), 0);
        assert_eq!(v["version"], v0 + 1);
        let entries = h.recorded.entries();
        let e = entries
            .iter()
            .find(|e| e.event == EVENT_APIKEY_DELETED)
            .expect("删除没留痕");
        assert_eq!(e.level, LogLevel::Warn);
        assert_eq!(e.fields.get("id").unwrap(), &json!(id));
    }

    /// ⚠️ **`version` 走查询参数**：缺失 / 空值 / 非法一律 400，不匹配 409。
    ///
    /// 🔴 空值那一档是对 TS 的刻意偏离（`Number("")` 是 0）：
    /// 一道**静默变弱的**并发护栏比没有护栏更坏。
    #[tokio::test]
    async fn the_delete_version_comes_from_the_query_string() {
        let h = harness().await;
        let (id, _, v0) = issue_one(&h, "a").await;
        for q in [
            "",
            "?version=",
            "?version=abc",
            "?version=-1",
            "?version=1.5",
            "?v=1",
        ] {
            let (s, b) = call(
                &h,
                Method::DELETE,
                &format!("/admin/api/apikeys/{id}{q}"),
                None,
            )
            .await;
            let v: Value = serde_json::from_str(&b).unwrap();
            assert_eq!(s, StatusCode::BAD_REQUEST, "q={q}");
            assert_eq!(v["error"]["code"], "version_not_a_number", "q={q}");
        }
        // 版本不匹配 ⇒ 409，记录还在。
        let (s, b) = call(
            &h,
            Method::DELETE,
            &format!("/admin/api/apikeys/{id}?version={}", v0 + 7),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(
            serde_json::from_str::<Value>(&b).unwrap()["error"]["code"],
            "stale_write"
        );
        let (_, v) = get(&h, "/admin/api/apikeys").await;
        assert_eq!(v["keys"].as_array().unwrap().len(), 1);
    }

    /// ⚠️ **删除刻意没有「必须先停用才能删」那道前置**（上游池那边有）。
    ///
    /// 理由：吊销一把已泄漏的密钥要越快越好，而这一族删掉的只是我们自己签发的
    /// 一条验证记录，重发一把是完全正常的操作。
    #[tokio::test]
    async fn deleting_an_enabled_key_needs_no_prior_disable() {
        let h = harness().await;
        let (id, _, v0) = issue_one(&h, "a").await;
        let (s, _) = call(
            &h,
            Method::DELETE,
            &format!("/admin/api/apikeys/{id}?version={v0}"),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT, "这一族没有 must_disable_first");
    }

    /// 删掉之后那把密钥真的不认了（持有者被作废）。
    #[tokio::test]
    async fn a_deleted_key_stops_authenticating() {
        let h = harness().await;
        let (id, secret, v0) = issue_one(&h, "a").await;
        let ok = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .header("authorization", format!("Bearer {secret}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(ok.status(), StatusCode::UNAUTHORIZED);

        call(
            &h,
            Method::DELETE,
            &format!("/admin/api/apikeys/{id}?version={v0}"),
            None,
        )
        .await;
        let after = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .header("authorization", format!("Bearer {secret}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
    }

    // ── 清理失效 ───────────────────────────────────────────────────────────

    /// 🔴 **删掉的恰好是「此刻用不了的那些」，可用的一把不动。**
    ///
    /// 判据与面板那两张统计卡之和同源；另写一条判据会让按钮删掉的条数
    /// 与它自己写的那个数字对不上。
    #[tokio::test]
    async fn purging_removes_exactly_the_unusable_ones() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let now = crate::adapters::now_ms();
        seed_table(
            &mem,
            4,
            vec![
                raw_record("active000001", None, None, Value::Null),
                raw_record("active000002", None, Some(false), json!(now + 600_000)),
                raw_record("disabled0001", None, Some(true), Value::Null),
                raw_record("expired00001", None, None, json!(now - 1)),
                // 既停用又过期 —— 只算一条。
                raw_record("both00000001", None, Some(true), json!(now - 1)),
            ],
        )
        .await;
        let h = harness_with(mem, true).await;
        let (s, v) = post(&h, APIKEYS_PURGE_PATH, r#"{"version":4}"#).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["deleted"], 3);
        assert_eq!(v["remaining"], 2);
        assert_eq!(v["version"], 5);

        let (_, l) = get(&h, "/admin/api/apikeys").await;
        let ids: Vec<&str> = l["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["active000001", "active000002"]);
    }

    /// 没有可清的时候：`deleted:0`，**不打事件**，但版本号照样推进
    /// （整表覆写恒是一次 put —— 契约里那个 `version` 是「下一版」）。
    #[tokio::test]
    async fn purging_nothing_logs_nothing() {
        let h = harness().await;
        issue_one(&h, "a").await;
        let (s, v) = post(&h, APIKEYS_PURGE_PATH, r#"{"version":1}"#).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["deleted"], 0);
        assert_eq!(v["remaining"], 1);
        assert_eq!(v["version"], 2);
        assert!(!h.recorded.events().contains(&EVENT_APIKEY_DELETED));
    }

    /// 清理打的事件**只记数，不逐条列 id**（`fields` 的值是标量）。
    #[tokio::test]
    async fn the_purge_event_carries_counts_not_ids() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        seed_table(
            &mem,
            1,
            vec![
                raw_record("disabled0001", None, Some(true), Value::Null),
                raw_record("active000001", None, None, Value::Null),
            ],
        )
        .await;
        let h = harness_with(mem, true).await;
        post(&h, APIKEYS_PURGE_PATH, r#"{"version":1}"#).await;
        let entries = h.recorded.entries();
        let e = entries
            .iter()
            .find(|e| e.event == EVENT_APIKEY_DELETED)
            .expect("清理没留痕");
        assert_eq!(e.level, LogLevel::Warn);
        assert_eq!(e.fields.get("count").unwrap(), &json!(1));
        assert_eq!(e.fields.get("remaining").unwrap(), &json!(1));
        assert!(
            !serde_json::to_string(&e.fields)
                .unwrap()
                .contains("disabled0001"),
            "一次清理的 id 可能有上百个，不许逐条挂在事件上"
        );
    }

    /// 清理也收 `version`，而且走**请求体**（不是查询参数）。
    #[tokio::test]
    async fn purging_requires_the_version_in_the_body() {
        let h = harness().await;
        issue_one(&h, "a").await;
        for (uri, body, code) in [
            (APIKEYS_PURGE_PATH, "{}", "version_not_a_number"),
            (APIKEYS_PURGE_PATH, r#"{"version":0}"#, "stale_write"),
            (
                APIKEYS_PURGE_PATH,
                r#"{"version":1,"x":1}"#,
                "unknown_field",
            ),
        ] {
            let (_, v) = post(&h, uri, body).await;
            assert_eq!(v["error"]["code"], code, "{body}");
        }
        // 查询参数里的 version 对这条端点无效（它只读请求体）。
        let (s, v) = post(&h, &format!("{APIKEYS_PURGE_PATH}?version=1"), "{}").await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], "version_not_a_number");
    }

    // ── 存储失败 ───────────────────────────────────────────────────────────

    /// 读存储失败 ⇒ **500 + 一条 `admin.storage_failed`**，
    /// 且 🔴 **错误文本不进响应体**（它可能带路径）。
    #[tokio::test]
    async fn a_storage_failure_is_a_500_that_leaves_a_trace_but_no_details() {
        let mem = Arc::new(MemoryStorage::new());
        let h = harness_with(mem.clone(), true).await;
        // ⚠️ 装配之后才布故障：装配自己也读存储，提前布会让这一格测的是别人。
        mem.fail_next("EACCES /data/store.json");
        let (s, body) = call(&h, Method::GET, "/admin/api/apikeys", None).await;
        assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!body.contains("EACCES"), "存储错误文本漏进了响应体：{body}");
        assert!(
            h.recorded.events().contains(&EVENT_ADMIN_STORAGE_FAILED),
            "500 不许静默"
        );
    }

    // ── 纯函数那几格 ───────────────────────────────────────────────────────

    /// 🔴 `core` 那两个问题枚举吐出来的码，**每一个**都在 [`AdminErrorCode`] 的闭集里。
    ///
    /// 回归对象：`code_of` 表外 panic；给 `core` 加一档新问题而忘了加对应的管理码时，
    /// 线上现象是**那一条校验路径整个 500**。
    #[test]
    fn every_core_problem_code_maps_to_an_admin_code() {
        for c in [
            ApiKeyNameProblem::NameNotAString.code(),
            ApiKeyNameProblem::NameEmpty.code(),
            ApiKeyNameProblem::NameTooLong.code(),
            ApiKeyExpiryProblem::ExpiresNotANumber.code(),
            ApiKeyExpiryProblem::ExpiresInThePast.code(),
        ] {
            assert_eq!(code_of(c).as_str(), c);
        }
    }

    /// `?version=` 的解析：**空值、缺席、非法一律 `Err`**，合法值原样取回。
    ///
    /// ⚠️ 重复键取第一个（与 `URLSearchParams.get` 同语义）。
    #[test]
    fn parsing_the_version_query_param() {
        assert_eq!(version_from_query(Some("version=7")).unwrap(), 7);
        assert_eq!(version_from_query(Some("version=0")).unwrap(), 0);
        assert_eq!(version_from_query(Some("version=7.0")).unwrap(), 7);
        assert_eq!(
            version_from_query(Some("a=1&version=3&version=9")).unwrap(),
            3
        );
        for raw in [
            None,
            Some(""),
            Some("version="),
            Some("version=abc"),
            Some("version=-1"),
            Some("version=1.5"),
            Some("v=1"),
        ] {
            assert!(version_from_query(raw).is_err(), "{raw:?} 该被拒");
        }
    }

    /// 请求体里的 `version`：整数 / `3.0` 放行；字符串、布尔、负数、小数一律拒。
    #[test]
    fn checking_the_body_version() {
        assert_eq!(checked_version(Some(&json!(0))).unwrap(), 0);
        assert_eq!(checked_version(Some(&json!(3.0))).unwrap(), 3);
        for v in [json!("3"), json!(true), json!(-1), json!(1.5), json!(null)] {
            assert!(checked_version(Some(&v)).is_err(), "{v} 该被拒");
        }
        assert!(checked_version(None).is_err());
    }

    /// `require_fresh` 的 `params` 是**两个数**，一个汉字都没有。
    ///
    /// 🔴 `params` 会被插进 ja/ko 的字典串里，混进中文等于把中文漏回屏幕上。
    #[tokio::test]
    async fn the_stale_write_params_are_two_plain_numbers() {
        let t = ApiKeyTable {
            version: 9,
            keys: Vec::new(),
        };
        assert!(require_fresh(&t, 9).is_ok(), "版本对得上就该放行");
        let res = require_fresh(&t, 3).unwrap_err().into_response();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v["error"]["code"], "stale_write");
        assert_eq!(v["error"]["params"], json!({"expected": 3, "actual": 9}));
        let params = v["error"]["params"].to_string();
        assert!(params.is_ascii(), "params 里一个汉字都不许有：{params}");
    }

    /// 本组用到的每一条码都在 [`AdminErrorCode`] 的闭集里，且 `params` 键集
    /// ⊆ 它声明的允许集。
    ///
    /// 🔴 `params` 会被原样画到面板上，所以它的面必须是闭的。
    #[tokio::test]
    async fn every_param_this_group_emits_is_declared_on_its_code() {
        let h = harness().await;
        let (id, _, v0) = issue_one(&h, "a").await;
        let probes: Vec<(Method, String, Option<String>)> = vec![
            (
                Method::POST,
                "/admin/api/apikeys".into(),
                Some(json!({"name": "x".repeat(65)}).to_string()),
            ),
            (
                Method::POST,
                "/admin/api/apikeys".into(),
                Some(r#"{"name":"a","nope":1}"#.into()),
            ),
            (
                Method::PATCH,
                format!("/admin/api/apikeys/{id}"),
                Some(json!({"version": v0, "disabled": 1}).to_string()),
            ),
            (
                Method::PATCH,
                format!("/admin/api/apikeys/{id}"),
                Some(json!({"version": 99, "name": "b"}).to_string()),
            ),
        ];
        for (m, uri, body) in probes {
            let (_, v) = json_call(&h, m, &uri, body.as_deref()).await;
            let code = v["error"]["code"].as_str().expect("这一族必须带 code");
            let declared = AdminErrorCode::ALL
                .iter()
                .find(|c| c.as_str() == code)
                .unwrap_or_else(|| panic!("{code} 不在闭集里"))
                .allowed_params();
            for k in v["error"]["params"].as_object().unwrap().keys() {
                assert!(declared.contains(&k.as_str()), "{code} 不许带 params.{k}");
            }
        }
    }
}
