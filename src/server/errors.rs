//! 三种错误信封 = **三个 Rust 类型**。
//!
//! 🔴 **不许合并成一个带 `Option<code>` 的结构。** TS 版在这一点上逐字论证过：
//! 合成一个之后，「顺手给网关那一族也传一个码」就只差一个参数 —— 而那正是明确
//! 不做的那一半，半做会造出「一半端点有码、一半没有」的**第三种状态**。
//!
//! | 信封 | 形状 | 谁用 |
//! |---|---|---|
//! | [`GatewayError`] | `{"error":{"type","message"}}` | 四协议路由、兜底 404/405/500 |
//! | [`AdminError`] | `{"error":{"type","code","message","params"}}` | `/admin/api/*` |
//! | [`DispatchError`] | `{"error":{"reason","message"}}` | 转发失败（⚠️ 用的是 `reason` 不是 `type`） |
//!
//! # 三条硬约束（对标 kiro 03 §5.3）
//!
//! 1. 🔴 **对外消息永不含凭据、内部堆栈、上游原始体。** 真正的细节只进日志。
//! 2. **能对客户端说清楚的就说全**：确定性的、可安全外露的错误要把「人能照着做的
//!    动作」写出来。
//! 3. **每个协议有自己的错误体形状**，所以上游异常要当**一等结果**往上传，
//!    由各协议层自己成型 —— 不在这一层拍死。

use axum::Json;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Map, Value};
use std::fmt;

// ⚠️ **三个信封一律用 `#[derive(Serialize)]` 的结构体出，不用 `json!` 宏。**
//
// 理由是实打实的：`serde_json` 默认的 `Map` 是 `BTreeMap`，`json!({"type":…,"message":…})`
// 出来的键**按字典序重排**（`message` 跑到 `type` 前面），而 TS 版的对象字面量是
// **插入序**。JSON 的键序在语义上不重要，但这三个信封被五语言 API.md **逐字写死**，
// 文档里的示例与真实响应对不上就是在骗人；移植期拿 `diff` 对两侧响应也会全是噪声。
// 结构体的字段顺序 = 序列化顺序，这条不需要额外依赖就能钉住。
//
// ⚠️ **剩下的一处偏差如实登记**：`AdminError::params` 里面那一层仍然是 `Map`，
// 所以多于一个参数时它的键是字典序（`stale_write` 的 `actual` 会排在 `expected` 前）。
// 要抹平就得给 serde_json 开 `preserve_order` feature（多一个 indexmap 依赖），
// 本轮**不开**：那一层是给面板做插值用的，面板按键名取值，不按顺序。

// ── 1. 网关信封 ─────────────────────────────────────────────────────────────

/// `{"error":{"type","message"}}`。**被五语言 API.md 逐字写死，是对外契约。**
#[derive(Debug, Clone)]
pub struct GatewayError {
    pub status: StatusCode,
    /// 对外错误类型串。**不泄露内部细节。**
    pub err_type: &'static str,
    pub message: String,
    /// 协议要求的额外响应头。
    ///
    /// 今天唯一的用户：**405 必须带 `Allow`**（RFC 9110 §15.5.6 是 MUST）。
    /// 做成信封的一部分而不是让调用方自己拼 `Response`，是因为**信封只能有一个出口**——
    /// 否则「哪天要给 `error` 加一个字段」会漏掉绕过去的那几处。
    pub extra_headers: Vec<(HeaderName, HeaderValue)>,
}

