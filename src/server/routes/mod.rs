//! 对外路由：`/health` + 四协议 + 媒体三条。
//!
//! # 别名前缀
//!
//! `/openai/v1/*`、`/claude/v1/*`、`/gemini/v1beta/*` 与裸路径**走同一个 handler**。
//! 🔴 这条必须有判据钉住：改一处漏一处的现象是
//! 「换个 base_url 行为就不一样」—— 最难查的那一种。
//! 收益是一个网关地址同时喂多个只认固定基址的客户端。
//!
//! ⚠️ **超时档不在别名这一层分叉**：每条 handler 自己 `if stream { … } else { … }`，
//! 而 `protocol_router()` 里别名与裸路径填的是**同一个函数项**
//! ⇒ 别名拿到的档位与主路径同一份，没有第二处可改。
//! 钉住这件事的是 `app::tests::alias_prefixes_hit_the_very_same_handler`（行为同一）
//! 与 `app::tests::the_route_table_does_not_shrink_when_features_are_off`
//! （逐条覆盖别名前缀族，每条都要走到转发内核）；档位本身由各路由自己那一格
//! （`the_timeout_profile_follows_*`）钉住。**这里不重复造。**
//!
//! # `/v1/ping` 不做
//!
//! 它落在 `/v1/*` 鉴权闸的射程内，为它开一个豁免口是**净负收益**
//! （豁免口本身还要一格判据钉死「只豁免这一条」），而它的信息量与 `/health` 完全重合。
//! 将来真要，**放 `/ping` 而不是 `/v1/ping`**。
//!
//! # `?key=` 在 `/v1/*` 上是许的
//!
//! 那是 Gemini 官方 SDK 的形态，对象是网关口令 / 子密钥。
//! 🔴 **但绝不许给 `/admin` 加** —— 那条禁令的对象是管理口令。
//! 两把钥匙，别把其中一条当成另一条的推论。

pub mod anthropic;
pub mod gemini;
pub mod health;
pub mod media;
pub mod openai;
pub mod responses;

use crate::core::protocol::request_shape::RequestShapeError;
use crate::core::protocol::sse::{ByteChunk, UpstreamStreamError};
use crate::pool::dispatcher::{DispatchDeps, Dispatched, OkBody};
use crate::ports::fetcher::BodyStream;
use crate::server::errors::GatewayError;
use crate::server::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::FromRequest;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::Value;
use std::sync::Arc;

/// 🔴 请求体上限。**32 MiB，且这个数是一次裁定而不是默认值。**
///
/// # 为什么不照搬 TS 版的「不设上限」
///
/// Hono 版没有这道闸，所以「与 TS 行为一致」= 完全不限。**本轮不采这一支**：
/// 本仓的每条协议路由都把整份请求体读进内存（`Bytes` 提取 → `serde_json::from_slice`），
/// 三条转换协议还会再序列化出一份 [`crate::core::protocol::InternalRequest`] 拷贝
/// ⇒ 不设上限就是让**单个客户端按它自己的输入长度决定网关分配多少内存**，
/// 而这条路径上没有任何别的背压。
///
/// 同一条论证在本仓**已经被接受过一次**：转发内核对**响应**侧读整段 JSON 设了
/// `MAX_JSON_BODY_BYTES = 8 MiB`，理由逐字是「超过就当行为异常，不再往内存里灌」。
/// 请求侧无上限与它自相矛盾。
///
/// # 为什么不是 axum 默认的 2 MiB
///
/// 2 MiB **是一条真的功能退化**：透传的 `/v1/chat/completions` 里放一张 base64 图片、
/// 或一段长对话历史轻松过 2 MB，而这些请求上游本来是接受的 ——
/// 网关凭空新增了一条「上游能办、我不让办」的拒绝路径。
///
/// # 为什么是 32 MiB
///
/// 对标 kiro2api（`03 §8.1` 两处 `DefaultBodyLimit::max(32 * 1024 * 1024)`），
/// 它比任何真实对话请求高一个数量级，同时仍是一个**硬上界**。
///
/// 🔴 **无论上限定在哪，413 都必须是 JSON 信封** —— 见 [`RawBody`]。
pub const MAX_REQUEST_BODY_BYTES: usize = 32 * 1024 * 1024;

