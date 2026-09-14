//! 出站 HTTP 端口。
//!
//! # 本轮的状态：**只有 trait 与 DTO，没有任何实现**
//!
//! 生产实现 `adapters::fetcher_reqwest` 属于 P1-a 的 a2，**且规格 §8.1 把它标成
//! 阻塞项**：TS 版的 `src/adapters/fetcher-native.ts` 至今没有一手核对过，
//! 而「出站响应头白名单剥掉 content-encoding」这条论证建立在它的具体行为上。
//! 在补完那一轮勘察之前先写实现，等于在转述上盖楼。
//!
//! 定义 trait 本身没有这个风险：它是**边界**，不是行为。

use async_trait::async_trait;
use bytes::Bytes;
use futures_core::Stream;
use std::fmt;
use std::pin::Pin;

/// 响应体：一条**按块到达**的字节流。
///
/// 🔴 **绝不许换成 `Vec<u8>`**：`stream-parity` 那 21 格判据钉的正是
/// 「网关不整体缓冲 —— 第一个正文字必须在上游第二块之前到达」。
/// 一旦这里变成完整 body，那族判据就失去了对象，而现象是「流式对话变成
/// 等几十秒然后一次性吐出来」。
pub type BodyStream = Pin<Box<dyn Stream<Item = Result<Bytes, FetchError>> + Send>>;

/// 出站请求。**刻意不含 key**：选哪把 key、怎么放进 `Authorization`，
/// 是 `pool::dispatcher` 的职责，端口只管把字节发出去。
pub struct OutboundRequest {
    pub method: String,
    pub url: String,
    /// 头列表用 `Vec` 而不是 map：同名头（`set-cookie` 那一族）必须能重复，
    /// 而且**发送顺序要可复现**（判据要对着字节比）。
    pub headers: Vec<(String, String)>,
    pub body: Option<Bytes>,
    /// 首字节超时（毫秒）。两档超时（对话 vs 同步端点）的选择在调用方，
    /// 端口只拿到已经选好的那一个值。
    pub timeout_ms: i64,
}

/// 出站响应。**头与体分开到手**，这是「传输失败分三档」那条判据的前提：
/// 「响应头没来」「正文没来」「根本连不上」三种失败**不许混成一句**。
pub struct OutboundResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: BodyStream,
}

/// 出站失败。**分档依据是「处置方式不同」，不是「错误来源不同」**（对标 kiro）。
#[derive(Debug, Clone)]
pub enum FetchError {
    /// 连都没连上（DNS / TCP / TLS）。
    Connect(String),
    /// 连上了，但在**响应头到达之前**超时。
    TimeoutBeforeHeaders,
    /// 响应头到手了，但读正文时断了或超时。
    ///
    /// ⚠️ 与上一档分开是硬要求：`admin-upstream-models` 那 18 格钉着
    /// 「响应头没来 / 正文没来 / 根本连不上」三档不许混成一句。
    BodyInterrupted(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(d) => write!(f, "连接上游失败: {d}"),
            Self::TimeoutBeforeHeaders => f.write_str("等待上游响应头超时"),
            Self::BodyInterrupted(d) => write!(f, "读取上游响应体中断: {d}"),
        }
    }
}

impl std::error::Error for FetchError {}

/// 出站 HTTP。
#[async_trait]
pub trait Fetcher: Send + Sync {
    async fn send(&self, req: OutboundRequest) -> Result<OutboundResponse, FetchError>;
}
