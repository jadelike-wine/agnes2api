//! `/v1*` 与 `/v1beta*` 的鉴权闸。**两把钥匙里的第一把。**
//!
//! # 四路凭据 + 两段验证
//!
//! 四路凭据不是「宽容」，是四个客户端生态各自的既定形态：
//! `Authorization: Bearer`（OpenAI / Anthropic SDK）、`x-api-key`（Anthropic）、
//! `x-goog-api-key` 与 **URL 查询参数 `key`**（Gemini 官方 SDK）。
//!
//! 🔴 **`?key=` 在 `/v1*` 上是许的，但绝不许给 `/admin` 加。**
//! 这两条不是同一条规则的两面：
//! - 这里的 `?key=` 对象是**网关口令 / 子密钥**，是 Gemini SDK 的形态，删了官方 SDK 当场不通；
//! - `/admin` 那条禁令的对象是**管理口令**，它进 URL 会落进浏览器历史 / Referer / 各级访问日志。
//!
//! # 🔴 两段验证的顺序不可交换
//!
//! ```text
//! ① presented == gateway_token（常数时间）→ apiKeyId = "master"   ← 零存储 I/O
//! ② 否则（且 api_keys 已接线）：holder.ensure_fresh() → 按摘要查表
//! ③ 全部落空 → 401
//! ```
//!
//! 🔴 **①在前不是性能优化，它就是逃生口本身**：`apikeys` 键被写坏、读不出来、
//! 持有者第一次装载就失败时，①这一支**一个字节都不受影响**。
//! 反过来写在功能上等价，**在逃生口上是完全不同的东西**。
//!
//! 🔴 **没带凭据的请求在第一步就结束，一次存储读都不产生**
//! （扫描器打 `/v1/chat/completions` 不带任何头是最常见形态，让它撬不动存储）。

use crate::adapters::now_ms;
use crate::core::keys::api_keys::{
    ApiKeyTableRead, api_key_bucket, constant_time_equal, digest, find_by_digest, is_api_key_usable,
};
use crate::ports::logger::{LogEntry, LogLevel};
use crate::server::state::AppState;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// 归属：主口令的保留伪桶。
pub const MASTER_API_KEY_ID: &str = "master";

/// 子密钥被拒时打的事件。**只记 id 与 bucket，绝不记明文。**
pub const EVENT_APIKEY_REJECTED: &str = "apikey.rejected";

/// 这次请求是谁打的（Tier-2 归因用）。
///
/// 结构上不可达的缺席归到 `"unattributed"`，而**那个兜底在 sink 里，不在路由上**
/// —— 路由上兜底会让「归属丢了」这件事在每条路由上各有一份实现。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyId(pub String);

/// 受鉴权闸管辖的路径前缀。
///
/// ⚠️ **这是一份「前缀清单」而不是「已注册路由清单」**，语义与 Hono 的
/// `app.use("/v1/*", auth)` 逐字相同：**`/v1/不存在的路径` 也要先过闸再谈 404**。
/// 差别是可观测的 —— 前缀式会回 401，路由式会回 404，
/// 而后者等于把「本网关有哪些端点」免费告诉扫描器。
///
/// 别名前缀在列：一个网关地址要能同时喂多个只认固定基址的客户端。
/// 🔴 别名与裸路径**必须走同一个 handler**，否则「换个 base_url 行为就不一样」。
pub const GUARDED_PREFIXES: [&str; 5] = [
    "/v1",
    "/v1beta",
    "/openai/v1",
    "/claude/v1",
    "/gemini/v1beta",
];

/// 这条路径归不归鉴权闸管。
///
/// ⚠️ 判的是「整段前缀」而不是裸 `starts_with`：`/v1beta` 与 `/v1` 是两棵树，
/// 而 `/v1betafoo` 不该因为拼写像就被算进来。
#[must_use]
pub fn is_guarded(path: &str) -> bool {
    GUARDED_PREFIXES
        .iter()
        .any(|p| path == *p || path.strip_prefix(p).is_some_and(|r| r.starts_with('/')))
}

/// 401 的线格式。
///
/// ⚠️ **字段顺序是 `message` 在前**，与 [`crate::server::errors::GatewayError`] 相反。
/// 这不是笔误：五语言 API.md 把这个响应逐字写死过。
/// 序列化顺序 = 结构体字段顺序，所以这里只能另写一个结构体，不能复用那个信封。
#[derive(Serialize)]
struct UnauthorizedWire {
    error: UnauthorizedInner,
}

