//! 出站 HTTP 的生产实现。移植自 TS 版 `src/adapters/fetcher-native.ts`
//! （7 行 —— 它就是一层裸 `fetch`，所以**真正要移植的是 `fetch` 的语义**，
//! 而不是那七行代码）。
//!
//! # 🔴 这一层的三条硬性质，全部来自别处的判据
//!
//! 1. **响应体是流，不是 `Vec<u8>`。**
//!    `stream-parity` 那 21 格钉的是「网关不整体缓冲：第一个正文字必须在上游
//!    第二块之前到达」。TS 版第一版对错误体用了 `await res.text()`，被判据当场抓住 ——
//!    **一个永不关闭的 `ReadableStream` 会让 `text()` 挂死**。真实上游会关闭错误体，
//!    **但半开连接不会**。
//! 2. **响应头与响应体分两步到手。**
//!    `admin-upstream-models` 那 18 格钉的是「响应头没来 / 正文没来 / 根本连不上」
//!    **三档不许混成一句**。所以 [`FetchError`] 有三个变体，而这里必须在
//!    「拿到响应头」这一刻把超时**解除**。
//! 3. **`timeout_ms` 是首字节预算，不是整条请求的预算**（数据面）。
//!    规格 01 §3.6 步骤 c 原话：「首字节超时：首字节到达后立刻解除，让响应体自由流动」。
//!    ⇒ **不许用 `reqwest` 的 `RequestBuilder::timeout`**：它盖住整条请求（含读完
//!    响应体），会把一条正常的长流式对话在 8 秒处剪断。
//!
//! # 裁定 K：这一层**刻意不抄** kiro2api 的四样东西
//!
//! | kiro2api 做了 | 这里不做 | 理由 |
//! |---|---|---|
//! | `native-tls-vendored` 可切后端 | **只 rustls** | 那是给自签 CA / 被墙代理场景的投资，agnes 侧没有这个场景的证据；vendored OpenSSL 会给 Dockerfile 拖来 perl/make 与交叉编译 `AR` 两个坑 |
//! | `pool_max_idle_per_host(0)`（连接不复用） | **用默认连接池** | kiro 那条是「同一条 TCP 上轮换几十个身份」这个**风控证据链**的对策，代价是每请求多一次 TLS 握手 |
//! | `http1_only()` | **默认 ALPN** | 同上，是对齐 `aws-sdk-js on Node` 的协议版本，属于指纹对抗 |
//! | UA / machineId 伪装 | **诚实自报** | 见下面 [`USER_AGENT`] |
//!
//! 🔴 规格 §4.2 原话：这四条「**在 agnes 侧没有证据**，除非拿到实测证据，否则不做」。
//! 谁将来要加回来，**先拿实测**（记忆里 agnes 已知的是上游 CF 边缘限流与应用层限流
//! 两道，**与 TLS 指纹不是一回事**）。
//!
//! # ⚠️ `accept-encoding` 与 `content-encoding` 的耦合（规格 §7 矛盾 7）
//!
//! 出站响应头白名单（`SAFE_RESPONSE_HEADERS`）**剥掉了 `content-length` 与
//! `content-encoding`**，理由是「响应体在这里被重新包装，**fetch 已解压**」。
//! 这条论证要成立，Rust 侧就必须让 `reqwest` 真的解压 ⇒ `Cargo.toml` 里
//! `gzip`/`brotli`/`deflate` 三个 feature **必须开着**。
//!
//! 连带代价与本文件的**显式决定**，逐条写在 [`build_headers`] 上。

use crate::ports::fetcher::{BodyStream, FetchError, Fetcher, OutboundRequest, OutboundResponse};
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::time::Duration;

/// 出站 UA。**诚实自报，不是伪装。**
///
/// 裁定 K 禁的是「假装成某个真实客户端」（kiro 对齐 `aws-sdk-js on Node` 那一套），
/// 不是「不许有 UA」。`reqwest` 默认**一个 UA 都不发**，而「完全没有 UA」本身
/// 就是一个显眼且难排障的 wire 特征。
///
/// ⚠️ 调用方在 `OutboundRequest.headers` 里显式给 `user-agent` 时**以调用方为准**
/// （逐请求头覆盖默认头，这是 `reqwest` 的既定语义）。
const USER_AGENT: &str = concat!("agnes2api/", env!("CARGO_PKG_VERSION"));

