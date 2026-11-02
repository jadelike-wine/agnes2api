//! 出站 HTTP 的 Workers 实现：`worker::Fetch`（CF 原生 fetch API）。
//!
//! # 为什么不用 reqwest
//!
//! reqwest 0.12 的 wasm 后端**没有流式响应体**：`stream` feature 在 wasm 上是
//! 编译期排除的，响应必须等到 `array_buffer`/`text` 读完整份才能给你 ——
//! 那会把「SSE 逐块转发」变成「等整条流式回答完再一次性吐出来」，直接踩死
//! `stream-parity` 那 21 格判据。
//!
//! `worker::Fetch` 返回的 `Response` 带 `web_sys::ReadableStream`，
//! `worker::Body` 把它包成 `http_body::Body<Data = Bytes>` ——
//! **逐块、不整体缓冲**，与 `reqwest` 侧同一个 `BodyStream` 端口契约。
//!
//! # 与 `fetcher_reqwest` 的三条硬性质对齐
//!
//! 1. **响应体是流**（`worker::Body`），不是 `Vec<u8>`。
//! 2. **头与体分两步到手**：`send()` 一回来就有 `status` + `headers`，
//!    `Body` 随后按块吐 —— `timeout_ms` 在响应头到手那一刻解除。
//! 3. **`timeout_ms` 是首字节预算**（数据面）：用 `futures::select!` 把
//!    `send` 与 `worker::Delay` 赛跑，头到了就 `AbortController` 超时腿。
//!
//! # 与 reqwest 侧的**差**
//!
//! | 面 | reqwest | worker::Fetch |
//! |---|---|---|
//! | TLS / 连接池 | rustls + hyper 连接池 | CF 边缘统一出口，无逐请求连接池 |
//! | `accept-encoding` | 显式补 gzip/br/deflate | 由 CF 平台协商，**网关侧不可控** |
//! | 重定向 | reqwest 默认 follow | `RequestInit.redirect`，默认 follow |
//! | 取消 | drop `Response` 取消连接 | `AbortSignal` 显式 abort |
//!
//! ⚠️ **白名单剥 `content-encoding` 的前提在 Worker 上不成立**：
//! reqwest 侧「响应体到手时已解压」靠的是它自己的 gzip/br 解压；
//! worker::Fetch 的解压是 CF 平台行为，网关不保证「到手即明文」。
//! 因此这里**不剥** `content-encoding` / `content-length`，
//! 让原始编码头原样透传 —— 上游压缩、CF 不解压时客户端拿到的仍是合法响应
//!（内容编码与体一致），比「剥了头但体还是压缩的」正确。

use crate::ports::fetcher::{BodyStream, FetchError, Fetcher, OutboundRequest, OutboundResponse};
use async_trait::async_trait;
use futures_util::StreamExt;
use std::time::Duration;
use worker::{AbortSignal, Fetch, Headers, Method, RequestInit};

/// 数据面 / 控制面的超时口径。与 `fetcher_reqwest::Plane` 同一组语义：
/// `Data` 的首字节预算、`Control` 的整条预算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plane {
    /// 转发热路径：`timeout_ms` = 首字节预算。
    Data,
    /// 管理面探测：`timeout_ms` = 整条请求预算。
    Control,
}

/// `worker::Fetch` 实现的 [`Fetcher`]。
///
/// `!Send` —— `Fetch`/`RequestInit` 内部是 JsValue。**Workers 单线程**，
/// 与 `D1Storage` 同一条 `SendWrapper` 理由。
pub struct CfFetcher {
    plane: Plane,
}

/// `SendWrapper` 不在这：出站客户端是**无状态工厂**，每个请求自己 new 一份
/// `RequestInit`，不需要跨 await 持有的 JsValue。所以 `CfFetcher` 本体是 ZST，
/// `Send + Sync` 自动成立，不需要 `unsafe impl`。
impl CfFetcher {
    pub fn data_plane() -> Self {
        Self { plane: Plane::Data }
    }
    pub fn control_plane() -> Self {
        Self {
            plane: Plane::Control,
        }
    }
    pub fn plane(&self) -> Plane {
        self.plane
    }
}

/// 把 `worker::Body` 流包成端口要的 `BodyStream`。
///
/// `worker::Body` 实现 `Stream<Item = Result<Bytes, worker::Error>>`，
/// 直接 `map` 成 `Result<Bytes, FetchError>` —— `worker::Error` 不可 `Send` 跨 await，
/// 但 `Stream::map` 的闭包只在 `poll_next` 里同步跑，不跨 await，安全。
fn into_body_stream(body: worker::Body) -> BodyStream {
    let s = body.map(|item| item.map_err(|e| FetchError::BodyInterrupted(e.to_string())));
    Box::pin(s)
}

