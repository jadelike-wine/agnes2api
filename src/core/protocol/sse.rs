//! SSE 原语（`protocol/sse.ts`，01 §6.3）：帧边界、负载抽取、事件拼装、增量解析。
//!
//! **零 IO**：这里只做「字节流 → 负载串」与「数据 → 事件串」的纯变换。
//! 谁去打上游、谁去写 socket，都不在这一层。
//!
//! # Rust 侧与 TS 侧的两处结构差异（都是有意的）
//!
//! 1. **`signal` / `AbortController` 那一整套不移植**（01 §6.3 明写「JS 特有病」）。
//!    TS 里必须靠带外 `reader.cancel()` 才能打断一个正阻塞在 `read()` 上的异步生成器
//!    （`.return()` 会排在已在飞行中的 `.next()` 后面，永远轮不到执行）。
//!    Rust 的 `Stream` 没有这个队列：**调用方 `drop` 掉返回的流，底下的上游流跟着 `drop`，
//!    连接当场释放。** 语义（「客户端断连必须真的释放上游连接」）一字不少地保留了，
//!    ⚠️ 但它的**保障方式变成了所有权**——所以本模块不许把上游流 `Box::leak` 或塞进
//!    任何比返回流活得久的地方。
//! 2. **错误类型是一个不带任何文字的单元结构** [`UpstreamStreamError`]。
//!    这不是偷懒：🔴 三条协议的断流分支都**只许**说 [`UPSTREAM_STREAM_INTERRUPTED`]
//!    那一句，**绝不回显上游异常的 message**（它可能带上游 URL 与栈帧）。
//!    让错误类型**结构上装不下任何文字**，这条纪律就不可能被下一个人顺手违反。

use bytes::Bytes;
use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::Value;

/// 🔴 三条协议断流时**唯一**许可的那句话。
///
/// ⚠️ **只报这一句，不回显上游异常的 message**：它可能带上游 URL 与栈帧。
/// 三条协议（Anthropic 的 `error` 事件、Gemini 的 `finishReason: "OTHER"` 终帧、
/// Responses 的 `response.failed`）各按自己的形状说这同一句话。
pub const UPSTREAM_STREAM_INTERRUPTED: &str = "上游流式响应中断，本次回答不完整";

/// 上游流在半路读失败了。**刻意不带任何文字**，理由见模块头第 2 条。
///
/// 适配器（`reqwest` 那一侧）把任何一种读失败都塞成这一个值 —— core 是零 IO 岛，
/// 不认识 `reqwest::Error`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpstreamStreamError;

impl std::fmt::Display for UpstreamStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(UPSTREAM_STREAM_INTERRUPTED)
    }
}

impl std::error::Error for UpstreamStreamError {}

/// 上游响应体的块类型。端口 DTO 与 axum / reqwest 共用同一个 [`Bytes`]，避免来回拷。
pub type ByteChunk = Result<Bytes, UpstreamStreamError>;

/// 一帧的边界 = **一个空行**，`\n\n` 与 `\r\n\r\n` 两种都认。返回 `(起始下标, 分隔符长度)`。
///
/// ⚠️ **为什么必须认 CRLF**：网关自己发的是 `\n\n`，但**上游不归我们管**——上游或中间
/// 任何一层反代把行尾改写成 CRLF 时，只认 `\n\n` 的话永远找不到边界，于是这条流会
/// 一路攒到上游关闭。**失败形态是「退化成一次性 + 全流无界缓冲」，不是「一条都读不出来」**
/// （实测），而**中途断流时收尾那次 flush 根本不跑 ⇒ 已经该交出去的负载 100% 丢失。**
///
/// ⚠️ **只认这两种，不认裸 `\r`**（EventSource 规范里 CR 单独也算换行）：本仓至今没有
/// 见过只发裸 CR 的上游，而多认一种就要多一条没有真实样本支撑的分支。
/// **如实登记，不假装它是完备的 SSE 实现。**
///
/// ⚠️ **取两者里靠前的那一个**，不是「先找 LF 找不到再找 CRLF」：同一段缓冲里两种行尾
/// 混着出现（换代理、断点续传）时，后者会把中间整段当成一帧，**帧边界就错位了**。
#[must_use]
pub fn frame_end(buf: &str) -> Option<(usize, usize)> {
    let lf = buf.find("\n\n");
    let crlf = buf.find("\r\n\r\n");
    match (lf, crlf) {
        (_, Some(c)) if lf.is_none_or(|l| c < l) => Some((c, 4)),
        (Some(l), _) => Some((l, 2)),
        _ => None,
    }
}