/// 这条链路是**数据面**还是**控制面**。两者对 `timeout_ms` 的解释不同。
///
/// 🔴 **差别只有一处，别再往里加东西**：`timeout_ms` 盖住多少。
/// TLS 后端、连接池、协议版本、解压行为两面**完全一致** —— 两套不同的 wire 行为
/// 会让「数据面能过、控制面被上游拒」这类问题变得只能靠读代码回答。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plane {
    /// 转发热路径。`timeout_ms` = **首字节**预算；响应头到手即解除，正文自由流动。
    ///
    /// ⚠️ **代价明写**：正文这一侧因此**没有超时护栏**。一个半开连接会把这条流
    /// 挂到客户端自己断开为止（客户端一断，axum 丢掉这条流 ⇒ `reqwest` 取消上游）。
    /// 这与 TS 版的行为**逐字相同**（那边也是首字节到达就 `clearTimeout`）。
    /// 要不要补一个「块间静默超时」是 P1.5 的候选，**改之前要先改判据**。
    Data,
    /// 管理面探测（模型清单、key 验证、模型连通性测试）。
    ///
    /// `timeout_ms` = **整条请求**的预算（含读完响应体）。理由：这些调用方
    /// **一定会把整个正文读完**，而且它们背后是一个正在等待的管理请求 ——
    /// 那里没有「客户端自己会断」这条兜底，一个半开的正文会把管理请求永远挂住。
    Control,
}

/// `reqwest` 实现的 [`Fetcher`]。
pub struct ReqwestFetcher {
    client: reqwest::Client,
    plane: Plane,
}

impl ReqwestFetcher {
    /// 数据面客户端工厂。
    pub fn data_plane() -> Self {
        Self {
            client: build_client(),
            plane: Plane::Data,
        }
    }

    /// 控制面客户端工厂。
    pub fn control_plane() -> Self {
        Self {
            client: build_client(),
            plane: Plane::Control,
        }
    }

    /// 这条链路是哪一面。**只给判据与装配层的断言用。**
    pub fn plane(&self) -> Plane {
        self.plane
    }
}

/// 客户端工厂的**唯一**一份配置。
///
/// 🔴 数据面与控制面共用它，不是图省事：两处各写一遍必然漂移，
/// 而这里的每一项（TLS、连接池、协议版本、解压）都是 **wire 上可观测的**。
///
/// ⚠️ **构造失败时回落 `Client::new()` 而不是 panic**（照抄 kiro）：
/// 失败概率极低，而 panic 的后果是「网关起不来」，回落的后果只是这个实例
/// 少一条默认头 —— **不对称，所以选回落**。
fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        // 裁定 K：只 rustls。
        .use_rustls_tls()
        .user_agent(USER_AGENT)
        // ⚠️ 刻意**不设** `.timeout()` / `.read_timeout()`：超时一律逐请求算
        // （两档预算由调用方选，见 `Plane`），客户端级的硬顶会把长流式对话剪断。
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// 出站请求头的构造 —— **`accept-encoding` 的显式决定住在这里**（规格 §7 矛盾 3）。
///
/// # 决定：**剥掉调用方给的 `accept-encoding`，让 `reqwest` 自己声明**
///
/// 开了 `gzip`/`brotli`/`deflate` 三个 feature 之后 `reqwest` 的行为是：
/// 请求里**没有** `accept-encoding` ⇒ 它自动补一个并**负责解压**；
/// 请求里**有** ⇒ 它不覆盖，但解压与否仍按 client 的配置来。
///
/// kiro2api 的选择是 `no_gzip/no_brotli/no_deflate`（一个字节都不声明），
/// 因为它要对齐 `aws-sdk-js on Node` 的请求指纹 —— **那条理由在 agnes 侧不存在**
/// （裁定 K），而这里有一条**相反方向**的硬约束：
///
/// 🔴 出站响应头白名单剥掉了 `content-encoding` / `content-length`，
/// 这个白名单成立的**前提**是「响应体到手时已经是解压过的明文」。
/// 如果放任调用方声明一个 `reqwest` 解不开的编码（例如 `zstd`，本仓没开那个
/// feature），后果是：上游按它压、`reqwest` 解不开、白名单又把
/// `content-encoding` 剥掉 ⇒ **客户端收到一坨没有任何标记的压缩字节**。
/// 那是一条静默的正确性故障，排障时看到的现象是「响应是乱码」。
///
/// ⇒ **这里把它剥掉**，把「到手即明文」变成这一层的不变量。
/// 代价明写：调用方**没法**指定出站编码。今天没有调用方需要这件事
/// （`dispatcher` 发给上游的头只有 `content-type` 与 `authorization` 两个），
/// 将来真要，得连同白名单一起重新裁。
///
/// # 顺带剥掉 `content-length`
///
/// 它必须由**实际发出去的字节数**推导。调用方给一个与 `body` 不符的值会造成
/// 帧长度错误（最典型的是改了 body 忘了改它），而 `reqwest` 会按 body 自己算一个 ——
/// **两个 `content-length` 的请求是坏请求**。
fn build_headers(headers: &[(String, String)]) -> Result<HeaderMap, FetchError> {
    let mut map = HeaderMap::new();
    for (k, v) in headers {
        if is_stripped_request_header(k) {
            continue;
        }
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| FetchError::Connect(format!("出站请求头名非法（{k}）: {e}")))?;
        let value = HeaderValue::from_str(v)
            .map_err(|e| FetchError::Connect(format!("出站请求头值非法（{k}）: {e}")))?;
        // `append` 而不是 `insert`：同名头必须能重复，且发送顺序要可复现。
        map.append(name, value);
    }
    Ok(map)
}

