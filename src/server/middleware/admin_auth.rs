//! `/admin/api/*` 的鉴权闸。**两把钥匙里的第二把，与第一把严格隔离。**
//!
//! # 🔴 只认 `X-Admin-Key`
//!
//! - 🔴 **不接受 `Authorization: Bearer`** —— 两把钥匙严格隔离。
//!   接受它的后果不是「方便」，是「一个手里有中转口令的人会顺手拿去试管理口」，
//!   而更坏的是**反过来**：运维会开始把管理口令填进 OpenAI 客户端的 api_key 栏里。
//! - 🔴 **不接受 `?key=`** —— 口令进 URL 会落进浏览器历史、Referer、各级访问日志。
//!
//! ## ⚠️⚠️ 这条禁令的连带后果：**面板的事件流只能轮询，不能用 SSE**
//!
//! 浏览器的 `EventSource` **设不了请求头**，所以任何「用 SSE 推实时日志」的方案
//! 都只能把口令放进 URL —— 那正是上面这条禁掉的东西。
//! kiro2api 那边用的是 `?api_key=`，本仓**有成文的相反立场**，不照抄。
//!
//! 🔴 **别把这一段读成「暂时还没做 SSE」**：它是一个被裁定过的取舍。
//! 要改，得先推翻「管理口令不进 URL」这条，或者先落地
//! 「POST 换一张短时效一次性票 → 拿票连 SSE」那条备选 —— 两者都是独立的设计决定。

use crate::adapters::now_ms;
use crate::core::keys::api_keys::constant_time_equal;
use crate::ports::logger::{LogEntry, LogLevel};
use crate::server::client_ip::client_ip;
use crate::server::errors::{AdminError, AdminErrorCode};
use crate::server::state::AppState;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// 管理口令的请求头名。**唯一的那一个。**
pub const ADMIN_KEY_HEADER: &str = "x-admin-key";

/// 口令长度下界。
///
/// 🟡 **它是唯一的防线**：`ADMIN_TOKEN` 没有任何登录限速。
/// TS 版承认「限速这件事没有做，不是做不了」（单进程本来做得起）。
/// Rust 版补上它属于加分项，但**那是一次独立的功能决定**
/// （要定策略、五语言文案、考虑反代已在限速的部署）。
pub const ADMIN_TOKEN_MIN_LENGTH: usize = 24;

/// 失败登录事件。
pub const EVENT_ADMIN_LOGIN_FAILED: &str = "admin.login_failed";
/// 两把口令相同时的事件（**原因只进日志，不进响应体**）。
pub const EVENT_ADMIN_TOKEN_CONFLICT: &str = "admin.token_conflict";

/// 事件里 `path` 的截断长度。路径由客户端控制，**不许原样落进日志行**。
const PATH_LOG_MAX: usize = 200;

/// `ADMIN_TOKEN` 不合规的四种形态。**顺序有意义。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminTokenProblem {
    /// 首尾有空白。**排第一**：它是唯一在配置文件里**看不见**的那一种。
    WhitespacePadded,
    /// 出现了送不出去（或送出去也会被 HTTP 解析器毙掉）的字符。
    ///
    /// ⚠️ **排在长度之前**：一串汉字口令会同时触发这条和「太短」，
    /// 而报「长度不足」会把人引向**加长它** —— 加长之后照样送不出去。
    NotSendable,
    /// 短于 [`ADMIN_TOKEN_MIN_LENGTH`]。
    TooShort,
}

impl AdminTokenProblem {
    /// 进日志的原因串。
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::WhitespacePadded => "whitespace_padded",
            Self::NotSendable => "not_sendable",
            Self::TooShort => "too_short",
        }
    }
}