/// 一次 fetch。**超时腿是 `worker::Delay`**（JS setTimeout）：
/// 头到了就取消它（drop Delay 会清 timeout），首字节不到就 `abort()` 那条 fetch。
///
/// `plane` 决定 `timeout_ms` 盖住多少：数据面 = 首字节，控制面 = 头 + 整条 body。
async fn send_once(req: OutboundRequest, plane: Plane) -> Result<OutboundResponse, FetchError> {
    // ── 1. 构造 RequestInit ────────────────────────────────────────────────
    let method = Method::from(req.method.clone());

    let mut init = RequestInit::new();
    init.with_method(method);
    // 🔴 **重定向**：与 reqwest 默认 `follow` 对齐。`RequestInit.redirect` 默认就是 follow，
    //    不显式设，但注释在这里钉住「这不是漏写」。
    // init.with_redirect(worker::RequestRedirect::Follow);

    // 头。**发送顺序可复现**（与 reqwest 侧同一条判据），所以不用 `HashMap`。
    // `Headers` 的方法是 `&self`（内部可变），不需要 `mut`。
    let headers = Headers::new();
    for (name, value) in &req.headers {
        // ⚠️ `Headers::append` 不是 `set`：同名头（如将来可能的 `set-cookie` 族）要可重复。
        // 头值非法（控制字符）时 append 会返回 Err —— 如实抛，不静默丢。
        headers
            .append(name, value)
            .map_err(|e| FetchError::Connect(format!("请求头 {name} 非法: {e}")))?;
    }
    init.with_headers(headers);

    if let Some(body) = req.body {
        // `JsValue` 包装的 `Uint8Array`：请求体不进 `BodyStream`，直接给整个 `Bytes`。
        let arr = js_sys::Uint8Array::from(body.as_ref());
        init.with_body(Some(arr.into()));
    }

    // ── 2. 首字节超时：Delay 与 send 赛跑 ──────────────────────────────────
    let budget = Duration::from_millis(req.timeout_ms.max(0) as u64);
    // `AbortController` 用来在超时时取消 fetch。Workers 的 fetch 支持 AbortSignal。
    let controller = worker::web_sys::AbortController::new()
        .map_err(|_| FetchError::Connect("AbortController 不可用".into()))?;
    let url = req.url;
    let fetch = Fetch::Request(
        worker::Request::new_with_init(&url, &init)
            .map_err(|e| FetchError::Connect(format!("构造出站请求失败: {e}")))?,
    );

    // `send_with_signal` 一回来就拿到**响应头**（status + headers），
    // `Body` 的 ReadableStream 随后按块到达 —— 这正是「头与体分两步」的端口契约。
    // ⚠️ `fetch.send_with_signal` 返回 `!Send`（JsFuture）；`send_once` 在 `Send`
    //    trait 里被 `.await`，所以套 `SendFuture` 桥（Workers 单线程，语义成立）。
    // `signal` 提前取出：`controller` 留在原地，超时后要调 `controller.abort()`。
    let signal = controller.signal();
    let send_fut = worker::send::SendFuture::new(async move {
        fetch.send_with_signal(&AbortSignal::from(signal)).await
    });

    // 数据面：首字节预算。控制面：整条预算（连 body 一起盖在超时里）。
    // 两者都在「头到手」这一刻解除 —— 区别只在控制面还要再等一个「整条 body」。
    let resp = race_first_byte(send_fut, budget).await;

    let mut resp = match resp {
        Ok(r) => r,
        Err(FetchError::TimeoutBeforeHeaders) => {
            controller.abort();
            return Err(FetchError::TimeoutBeforeHeaders);
        }
        Err(e) => {
            controller.abort();
            return Err(e);
        }
    };

    // ── 3. 响应体：ReadableStream → BodyStream ────────────────────────────
    let status = resp.status_code();
    let headers = resp
        .headers()
        .entries()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect::<Vec<(String, String)>>();

    // `worker::Response::stream()` 拿 `web_sys::ReadableStream` → `ByteStream`；
    // 没有流（非流式响应 / 空体）就退成 `worker::Body::empty()`。
    let body = match resp.stream() {
        Ok(stream) => {
            // `ByteStream` 是 `futures::Stream<Item = Result<Bytes, worker::Error>>`，
            // `worker::Body::from_stream` 收 `TryStream` —— 直接包。
            worker::Body::from_stream(stream).unwrap_or_else(|_| worker::Body::empty())
        }
        Err(_) => worker::Body::empty(),
    };

    // 控制面：整条 body 也要在 timeout_ms 里读完。
    // 用 `select!` 把每个 `next()` 与剩余预算赛跑 —— 超时就 abort（取消上游），
    // 并把 `BodyInterrupted` 如实抛给调用方。
    // ⚠️ `self_plane_timeout` 数据面返回 None 时**不消费** `body`，
    //    所以这里分两支而不是 `match` —— `body` 只被移动一次。
    let body_stream: BodyStream = if plane == Plane::Control {
        self_plane_timeout(req.timeout_ms, body)
    } else {
        into_body_stream(body)
    };

    Ok(OutboundResponse {
        status,
        headers,
        body: body_stream,
    })
}