/// 剥掉哪些出站请求头。判据见 [`build_headers`]。
fn is_stripped_request_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("accept-encoding") || name.eq_ignore_ascii_case("content-length")
}

/// 上游响应头 → `Vec<(String, String)>`。
///
/// ⚠️ **非 UTF-8 的头值走 lossy 而不是丢弃**：丢弃会让下游的白名单面对一个
/// 「这个头到底有没有」说不清的状态；而真正决定哪些头出得去的是白名单，
/// 不是这里。头名在 `http` crate 里天然是小写 ASCII。
fn collect_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect()
}

#[async_trait]
impl Fetcher for ReqwestFetcher {
    async fn send(&self, req: OutboundRequest) -> Result<OutboundResponse, FetchError> {
        // 非正预算 ⇒ **一个字节都不发**。
        //
        // 调用方（`dispatcher` 的同步档）在预算耗尽时本来就该 `break`，
        // 这里是第二道闸：发一个注定被立刻 abort 的请求，代价是一次真实的
        // TCP/TLS 握手，而收益是零 —— 更糟的是上游会看到一串立刻断开的连接。
        let budget = match u64::try_from(req.timeout_ms) {
            Ok(ms) if ms > 0 => Duration::from_millis(ms),
            _ => return Err(FetchError::TimeoutBeforeHeaders),
        };
        let deadline = tokio::time::Instant::now() + budget;

        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| FetchError::Connect(format!("出站方法名非法（{}）: {e}", req.method)))?;

        let mut builder = self
            .client
            .request(method, &req.url)
            .headers(build_headers(&req.headers)?);
        if let Some(body) = req.body {
            builder = builder.body(body);
        }
        // `build()` 在这里做 URL 解析 —— 失败是**构造**失败，不是网络失败。
        // 归到 `Connect` 这一档是有意的：三档的判据是**处置方式**
        // （本次尝试没有到达上游、该换下一把 key 再试），不是错误来源。
        let request = builder
            .build()
            .map_err(|e| FetchError::Connect(format!("构造出站请求失败: {e}")))?;

        // ── 第一段：等响应头 ────────────────────────────────────────────────
        let response = match tokio::time::timeout_at(deadline, self.client.execute(request)).await {
            Err(_) => return Err(FetchError::TimeoutBeforeHeaders),
            Ok(Err(e)) if e.is_timeout() => return Err(FetchError::TimeoutBeforeHeaders),
            Ok(Err(e)) => return Err(FetchError::Connect(e.to_string())),
            Ok(Ok(r)) => r,
        };

        let status = response.status().as_u16();
        let headers = collect_headers(response.headers());

        // ── 第二段：正文。**超时在这一刻按 `Plane` 决定是否解除。** ──────────
        let body_deadline = match self.plane {
            Plane::Data => None,
            Plane::Control => Some(deadline),
        };