impl GatewayError {
    pub fn new(status: StatusCode, err_type: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            err_type,
            message: message.into(),
            extra_headers: Vec::new(),
        }
    }

    /// 请求体不是合法 JSON。
    ///
    /// 🔴 **必须是 400 JSON，不是 500、更不是 axum 默认的 422 纯文本**：
    /// OpenAI 生态的客户端一律按 `error.message` 解析错误，拿到纯文本会在解析处
    /// **二次抛错**（JSONDecodeError），把「少传一个字段」这种一眼可修的参数问题
    /// 变成客户端内部错误。
    pub fn bad_json() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "请求体不是合法的 JSON",
        )
    }

    /// 请求体超过网关上限。
    ///
    /// 🔴 **必须是 JSON 信封**，理由与 [`Self::bad_json`] 逐字相同：axum 默认的 413 是
    /// `content-type: text/plain` + `"Failed to buffer the request body: length limit exceeded"`,
    /// 而 OpenAI 生态的客户端一律先 `response.json()` ⇒ 拿到纯文本当场抛 `JSONDecodeError`,
    /// 把「请求体太大」变成「客户端内部错误」。
    ///
    /// ⚠️ **那个数要写进报文**：不说上限是多少，调用方只能靠二分试出来。
    /// 它不是秘密（上限本身由部署方定，且任何一次超限请求都能观测到）。
    pub fn payload_too_large(max_bytes: usize) -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_request_error",
            format!("请求体超过网关上限（{max_bytes} 字节）"),
        )
    }

    /// 没有这条路由。**是 JSON 信封，不是 axum 默认的空体 404。**
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "没有这条路由")
    }

    /// 方法不对。`allow` = 逗号分隔的候选方法集。
    ///
    /// 🔴 **`Allow` 必须真的出现**（RFC 9110 §15.5.6 是 MUST），但它有两种来路：
    /// - `Some(..)` —— 调用方手上已经有那一份候选集；
    /// - `None` —— **交给路由框架按它的实际注册表填**。`app.rs` 的接线走这一支：
    ///   `02 §2.2` 第 1 步要的就是「从当前实际注册表里取候选方法集，不写死常量表」，
    ///   而手抄一张方法表迟早与真路由漂开 —— 现象是报错信息教客户端去用一个不存在的方法。
    ///
    /// ⚠️ `None` 这一支**刻意不塞一个空的 `Allow`**：空头值会占住那一格，
    /// 反而把框架要填的那份挤掉。
    pub fn method_not_allowed(allow: Option<&str>) -> Self {
        let mut e = Self::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "这条路径不接受该方法",
        );
        if let Some(allow) = allow
            && let Ok(v) = HeaderValue::from_str(allow)
        {
            e.extra_headers.push((axum::http::header::ALLOW, v));
        }
        e
    }

    /// 未预期异常的兜底。
    ///
    /// 🔴 **绝不回显内部错误文本**（可能含上游 URL、路径、栈帧）；
    /// 🔴 **也绝不静默吞掉** —— 调用方必须同时打一条 `http.unhandled_error` 事件。
    /// 实测过 14 次 500 之后事件流零记录，运维无从排障。
    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "网关内部错误",
        )
    }
}

impl fmt::Display for GatewayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}: {}",
            self.status.as_u16(),
            self.err_type,
            self.message
        )
    }
}

impl std::error::Error for GatewayError {}

/// 线格式。字段顺序 = 序列化顺序，见文件头的说明。
#[derive(Serialize)]
struct GatewayWire {
    error: GatewayWireInner,
}

#[derive(Serialize)]
struct GatewayWireInner {
    r#type: &'static str,
    message: String,
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let body = GatewayWire {
            error: GatewayWireInner {
                r#type: self.err_type,
                message: self.message,
            },
        };
        let mut res = (self.status, Json(body)).into_response();
        // ⚠️ 额外头在 content-type 之后加，理论上能盖掉它。**别拿它干这件事**：
        //    信封的类型就是信封的一部分。
        for (k, v) in self.extra_headers {
            res.headers_mut().insert(k, v);
        }
        res
    }
}

// ── 2. 管理信封 ─────────────────────────────────────────────────────────────

/// 管理接口错误码的**闭集**。
///
/// ⚠️ **`code` 才是对外契约，`message` 不是。** 面板靠 `code` 查五语言字典，
/// **不许解析 `message`**；`message` 的措辞可以随时改而不算破坏兼容。
/// 反过来，`message` 也**一个字都不许删**：它是给日志与 API 客户端的排障信息，
/// `code` 是**并列**新增的一格，不是替换。
///
/// 🔴 **网关那一族（[`GatewayError`]）一个码都不许有。** 半做会造出第三种状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AdminErrorCode {
    AdminUnavailable,
    AdminUnauthorized,
    BadJson,
    BodyNotAnObject,
    UnknownField,
    KeyNotFound,
    MustDisableFirst,
    NotABoolean,
    NoteNotAString,
    NoteTooLong,
    EmptyPatch,
    KeysNotAStringArray,
    TooManyImportKeys,
    NotABulkOp,
    IdsNotAStringArray,
    TooManyBulkIds,
    ApikeyNotFound,
    NameNotAString,
    NameEmpty,
    NameTooLong,
    ExpiresNotANumber,
    ExpiresInThePast,
    TooManyApikeys,
    VersionNotANumber,
    StaleWrite,
    ApikeysUnreadable,
}