/// 原始请求体。**四条协议与媒体三条一律用它，不用裸 `Bytes`。**
///
/// 它存在的**唯一**理由是拒绝报文的形状：裸 `Bytes` 的提取失败走 axum 自己的
/// [`BytesRejection`](axum::extract::rejection::BytesRejection)，那是一条
/// `content-type: text/plain` 的响应 —— 而本仓对畸形 JSON / 404 / 405 已经论证过
/// 三次「纯文本的 4xx 会让客户端在 `response.json()` 处二次抛错」。
/// 413 没有理由是那三条的例外。
///
/// ⚠️ **它只换拒绝报文，不换语义**：成功路径拿到的就是 `Bytes` 本身，
/// 上限仍然由 [`axum::extract::DefaultBodyLimit`] 在装配处设（见
/// [`MAX_REQUEST_BODY_BYTES`]），这里不重复实现一套长度判定。
///
/// ⚠️ **刻意不顺手把 JSON 也解析掉**：Gemini 那条路由的顺序是
/// 「先判路径方法名（未知方法 404）→ 再解析请求体」，
/// 在提取器里解析会把那个顺序反过来，让一条方法名写错的请求回 400 而不是 404。
pub struct RawBody(pub Bytes);

impl<S> FromRequest<S> for RawBody
where
    S: Send + Sync,
{
    type Rejection = GatewayError;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        match Bytes::from_request(req, state).await {
            Ok(b) => Ok(Self(b)),
            Err(rej) if rej.status() == StatusCode::PAYLOAD_TOO_LARGE => {
                Err(GatewayError::payload_too_large(MAX_REQUEST_BODY_BYTES))
            }
            // 读不动请求体的其余情形（连接中途断了之类）。
            // 🔴 **不回显 axum 的原文**：它是纯文本，且措辞是框架的实现细节。
            Err(_) => Err(GatewayError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "读取请求体失败",
            )),
        }
    }
}

/// 把请求体解析成 JSON。
///
/// 🔴 **畸形 JSON 一律 400 + JSON 信封**，不是 axum 默认的 422 纯文本。
/// OpenAI 生态的客户端一律先 `response.json()`，拿到纯文本会在解析处**二次抛错**
/// （`JSONDecodeError`），把「少传一个字段」这种一眼可修的参数问题
/// 变成「客户端内部错误」。
pub fn parse_json(body: &Bytes) -> Result<Value, GatewayError> {
    serde_json::from_slice(body).map_err(|_| GatewayError::bad_json())
}

/// 请求整形失败 → 400。
///
/// 🔴 报文逐字来自 [`RequestShapeError`] 的 `Display`，**那三句是对外契约**
/// （五语言 API.md 逐字写着）。特别是 `tools` / 多模态那一族：
/// **做不到就明说，别假装做到了** —— 实测过同一份带 `tools` 的请求走 `/v1/messages`
/// 返回 200、正文是模型自己吐的 `<tool_call>` 乱码而 `stop_reason` 还写着 `end_turn`。
pub fn shape_error(e: &RequestShapeError) -> GatewayError {
    GatewayError::new(
        StatusCode::BAD_REQUEST,
        "invalid_request_error",
        e.to_string(),
    )
}

/// 取一份转发依赖。
///
/// ⚠️ `config` 是调用方**在本次请求开头取的一份快照**，整个请求内保持一致。
pub fn deps<'a>(
    st: &'a AppState,
    config: &'a crate::core::config::GatewayConfig,
) -> DispatchDeps<'a> {
    deps_for(st, config, None)
}

/// 同上，但带上这次请求的密钥归属。
///
/// ⚠️ **归属只在这里取一次**，四条路由都走同一份 —— 各取各的会让
/// 「哪一条路由把归属漏了」变成一件要逐条去看的事。
pub fn deps_for<'a>(
    st: &'a AppState,
    config: &'a crate::core::config::GatewayConfig,
    api_key_id: Option<String>,
) -> DispatchDeps<'a> {
    DispatchDeps {
        repo: st.repo.as_ref(),
        fetcher: st.fetcher.as_ref(),
        config,
        logger: st.logger.as_ref(),
        lb_mode: st.lb_mode,
        cursor: st.key_cursor.as_ref(),
        sticky: st.sticky_key.as_ref(),
        usage: st.usage_sink.as_deref(),
        api_key_id,
    }
}