        // ⚠️ `response` 被 move 进这个生成器 ⇒ **丢掉这条流就等于 drop 掉
        //    `reqwest::Response`，底层连接随之被取消**。
        //    `dispatcher` 换 key 重试时「要取消掉被丢弃的那个上游响应体」
        //    （01 §3.3 的 `discard`）在 Rust 侧因此就是一次 `drop`，不需要额外 API。
        let body: BodyStream = Box::pin(async_stream::stream! {
            let mut response = response;
            loop {
                let next = match body_deadline {
                    Some(d) => match tokio::time::timeout_at(d, response.chunk()).await {
                        Err(_) => {
                            yield Err(FetchError::BodyInterrupted(
                                "读取上游响应体超过控制面的总预算".to_string(),
                            ));
                            return;
                        }
                        Ok(v) => v,
                    },
                    None => response.chunk().await,
                };
                match next {
                    Ok(Some(chunk)) => yield Ok(chunk),
                    Ok(None) => return,
                    Err(e) => {
                        yield Err(FetchError::BodyInterrupted(e.to_string()));
                        return;
                    }
                }
            }
        });

        Ok(OutboundResponse {
            status,
            headers,
            body,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据
//
// 🔴 端口层最大的风险是「替身全绿而真实现坏了」，所以这一族必须打**真 HTTP**。
//    两种台子各有各的射程，都要：
//    - `wiremock`：请求形状（方法、头、正文）与响应形状（状态码、解压、头剥离）；
//    - **手写 TCP 服务端**：分块到达的**时序**、只发头不发正文、发到一半就断
//      —— 这三种 `wiremock` 构造不出来，而它们恰好是三档 `FetchError` 的对象。
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures_util::StreamExt;
    use std::time::Instant;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use wiremock::matchers::{method as m_method, path as m_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// `printf 'hello, 压缩过的正文' | gzip -n -9` 的字节。
    ///
    /// 手工固化成常量，是为了在**不引第三个压缩库**的前提下把
    /// 「reqwest 真的解压了」这件事钉住（规格 §7 矛盾 7）。
    const GZIP_BODY: &[u8] = &[
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x03, 0xcb, 0x48, 0xcd, 0xc9, 0xc9,
        0xd7, 0x51, 0x78, 0xda, 0xd7, 0xfd, 0x7c, 0xcf, 0xca, 0x17, 0xfb, 0xdb, 0x9f, 0xcf, 0x6a,
        0x79, 0xb6, 0x76, 0xf1, 0xb3, 0x69, 0xed, 0x00, 0xfd, 0x03, 0x79, 0x3b, 0x19, 0x00, 0x00,
        0x00,
    ];

    fn req(url: String) -> OutboundRequest {
        OutboundRequest {
            method: "POST".into(),
            url,
            headers: vec![("content-type".into(), "application/json".into())],
            body: Some(Bytes::from_static(b"{\"a\":1}")),
            timeout_ms: 5_000,
        }
    }

    /// 把一条 `BodyStream` 读干净。
    async fn drain(mut body: BodyStream) -> Result<Vec<u8>, FetchError> {
        let mut out = Vec::new();
        while let Some(chunk) = body.next().await {
            out.extend_from_slice(&chunk?);
        }
        Ok(out)
    }

    /// 起一个手写 TCP 服务端：收到一个连接后把 `script` 交给它处理，
    /// 返回监听地址。**只服务一个连接**，用完即止。
    async fn raw_server<F, Fut>(script: F) -> String
    where
        F: FnOnce(tokio::net::TcpStream) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                script(stream).await;
            }
        });
        format!("http://{addr}")
    }

    /// 把请求头吃掉（读到 `\r\n\r\n` 为止），免得对端还没读完就被关。
    async fn eat_request(stream: &mut tokio::net::TcpStream) {
        let mut buf = [0u8; 4096];
        let mut seen = Vec::new();
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => seen.extend_from_slice(&buf[..n]),
            }
        }
    }

    // ── wiremock：请求与响应的形状 ──────────────────────────────────────────

    /// 状态码、响应头、正文完整往返。
    #[tokio::test]
    async fn round_trips_status_headers_and_body() {
        let server = MockServer::start().await;
        Mock::given(m_method("POST"))
            .and(m_path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_string("上游正文")
                    .append_header("content-type", "text/plain; charset=utf-8")
                    .append_header("x-upstream-internal", "泄漏面"),
            )
            .mount(&server)
            .await;

        let f = ReqwestFetcher::data_plane();
        let res = f
            .send(req(format!("{}/v1/chat/completions", server.uri())))
            .await
            .unwrap();

        assert_eq!(res.status, 201);
        // 🔴 这一层**原样交出全部响应头**，剥离是白名单那一层的事
        // （`dispatcher` 的 `SAFE_RESPONSE_HEADERS`）。这里少剥一个都不许 ——
        // 两层各剥一半会让「到底谁负责」说不清。
        let names: Vec<&str> = res.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"content-type"));
        assert!(names.contains(&"x-upstream-internal"), "本层不许替上层剥头");
        assert_eq!(drain(res.body).await.unwrap(), "上游正文".as_bytes());
    }

    /// 调用方给的头真的发出去了，且**方法与正文**逐字到达上游。
    #[tokio::test]
    async fn caller_headers_method_and_body_reach_upstream() {
        let server = MockServer::start().await;
        Mock::given(m_method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let f = ReqwestFetcher::data_plane();
        let mut r = req(server.uri());
        r.headers
            .push(("authorization".into(), "Bearer 这是一把假的".into()));
        f.send(r).await.unwrap();

        let got = server.received_requests().await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].method.as_str(), "POST");
        assert_eq!(got[0].body, b"{\"a\":1}");
        assert_eq!(got[0].headers["content-type"], "application/json");
        assert_eq!(got[0].headers["authorization"], "Bearer 这是一把假的");
    }

    /// 🔴 **`accept-encoding` 的显式决定**（规格 §7 矛盾 3）。
    ///
    /// 两件事一起钉：
    /// ① 调用方声明的编码被**剥掉**（否则上游会按一个我们解不开的编码压）；
    /// ② `reqwest` 自己补的那个**真的发出去了**（否则「到手即明文」这条不变量
    ///    就退化成「上游恰好没压」这个运气）。
    #[tokio::test]
    async fn caller_supplied_accept_encoding_is_stripped_and_replaced() {
        let server = MockServer::start().await;
        Mock::given(m_method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let f = ReqwestFetcher::data_plane();
        let mut r = req(server.uri());
        r.headers.push(("accept-encoding".into(), "zstd".into()));
        r.headers.push(("content-length".into(), "99999".into()));
        f.send(r).await.unwrap();

        let got = server.received_requests().await.unwrap();
        let ae = got[0].headers["accept-encoding"].to_str().unwrap();
        assert!(!ae.contains("zstd"), "调用方的编码没被剥掉: {ae}");
        assert!(ae.contains("gzip"), "reqwest 自己那份没发出去: {ae}");
        // 帧长度必须由真实字节数推导，不是调用方说了算。
        assert_eq!(got[0].headers["content-length"], "7");
    }

    /// 🔴 **压缩正文必须在这一层就被解开**（规格 §7 矛盾 7）。
    ///
    /// 回归对象：有人把 `Cargo.toml` 的 `gzip`/`brotli`/`deflate` 关掉
    /// 或者在 builder 上加 `no_gzip()` —— 那时白名单剥掉 `content-encoding`
    /// 的动作会让客户端收到一坨没有标记的压缩字节，**现象是「响应是乱码」**。
    #[tokio::test]
    async fn gzip_bodies_arrive_decompressed_and_lose_their_encoding_headers() {
        let server = MockServer::start().await;
        Mock::given(m_method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(GZIP_BODY.to_vec(), "text/plain")
                    .append_header("content-encoding", "gzip"),
            )
            .mount(&server)
            .await;

        let f = ReqwestFetcher::data_plane();
        let res = f.send(req(server.uri())).await.unwrap();
        let names: Vec<&str> = res.headers.iter().map(|(k, _)| k.as_str()).collect();

        assert_eq!(
            String::from_utf8(drain(res.body).await.unwrap()).unwrap(),
            "hello, 压缩过的正文"
        );
        // 解压之后这两个头描述的是**旧字节**，必须已经不在了。
        assert!(!names.contains(&"content-encoding"), "{names:?}");
        assert!(!names.contains(&"content-length"), "{names:?}");
    }

    /// GET 不带正文一样能发。
    #[tokio::test]
    async fn get_without_a_body_works() {
        let server = MockServer::start().await;
        Mock::given(m_method("GET"))
            .and(m_path("/v1/videos/abc"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let f = ReqwestFetcher::data_plane();
        let res = f
            .send(OutboundRequest {
                method: "GET".into(),
                url: format!("{}/v1/videos/abc", server.uri()),
                headers: vec![("authorization".into(), "Bearer x".into())],
                body: None,
                timeout_ms: 5_000,
            })
            .await
            .unwrap();
        assert_eq!(res.status, 200);
        assert_eq!(drain(res.body).await.unwrap(), b"ok");
    }

    /// 同名响应头（`set-cookie` 那一族）**必须两条都在**。
    ///
    /// 回归对象：把 `Vec<(String, String)>` 换成 map —— 后一条会盖掉前一条，
    /// 而端口 DTO 注释里写死了「同名头必须能重复」。
    #[tokio::test]
    async fn duplicate_response_headers_are_all_preserved() {
        let server = MockServer::start().await;
        Mock::given(m_method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("set-cookie", "a=1")
                    .append_header("set-cookie", "b=2"),
            )
            .mount(&server)
            .await;

        let f = ReqwestFetcher::data_plane();
        let res = f.send(req(server.uri())).await.unwrap();
        let cookies: Vec<&str> = res
            .headers
            .iter()
            .filter(|(k, _)| k == "set-cookie")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(cookies, vec!["a=1", "b=2"]);
    }

    /// 🔴 **丢掉响应体 = 取消上游**，而且客户端还能接着用。
    ///
    /// `dispatcher` 每一条 return 路径都要先把攒着的上一个上游响应体取消掉，
    /// 否则那条响应体永远没人消费、连接也回不了池。这一格钉的是
    /// 「Rust 侧的取消动作就是一次 `drop`，不需要额外 API」。
    #[tokio::test]
    async fn dropping_a_body_releases_the_upstream() {
        let server = MockServer::start().await;
        Mock::given(m_method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(64 * 1024)))
            .mount(&server)
            .await;

        let f = ReqwestFetcher::data_plane();
        let first = f.send(req(server.uri())).await.unwrap();
        drop(first.body); // 一个字节都没读就扔掉

        let second = f.send(req(server.uri())).await.unwrap();
        assert_eq!(second.status, 200);
        assert_eq!(drain(second.body).await.unwrap().len(), 64 * 1024);
    }

    /// 非正预算 ⇒ **一个请求都不许发出去**。
    ///
    /// 回归对象：把这道闸去掉 —— 上游会看到一串握手完立刻断开的连接，
    /// 而那正是限流系统最喜欢盯的形态。
    #[tokio::test]
    async fn a_non_positive_budget_sends_nothing() {
        let server = MockServer::start().await;
        Mock::given(m_method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let f = ReqwestFetcher::data_plane();
        for ms in [0, -1, i64::MIN] {
            let mut r = req(server.uri());
            r.timeout_ms = ms;
            assert!(matches!(
                f.send(r).await,
                Err(FetchError::TimeoutBeforeHeaders)
            ));
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// 畸形的方法名 / URL / 头，一律是 `Connect` 档，**且绝不 panic**。
    #[tokio::test]
    async fn malformed_requests_fail_as_connect_never_panic() {
        let f = ReqwestFetcher::data_plane();

        let mut bad_method = req("http://127.0.0.1:1/".into());
        bad_method.method = "不是方法名".into();
        assert!(matches!(
            f.send(bad_method).await,
            Err(FetchError::Connect(_))
        ));

        let mut bad_url = req("这不是一个 URL".into());
        bad_url.method = "POST".into();
        assert!(matches!(f.send(bad_url).await, Err(FetchError::Connect(_))));

        let mut bad_header = req("http://127.0.0.1:1/".into());
        bad_header.headers.push(("有 空 格".into(), "v".into()));
        assert!(matches!(
            f.send(bad_header).await,
            Err(FetchError::Connect(_))
        ));

        let mut bad_value = req("http://127.0.0.1:1/".into());
        bad_value.headers.push(("x-a".into(), "换\n行".into()));
        assert!(matches!(
            f.send(bad_value).await,
            Err(FetchError::Connect(_))
        ));
    }

    /// 根本连不上 ⇒ `Connect`，**不是**两种超时里的任何一种。
    #[tokio::test]
    async fn a_refused_connection_is_connect_not_timeout() {
        let f = ReqwestFetcher::data_plane();
        // 先占一个端口再立刻释放，拿一个大概率没人听的号。
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);

        let Err(err) = f.send(req(format!("http://{addr}/"))).await else {
            panic!("连不上的端口不该返回成功响应");
        };
        assert!(matches!(err, FetchError::Connect(_)), "实际是 {err:?}");
    }

    // ── 手写 TCP：时序与两种断法 ─────────────────────────────────────────────

    /// 🔴 **响应头没来** ⇒ `TimeoutBeforeHeaders`。
    ///
    /// 对端接受连接、收下请求，然后**什么都不写**。
    #[tokio::test]
    async fn silence_before_headers_is_timeout_before_headers() {
        let url = raw_server(|mut s| async move {
            eat_request(&mut s).await;
            tokio::time::sleep(Duration::from_secs(30)).await;
        })
        .await;

        let f = ReqwestFetcher::data_plane();
        let mut r = req(url);
        r.timeout_ms = 200;
        let Err(err) = f.send(r).await else {
            panic!("响应头没来却返回了成功响应");
        };
        assert!(matches!(err, FetchError::TimeoutBeforeHeaders), "{err:?}");
    }

    /// 🔴 **正文没来**（头到了、正文写到一半就断）⇒ `BodyInterrupted`。
    ///
    /// 这三档不许混成一句，是 `admin-upstream-models` 那 18 格的直接要求。
    #[tokio::test]
    async fn a_truncated_body_is_body_interrupted_not_connect() {
        let url = raw_server(|mut s| async move {
            eat_request(&mut s).await;
            // 声明 100 字节，只发 5 个，然后直接断。
            let _ = s
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nhello")
                .await;
            let _ = s.flush().await;
            drop(s);
        })
        .await;

        let f = ReqwestFetcher::data_plane();
        let mut r = req(url);
        r.timeout_ms = 5_000;
        let res = f.send(r).await.unwrap();
        assert_eq!(res.status, 200, "响应头是到了的");
        let err = drain(res.body).await.unwrap_err();
        assert!(matches!(err, FetchError::BodyInterrupted(_)), "{err:?}");
    }

    /// 🔴 **网关不整体缓冲**：第一块必须在上游写第二块**之前**就到手。
    ///
    /// 回归对象：把响应体换成 `.bytes().await` / `.text().await` ——
    /// 那一版会一直等到上游把整条流写完（或者对着一个永不关闭的流挂死）。
    /// 判据用「第一块到手时耗时远小于两块之间的间隔」来观察这件事。
    #[tokio::test]
    async fn the_first_chunk_arrives_before_the_upstream_writes_the_second() {
        const GAP: Duration = Duration::from_millis(600);
        let url = raw_server(|mut s| async move {
            eat_request(&mut s).await;
            let _ = s
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                      Transfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n",
                )
                .await;
            let _ = s.flush().await;
            tokio::time::sleep(GAP).await;
            let _ = s.write_all(b"5\r\nworld\r\n0\r\n\r\n").await;
            let _ = s.flush().await;
        })
        .await;

        let f = ReqwestFetcher::data_plane();
        let mut r = req(url);
        r.timeout_ms = 5_000;

        let started = Instant::now();
        let res = f.send(r).await.unwrap();
        let mut body = res.body;
        let first = body.next().await.unwrap().unwrap();
        let first_at = started.elapsed();

        assert_eq!(first.as_ref(), b"hello");
        assert!(
            first_at < GAP / 2,
            "第一块等了 {first_at:?}，说明正文被整体缓冲了"
        );

        // 剩下的照样收得到，收尾正常。
        let rest = drain(body).await.unwrap();
        assert_eq!(rest, b"world");
        assert!(started.elapsed() >= GAP);
    }

    /// 🔴 **数据面：首字节到手之后超时就解除了。**
    ///
    /// 预算 300 毫秒、两块之间隔 600 毫秒 —— 整条流照样读得完。
    /// 回归对象：用 `RequestBuilder::timeout()` 代替这里的两段式 ——
    /// 那会把一条正常的长流式对话在预算处剪断。
    #[tokio::test]
    async fn data_plane_lets_a_slow_body_finish_after_the_headers_arrived() {
        let url = raw_server(|mut s| async move {
            eat_request(&mut s).await;
            let _ = s
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n")
                .await;
            let _ = s.flush().await;
            tokio::time::sleep(Duration::from_millis(600)).await;
            let _ = s.write_all(b"5\r\nworld\r\n0\r\n\r\n").await;
            let _ = s.flush().await;
        })
        .await;

        let f = ReqwestFetcher::data_plane();
        let mut r = req(url);
        r.timeout_ms = 300;
        let res = f.send(r).await.unwrap();
        assert_eq!(drain(res.body).await.unwrap(), b"helloworld");
    }

    /// 🔴 **控制面：预算盖住整条请求。**
    ///
    /// 同一个慢正文，在控制面必须被 `BodyInterrupted` 截停 ——
    /// 管理请求背后没有「客户端自己会断」这条兜底。
    ///
    /// 这一格与上一格是**同一个上游、同一个预算**，只换了 `Plane`：
    /// 两者一起读才说得清 `Plane` 到底是什么。
    #[tokio::test]
    async fn control_plane_cuts_a_body_that_overruns_the_total_budget() {
        let url = raw_server(|mut s| async move {
            eat_request(&mut s).await;
            let _ = s
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n")
                .await;
            let _ = s.flush().await;
            tokio::time::sleep(Duration::from_millis(600)).await;
            let _ = s.write_all(b"5\r\nworld\r\n0\r\n\r\n").await;
            let _ = s.flush().await;
        })
        .await;

        let f = ReqwestFetcher::control_plane();
        assert_eq!(f.plane(), Plane::Control);
        let mut r = req(url);
        r.timeout_ms = 300;
        let res = f.send(r).await.unwrap();
        assert_eq!(res.status, 200, "响应头在预算内到了");
        let err = drain(res.body).await.unwrap_err();
        assert!(matches!(err, FetchError::BodyInterrupted(_)), "{err:?}");
    }

    /// 两个工厂造出来的客户端**除了 `Plane` 之外没有第二处差别**。
    ///
    /// 这一格钉的是「别往 `Plane` 里加东西」那条注释：一旦两面的 wire 行为分叉，
    /// 「数据面能过、控制面被拒」这类问题就只能靠读代码回答。
    #[tokio::test]
    async fn both_planes_put_the_same_bytes_on_the_wire() {
        let server = MockServer::start().await;
        Mock::given(m_method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        ReqwestFetcher::data_plane()
            .send(req(server.uri()))
            .await
            .unwrap();
        ReqwestFetcher::control_plane()
            .send(req(server.uri()))
            .await
            .unwrap();

        let got = server.received_requests().await.unwrap();
        assert_eq!(got.len(), 2);
        let names = |r: &wiremock::Request| {
            let mut v: Vec<String> = r.headers.keys().map(|k| k.as_str().to_string()).collect();
            v.sort();
            v
        };
        assert_eq!(names(&got[0]), names(&got[1]));
        assert_eq!(got[0].headers["user-agent"], got[1].headers["user-agent"]);
    }

    /// UA **诚实自报**，且带着本网关的版本号。
    ///
    /// 回归对象：有人为了「像个真实客户端」把它改成某个浏览器/SDK 的 UA ——
    /// 裁定 K 禁的就是那件事，而且那种改动**没有任何实测证据支持**。
    #[tokio::test]
    async fn the_user_agent_identifies_this_gateway_honestly() {
        let server = MockServer::start().await;
        Mock::given(m_method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        ReqwestFetcher::data_plane()
            .send(req(server.uri()))
            .await
            .unwrap();

        let got = server.received_requests().await.unwrap();
        let ua = got[0].headers["user-agent"].to_str().unwrap();
        assert!(ua.starts_with("agnes2api/"), "UA 被改成了别人: {ua}");
        assert!(ua.contains(env!("CARGO_PKG_VERSION")));
    }

    /// 调用方显式给的 `user-agent` 覆盖默认值（默认头的既定语义）。
    #[tokio::test]
    async fn a_caller_supplied_user_agent_wins() {
        let server = MockServer::start().await;
        Mock::given(m_method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut r = req(server.uri());
        r.headers.push(("user-agent".into(), "调用方说了算".into()));
        ReqwestFetcher::data_plane().send(r).await.unwrap();

        let got = server.received_requests().await.unwrap();
        assert_eq!(got[0].headers["user-agent"], "调用方说了算");
    }

    /// 剥离判据本身：大小写不敏感，且**只剥这两个**。
    #[test]
    fn only_two_request_headers_are_stripped() {
        assert!(is_stripped_request_header("Accept-Encoding"));
        assert!(is_stripped_request_header("ACCEPT-ENCODING"));
        assert!(is_stripped_request_header("Content-Length"));
        for keep in [
            "authorization",
            "content-type",
            "user-agent",
            "accept",
            "x-goog-api-key",
        ] {
            assert!(!is_stripped_request_header(keep), "{keep} 不该被剥掉");
        }
    }

    /// 非 UTF-8 的头值走 lossy，**不许丢掉这个头**。
    #[test]
    fn non_utf8_header_values_survive_as_lossy_text() {
        let mut h = HeaderMap::new();
        h.insert("x-weird", HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap());
        let out = collect_headers(&h);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "x-weird");
        assert!(!out[0].1.is_empty());
    }
}