impl AdminErrorCode {
    /// 🔴 **唯一真源。** 判据遍历它；面板字典对着它对账。
    pub const ALL: &'static [AdminErrorCode] = &[
        Self::AdminUnavailable,
        Self::AdminUnauthorized,
        Self::BadJson,
        Self::BodyNotAnObject,
        Self::UnknownField,
        Self::KeyNotFound,
        Self::MustDisableFirst,
        Self::NotABoolean,
        Self::NoteNotAString,
        Self::NoteTooLong,
        Self::EmptyPatch,
        Self::KeysNotAStringArray,
        Self::TooManyImportKeys,
        Self::NotABulkOp,
        Self::IdsNotAStringArray,
        Self::TooManyBulkIds,
        Self::ApikeyNotFound,
        Self::NameNotAString,
        Self::NameEmpty,
        Self::NameTooLong,
        Self::ExpiresNotANumber,
        Self::ExpiresInThePast,
        Self::TooManyApikeys,
        Self::VersionNotANumber,
        Self::StaleWrite,
        Self::ApikeysUnreadable,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AdminUnavailable => "admin_unavailable",
            Self::AdminUnauthorized => "admin_unauthorized",
            Self::BadJson => "bad_json",
            Self::BodyNotAnObject => "body_not_an_object",
            Self::UnknownField => "unknown_field",
            Self::KeyNotFound => "key_not_found",
            Self::MustDisableFirst => "must_disable_first",
            Self::NotABoolean => "not_a_boolean",
            Self::NoteNotAString => "note_not_a_string",
            Self::NoteTooLong => "note_too_long",
            Self::EmptyPatch => "empty_patch",
            Self::KeysNotAStringArray => "keys_not_a_string_array",
            Self::TooManyImportKeys => "too_many_import_keys",
            Self::NotABulkOp => "not_a_bulk_op",
            Self::IdsNotAStringArray => "ids_not_a_string_array",
            Self::TooManyBulkIds => "too_many_bulk_ids",
            Self::ApikeyNotFound => "apikey_not_found",
            Self::NameNotAString => "name_not_a_string",
            Self::NameEmpty => "name_empty",
            Self::NameTooLong => "name_too_long",
            Self::ExpiresNotANumber => "expires_not_a_number",
            Self::ExpiresInThePast => "expires_in_the_past",
            Self::TooManyApikeys => "too_many_apikeys",
            Self::VersionNotANumber => "version_not_a_number",
            Self::StaleWrite => "stale_write",
            Self::ApikeysUnreadable => "apikeys_unreadable",
        }
    }

    /// 这条码允许携带哪些 `params` 键。
    ///
    /// 存在的理由：`params` **会被原样画到面板上**，所以它的面必须是闭的。
    /// 判据对着它逐条查「这条码回的 params 键集 ⊆ 这里声明的集合」。
    pub const fn allowed_params(self) -> &'static [&'static str] {
        match self {
            Self::UnknownField => &["fields"],
            Self::NotABoolean => &["field"],
            Self::NoteTooLong
            | Self::TooManyImportKeys
            | Self::TooManyBulkIds
            | Self::NameTooLong
            | Self::TooManyApikeys => &["max"],
            Self::NotABulkOp => &["ops"],
            Self::StaleWrite => &["expected", "actual"],
            _ => &[],
        }
    }
}

impl fmt::Display for AdminErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `{"error":{"type","code","message","params"}}`。
#[derive(Debug, Clone)]
pub struct AdminError {
    pub status: StatusCode,
    pub err_type: &'static str,
    pub code: AdminErrorCode,
    pub message: String,
    /// 🔴 三条口径，每条都要有一格判据：
    /// ① **一个汉字都不许有**（它会被插进 ja/ko 的句子里）；
    /// ② **永不放凭据，也永不放请求体里任何字段的值**；
    /// ③ ⚠️ **但它确实会逐字回显调用方送来的字段名**（`unknown_field` 的 `fields`）——
    ///    这是那条码的用途，不是漏：不说是哪几个字段，`unknown_field` 就退化成
    ///    一句「有个字段不对」。
    pub params: Map<String, Value>,
}