/// 把上游的字节流接到协议转换层要的那种流上。
///
/// ⚠️ 这里把三档出站失败**压成一档** `UpstreamStreamError`：
/// 协议层对断流只有一种处置（发自己那一档的 error 事件然后停）。
/// **三档的区分在 dispatcher 那一侧已经用过了**，不是在这里丢的。
pub fn to_byte_chunks(body: BodyStream) -> impl Stream<Item = ByteChunk> {
    body.map(|r| r.map_err(|_| UpstreamStreamError))
}

/// 把协议层产出的 SSE 文本流接成 HTTP 响应体。
pub fn sse_body(s: impl Stream<Item = String> + Send + 'static) -> Body {
    Body::from_stream(s.map(|t| Ok::<Bytes, std::io::Error>(Bytes::from(t))))
}

/// 用白名单头 + 指定状态码搭一个响应。
pub fn respond(status: u16, headers: &[(String, String)], body: Body) -> Response {
    let mut res = Response::new(body);
    *res.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    for (k, v) in headers {
        if let (Ok(name), Ok(val)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            res.headers_mut().insert(name, val);
        }
    }
    res
}

/// 四条协议共用的「流式响应头」。
///
/// ⚠️ **`cache-control: no-cache` 不是装饰**：中间代理缓存住一条 SSE 会让
/// 第二个客户端拿到第一个客户端的回答。
pub fn set_stream_headers(res: &mut Response, content_type: &'static str) {
    res.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static(content_type),
    );
    res.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
}

/// `text/event-stream; charset=utf-8`。
pub const SSE_CONTENT_TYPE: &str = "text/event-stream; charset=utf-8";

/// 随机 id。🔴 **CSPRNG**：它会进 message id / response id，
/// 下游按它做日志、追踪、去重、缓存，**每个流式响应必须各不相同**。
#[must_use]
pub fn random_id(prefix: &str) -> String {
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).expect("系统熵池不可用");
    format!("{prefix}{}", hex::encode(buf))
}

/// 从 [`Dispatched`] 里取出「成功」那一支；否则把已成型的响应直接交回去。
///
/// 这一层存在的理由：四条协议的 handler 结构完全一样，
/// 各写一份 `match` 迟早会出现「某条协议忘了处理某一支」。
pub enum Success {
    Stream {
        status: u16,
        headers: Vec<(String, String)>,
        body: BodyStream,
    },
    Json {
        status: u16,
        headers: Vec<(String, String)>,
        body: Value,
    },
}

pub fn split(d: Dispatched) -> Result<Success, Response> {
    match d {
        Dispatched::Ready(res) => Err(res),
        Dispatched::Ok {
            status,
            headers,
            body: OkBody::Stream(body),
        } => Ok(Success::Stream {
            status,
            headers,
            body,
        }),
        Dispatched::Ok {
            status,
            headers,
            body: OkBody::Json(body),
        } => Ok(Success::Json {
            status,
            headers,
            body,
        }),
    }
}

/// 从请求扩展里取归属（Tier-2 用）。
///
/// ⚠️ **缺席时不在这里兜底**：兜底在 sink 里（`"unattributed"`）。
/// 放在路由上会让「归属丢了」这件事在每条路由上各有一份实现，
/// 而它们迟早会写出两个不同的伪桶名。
#[must_use]
pub fn attribution(ext: &axum::http::Extensions) -> Option<Arc<str>> {
    ext.get::<crate::server::middleware::auth::ApiKeyId>()
        .map(|v| Arc::from(v.0.as_str()))
}

