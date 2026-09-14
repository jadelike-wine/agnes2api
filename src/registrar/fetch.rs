//! 注册机所有出站请求的**唯一出口**。
//!
//! 移植自 TS 版 `src/core/registrar/fetch.ts`（48 行），规则真源
//! `docs/porting/01-core-rules.md §10.10`。
//!
//! # 🔴 `fetch` 有两种失败，它们此前的待遇完全不同
//!
//! - **发得出去、上游回非 2xx** ⇒ 调用方自己拼诊断，地址是脱敏的；
//! - **压根没发出去**（URL 带 userinfo、DNS 失败、TLS 错误、超时中止）⇒
//!   **运行时抛的那个错误直接穿出去，它的消息里可能带着完整的原始 URL**，
//!   一路进事件板块与容器 stdout。
//!
//! ⇒ **脱敏必须落在「发请求这件事」上，而不是落在「非 2xx」那一支上。**
//!
//! # 🔴 为什么做成一个出口，而不是在每个调用点各写一次
//!
//! 后者的失败形态是**「下一个人新加的那一处忘了包」**，
//! **而那种遗漏不会有任何东西变红。**

use super::url::{redact_in_message, redact_url};
use crate::ports::fetcher::{Fetcher, OutboundRequest};
use futures_util::StreamExt;

/// 一次出站的结果。**状态码与正文都完整交出去。**
///
/// ⚠️ **正文在这里不截断、不脱敏**：那是调用点的职责
///（**它才知道这一步有哪些凭据在场**）。**本函数只负责把证据完整交出去。**
#[derive(Debug, Clone)]
pub struct ChannelReply {
    pub status: u16,
    pub body: String,
}