impl AdminError {
    pub fn new(
        status: StatusCode,
        err_type: &'static str,
        code: AdminErrorCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            err_type,
            code,
            message: message.into(),
            params: Map::new(),
        }
    }

    #[must_use]
    pub fn param(mut self, k: &str, v: impl Into<Value>) -> Self {
        self.params.insert(k.to_string(), v.into());
        self
    }

    /// 管理树自己的「请求体不是合法 JSON」。
    ///
    /// ⚠️ **不能用 [`GatewayError::bad_json`]**：那一份抛的信封**没有 `code`**，
    /// 而畸形 JSON 这一档在面板上是**看得见**的（导入弹窗里粘错东西就会走到）。
    /// 走那一份等于在这条最常见的路径上把破口原样留着。
    pub fn bad_json() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            AdminErrorCode::BadJson,
            "请求体不是合法的 JSON",
        )
    }
}

/// 线格式。字段顺序 = 序列化顺序，见文件头的说明。
#[derive(Serialize)]
struct AdminWire {
    error: AdminWireInner,
}

#[derive(Serialize)]
struct AdminWireInner {
    r#type: &'static str,
    code: &'static str,
    message: String,
    /// params 为空时**不出现这一格**（与 TS 逐字一致）：
    /// 空对象会让面板的「有没有参数」判断多出一种形态。
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Map<String, Value>>,
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        let body = AdminWire {
            error: AdminWireInner {
                r#type: self.err_type,
                code: self.code.as_str(),
                message: self.message,
                params: (!self.params.is_empty()).then_some(self.params),
            },
        };
        (self.status, Json(body)).into_response()
    }
}

// ── 3. 转发失败信封 ─────────────────────────────────────────────────────────

/// 转发失败的归因。⚠️ **这一族用的是 `reason` 而不是 `type`**，别与网关信封搞混。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailReason {
    /// 池子里一把 key 都没有。
    PoolEmpty,
    /// 全部在冷却中。**只有这一条会自愈**，所以只有它带 `Retry-After`。
    AllCooling,
    /// 全部被停用。
    AllDisabled,
    /// 全部被剔除。
    AllEvicted,
    /// 每一把都试过，上游都失败了。
    UpstreamError,
    /// 同步端点耗尽了整个超时预算。
    UpstreamTimeout,
    /// 上游回了 200，但响应体不是 JSON（只在期望 JSON 的路由上）。
    UpstreamBadBody,
}