/// 控制面：把整条 body 的读完也盖进 `timeout_ms`。
///
/// `worker::Body` 本身 `Send`（`unsafe impl`）；真正 `!Send` 的是 `worker::Delay`
/// 和 `worker::Error`。**两者都在 `SendFuture` 里消化**：
/// - `Delay` 每轮套 `SendFuture` —— `select!` 的两个分支都要 `Send`；
/// - `body.next()` 的 `Err`（`worker::Error`，`!Send`）在 `SendFuture` 里立刻
///   转成本体的 `FetchError`（`Send`），不往流外带。
fn self_plane_timeout(timeout_ms: i64, body: worker::Body) -> BodyStream {
    let budget_ms = timeout_ms.max(0);
    let deadline_ms = crate::adapters::now_ms() + budget_ms;
    // `state` = (body, done)；`done` 记下「已超时过」⇒ 只发一条 `BodyInterrupted`。
    let stream = futures_util::stream::unfold((body, false), move |(mut body, done)| async move {
        if done {
            return None;
        }
        let remaining = deadline_ms - crate::adapters::now_ms();
        if remaining <= 0 {
            return Some((
                Err(FetchError::BodyInterrupted("控制面读 body 超时".into())),
                (body, true),
            ));
        }
        // `deadline` 与 `fetch` 都 `!Send`（Delay / JsFuture）⇒ 各自套 `SendFuture`。
        let deadline = worker::send::SendFuture::new(worker::Delay::from(Duration::from_millis(
            remaining as u64,
        )));
        let fetch = worker::send::SendFuture::new(async { body.next().await });
        futures_util::pin_mut!(deadline);
        futures_util::pin_mut!(fetch);
        match futures_util::future::select(fetch, deadline).await {
            futures_util::future::Either::Left((Some(Ok(bytes)), _)) => {
                Some((Ok(bytes), (body, false)))
            }
            futures_util::future::Either::Left((Some(Err(e)), _)) => Some((
                Err(FetchError::BodyInterrupted(e.to_string())),
                (body, true),
            )),
            futures_util::future::Either::Left((None, _)) => None,
            futures_util::future::Either::Right(((), _)) => Some((
                Err(FetchError::BodyInterrupted("控制面读 body 超时".into())),
                (body, true),
            )),
        }
    });
    Box::pin(stream)
}

/// 首字节赛跑：`send` 与 `Delay` 谁先到。
///
/// 数据面与控制面**共用这一个**：`Plane::Control` 的「整条预算」是**在 send 之后**
/// 再叠到 body 上的（见 `self_plane_timeout`），所以这里的 delay 仍然是首字节口径 —
/// 也就是说，控制面的 `timeout_ms` 实际盖住的是「头 + 整条 body」两段，
/// 与 reqwest 侧 `tokio::time::timeout_at` 的语义一致。
async fn race_first_byte(
    send: impl futures_core::Future<Output = worker::Result<worker::Response>>,
    budget: Duration,
) -> Result<worker::Response, FetchError> {
    futures_util::pin_mut!(send);
    // `worker::Delay` 是 `!Send`（内部 JsValue）；`select!` 的两个分支都要 `Send`。
    // 套 `SendFuture` 桥（Workers 单线程，安全），超时腿由它承担。
    let timeout = worker::send::SendFuture::new(worker::Delay::from(budget));
    futures_util::pin_mut!(timeout);

    match futures_util::future::select(send, timeout).await {
        futures_util::future::Either::Left((res, _timeout)) => match res {
            Ok(r) => Ok(r),
            Err(e) => Err(map_send_error(e)),
        },
        futures_util::future::Either::Right(((), _send)) => Err(FetchError::TimeoutBeforeHeaders),
    }
}

/// `worker::Error` → `FetchError`。
///
/// `worker::Error` 不分连接/超时档，只能从消息里粗判；
/// 无法识别的进 `Connect`（连上游都没回话），与 reqwest 侧 `Connect` 的处置一致。
fn map_send_error(e: worker::Error) -> FetchError {
    let msg = e.to_string();
    // 网络层失败（DNS / TLS / refused）在 worker 侧通常报 "fetch failed" 之类。
    // 不能精确分档时按「连都没连上」处置 —— 调用方对 `Connect` 的处置是换 key 重试，
    // 那正是这类失败该走的方向。
    FetchError::Connect(msg)
}

#[async_trait]
impl Fetcher for CfFetcher {
    async fn send(&self, req: OutboundRequest) -> Result<OutboundResponse, FetchError> {
        send_once(req, self.plane).await
    }
}