/// 判据用：把这条路由**真正发出去的超时预算**看见。**`#[cfg(test)]`，不进产物。**
///
/// # 它补的是哪一条缝
///
/// 「这条路由选了哪一档超时」= 传给 [`crate::pool::dispatcher::dispatch`] 的
/// `TimeoutProfile`，而那是一个**入参**。于是 `pool::dispatcher` 那一侧的判据
/// 只钉得住「拿到哪一档就真的发哪个预算」，**钉不住「这条路由传进去的是哪一档」**——
/// 两侧的清单都没有列这条，于是没人补。
///
/// 🔴 **缺口是实测过的**：把 `POST /v1/images/generations` 的 `TimeoutProfile::Sync`
/// 改成 `TimeoutProfile::FirstByte`，跑全量 ⇒ **600 格判据一格都不红**。
/// 而 `01 §3.5` 记着的正是这条改动的真实事故：图片生成拿到短预算 ⇒ **100% 失败**，
/// 且每次请求把池中每把 key 各记一次 strike，三次请求把整池打进 30 分钟长冷却。
///
/// # 观测量
///
/// [`OutboundRequest::timeout_ms`] —— 档位到了端口那一层就只剩这一个数，
/// 而它正是唯一一个「档位选错了」会变的东西。
///
/// # ⚠️ 拿变异验判据时的一个坑（留给下一个人的路标）
///
/// 变异脚本**还原源码时不许保留原 mtime**（Python 的 `shutil.copy2`、
/// `cp -p`、`rsync -t` 都会）：cargo 靠 mtime 判新旧，还原出来的文件比目标还旧
/// ⇒ 它**复用带变异的旧二进制**，现象是「源码明明已还原，判据还是红的」
/// —— 会被误读成判据自己坏了。用 `shutil.copy` / `cp` 并在还原后 `touch` 一下。
#[cfg(test)]
pub mod timeout_probe {
    use crate::adapters::now_ms;
    use crate::adapters::storage_test_doubles::MemoryStorage;
    use crate::ports::fetcher::{FetchError, Fetcher, OutboundRequest, OutboundResponse};
    use crate::server::test_support::{ScriptedFetcher, TestApp, UpstreamReply, state_with};
    use parking_lot::Mutex;
    use std::sync::Arc;

    /// `TimeoutProfile::FirstByte` 这一档发出去的预算。
    ///
    /// ⚠️ **手写字面量，不从 `upstream_timeout_ms` 现算** —— 现算的话
    /// 「把两档的配置字段互换一下」这类改动会让判据跟着一起变，等于没测。
    /// 字面量对不对由 [`state_with_timeout_spy`] 里那两条守卫负责。
    pub const FIRST_BYTE_BUDGET_MS: i64 = 8_000;

    /// `TimeoutProfile::Sync` 这一档**单次尝试**发出去的预算
    /// （总预算 120_000 被 `SYNC_ATTEMPT_SHARE` 切成两份）。
    ///
    /// ⚠️ 同样**手写字面量，不从 `SYNC_ATTEMPT_SHARE` 现算**：现算的话
    /// 把它从 2 改成 1 这一族判据会跟着一起变绿。
    pub const SYNC_BUDGET_MS: i64 = 60_000;

    /// 脚本化上游外面套一层，把每次出站带的 `timeout_ms` 记下来。
    ///
    /// ⚠️ **不自己重造一套脚本上游**：套在 [`ScriptedFetcher`] 外面，
    /// 「脚本用完 = 判据写错了」那条纪律照样生效。
    pub struct TimeoutSpy {
        inner: Arc<ScriptedFetcher>,
        budgets: Mutex<Vec<i64>>,
    }

    impl TimeoutSpy {
        /// 恰好发了一次上游调用，取那一次的预算。
        ///
        /// **次数本身是判据的一部分**：同步档重试时第二次拿的是**剩下的**预算，
        /// 混进来会让这一族判据观察到一个说不清是哪来的数。
        pub fn only_budget(&self) -> i64 {
            let b = self.budgets.lock();
            assert_eq!(b.len(), 1, "应当恰好发一次上游调用");
            b[0]
        }
    }

    #[async_trait::async_trait]
    impl Fetcher for TimeoutSpy {
        async fn send(&self, req: OutboundRequest) -> Result<OutboundResponse, FetchError> {
            self.budgets.lock().push(req.timeout_ms);
            self.inner.send(req).await
        }
    }