/// 一个块里抽出来的东西。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Payloads {
    /// 逐条 `data:` 负载（已 `trim`，已剔掉空串）。
    pub payloads: Vec<String>,
    /// 见到了 `[DONE]`。**它本身不进 `payloads`**，但它之前收集到的照常交出。
    pub done: bool,
}

/// 逐行；只看 `data:` 开头的；取 `line[5..].trim()`；`== "[DONE]"` ⇒ 立刻返回 `done`；空串跳过。
///
/// ⚠️ **行内那个 `\r` 由这里的 `trim()` 吃掉，不许再加第二次 trim** —— 那会把负载正文里
/// 合法的前后空白也一起吃掉。
#[must_use]
pub fn extract_payloads(block: &str) -> Payloads {
    let mut payloads = Vec::new();
    for line in block.split('\n') {
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = rest.trim();
        if payload == "[DONE]" {
            return Payloads {
                payloads,
                done: true,
            };
        }
        if !payload.is_empty() {
            payloads.push(payload.to_string());
        }
    }
    Payloads {
        payloads,
        done: false,
    }
}

/// 拼一个 SSE 事件串：`data: {json}\n\n`，`event` 非 `None` 时前面加 `event: {name}\n`。
///
/// ⚠️ Gemini 那条协议**不带事件名**（纯 `data:`），所以 `event` 是 `Option`，不是必填。
#[must_use]
pub fn sse_event(event: Option<&str>, data: &Value) -> String {
    // serde_json 的 `to_string` 对 `Value` 不会失败（不含 NaN/Inf —— 它们在 Value 里
    // 根本构造不出来）。真到了不可能的那一支，宁可发一个空对象也不 panic：
    // 这条路径在流式响应中途，panic 会让客户端拿到一个半截连接而不是任何诊断信息。
    let body = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
    match event {
        Some(name) => format!("event: {name}\ndata: {body}\n\n"),
        None => format!("data: {body}\n\n"),
    }
}

/// 增量 UTF-8 解码器 —— 对齐 TS 的 `new TextDecoder()` + `decode(v, {stream:true})`。
///
/// 🔴 **它不可省**：一个多字节字符被切在两个 TCP 块之间是**常态**而不是边角情况
/// （中文正文下每几十字节就有一次机会）。直接 `String::from_utf8_lossy` 每块解一次，
/// 那个字符就会变成两个 `U+FFFD`，**而且是静默的**。
#[derive(Debug, Default)]
struct Utf8Incremental {
    /// 上一块尾部那几个「还不成字」的字节。
    pending: Vec<u8>,
}

impl Utf8Incremental {
    /// 喂一块字节，交出**这一块能确定下来**的那部分文本。不成字的尾巴留到下一块。
    fn push(&mut self, chunk: &[u8]) -> String {
        self.pending.extend_from_slice(chunk);
        let mut out = String::new();
        loop {
            let (valid, err_len) = match std::str::from_utf8(&self.pending) {
                Ok(_) => (self.pending.len(), None),
                Err(e) => (e.valid_up_to(), Some(e.error_len())),
            };
            // `..valid` 一定是合法 UTF-8（`valid_up_to` 的定义），这里不会失败。
            if let Ok(s) = std::str::from_utf8(&self.pending[..valid]) {
                out.push_str(s);
            }
            match err_len {
                // 整段都合法。
                None => {
                    self.pending.clear();
                    return out;
                }
                // 尾巴还没凑齐一个字符 —— 留着等下一块。
                Some(None) => {
                    self.pending.drain(..valid);
                    return out;
                }
                // 真的是坏字节：照 TextDecoder 的行为替换成 U+FFFD 再往下走。
                // `n >= 1` 保证每轮都有进展，循环必定终止。
                Some(Some(n)) => {
                    out.push('\u{FFFD}');
                    self.pending.drain(..valid + n);
                }
            }
        }
    }