/// 读正文时的上界。
///
/// 🔴 **不是为了省内存，是为了挡住「上游回了一个几百 MB 的维护页」那一档**：
/// 注册链上的每一份正文都会进诊断，而诊断会进事件环、进容器 stdout。
/// 这个上界与 `domain_ledger::BODY_SNIPPET_MAX`（512）**不是一回事**：
/// 那个管的是「进事件的那一小段」，这个管的是「我们愿意读进内存多少」。
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// 发一次注册机的出站请求。
///
/// ⚠️ **`method` 从请求里取，不另开一个参数**：另开一个就多了一处
/// **能与真正发出去的方法对不上**的地方。
pub async fn fetch_channel(
    fetcher: &dyn Fetcher,
    req: OutboundRequest,
    channel: &str,
    action: &str,
) -> Result<ChannelReply, String> {
    let method = req.method.clone();
    let raw_url = req.url.clone();

    let res = match fetcher.send(req).await {
        Ok(r) => r,
        Err(e) => {
            // 🔴 **这一支就是脱敏的落点。** 运行时的错误消息可能整段带着原始 URL。
            return Err(format!(
                "{channel} {action}失败: {} ({method} {})",
                redact_in_message(&e.to_string(), &raw_url),
                redact_url(&raw_url),
            ));
        }
    };

    let status = res.status;
    let mut body = Vec::new();
    let mut stream = res.body;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(b) => {
                if body.len() + b.len() > MAX_BODY_BYTES {
                    let room = MAX_BODY_BYTES.saturating_sub(body.len());
                    body.extend_from_slice(&b[..room]);
                    break;
                }
                body.extend_from_slice(&b);
            }
            Err(e) => {
                // **正文读了一半断了**也走同一条脱敏出口 —— 与「压根没发出去」
                // 用同一套措辞，因为调用方对这两者的处置是一样的：没拿到一份可判的正文。
                return Err(format!(
                    "{channel} {action}失败: {} ({method} {})",
                    redact_in_message(&e.to_string(), &raw_url),
                    redact_url(&raw_url),
                ));
            }
        }
    }

    Ok(ChannelReply {
        status,
        // ⚠️ 非 UTF-8 用 lossy 而不是报错：一份乱码正文**仍然是证据**
        //（「上游返回的是二进制/压缩内容」本身就是一条诊断结论），
        // 而在这里报错会把它变成「请求失败」，真因就丢了。
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::fetcher::{BodyStream, FetchError, OutboundResponse};
    use async_trait::async_trait;
    use bytes::Bytes;
    use std::sync::Mutex;

    fn stream_of(chunks: Vec<Result<Bytes, FetchError>>) -> BodyStream {
        Box::pin(futures_util::stream::iter(chunks))
    }

    struct Fake {
        reply: Mutex<Option<Result<OutboundResponse, FetchError>>>,
        seen: Mutex<Option<(String, String)>>,
    }
    #[async_trait]
    impl Fetcher for Fake {
        async fn send(&self, req: OutboundRequest) -> Result<OutboundResponse, FetchError> {
            *self.seen.lock().unwrap() = Some((req.method.clone(), req.url.clone()));
            self.reply.lock().unwrap().take().unwrap()
        }
    }

    fn req(url: &str) -> OutboundRequest {
        OutboundRequest {
            method: "POST".into(),
            url: url.into(),
            headers: vec![],
            body: None,
            timeout_ms: 15_000,
        }
    }

    async fn run(
        url: &str,
        reply: Result<OutboundResponse, FetchError>,
    ) -> (Result<ChannelReply, String>, Option<(String, String)>) {
        let f = Fake {
            reply: Mutex::new(Some(reply)),
            seen: Mutex::new(None),
        };
        let out = fetch_channel(&f, req(url), "yyds", "取域名列表").await;
        let seen = f.seen.lock().unwrap().clone();
        (out, seen)
    }

    fn ok_body(status: u16, body: &str) -> Result<OutboundResponse, FetchError> {
        Ok(OutboundResponse {
            status,
            headers: vec![],
            body: stream_of(vec![Ok(Bytes::from(body.to_string()))]),
        })
    }

    #[tokio::test]
    async fn a_successful_call_hands_the_status_and_the_whole_body_back() {
        let (out, _) = run("https://h.test/v1", ok_body(404, r#"{"e":"nope"}"#)).await;
        let r = out.expect("非 2xx 不是错误 —— 调用方要拿它去归因");
        assert_eq!(r.status, 404);
        assert_eq!(
            r.body, r#"{"e":"nope"}"#,
            "🔴 正文在这里不截断不脱敏 —— 那是调用点的职责，它才知道这一步有哪些凭据在场",
        );
    }

    /// 🔴 **这一格钉的是那条「脱敏落在发请求这件事上」。**
    ///
    /// 变异对象：把 `Err` 那一支的 `redact_in_message` 换成原样 `e.to_string()` ——
    /// 运行时错误消息里的完整原始 URL（含口令）会一路进事件板块与容器 stdout。
    #[tokio::test]
    async fn a_request_that_was_never_sent_never_leaks_the_raw_url() {
        let url = "https://user:topsecret@h.test/v1";
        let (out, _) = run(
            url,
            Err(FetchError::Connect(format!(
                "Request cannot be constructed from a URL that includes credentials: {url}"
            ))),
        )
        .await;
        let e = out.expect_err("连都没连上却报成了成功");
        assert!(!e.contains("topsecret"), "🔴 口令漏进了诊断：{e}");
        assert!(e.contains("***@"), "抹掉了却没留可见标记：{e}");
        assert!(e.contains("yyds 取域名列表失败"), "形态不对：{e}");
    }

    /// 正文读了一半断了 ⇒ 走同一条脱敏出口。
    #[tokio::test]
    async fn a_body_that_breaks_midway_takes_the_same_redacted_exit() {
        let url = "https://u:pw@h.test/v1";
        let res = Ok(OutboundResponse {
            status: 200,
            headers: vec![],
            body: stream_of(vec![
                Ok(Bytes::from_static(b"{\"partial\":")),
                Err(FetchError::BodyInterrupted(format!(
                    "reset while reading {url}"
                ))),
            ]),
        });
        let (out, _) = run(url, res).await;
        let e = out.expect_err("正文断了却报成了成功");
        assert!(!e.contains("pw@"), "🔴 口令漏进了诊断：{e}");
    }

    /// ⚠️ **`method` 从请求里取** —— 另开一个参数就多了一处能对不上的地方。
    #[tokio::test]
    async fn the_method_in_the_diagnostic_is_the_one_that_was_actually_sent() {
        let (out, seen) = run("https://h.test/v1", Err(FetchError::TimeoutBeforeHeaders)).await;
        let e = out.unwrap_err();
        assert_eq!(seen.unwrap().0, "POST");
        assert!(e.contains("(POST "), "诊断里报了一个它没发过的方法：{e}");
    }

    /// 查询串在诊断里也要脱敏。
    #[tokio::test]
    async fn a_query_string_is_redacted_in_the_diagnostic_too() {
        let (out, _) = run(
            "https://h.test/v1?token=secret",
            Err(FetchError::Connect("boom".into())),
        )
        .await;
        let e = out.unwrap_err();
        assert!(!e.contains("secret"), "🔴 查询串里的凭据漏了：{e}");
        assert!(e.contains("?<redacted>"), "{e}");
    }

    /// 🔴 正文有上界 —— 上游回一个几百 MB 的维护页时不许把它整份读进内存。
    #[tokio::test]
    async fn the_body_is_bounded() {
        let big = "x".repeat(MAX_BODY_BYTES + 5000);
        let (out, _) = run("https://h.test/v1", ok_body(200, &big)).await;
        assert_eq!(out.unwrap().body.len(), MAX_BODY_BYTES);
    }

    /// ⚠️ 非 UTF-8 正文**仍然是证据**，不许因为它报「请求失败」。
    #[tokio::test]
    async fn a_non_utf8_body_is_still_handed_back_as_evidence() {
        let res = Ok(OutboundResponse {
            status: 502,
            headers: vec![],
            body: stream_of(vec![Ok(Bytes::from_static(&[0xff, 0xfe, 0x00]))]),
        });
        let (out, _) = run("https://h.test/v1", res).await;
        let r = out.expect("乱码正文被当成了「请求失败」—— 真因就丢了");
        assert_eq!(r.status, 502);
        assert!(!r.body.is_empty());
    }
}