/// 装配期的三条规则（第四条在每个请求上复查，见 [`admin_auth`]）。
///
/// 字符集三段的实测边界（**照抄，别自己推**）：
/// - (a) 码点 > U+00FF、`NUL`/`LF`/`CR` → 浏览器 `Headers.set` 直接抛 TypeError，
///   请求根本发不出去；
/// - (b) `0x01–0x08`、`0x0B`、`0x0C`、`0x0E–0x1F`、`0x7F`（29 个）→ 浏览器送得出去，
///   HTTP 解析器判 400；
/// - (c) `TAB 0x09` 与整段 `0x80–0xFF` → **送得出去、也真的能用**，拦它们是稳健性取舍
///   （TAB 是不可见字符；`0x80–0xFF` 是编码口径问题：env 按 UTF-8 解、
///   头值按 Latin-1 解，RFC 9110 已把 obs-text 标为弃用）。
///
/// **空格 `0x20` 是允许的**：passphrase 式口令有价值，且纯 ASCII 零编码歧义。
///
/// 🔴 **空串必须落进「未配置」分支**（调用方判），不是落进这里 ——
/// 历史上出过一次实际的鉴权绕过，成因就是空串在 `??` 下不下坠。
pub fn validate_admin_token(token: &str) -> Result<(), AdminTokenProblem> {
    if token.trim() != token {
        return Err(AdminTokenProblem::WhitespacePadded);
    }
    if !token.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
        return Err(AdminTokenProblem::NotSendable);
    }
    if token.len() < ADMIN_TOKEN_MIN_LENGTH {
        return Err(AdminTokenProblem::TooShort);
    }
    Ok(())
}

fn unauthorized() -> Response {
    AdminError::new(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        AdminErrorCode::AdminUnauthorized,
        "未授权",
    )
    .into_response()
}

/// 🔴 **响应体不说原因。**
///
/// 这个分支跑在**验证凭据之前**，说出「两把口令相同」等于告诉一个手里已经有中转口令
/// 的人「管理口令就是你手上那把」。原因只进日志。
///
/// 🔴 **不许把这条 503 改成 401**：面板规则是「401 = 会话失效，清凭据踢回登录闸」，
/// 改成 401 会让运维被踢出去并被告知「口令无效」，而他手里那把是对的。
fn service_unavailable() -> Response {
    AdminError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "service_unavailable",
        AdminErrorCode::AdminUnavailable,
        "管理接口不可用",
    )
    .into_response()
}