    /// 收尾：还剩着的那几个字节永远凑不齐了，照 `TextDecoder.decode()` 的行为吐一个 U+FFFD。
    fn flush(&mut self) -> String {
        if self.pending.is_empty() {
            String::new()
        } else {
            self.pending.clear();
            "\u{FFFD}".to_string()
        }
    }
}

/// 上游字节流 → 逐条 `data:` 负载。
///
/// 行为逐条对齐 `sse.ts:48-101`：
///
/// * 按 [`frame_end`] 切帧，切一帧就把里面的负载**立刻**交出去（🔴 **不许整体缓冲**：
///   「第一个正文字必须在上游第二块之前到达」是一条被判据钉住的性质）；
/// * 见到 `[DONE]` 立刻结束，且**已收集的照常交出**；
/// * 🔴 **流尾那次 flush 不许省**：流可能不以完整空行结尾（连接中断、非 `[DONE]` 式终止、
///   代理截断都会），省掉的话**最后一个事件被静默丢弃**；
/// * 上游读失败 ⇒ 交出一个 `Err` 之后立刻结束。调用方据此发自己那一档的断流事件。
///
/// ⚠️ **取消**：`drop` 掉返回的流即可 —— `body` 被它拥有，跟着一起 `drop`，
/// 上游连接当场释放（见模块头）。
pub fn parse_sse_stream<S>(body: S) -> impl Stream<Item = Result<String, UpstreamStreamError>>
where
    S: Stream<Item = ByteChunk>,
{
    async_stream::stream! {
        let mut decoder = Utf8Incremental::default();
        let mut buf = String::new();
        let body = body;
        futures_util::pin_mut!(body);

        while let Some(chunk) = body.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };
            buf.push_str(&decoder.push(&chunk));

            while let Some((idx, len)) = frame_end(&buf) {
                let block = buf[..idx].to_string();
                buf.drain(..idx + len);
                let found = extract_payloads(&block);
                for p in found.payloads {
                    yield Ok(p);
                }
                if found.done {
                    return;
                }
            }
        }

        // 🔴 收尾那次 flush：没有它，最后一个事件被静默丢弃。
        buf.push_str(&decoder.flush());
        for p in extract_payloads(&buf).payloads {
            yield Ok(p);
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! 四条协议的流式判据共用的上游替身。
    use super::{ByteChunk, UpstreamStreamError};
    use bytes::Bytes;
    use futures_core::Stream;

    /// 把一段文本当成**一整块**发过来的上游。
    pub fn one_chunk(text: &str) -> impl Stream<Item = ByteChunk> {
        let owned = text.to_string();
        futures_util::stream::once(async move { Ok(Bytes::from(owned)) })
    }

    /// 把若干块按顺序发过来的上游。
    pub fn chunks(parts: &[&str]) -> impl Stream<Item = ByteChunk> {
        let owned: Vec<ByteChunk> = parts
            .iter()
            .map(|p| Ok(Bytes::from((*p).to_string())))
            .collect();
        futures_util::stream::iter(owned)
    }

    /// 先发若干块，然后**读失败**（上游中途断了）。
    pub fn chunks_then_error(parts: &[&str]) -> impl Stream<Item = ByteChunk> {
        let mut items: Vec<ByteChunk> = parts
            .iter()
            .map(|p| Ok(Bytes::from((*p).to_string())))
            .collect();
        items.push(Err(UpstreamStreamError));
        futures_util::stream::iter(items)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{chunks, chunks_then_error, one_chunk};
    use super::*;
    use futures_util::StreamExt;
    use serde_json::json;

    async fn collect(s: impl Stream<Item = Result<String, UpstreamStreamError>>) -> Vec<String> {
        s.filter_map(|x| async move { x.ok() }).collect().await
    }

    // ── frame_end ──────────────────────────────────────────────────────────

    /// 两种行尾都认，且长度各自不同（LF 2 字节、CRLF 4 字节）。
    #[test]
    fn both_line_endings_are_recognised_with_their_own_lengths() {
        assert_eq!(frame_end("a\n\nb"), Some((1, 2)));
        assert_eq!(frame_end("a\r\n\r\nb"), Some((1, 4)));
        assert_eq!(frame_end("no boundary here"), None);
    }

    /// 🔴 **取两者里靠前的那一个**，不是「先找 LF 找不到再找 CRLF」。
    ///
    /// 回归对象：同一段缓冲里两种行尾混着出现（换代理、断点续传）时，
    /// 「LF 优先」会把中间整段当成一帧 —— **帧边界错位**，而错位是静默的。
    #[test]
    fn the_earlier_boundary_wins_when_both_endings_are_present() {
        // CRLF 在前：必须取它。若实现是「先找 LF」，会取到下标 8 那个 LF。
        let buf = "a\r\n\r\nb\n\nc";
        assert_eq!(frame_end(buf), Some((1, 4)));
        // LF 在前：必须取 LF。
        let buf = "a\n\nb\r\n\r\nc";
        assert_eq!(frame_end(buf), Some((1, 2)));
    }

    /// ⚠️ **裸 `\r` 不认**：本仓没有见过只发裸 CR 的上游，
    /// 多认一种就是多一条没有真实样本支撑的分支。**如实登记，不假装完备。**
    #[test]
    fn a_bare_cr_is_deliberately_not_a_boundary() {
        assert_eq!(frame_end("a\r\rb"), None);
    }

    // ── extract_payloads ───────────────────────────────────────────────────

    /// 只看 `data:` 开头的行；注释行（`: ping`）与空行跳过。
    #[test]
    fn only_data_lines_are_collected() {
        let got = extract_payloads(": ping\nevent: x\ndata: {\"a\":1}\n\ndata: {\"b\":2}");
        assert_eq!(got.payloads, vec!["{\"a\":1}", "{\"b\":2}"]);
        assert!(!got.done);
    }

    /// `[DONE]` 立刻收尾，**它本身不交出去**，但它之前的照常交出。
    #[test]
    fn done_marker_ends_collection_but_keeps_what_came_before() {
        let got = extract_payloads("data: {\"a\":1}\ndata: [DONE]\ndata: {\"never\":1}");
        assert_eq!(got.payloads, vec!["{\"a\":1}"]);
        assert!(got.done);
    }

    /// ⚠️ 行内那个 `\r` 由 `trim()` 吃掉 —— 这就是 CRLF 上游下负载仍然干净的原因。
    #[test]
    fn a_trailing_cr_inside_the_line_is_trimmed_away() {
        let got = extract_payloads("data: {\"a\":1}\r");
        assert_eq!(got.payloads, vec!["{\"a\":1}"]);
    }

    /// 空负载（`data:` 后面什么都没有）跳过，不交一个空串出去。
    #[test]
    fn empty_payloads_are_skipped() {
        assert!(extract_payloads("data:\ndata:   ").payloads.is_empty());
    }

    // ── sse_event ──────────────────────────────────────────────────────────

    /// 带事件名 ⇒ 两行；不带 ⇒ 只有 data 行（Gemini 那条就是不带的）。
    #[test]
    fn event_name_is_optional_and_changes_the_wire_shape() {
        assert_eq!(
            sse_event(Some("message_start"), &json!({ "type": "x" })),
            "event: message_start\ndata: {\"type\":\"x\"}\n\n"
        );
        assert_eq!(sse_event(None, &json!({ "a": 1 })), "data: {\"a\":1}\n\n");
    }

    /// 每个事件都以**空行**收尾 —— 这正是下游 [`frame_end`] 要找的那个边界。
    /// 两个函数互为逆运算，这一格把它们接上。
    #[test]
    fn what_we_emit_is_what_our_own_parser_can_cut() {
        let wire = format!(
            "{}{}",
            sse_event(None, &json!({ "a": 1 })),
            sse_event(None, &json!({ "a": 2 }))
        );
        let got = extract_payloads(&wire[..frame_end(&wire).unwrap().0]);
        assert_eq!(got.payloads, vec!["{\"a\":1}"]);
    }

    // ── parse_sse_stream ───────────────────────────────────────────────────

    #[tokio::test]
    async fn payloads_come_out_one_by_one() {
        let got = collect(parse_sse_stream(one_chunk(
            "data: {\"a\":1}\n\ndata: {\"a\":2}\n\n",
        )))
        .await;
        assert_eq!(got, vec!["{\"a\":1}", "{\"a\":2}"]);
    }

    #[tokio::test]
    async fn done_marker_stops_the_stream_and_is_not_yielded() {
        let got = collect(parse_sse_stream(one_chunk(
            "data: {\"a\":1}\n\ndata: [DONE]\n\ndata: {\"never\":1}\n\n",
        )))
        .await;
        assert_eq!(got, vec!["{\"a\":1}"]);
    }

    /// 被切在块边界中间的事件要能拼回来。
    #[tokio::test]
    async fn an_event_split_across_chunks_is_reassembled() {
        let got = collect(parse_sse_stream(chunks(&["data: {\"a\"", ":1}\n\n"]))).await;
        assert_eq!(got, vec!["{\"a\":1}"]);
    }

    /// 🔴 **流尾那次 flush 不许省**：流不以完整空行结尾时（连接中断、非 `[DONE]` 式终止、
    /// 代理截断），没有它最后一个事件被**静默**丢弃。
    #[tokio::test]
    async fn the_last_event_survives_a_stream_that_ends_without_a_blank_line() {
        for tail in ["data: {\"a\":2}", "data: {\"a\":2}\n"] {
            let text = format!("data: {{\"a\":1}}\n\n{tail}");
            let got = collect(parse_sse_stream(one_chunk(&text))).await;
            assert_eq!(got, vec!["{\"a\":1}", "{\"a\":2}"], "尾巴={tail:?}");
        }
    }

    /// 🔴 **CRLF 上游下必须是「逐帧交出」而不是「攒到流末一次性交出」。**
    ///
    /// TS 侧这条性质要靠一道由测试控制的闸才能观察（一次性给完的上游对两种实现
    /// 逐字等价 —— 本仓登记的第 8 种假阳性）。Rust 侧同理，所以这里**故意不 close**：
    /// 上游只发第一帧，之后永远挂着。`next()` 必须在此时就给出第一条负载。
    ///
    /// **变红条件**：撤掉 [`frame_end`] 里认 `\r\n\r\n` 的那一支
    /// ⇒ 这条流会一路攒到上游关闭（而它永远不关）⇒ 这一格超时失败。
    #[tokio::test(start_paused = true)]
    async fn crlf_upstream_still_yields_before_it_closes() {
        // 先发一帧，然后永远挂着（既不再发、也不结束）—— 真实的「上游还在想」。
        let upstream = futures_util::stream::iter(vec![Ok(Bytes::from("data: {\"a\":1}\r\n\r\n"))])
            .chain(futures_util::stream::pending());
        let s = parse_sse_stream(upstream);
        futures_util::pin_mut!(s);
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), s.next()).await;
        assert_eq!(
            first.expect("上游 close 之前一条都没交出来 —— 流式退化成了一次性"),
            Some(Ok("{\"a\":1}".to_string()))
        );
    }

    /// 反向控制：同一套装置喂 LF **今天就必须绿**。
    /// 少了它，上面那一格红了分不出是「CRLF 真的坏了」还是「这套装置谁都过不去」。
    #[tokio::test(start_paused = true)]
    async fn control_group_the_same_harness_passes_with_lf_today() {
        let upstream = futures_util::stream::iter(vec![Ok(Bytes::from("data: {\"a\":1}\n\n"))])
            .chain(futures_util::stream::pending());
        let s = parse_sse_stream(upstream);
        futures_util::pin_mut!(s);
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), s.next()).await;
        assert_eq!(first.unwrap(), Some(Ok("{\"a\":1}".to_string())));
    }

    /// 上游读失败 ⇒ 交出一个 `Err` 然后结束。**调用方据此发自己那一档的断流事件。**
    #[tokio::test]
    async fn a_mid_stream_read_failure_surfaces_as_one_err_then_ends() {
        let s = parse_sse_stream(chunks_then_error(&["data: {\"a\":1}\n\n"]));
        let got: Vec<_> = s.collect().await;
        assert_eq!(
            got,
            vec![Ok("{\"a\":1}".to_string()), Err(UpstreamStreamError)]
        );
    }

    /// 🔴 错误类型**结构上装不下任何文字**，所以「回显上游 message」这件事不可能发生。
    ///
    /// 回归对象：上游异常的 message 可能带上游 URL 与栈帧，它一个字节都不许到客户端。
    #[test]
    fn the_error_type_cannot_carry_upstream_text_at_all() {
        assert_eq!(
            std::mem::size_of::<UpstreamStreamError>(),
            0,
            "它一旦能装下字符串，下一个人就会把上游的 message 塞进来"
        );
        assert_eq!(UpstreamStreamError.to_string(), UPSTREAM_STREAM_INTERRUPTED);
    }

    // ── 增量 UTF-8 解码 ────────────────────────────────────────────────────

    /// 🔴 **多字节字符被切在两个块之间必须拼得回来。**
    ///
    /// 回归对象：每块各自 `from_utf8_lossy` 的话，「中」会变成两个 `U+FFFD`，
    /// 而且**是静默的** —— 客户端看到的是乱码，网关日志里什么都没有。
    #[tokio::test]
    async fn a_multibyte_char_split_across_chunks_is_decoded_whole() {
        let text = "data: {\"t\":\"中\"}\n\n";
        let bytes = text.as_bytes();
        // 「中」是 E4 B8 AD，在它中间切一刀。
        let cut = text.find('中').unwrap() + 1;
        let upstream = futures_util::stream::iter(vec![
            Ok(Bytes::copy_from_slice(&bytes[..cut])),
            Ok(Bytes::copy_from_slice(&bytes[cut..])),
        ]);
        let got = collect(parse_sse_stream(upstream)).await;
        assert_eq!(got, vec!["{\"t\":\"中\"}"]);
    }

    /// 真的坏字节照 `TextDecoder` 的行为替换成 U+FFFD，**不 panic、不丢整条流**。
    #[test]
    fn genuinely_invalid_bytes_become_replacement_chars() {
        let mut d = Utf8Incremental::default();
        assert_eq!(d.push(&[0xFF, b'a']), "\u{FFFD}a");
    }

    /// 收尾时还剩不成字的尾巴 ⇒ 一个 U+FFFD，且解码器被清空。
    #[test]
    fn an_incomplete_tail_at_flush_time_becomes_one_replacement_char() {
        let mut d = Utf8Incremental::default();
        assert_eq!(d.push(&[0xE4, 0xB8]), "");
        assert_eq!(d.flush(), "\u{FFFD}");
        assert_eq!(d.flush(), "");
    }
}