#[derive(Serialize)]
struct UnauthorizedInner {
    message: &'static str,
    #[serde(rename = "type")]
    err_type: &'static str,
}

/// 🔴 **401 不区分**「没这把密钥」/「停用了」/「过期了」——
/// 那是给攻击者的枚举信号。区分只进事件日志。
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(UnauthorizedWire {
            error: UnauthorizedInner {
                message: "未授权：缺少或无效的凭据",
                err_type: "unauthorized",
            },
        }),
    )
        .into_response()
}

/// 从请求里取出客户端声称的凭据。空串 / 缺失一律当「无凭据」。
#[must_use]
pub fn extract(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    // 1. `Authorization` 存在就**只看它**（别的头不再兜底）：
    //    客户端既然发了这个头，它的意图是明确的；再去别处捡会让
    //    「带了一个错的 Authorization + 一个对的 x-api-key」变成成功，
    //    而那种请求几乎一定是客户端配置写错了。
    if let Some(raw) = headers.get(header::AUTHORIZATION) {
        // ⚠️ **读不成 ASCII 的头值同样到此为止。** HTTP 头值允许 obs-text（0x80-0xFF），
        //    这类值 `to_str()` 返回 `Err`。写成 `.and_then(|v| v.to_str().ok())` 的话
        //    「读不出来」与「这个头不存在」变成同一件事，控制流掉到后面三路 ——
        //    于是上面那条不变量在一个**真实可达的输入**上不成立
        //    （`Bearer \xe9bad` + 一把对的 `x-api-key` ⇒ 实测 200）。
        let Ok(v) = raw.to_str() else { return None };
        // ⚠️ **只 trim 左边**：整体 trim 会把 `"Bearer   "`（空凭据）折成 `"Bearer"`，
        //    于是它被当成一把名叫 `Bearer` 的裸密钥 —— 一个空凭据变成了一次真的查表。
        let v = v.trim_start();
        // ⚠️ `Bearer` **大小写不敏感**（RFC 7235 的 auth-scheme 就是 case-insensitive，
        //    而真实客户端里 `bearer` 小写的不少）。
        if let Some(rest) = strip_bearer(v) {
            let rest = rest.trim();
            return (!rest.is_empty()).then(|| rest.to_string());
        }
        // 不是 Bearer 形态 ⇒ 把整个原值当凭据。
        // 这条兼容的是「直接把 key 填进 Authorization」的客户端，它真实存在。
        let v = v.trim_end();
        return (!v.is_empty()).then(|| v.to_string());
    }
    for name in ["x-api-key", "x-goog-api-key"] {
        if let Some(v) = headers.get(name).and_then(|v| v.to_str().ok()) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    // 4. URL 查询参数 `key`（Gemini 官方 SDK 的形态）。
    let q = query?;
    for (k, v) in url::form_urlencoded::parse(q.as_bytes()) {
        if k == "key" && !v.is_empty() {
            return Some(v.into_owned());
        }
    }
    None
}

fn strip_bearer(v: &str) -> Option<&str> {
    let (scheme, rest) = v.split_once(char::is_whitespace)?;
    scheme.eq_ignore_ascii_case("bearer").then_some(rest)
}

/// 鉴权闸。
pub async fn gateway_auth(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    if !is_guarded(req.uri().path()) {
        return next.run(req).await;
    }
    let presented = extract(req.headers(), req.uri().query());
    // 🔴 没带凭据 ⇒ 到此为止，**一次存储读都不产生**。
    let Some(presented) = presented else {
        return unauthorized();
    };

    let cfg = st.config.current();
    // ① 主口令。**零存储 I/O，且这一支就是逃生口本身。**
    //    ⚠️ 空口令必须落进「不通过」：`constant_time_equal("", "")` 为真，
    //       而 fail-closed 保证了口令不会是空串 —— 这一行是第二道保险。
    if !cfg.gateway_token.is_empty() && constant_time_equal(&presented, &cfg.gateway_token) {
        req.extensions_mut()
            .insert(ApiKeyId(MASTER_API_KEY_ID.to_string()));
        return next.run(req).await;
    }

    // ② 子密钥。**只有走到这里才碰存储**，且被 TTL 摊平。
    if let Some(holder) = &st.api_keys {
        let now = now_ms();
        holder.ensure_fresh(now).await;
        // ⚠️ `Invalid`（结构不认）与 `Absent`（还没签发过）在这里处置**相同**：
        //    一把子密钥都不认。但它们在日志与写路径上是两回事，所以类型上不合并。
        if let ApiKeyTableRead::Ok(table) = holder.current() {
            let d = digest(&presented);
            if let Some(rec) = find_by_digest(&table.keys, &d) {
                if is_api_key_usable(rec, now) {
                    req.extensions_mut().insert(ApiKeyId(rec.id.clone()));
                    return next.run(req).await;
                }
                // 存在但不可用 ⇒ 留一条线索。
                // 🔴 **只记 id 与 bucket**：记明文就是给攻击者字典添砖。
                st.logger.log(
                    LogEntry::new(
                        LogLevel::Warn,
                        EVENT_APIKEY_REJECTED,
                        "子密钥存在但不可用（已停用或已过期）",
                    )
                    .with("id", rec.id.clone())
                    .with("bucket", api_key_bucket(rec, now).as_str()),
                );
            }
        }
    }

    // ③ 全部落空。
    unauthorized()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::apikey;
    use crate::core::keys::api_keys::ApiKeyRecord;
    use crate::ports::storage::Storage;
    use crate::server::test_support::{TestApp, state_with_storage};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    const TOKEN: &str = "t-abcdefghijklmnop";

    fn app(st: AppState) -> Router {
        Router::new()
            .route("/health", get(|| async { "ok" }))
            .route(
                "/v1/models",
                get(|req: Request| async move {
                    // 把归属回显出来，这样判据能观察到「归到谁头上」。
                    req.extensions()
                        .get::<ApiKeyId>()
                        .map_or("none".to_string(), |v| v.0.clone())
                }),
            )
            .route("/openai/v1/models", get(|| async { "alias" }))
            .layer(axum::middleware::from_fn_with_state(
                st.clone(),
                gateway_auth,
            ))
            .with_state(st)
    }

    async fn call(app: &Router, req: HttpRequest<Body>) -> (StatusCode, String) {
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    fn get_req(uri: &str, headers: &[(&str, &str)]) -> HttpRequest<Body> {
        let mut b = HttpRequest::builder().uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(Body::empty()).unwrap()
    }

    // ── 凭据提取 ────────────────────────────────────────────────────────────

    /// 四路凭据都认得。
    #[test]
    fn all_four_credential_channels_are_understood() {
        let mk = |k: &str, v: &str| {
            let mut h = HeaderMap::new();
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(v).unwrap(),
            );
            h
        };
        assert_eq!(
            extract(&mk("authorization", "Bearer abc"), None).as_deref(),
            Some("abc")
        );
        // ⚠️ scheme 大小写不敏感。
        assert_eq!(
            extract(&mk("authorization", "bearer   abc"), None).as_deref(),
            Some("abc")
        );
        // 不是 Bearer 形态 ⇒ 整个原值。
        assert_eq!(
            extract(&mk("authorization", "abc"), None).as_deref(),
            Some("abc")
        );
        assert_eq!(
            extract(&mk("x-api-key", "abc"), None).as_deref(),
            Some("abc")
        );
        assert_eq!(
            extract(&mk("x-goog-api-key", "abc"), None).as_deref(),
            Some("abc")
        );
        // 🔴 Gemini 官方 SDK 的形态，**删了它 SDK 当场不通**。
        assert_eq!(
            extract(&HeaderMap::new(), Some("alt=sse&key=abc")).as_deref(),
            Some("abc")
        );
    }

    /// 🔴 **`Authorization` 存在就只看它 —— 哪怕它的值读不成 ASCII。**
    ///
    /// HTTP 头值允许 obs-text（0x80-0xFF），这类值 `HeaderValue::to_str()` 返回 `Err`。
    /// 从前这里写的是 `.get(AUTHORIZATION).and_then(|v| v.to_str().ok())`，
    /// 于是「读不出来」与「这个头根本不存在」被当成了同一件事，
    /// 控制流掉到后面三路 —— 本函数注释里写明的不变量在一个**真实可达的输入**上不成立。
    ///
    /// ⚠️ 后果不是越权，是**一类配置错误被静默放行**：
    /// 运维排查「我明明换过口令了，为什么旧配置还能用」时零线索。
    #[test]
    fn a_non_ascii_authorization_never_falls_through_to_the_other_channels() {
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            axum::http::HeaderValue::from_bytes(b"Bearer \xe9bad").unwrap(),
        );
        // 后面三路各放一份**对的**凭据：它们一个都不许被捡起来。
        h.insert("x-api-key", "abc".parse().unwrap());
        h.insert("x-goog-api-key", "abc".parse().unwrap());
        assert_eq!(extract(&h, Some("key=abc")), None);
    }

    /// 同一条性质在**真实闸**上的样子：401 而不是 200。
    #[tokio::test]
    async fn a_broken_authorization_header_beats_a_correct_x_api_key() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let req = HttpRequest::builder()
            .uri("/v1/models")
            .header(
                header::AUTHORIZATION,
                axum::http::HeaderValue::from_bytes(b"Bearer \xe9bad").unwrap(),
            )
            .header("x-api-key", TOKEN)
            .body(Body::empty())
            .unwrap();
        assert_eq!(call(&app(state), req).await.0, StatusCode::UNAUTHORIZED);
    }

    /// 空凭据一律当「没有」。
    #[test]
    fn empty_credentials_count_as_absent() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer   ".parse().unwrap());
        assert_eq!(extract(&h, None), None);
        assert_eq!(extract(&HeaderMap::new(), Some("key=")), None);
        assert_eq!(extract(&HeaderMap::new(), None), None);
    }

    // ── 闸的射程 ────────────────────────────────────────────────────────────

    /// 前缀判定：`/v1beta` 与 `/v1` 是两棵树，`/v1betafoo` 不算。
    #[test]
    fn the_guarded_prefixes_are_matched_by_segment() {
        assert!(is_guarded("/v1"));
        assert!(is_guarded("/v1/models"));
        assert!(is_guarded("/v1beta/models/x:generateContent"));
        assert!(is_guarded("/openai/v1/chat/completions"));
        assert!(is_guarded("/claude/v1/messages"));
        assert!(is_guarded("/gemini/v1beta/models"));
        assert!(!is_guarded("/v1betafoo"));
        assert!(!is_guarded("/health"));
        assert!(!is_guarded("/admin/api/session"));
        assert!(!is_guarded("/"));
    }

    /// 🔴 **`/v1/` 下不存在的路径也先过闸**（401 而不是 404）。
    ///
    /// 回归对象：回 404 等于把「本网关有哪些端点」免费告诉扫描器。
    #[tokio::test]
    async fn an_unknown_v1_path_is_challenged_before_it_is_404() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let (status, _) = call(&app(state), get_req("/v1/there-is-no-such-thing", &[])).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// `/health` 不在射程内。
    #[tokio::test]
    async fn health_is_not_behind_the_gate() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let (status, _) = call(&app(state), get_req("/health", &[])).await;
        assert_eq!(status, StatusCode::OK);
    }

    // ── 两段验证 ────────────────────────────────────────────────────────────

    /// 主口令通过，且**归到保留伪桶 `master`**。
    #[tokio::test]
    async fn the_master_token_passes_and_is_attributed_to_master() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let (status, body) = call(
            &app(state),
            get_req(
                "/v1/models",
                &[("authorization", &format!("Bearer {TOKEN}"))],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "master");
    }

    /// 🔴🔴 **主口令这一支零存储 I/O。**
    ///
    /// 回归对象：它是逃生口 —— `apikeys` 键坏掉时它必须一个字节都不受影响。
    /// 顺带钉住「全部客户端用主口令的部署每天 0 次子密钥读」这条结构性的零。
    #[tokio::test]
    async fn the_master_token_path_never_touches_storage() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let TestApp { state, .. } = state_with_storage(counting).await;
        let app = app(state);
        let before = counts.snapshot();
        for _ in 0..100 {
            call(
                &app,
                get_req(
                    "/v1/models",
                    &[("authorization", &format!("Bearer {TOKEN}"))],
                ),
            )
            .await;
        }
        assert_eq!(counts.snapshot(), before, "主口令这一支不许碰存储");
    }

    /// 🔴 **没带凭据 ⇒ 一次存储读都不产生。**
    ///
    /// 回归对象：扫描器打 `/v1/chat/completions` 不带任何头是最常见形态，
    /// 让它撬不动存储。
    #[tokio::test]
    async fn a_credential_less_request_costs_zero_storage_reads() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let TestApp { state, .. } = state_with_storage(counting).await;
        let app = app(state);
        let before = counts.snapshot();
        for _ in 0..1000 {
            call(&app, get_req("/v1/models", &[])).await;
        }
        assert_eq!(counts.snapshot(), before);
    }

    /// 签发的子密钥真能用，且**归属是它自己的 id**。
    #[tokio::test]
    async fn an_issued_subkey_works_and_is_attributed_to_itself() {
        let mem = Arc::new(MemoryStorage::new());
        let (secret, rec) = apikey::issue("n".into(), 0, None);
        apikey::save(mem.as_ref(), vec![rec.clone()], 0)
            .await
            .unwrap();
        let TestApp { state, .. } = state_with_storage(mem).await;
        let (status, body) = call(
            &app(state),
            get_req("/v1/models", &[("x-api-key", &secret)]),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, rec.id);
    }

    /// 停用 / 过期的子密钥 **401**，且**报文与「根本没这把」一模一样**。
    ///
    /// 🔴 回归对象：区分三种原因就是给攻击者的枚举信号。
    #[tokio::test]
    async fn a_disabled_subkey_is_rejected_with_an_indistinguishable_401() {
        let mem = Arc::new(MemoryStorage::new());
        let (secret, rec) = apikey::issue("n".into(), 0, None);
        let off = ApiKeyRecord {
            disabled: Some(true),
            ..rec
        };
        apikey::save(mem.as_ref(), vec![off], 0).await.unwrap();
        let TestApp { state, .. } = state_with_storage(mem).await;
        let app = app(state);

        let (s1, b1) = call(&app, get_req("/v1/models", &[("x-api-key", &secret)])).await;
        let (s2, b2) = call(&app, get_req("/v1/models", &[("x-api-key", "sk-nope")])).await;
        assert_eq!(s1, StatusCode::UNAUTHORIZED);
        assert_eq!(s1, s2);
        assert_eq!(b1, b2, "两种失败原因在对外报文上必须无法区分");
    }

    /// 🔴 **401 报文的字段顺序是 `message` 在前**（五语言 API.md 逐字写死过）。
    #[tokio::test]
    async fn the_401_envelope_is_pinned_byte_for_byte() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let (_, body) = call(&app(state), get_req("/v1/models", &[])).await;
        assert_eq!(
            body,
            r#"{"error":{"message":"未授权：缺少或无效的凭据","type":"unauthorized"}}"#
        );
        // 顺带把它也解析一遍，保证它是合法 JSON（SDK 会先 `response.json()`）。
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["type"], json!("unauthorized"));
    }

    /// 🔴🔴 **`apikeys` 坏掉时主口令仍然通得过** —— 这就是「逃生口」三个字的含义。
    #[tokio::test]
    async fn a_broken_subkey_table_never_locks_out_the_master_token() {
        let mem = Arc::new(MemoryStorage::new());
        mem.put("apikeys", json!("这不是一张表"), None)
            .await
            .unwrap();
        let TestApp { state, .. } = state_with_storage(mem).await;
        let (status, body) = call(
            &app(state),
            get_req(
                "/v1/models",
                &[("authorization", &format!("Bearer {TOKEN}"))],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "master");
    }

    /// 别名前缀同样受闸管辖（它们不是「绕过鉴权的后门」）。
    #[tokio::test]
    async fn alias_prefixes_are_behind_the_same_gate() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let app = app(state);
        assert_eq!(
            call(&app, get_req("/openai/v1/models", &[])).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(
                &app,
                get_req(
                    "/openai/v1/models",
                    &[("authorization", &format!("Bearer {TOKEN}"))]
                )
            )
            .await
            .0,
            StatusCode::OK
        );
    }

    /// `?key=` 这一路真的能过（**别因为读到「口令不许进 URL」就把它删了**）。
    #[tokio::test]
    async fn the_query_parameter_key_really_works() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let (status, _) = call(
            &app(state),
            get_req(&format!("/v1/models?key={TOKEN}"), &[]),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
}