/// 管理面鉴权。**必须是 `/admin/api` 子树内的第一道**。
///
/// 🔴 两条位置铁律：任何 `/admin/api/*` 端点都必须注册在**本闸之后、静态兜底之前**。
/// 挪到前面 = 一条免鉴权的管理端点；挪到后面 = **恒 404，拿着正确口令也是 404，
/// 没有任何报错**。
pub async fn admin_auth(State(st): State<AppState>, req: Request, next: Next) -> Response {
    // 没配口令时整棵树根本不注册（见 `app.rs`），走到这里说明装配写错了。
    // 兜住它，并且**兜向拒绝**。
    let Some(expected) = st.admin_token.clone() else {
        return unauthorized();
    };

    // 规则③：**每请求复查**。
    //
    // ⚠️ **刻意不在装配期拦**：它的另一个输入（网关口令）运行中会变，
    // 装配期拦会造成**分裂脑** —— 冲突期间冷启动的实例整棵树 404
    // 且**改回去仍 404、必须重启**，而冲突前建好的实例只是 503、改回去立刻恢复。
    let gateway_token = st.config.current().gateway_token.clone();
    if !gateway_token.is_empty() && constant_time_equal(expected.as_str(), &gateway_token) {
        st.logger.log(LogEntry::new(
            LogLevel::Error,
            EVENT_ADMIN_TOKEN_CONFLICT,
            "ADMIN_TOKEN 与当前生效的 GATEWAY_TOKEN 相同，管理接口暂停服务；改掉其中一个即可立刻恢复",
        ));
        return service_unavailable();
    }

    // 🔴 **只看这一个头。** `Authorization` 与 `?key=` 一个字都不读 ——
    //    不是「读了再拒」，是**根本没有那条路径**。
    let presented = req
        .headers()
        .get(ADMIN_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    if !presented.is_empty() && constant_time_equal(presented, expected.as_str()) {
        return next.run(req).await;
    }

    // ⚠️ **取 `OriginalUri` 而不是 `req.uri()`**：管理树是 `nest("/admin/api", ...)` 挂上去的，
    //    嵌套会把前缀从 `uri().path()` 里剥掉 ⇒ 事件里只剩 `/session`。
    //    运维按事件里的 path 去 grep 反代日志时，那个残缺的值对不上任何一行。
    let path = req
        .extensions()
        .get::<axum::extract::OriginalUri>()
        .map_or_else(|| req.uri().path().to_string(), |u| u.0.path().to_string());
    let mut entry = LogEntry::new(LogLevel::Warn, EVENT_ADMIN_LOGIN_FAILED, "管理接口鉴权失败")
        .with("path", path.chars().take(PATH_LOG_MAX).collect::<String>())
        // ⚠️ 记的是「有没有带这个头」而**不是带了什么**：
        //    猜错的口令常常只差一位，记下来就是攻击者字典的一部分。
        .with("hasHeader", !presented.is_empty());
    if let Some(ip) = client_ip(req.headers(), st.trust_proxy) {
        entry = entry.with("ip", ip);
    }
    st.logger.log(entry);
    let _ = now_ms(); // 时刻由 sink 自己打，这里不重复取。
    unauthorized()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::MemoryStorage;
    use crate::server::test_support::{TestApp, state_with_storage};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use serde_json::Value;
    use std::sync::Arc;
    use tower::ServiceExt;

    const ADMIN: &str = "admin-token-0123456789abcdef";
    const GATEWAY: &str = "t-abcdefghijklmnop";

    async fn app_with(admin: Option<&str>) -> Router {
        let TestApp { mut state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        state.admin_token = admin.map(|s| Arc::new(s.to_string()));
        Router::new()
            .route("/admin/api/session", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                admin_auth,
            ))
            .with_state(state)
    }

    async fn call(app: &Router, headers: &[(&str, &str)], uri: &str) -> (StatusCode, String) {
        let mut b = HttpRequest::builder().uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let res = app
            .clone()
            .oneshot(b.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    // ── 四条硬规则 ──────────────────────────────────────────────────────────

    /// 三条装配期规则，**顺序有意义**。
    #[test]
    fn the_three_assembly_time_rules_fire_in_the_documented_order() {
        assert_eq!(validate_admin_token(ADMIN), Ok(()));
        // ⓪ 空白最先 —— 它是唯一在配置文件里看不见的那一种。
        assert_eq!(
            validate_admin_token(&format!(" {ADMIN} ")),
            Err(AdminTokenProblem::WhitespacePadded)
        );
        // ① 字符集排在长度之前：一串汉字同时触发两条，
        //    报「太短」会把人引向加长它 —— 加长之后照样送不出去。
        assert_eq!(
            validate_admin_token("口令口令"),
            Err(AdminTokenProblem::NotSendable)
        );
        assert_eq!(
            validate_admin_token("这是一个很长很长很长很长很长很长的中文口令"),
            Err(AdminTokenProblem::NotSendable)
        );
        // ② 长度。
        assert_eq!(
            validate_admin_token("short"),
            Err(AdminTokenProblem::TooShort)
        );
        // 边界逐字钉住：23 不行、24 行。
        assert_eq!(
            validate_admin_token(&"a".repeat(23)),
            Err(AdminTokenProblem::TooShort)
        );
        assert_eq!(validate_admin_token(&"a".repeat(24)), Ok(()));
    }

    /// **空格是允许的**（passphrase 式口令有价值），TAB 与 `0x80–0xFF` 不行。
    #[test]
    fn a_passphrase_with_spaces_is_allowed_but_tabs_are_not() {
        assert_eq!(
            validate_admin_token("correct horse battery staple!"),
            Ok(())
        );
        assert_eq!(
            validate_admin_token("correct\thorse battery staple!"),
            Err(AdminTokenProblem::NotSendable)
        );
        assert_eq!(
            validate_admin_token("correct\u{00e9}horse battery stapl"),
            Err(AdminTokenProblem::NotSendable)
        );
    }

    /// 正确的 `X-Admin-Key` 通过。
    #[tokio::test]
    async fn the_right_header_passes() {
        let app = app_with(Some(ADMIN)).await;
        assert_eq!(
            call(&app, &[("x-admin-key", ADMIN)], "/admin/api/session")
                .await
                .0,
            StatusCode::OK
        );
    }

    /// 🔴🔴 **`Authorization: Bearer` 与 `?key=` 一律不认，哪怕值是对的。**
    ///
    /// 回归对象：两把钥匙严格隔离 + 口令不许进 URL。
    #[tokio::test]
    async fn neither_bearer_nor_query_string_is_accepted_even_when_correct() {
        let app = app_with(Some(ADMIN)).await;
        assert_eq!(
            call(
                &app,
                &[("authorization", &format!("Bearer {ADMIN}"))],
                "/admin/api/session"
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED,
            "管理树不许接受 Bearer"
        );
        assert_eq!(
            call(&app, &[("x-api-key", ADMIN)], "/admin/api/session")
                .await
                .0,
            StatusCode::UNAUTHORIZED,
            "管理树不许接受 x-api-key"
        );
        assert_eq!(
            call(&app, &[], &format!("/admin/api/session?key={ADMIN}"))
                .await
                .0,
            StatusCode::UNAUTHORIZED,
            "口令进 URL 会落进浏览器历史 / Referer / 访问日志"
        );
    }

    /// 401 走的是**管理信封**（带 `code`），不是网关信封。
    #[tokio::test]
    async fn the_401_uses_the_admin_envelope_with_a_code() {
        let app = app_with(Some(ADMIN)).await;
        let (_, body) = call(&app, &[], "/admin/api/session").await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["type"], "unauthorized");
        assert_eq!(v["error"]["code"], "admin_unauthorized");
    }

    /// 🔴 **两把口令相同 ⇒ 503，而且响应体不说原因、也不许是 401。**
    #[tokio::test]
    async fn identical_tokens_yield_a_503_that_explains_nothing() {
        let app = app_with(Some(GATEWAY)).await;
        let (status, body) = call(&app, &[("x-admin-key", GATEWAY)], "/admin/api/session").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "admin_unavailable");
        assert!(
            !body.contains("GATEWAY") && !body.contains("相同"),
            "响应体不许透出原因：{body}"
        );
    }

    /// 🔴 **规则③是每请求复查的**，因此它的判定跟着运行期配置走。
    ///
    /// 回归对象：装配期拦会造成分裂脑 —— 冲突期间冷启动的实例改回去仍 404、必须重启。
    #[tokio::test]
    async fn rule_three_is_rechecked_on_every_request() {
        let TestApp { mut state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let cfg = state.config.clone();
        let app = Router::new()
            .route("/admin/api/session", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                admin_auth,
            ))
            .with_state(state);

        assert_eq!(
            call(&app, &[("x-admin-key", ADMIN)], "/admin/api/session")
                .await
                .0,
            StatusCode::OK
        );
        // 运行中把网关口令改成与管理口令相同 —— **不必重启**，下一个请求就该 503。
        cfg.set(crate::core::config::GatewayConfig {
            gateway_token: ADMIN.to_string(),
            ..(*cfg.current()).clone()
        });
        assert_eq!(
            call(&app, &[("x-admin-key", ADMIN)], "/admin/api/session")
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        // 改回去**立刻**恢复。
        cfg.set(crate::core::config::GatewayConfig {
            gateway_token: GATEWAY.to_string(),
            ..(*cfg.current()).clone()
        });
        assert_eq!(
            call(&app, &[("x-admin-key", ADMIN)], "/admin/api/session")
                .await
                .0,
            StatusCode::OK
        );
    }

    /// 🔴 **猜错的口令绝不进日志。**
    ///
    /// 回归对象：猜错的口令常常只差一位，记下来就是攻击者字典的一部分。
    #[tokio::test]
    async fn a_wrong_token_is_never_written_to_the_event_log() {
        let TestApp {
            state, recorded, ..
        } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let mut state = state;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let app = Router::new()
            .route("/admin/api/session", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                admin_auth,
            ))
            .with_state(state);
        call(
            &app,
            &[("x-admin-key", "admin-token-0123456789abcdeX")],
            "/admin/api/session",
        )
        .await;

        let events = recorded.entries();
        let e = events
            .iter()
            .find(|e| e.event == EVENT_ADMIN_LOGIN_FAILED)
            .expect("失败登录必须留下一条事件");
        let dump = format!("{:?}", e.fields);
        assert!(!dump.contains("abcdeX"), "口令片段泄漏进日志了：{dump}");
        assert_eq!(e.fields["hasHeader"], serde_json::json!(true));
        assert_eq!(
            e.fields["path"],
            serde_json::json!("/admin/api/session"),
            "⚠️ path 必须是**完整**路径：嵌套路由会把 /admin/api 前缀剥掉，\
             只剩 /session 的话运维拿它去 grep 反代日志对不上任何一行"
        );
    }
}