    /// 装一套「上游按脚本回话 + 池里有一把 key + 出站预算被记下来」的夹具。
    ///
    /// 🔴 这里的两条守卫是这一族判据成立的前提，**不许删**：
    /// ① 两档的配置默认值没变（变了先回规格 §3.5，别让判据悄悄换了参照物）；
    /// ② **两档算出来的预算真的不一样** —— 一样的话这一族判据区分不了两档，
    ///    每一格都会变成「写了但什么都没钉住」。
    pub async fn state_with_timeout_spy(script: Vec<UpstreamReply>) -> (TestApp, Arc<TimeoutSpy>) {
        let spy = Arc::new(TimeoutSpy {
            inner: ScriptedFetcher::new(script),
            budgets: Mutex::new(Vec::new()),
        });
        let app = state_with(Arc::new(MemoryStorage::new()), spy.clone()).await;
        app.state
            .repo
            .add("sk-judge-0123456789abcdef", now_ms())
            .await
            .expect("判据用的 key 必须放得进去");

        // ⚠️ 这两个数**也是手写的**，不从上面那两个常量推 ——
        //    互推就成了同义反复（04 §3.3 第 ⑥ 条假阳性）。
        let cfg = app.state.config.current();
        assert_eq!(cfg.upstream_timeout_ms, 8_000, "默认值变了先回规格 §3.5");
        assert_eq!(
            cfg.upstream_sync_timeout_ms, 120_000,
            "默认值变了先回规格 §3.5"
        );
        assert_ne!(
            FIRST_BYTE_BUDGET_MS, SYNC_BUDGET_MS,
            "两档的预算一样的话，这一族判据区分不了档位"
        );
        (app, spy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::protocol::request_shape::RequestShapeError;

    /// 🔴 畸形 JSON **一律 400 + JSON 信封**，不是 axum 默认的 422 纯文本。
    #[test]
    fn a_malformed_body_becomes_a_json_400() {
        let e = parse_json(&Bytes::from_static(b"{oops")).unwrap_err();
        assert_eq!(e.status, StatusCode::BAD_REQUEST);
        assert_eq!(e.err_type, "invalid_request_error");
        // 合法 JSON 照常过。
        assert!(parse_json(&Bytes::from_static(br#"{"a":1}"#)).is_ok());
    }

    /// 🔴 整形失败的报文**逐字来自 `RequestShapeError` 的 `Display`**
    /// —— 那三句是对外契约（五语言 API.md 逐字写着），不许在这一层改写。
    #[test]
    fn shape_errors_are_relayed_verbatim() {
        let e = RequestShapeError::UnsupportedContent {
            block_type: "image".into(),
        };
        let g = shape_error(&e);
        assert_eq!(g.status, StatusCode::BAD_REQUEST);
        assert_eq!(g.message, e.to_string());
        assert!(g.message.contains("image"));
    }

    /// 流式响应头两条一起设。
    ///
    /// ⚠️ `cache-control: no-cache` 不是装饰：中间代理缓存住一条 SSE
    /// 会让第二个客户端拿到第一个客户端的回答。
    #[test]
    fn stream_headers_set_both_content_type_and_cache_control() {
        let mut res = Response::new(Body::empty());
        set_stream_headers(&mut res, SSE_CONTENT_TYPE);
        assert_eq!(res.headers()["content-type"], SSE_CONTENT_TYPE);
        assert_eq!(res.headers()["cache-control"], "no-cache");
    }

    /// `respond` 只放它收到的那些头（上游白名单已经在 dispatcher 那边筛过了），
    /// 并且**非法头值不会让整条响应炸掉**。
    #[test]
    fn respond_skips_header_values_it_cannot_encode() {
        let res = respond(
            200,
            &[
                ("content-type".into(), "application/json".into()),
                // 带控制字符的头值：跳过它，不 panic。
                ("cache-control".into(), "no-\ncache".into()),
            ],
            Body::empty(),
        );
        assert_eq!(res.headers()["content-type"], "application/json");
        assert!(res.headers().get("cache-control").is_none());
    }
}