impl FailReason {
    /// 线格式串。🔴 **固定枚举串，永不由内部错误文本派生** ——
    /// 这些响应走在未鉴权路径上，配置与上游细节一个字节都不许漏出去。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PoolEmpty => "pool_empty",
            Self::AllCooling => "all_cooling",
            Self::AllDisabled => "all_disabled",
            Self::AllEvicted => "all_evicted",
            Self::UpstreamError => "upstream_error",
            Self::UpstreamTimeout => "upstream_timeout",
            Self::UpstreamBadBody => "upstream_bad_body",
        }
    }

    pub const fn status(self) -> StatusCode {
        match self {
            Self::UpstreamTimeout => StatusCode::GATEWAY_TIMEOUT,
            Self::UpstreamBadBody => StatusCode::BAD_GATEWAY,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

/// `{"error":{"reason","message"}}`。
#[derive(Debug, Clone)]
pub struct DispatchError {
    pub reason: FailReason,
    pub message: String,
    /// 秒。🔴 **只有 [`FailReason::AllCooling`] 许带。**
    ///
    /// `all_disabled` / `all_evicted` **一律不带**：它们不会自愈，
    /// 带了等于让客户端每秒重试一把永远不会回来的 key。
    pub retry_after_secs: Option<u64>,
}

impl DispatchError {
    pub fn new(reason: FailReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
            retry_after_secs: None,
        }
    }

    /// 全池冷却，附上还要等多久。
    pub fn cooling(message: impl Into<String>, retry_after_secs: u64) -> Self {
        Self {
            reason: FailReason::AllCooling,
            message: message.into(),
            retry_after_secs: Some(retry_after_secs),
        }
    }
}

/// 线格式。⚠️ 第一格是 `reason` 不是 `type`，见本节开头。
#[derive(Serialize)]
struct DispatchWire {
    error: DispatchWireInner,
}

#[derive(Serialize)]
struct DispatchWireInner {
    reason: &'static str,
    message: String,
}

impl IntoResponse for DispatchError {
    fn into_response(self) -> Response {
        let body = DispatchWire {
            error: DispatchWireInner {
                reason: self.reason.as_str(),
                message: self.message,
            },
        };
        let mut res = (self.reason.status(), Json(body)).into_response();
        // 防御性：即便调用方给别的 reason 塞了 Retry-After，这里也只认 all_cooling。
        // 把规则钉在出口上，而不是指望每个构造点都记得。
        if let (FailReason::AllCooling, Some(secs)) = (self.reason, self.retry_after_secs)
            && let Ok(v) = HeaderValue::from_str(&secs.to_string())
        {
            res.headers_mut().insert(axum::http::header::RETRY_AFTER, v);
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::json;
    use std::collections::BTreeSet;

    /// 拿到响应体的**原始字节串**。键序要对着它比，不能对着 `Value` 比 ——
    /// `Value` 是 `BTreeMap`，反序列化回来就把顺序信息抹掉了。
    async fn raw_of(res: Response) -> String {
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    async fn body_of(res: Response) -> Value {
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// 网关信封的形状逐字节钉死 —— 它被五语言 API.md 写死。
    #[tokio::test]
    async fn gateway_envelope_shape_is_exact() {
        let res = GatewayError::bad_json().into_response();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert_eq!(res.headers()["content-type"], "application/json");
        assert_eq!(
            body_of(res).await,
            json!({ "error": { "type": "invalid_request_error", "message": "请求体不是合法的 JSON" } })
        );
    }

    /// 🔴 网关信封**一个 `code` 都不许有**。半做会造出第三种状态。
    #[tokio::test]
    async fn gateway_envelope_never_carries_a_code() {
        for e in [
            GatewayError::bad_json(),
            GatewayError::not_found(),
            GatewayError::internal(),
        ] {
            let body = body_of(e.into_response()).await;
            assert!(
                body["error"].get("code").is_none(),
                "网关信封里出现了 code: {body}"
            );
        }
    }

    /// 405 必须带 `Allow`（RFC 9110 §15.5.6 是 MUST）。
    #[test]
    fn method_not_allowed_carries_allow() {
        let res = GatewayError::method_not_allowed(Some("GET, POST")).into_response();
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(res.headers()["allow"], "GET, POST");
    }

    /// `None` 那一支**把 `Allow` 那一格让出来**（由路由框架按实际注册表填）——
    /// 它不许塞一个空的 `Allow` 把那一格占住。
    ///
    /// 「接线之后 `Allow` 真的还在」由 `app.rs` 那一格在真路由上钉。
    #[test]
    fn method_not_allowed_leaves_the_allow_slot_open_when_asked_to() {
        let res = GatewayError::method_not_allowed(None).into_response();
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert!(res.headers().get("allow").is_none());
    }

    /// 🔴 500 的对外文案**不回显任何内部细节**。
    #[tokio::test]
    async fn internal_error_reveals_nothing() {
        let body = body_of(GatewayError::internal().into_response()).await;
        assert_eq!(body["error"]["message"], "网关内部错误");
    }

    /// 管理码表是 26 条，期望值手写。
    ///
    /// ⚠️ 规格 §4.2 的枚举里写的是「25 条」，**一手数下来是 26 条**
    /// （那一行把 `apikeys_unreadable` 漏在了计数外）。以这里为准，
    /// 并把这条差异登记进移植说明。
    #[test]
    fn admin_code_table_size_is_pinned() {
        assert_eq!(AdminErrorCode::ALL.len(), 26);
    }

    /// 管理码的线格式串两两不重。
    #[test]
    fn admin_wire_strings_are_unique() {
        let s: BTreeSet<&str> = AdminErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(s.len(), AdminErrorCode::ALL.len());
    }

    /// 管理信封带 `code`，`params` 为空时**不出现那一格**。
    #[tokio::test]
    async fn admin_envelope_shape_is_exact() {
        let body = body_of(AdminError::bad_json().into_response()).await;
        assert_eq!(body["error"]["code"], "bad_json");
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert!(body["error"].get("params").is_none(), "空 params 不该出现");

        let e = AdminError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            AdminErrorCode::UnknownField,
            "有不认识的字段",
        )
        .param("fields", "foo, bar");
        let body = body_of(e.into_response()).await;
        assert_eq!(body["error"]["params"], json!({ "fields": "foo, bar" }));
    }

    /// `params` 的面是闭的：每条码声明了它许带哪些键。
    ///
    /// 这一格钉的是那张表本身自洽（不重复、不空串），
    /// 「实际回的 params ⊆ 声明」要等 handler 落地之后逐条查。
    #[test]
    fn allowed_params_table_is_well_formed() {
        for c in AdminErrorCode::ALL {
            let ps = c.allowed_params();
            let set: BTreeSet<&&str> = ps.iter().collect();
            assert_eq!(set.len(), ps.len(), "{c} 的 params 声明里有重复键");
            assert!(ps.iter().all(|p| !p.is_empty()), "{c} 的 params 里有空串");
        }
        assert_eq!(
            AdminErrorCode::StaleWrite.allowed_params(),
            &["expected", "actual"]
        );
        assert_eq!(AdminErrorCode::KeyNotFound.allowed_params(), &[] as &[&str]);
    }

    /// 转发失败用的是 `reason` **不是** `type`。
    #[tokio::test]
    async fn dispatch_envelope_uses_reason_not_type() {
        let body =
            body_of(DispatchError::new(FailReason::PoolEmpty, "池子是空的").into_response()).await;
        assert_eq!(
            body,
            json!({ "error": { "reason": "pool_empty", "message": "池子是空的" } })
        );
        assert!(body["error"].get("type").is_none());
    }

    /// 状态码分档：503 / 504 / 502 各归各的。
    #[test]
    fn dispatch_status_codes_are_pinned() {
        assert_eq!(
            FailReason::PoolEmpty.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            FailReason::AllCooling.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            FailReason::AllDisabled.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            FailReason::AllEvicted.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            FailReason::UpstreamError.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            FailReason::UpstreamTimeout.status(),
            StatusCode::GATEWAY_TIMEOUT
        );
        assert_eq!(
            FailReason::UpstreamBadBody.status(),
            StatusCode::BAD_GATEWAY
        );
    }

    /// 🔴 `Retry-After` **只有 `all_cooling` 能带**。
    ///
    /// 回归对象：给 `all_disabled` / `all_evicted` 带上它，等于让客户端每秒重试
    /// 一把**永远不会回来**的 key。
    #[test]
    fn retry_after_only_for_the_one_reason_that_heals() {
        let res = DispatchError::cooling("全池冷却中", 42).into_response();
        assert_eq!(res.headers()["retry-after"], "42");

        // 就算有人手动塞了，出口也不认。
        let mut e = DispatchError::new(FailReason::AllDisabled, "全停用了");
        e.retry_after_secs = Some(42);
        assert!(
            !e.into_response().headers().contains_key("retry-after"),
            "不会自愈的 reason 绝不许带 Retry-After"
        );
    }

    /// 🔴 **三个信封的键序逐字节钉死。**
    ///
    /// 回归对象（本轮真踩过）：用 `json!` 宏出信封时，`serde_json` 的 `Map` 是
    /// `BTreeMap`，键被**按字典序重排** —— `{"message":…,"type":…}`，
    /// 而五语言 API.md 里逐字写的是 `{"type":…,"message":…}`。
    /// 文档示例与真实响应对不上就是在骗人，移植期拿 `diff` 对两侧响应也会全是噪声。
    ///
    /// ⚠️ 这一格必须比**原始字节**：比 `serde_json::Value` 的话顺序信息已经没了，
    /// 那就是一格永远绿的假判据。
    #[tokio::test]
    async fn envelope_key_order_matches_the_published_contract() {
        let raw = raw_of(GatewayError::not_found().into_response()).await;
        assert_eq!(
            raw,
            r#"{"error":{"type":"not_found","message":"没有这条路由"}}"#
        );

        let raw = raw_of(AdminError::bad_json().into_response()).await;
        assert!(
            raw.starts_with(
                r#"{"error":{"type":"invalid_request_error","code":"bad_json","message":"#
            ),
            "管理信封键序不对: {raw}"
        );

        let raw =
            raw_of(DispatchError::new(FailReason::PoolEmpty, "池子是空的").into_response()).await;
        assert_eq!(
            raw,
            r#"{"error":{"reason":"pool_empty","message":"池子是空的"}}"#
        );
    }

    /// 失败归因的线格式串是手写字面量。
    #[test]
    fn fail_reason_strings_are_pinned_literals() {
        assert_eq!(FailReason::PoolEmpty.as_str(), "pool_empty");
        assert_eq!(FailReason::AllCooling.as_str(), "all_cooling");
        assert_eq!(FailReason::UpstreamTimeout.as_str(), "upstream_timeout");
        assert_eq!(FailReason::UpstreamBadBody.as_str(), "upstream_bad_body");
    }
}
