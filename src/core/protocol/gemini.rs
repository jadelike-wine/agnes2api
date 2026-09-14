//! Gemini `/v1beta/models/{model}:{method}`（`protocol/gemini.ts`，01 §6.6）。
//!
//! ⚠️ **model 从路径来，不在请求体里** —— 所以本模块的转换函数收一个 `model: &str`。

use super::openai::{MODELS, Modality};
use super::request_shape::{
    RequestShapeError, ShapeResult, WHY_FIRST_CANDIDATE_ONLY, WHY_NO_TOOL_CALLS,
    WHY_NOT_IN_UPSTREAM, opt_f64, opt_string_array, opt_u32, reject_param, require_array,
    require_object,
};
use super::sse::{ByteChunk, parse_sse_stream, sse_event};
use super::{InternalRequest, Message};
use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::{Map, Value, json};

/// 上游 `finish_reason` → Gemini `finishReason`。
#[must_use]
pub fn finish_reason(finish: &str) -> &'static str {
    match finish {
        "length" => "MAX_TOKENS",
        "content_filter" => "SAFETY",
        _ => "STOP",
    }
}

/// 本网关在 Gemini 这条协议上实现的两个方法。**白名单，不是黑名单。**
pub const GENERATE_CONTENT: &str = "generateContent";
/// 同上。
pub const STREAM_GENERATE_CONTENT: &str = "streamGenerateContent";

/// 路径里那一段 `{model}:{method}` 切不开或方法名不认识。
///
/// ⚠️ **两档的状态码不同，这是有意的**（见 [`split_model_and_method`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeminiPathError {
    /// 路径里根本没有冒号 ⇒ **400**：路径本身写坏了。
    MissingMethod,
    /// 方法名不在白名单里 ⇒ **404**：请求体没错，是这条端点本网关没实现。
    UnknownMethod(String),
}

impl std::fmt::Display for GeminiPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingMethod => f.write_str("路径缺少方法名"),
            Self::UnknownMethod(m) => write!(
                f,
                "本网关只实现 {GENERATE_CONTENT} 与 {STREAM_GENERATE_CONTENT} 两个方法，不认识 {m}"
            ),
        }
    }
}

impl std::error::Error for GeminiPathError {}

/// 把 `agnes-2.0-flash:generateContent` 切成 `(model, is_stream)`。
///
/// ⚠️ **按最后一个冒号切，不是第一个**：模型名本身可能含冒号。
///
/// 🔴 **方法名白名单**（2026-09-10 实测缺陷）。从前这里一个字都不校验，于是
/// `:countTokens` / `:embedContent` / 拼错的 `:generatecontent` **统统落进
/// generateContent 那个 handler，真打一次上游对话**。实测：带合法 `contents` 打
/// `:countTokens` 返回 **200**、正文是一段真回答、`candidatesTokenCount: 43`
/// ——一次「数一下 token」被当成完整对话烧掉了。
///
/// 代价链：① 白烧池中一把 key 的一次上游调用；② 吃掉上游前面那层 CF 的
/// **全网关共享**限流额度（约 2 次快请求就回 1015）；③ 客户端拿回的是 `candidates`
/// 结构、解析 `totalTokens` 拿到空 ——「计数功能坏了」与「所有人的通道莫名被限流」
/// 在面板上看不出因果。而 `google-genai` 的 `client.models.count_tokens()`、
/// 以及一切在发正式请求前先探一次 token 数的框架，走的正是这条路。
///
/// ⚠️ **未知方法是 404 而不是 400**：请求体没有错，是这条端点本网关没实现
/// ——真 Gemini 给的也是 NOT_FOUND。
pub fn split_model_and_method(rest: &str) -> Result<(&str, bool), GeminiPathError> {
    let idx = rest.rfind(':').ok_or(GeminiPathError::MissingMethod)?;
    let (model, method) = (&rest[..idx], &rest[idx + 1..]);
    match method {
        GENERATE_CONTENT => Ok((model, false)),
        STREAM_GENERATE_CONTENT => Ok((model, true)),
        other => Err(GeminiPathError::UnknownMethod(other.to_string())),
    }
}

/// 把一组 `parts` 压平成纯文本。
///
/// ⚠️ **Gemini 的 `Part` 是按键区分的联合体**（没有 `type` 判别符）：带正文的那一支是
/// `{ text }`，其余是 `{ inlineData }` / `{ fileData }` / `{ functionCall }` / …
///
/// 🔴 **非文本 part 一律 400，不再求值成空串**（2026-09-10 实测缺陷）：从前是
/// `parts.map(p => p.text ?? "").join("")`，`inlineData` / `fileData` / `functionCall`
/// **统统蒸发**。线上真打过——带一张 8×8 红色 PNG 的 `inline_data` 打 `:generateContent`
/// 回 **200**，正文逐字是 `NO_IMAGE_RECEIVED`（提示词里让模型在没收到图时这么回）。
///
/// ⚠️ **块类型取的是「除 `text` 之外的第一个键」**：part 没有 `type` 格，报文里要说得出
/// 是哪一种块，只能报键名。一个键都没有的空对象报 `"unknown"` —— 那也确实不是本网关
/// 认识的任何一种。
///
/// ⚠️ **「第一个」在 Rust 侧是字母序，在 TS 侧是插入序**（`serde_json` 的 `Map` 默认是
/// `BTreeMap`）。差异只影响**一个块同时带两个非 text 键**时报文里点哪一个 ——
/// 两种都是实话，而为此开 `preserve_order` feature 会改变**全仓所有** JSON 的字节序，
/// 代价远大于收益。如实登记在这里。
fn parts_text(parts: Option<&Value>, what: &str) -> ShapeResult<String> {
    let Some(Value::Array(parts)) = parts else {
        return Err(RequestShapeError::Invalid(format!("{what} 必须是一个数组")));
    };
    let mut out = String::new();
    for p in parts {
        if let Some(t) = p.get("text").and_then(Value::as_str) {
            out.push_str(t);
            continue;
        }
        let block_type = p
            .as_object()
            .and_then(|o| o.keys().find(|k| k.as_str() != "text"))
            .map_or("unknown", String::as_str)
            .to_string();
        return Err(RequestShapeError::UnsupportedContent { block_type });
    }
    Ok(out)
}

/// Gemini 请求体 → [`InternalRequest`]（`gemini.ts:55-88`）。
///
/// * `model` 来自**路径**；
/// * `stream` 来自**方法名**（`:streamGenerateContent`），同样不在请求体里
///   —— 所以它是显式入参，见 [`InternalRequest`] 的说明。
pub fn to_internal_request(req: &Value, model: &str, stream: bool) -> ShapeResult<InternalRequest> {
    let o = require_object(req, "请求体")?;
    let contents = require_array(o.get("contents"), "contents")?;

    // 三档参数裁定表（三条协议共用那一张）。
    reject_param(o, "tools", "tools", WHY_NO_TOOL_CALLS)?;
    reject_param(o, "toolConfig", "toolConfig", WHY_NO_TOOL_CALLS)?;

    // `generationConfig` 缺席时用一张空表，让下面四格的取值逻辑只有一条路。
    let empty = Map::new();
    let cfg = match o.get("generationConfig") {
        None | Some(Value::Null) => &empty,
        Some(v) => require_object(v, "generationConfig ")?,
    };
    reject_param(cfg, "topK", "generationConfig.topK", WHY_NOT_IN_UPSTREAM)?;
    // ⚠️ **`candidateCount: 1` 放行**：它与不传逐字等价，为它报 400 只是在骂人。
    // `> 1` 才是转换不了的那一档（本网关只转第一条 candidate ⇒
    // **客户端付了 n 份的钱只拿得到一份**）。
    if let Some(v) = cfg.get("candidateCount")
        && v.as_f64() != Some(1.0)
    {
        return Err(RequestShapeError::UnsupportedParam {
            field: "generationConfig.candidateCount".into(),
            why: WHY_FIRST_CANDIDATE_ONLY.into(),
        });
    }

    let mut messages = Vec::with_capacity(contents.len() + 1);
    if let Some(si) = o.get("systemInstruction")
        && !si.is_null()
    {
        messages.push(Message {
            role: "system".into(),
            content: parts_text(si.get("parts"), "systemInstruction.parts")?,
        });
    }
    for (i, c) in contents.iter().enumerate() {
        let co = require_object(c, &format!("contents[{i}] "))?;
        // `model` 是 Gemini 对「上一轮是助手说的」的叫法；缺席一律 `user`。
        let role = match co.get("role").and_then(Value::as_str) {
            Some("model") => "assistant",
            Some(r) => r,
            None => "user",
        };
        messages.push(Message {
            role: role.to_string(),
            content: parts_text(co.get("parts"), &format!("contents[{i}].parts"))?,
        });
    }

    Ok(InternalRequest {
        model: model.to_string(),
        messages,
        max_tokens: opt_u32(cfg, "maxOutputTokens", "generationConfig.maxOutputTokens")?,
        stream,
        temperature: opt_f64(cfg, "temperature")?,
        top_p: opt_f64(cfg, "topP")?,
        stop: opt_string_array(cfg, "stopSequences", "generationConfig.stopSequences")?,
    })
}

/// 上游 OpenAI 响应 → Gemini 非流式响应（`gemini.ts:90-107`）。
#[must_use]
pub fn to_gemini_response(openai: &Value, model: &str) -> Value {
    let choice = openai.pointer("/choices/0");
    let text = choice
        .and_then(|c| c.pointer("/message/content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let finish = choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(Value::as_str)
        .unwrap_or("stop");
    let prompt = usage_field(openai, "prompt_tokens");
    let completion = usage_field(openai, "completion_tokens");
    json!({
        "candidates": [{
            "content": { "role": "model", "parts": [{ "text": text }] },
            "finishReason": finish_reason(finish),
            "index": 0,
        }],
        "modelVersion": model,
        "usageMetadata": usage_metadata(prompt, completion),
    })
}

fn usage_field(openai: &Value, key: &str) -> u64 {
    openai
        .pointer("/usage")
        .and_then(|u| u.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn usage_metadata(prompt: u64, completion: u64) -> Value {
    json!({
        "promptTokenCount": prompt,
        "candidatesTokenCount": completion,
        // ⚠️ `totalTokenCount` 在本仓是**算出来的**，不是从上游读的：
        // 上游那份 `usage.total_tokens` 与这两格未必自洽，而这里的口径必须自洽。
        "totalTokenCount": prompt + completion,
    })
}

/// 上游 SSE 流 → Gemini 事件序列（`gemini.ts:109-195`）：**每个非空 delta 一帧，末尾一个终帧**。
///
/// 🔴 **这条流从前没有终帧：`finishReason` 与 `usageMetadata` 一次都不出现**（实测缺陷）。
/// 官方 `google-genai` 2.22.0 回放线上真吐的那串字节（零上游请求）：`text` 完整，
/// 而 `last.candidates[0].finish_reason` 与 `last.usage_metadata` **都是 `None`**。
///
/// ⚠️ **`finishReason` 是客户端判断「说完了 / 撞了 MAX_TOKENS / 被 SAFETY 拦了」的唯一信号**
/// ——少了它，**一个被截断的半截回答和一个完整回答在客户端看来逐字节不可区分**。
///
/// ⚠️ **「上游没给」这条辩护是堵死的**：同一网关 `/v1/chat/completions` 原样透传的流式末尾
/// 真实字节里含 `"finish_reason":"length"` 与 `"usage"` ——**上游在流里明确给了，
/// 是这条协议自己丢的。**
///
/// ⚠️ **Gemini 的帧不带事件名**（纯 `data:`），与另两条不同。
pub fn to_gemini_stream<S>(upstream: S, model: String) -> impl Stream<Item = String>
where
    S: Stream<Item = ByteChunk>,
{
    async_stream::stream! {
        let mut finish = "stop".to_string();
        let mut in_tok: u64 = 0;
        let mut out_tok: u64 = 0;

        let payloads = parse_sse_stream(upstream);
        futures_util::pin_mut!(payloads);
        while let Some(item) = payloads.next().await {
            let raw = match item {
                Ok(raw) => raw,
                Err(_) => {
                    // 上游中途断掉：发一帧 `finishReason: "OTHER"` 的终帧，
                    // 别让断流退化成一次静默的正常收尾。
                    //
                    // ⚠️ **刻意不发 `promptFeedback.blockReason`**：`promptFeedback` 说的是
                    // 「**这条提示词**被拦了」，那是一句关于**请求**的断言，而这里发生的是
                    // **响应**中途断了。用它等于替上游说一句它没说过的话。
                    // `OTHER` 是 Gemini 现行取值里「因为别的原因停下了」那一档，
                    // **不多说也不少说**。
                    //
                    // ⚠️ **这一帧不带 `usageMetadata`**：断流时那两个数是残缺的，
                    // 报出去等于给下游一个看起来能用的假账。
                    yield sse_event(None, &json!({
                        "candidates": [{
                            "content": { "role": "model", "parts": [] },
                            "finishReason": "OTHER",
                            "index": 0,
                        }],
                        "modelVersion": model,
                    }));
                    return;
                }
            };
            let Ok(chunk) = serde_json::from_str::<Value>(&raw) else { continue };
            if let Some(f) = chunk.pointer("/choices/0/finish_reason").and_then(Value::as_str) {
                finish = f.to_string();
            }
            if let Some(n) = chunk.pointer("/usage/prompt_tokens").and_then(Value::as_u64) {
                in_tok = n;
            }
            if let Some(n) = chunk.pointer("/usage/completion_tokens").and_then(Value::as_u64) {
                out_tok = n;
            }
            let text = chunk.pointer("/choices/0/delta/content").and_then(Value::as_str);
            if let Some(text) = text && !text.is_empty() {
                yield sse_event(None, &json!({
                    "candidates": [{
                        "content": { "role": "model", "parts": [{ "text": text }] },
                        "index": 0,
                    }],
                    "modelVersion": model,
                }));
            }
        }

        // 🔴 终帧。没有它，截断与完整逐字节不可区分。
        yield sse_event(None, &json!({
            "candidates": [{
                "content": { "role": "model", "parts": [] },
                "finishReason": finish_reason(&finish),
                "index": 0,
            }],
            "modelVersion": model,
            // 上游没给 usage 时仍是 0，那一档是「上游真没给」，
            // 与从前「拿得到也不给」不是一回事。
            "usageMetadata": usage_metadata(in_tok, out_tok),
        }));
    }
}

/// `GET /v1beta/models`（`gemini.ts:197-226`）。
///
/// ⚠️ 🔴 **`supportedGenerationMethods` 按形态给**（2026-09-10 实测缺陷）：从前三个图片模型
/// 与三个视频模型也被声明成支持那两条方法，而它们真正的端点是
/// `POST /v1/images/generations` 与 `POST /v1/videos` 的两段式。
/// 它是 Gemini 协议里**机器可读**的「这个模型能干什么」，照着它渲染下拉框的客户端会把
/// 视频模型列成可对话模型；运维选中发一次对话 ⇒
/// **白烧一把 key + 一次全网关共享的限流额度**。
///
/// ⚠️ **媒体模型仍然列出来，只是方法为空数组，不是从清单里删掉**：
/// `/v1/models` 与 `/v1beta/models` 交的是同一份模型全集，做模型发现的客户端只认其中
/// 一条 —— 少列一个模型对那个客户端就是「这个模型不存在」。**空数组说的正是实话。**
#[must_use]
pub fn model_list() -> Value {
    json!({
        "models": MODELS.iter().map(|m| json!({
            "name": format!("models/{}", m.id),
            "displayName": m.id,
            "supportedGenerationMethods": match m.modality {
                Modality::Chat => vec![GENERATE_CONTENT, STREAM_GENERATE_CONTENT],
                Modality::Image | Modality::Video => vec![],
            },
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::sse::test_support::{chunks_then_error, one_chunk};
    use super::*;

    fn internal(req: Value) -> InternalRequest {
        to_internal_request(&req, "agnes-2.0-flash", false).expect("应当转换成功")
    }

    async fn frames(s: impl Stream<Item = String>) -> Vec<Value> {
        let wire: String = s.collect::<Vec<_>>().await.join("");
        wire.split("\n\n")
            .filter(|b| !b.trim().is_empty())
            .map(|b| {
                let data = b.strip_prefix("data: ").expect("Gemini 的帧不带事件名");
                serde_json::from_str(data).expect("data 必须是合法 JSON")
            })
            .collect()
    }

    // ── 路径切分与方法名白名单 ─────────────────────────────────────────────

    /// ⚠️ **按最后一个冒号切** —— 模型名本身可能含冒号。
    #[test]
    fn the_split_happens_at_the_last_colon_not_the_first() {
        assert_eq!(
            split_model_and_method("weird:name:generateContent").unwrap(),
            ("weird:name", false)
        );
        assert_eq!(
            split_model_and_method("m:streamGenerateContent").unwrap(),
            ("m", true)
        );
    }

    /// 🔴 **白名单之外的方法名一律挡住。**
    ///
    /// 回归对象（实测）：不校验时 `:countTokens` 落进 generateContent handler
    /// **真打一次上游对话** —— 返回 200 + 一段真回答 + `candidatesTokenCount: 43`。
    /// 一次「数一下 token」被当成完整对话烧掉，同时吃掉全网关共享的限流额度。
    /// 而 `google-genai` 的 `client.models.count_tokens()` 走的正是这条路。
    #[test]
    fn anything_outside_the_two_method_whitelist_is_rejected() {
        for bad in [
            "m:countTokens",
            "m:embedContent",
            "m:generatecontent", // 大小写拼错
            "m:",
            "m:generateContentX",
        ] {
            assert_eq!(
                split_model_and_method(bad).unwrap_err(),
                GeminiPathError::UnknownMethod(bad.rsplit(':').next().unwrap().to_string()),
                "{bad}"
            );
        }
    }

    /// ⚠️ **两档错误分得开**：没有冒号是「路径写坏了」（400），
    /// 方法名不认识是「本网关没实现」（404，真 Gemini 也给 NOT_FOUND）。
    /// 合成一档就会把「端点不存在」说成「你的请求错了」。
    #[test]
    fn a_missing_colon_and_an_unknown_method_are_two_different_failures() {
        let missing = split_model_and_method("no-colon-here").unwrap_err();
        let unknown = split_model_and_method("m:countTokens").unwrap_err();
        assert_eq!(missing, GeminiPathError::MissingMethod);
        assert_eq!(missing.to_string(), "路径缺少方法名");
        assert_eq!(
            unknown.to_string(),
            "本网关只实现 generateContent 与 streamGenerateContent 两个方法，不认识 countTokens"
        );
        // 两句话必须不同：合成一档就会把「端点不存在」说成「你的请求错了」，
        // 而客户端会照后者去改自己的请求体 —— 改不出来。
        assert_ne!(missing.to_string(), unknown.to_string());
    }

    // ── 请求整形 ───────────────────────────────────────────────────────────

    /// `contents` 压平成 messages；`model` 角色映射成 `assistant`；缺席的角色是 `user`。
    #[test]
    fn contents_are_flattened_and_the_model_role_becomes_assistant() {
        let r = internal(json!({
            "contents": [
                { "role": "user", "parts": [{ "text": "a" }, { "text": "b" }] },
                { "role": "model", "parts": [{ "text": "c" }] },
                { "parts": [{ "text": "d" }] },
            ],
        }));
        assert_eq!(
            r.messages,
            vec![
                Message {
                    role: "user".into(),
                    content: "ab".into()
                },
                Message {
                    role: "assistant".into(),
                    content: "c".into()
                },
                Message {
                    role: "user".into(),
                    content: "d".into()
                },
            ]
        );
    }

    /// `systemInstruction` 转成首条 system 消息。
    #[test]
    fn system_instruction_becomes_the_first_system_message() {
        let r = internal(json!({
            "systemInstruction": { "parts": [{ "text": "be nice" }] },
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
        }));
        assert_eq!(r.messages[0].role, "system");
        assert_eq!(r.messages[0].content, "be nice");
    }

    /// `model` 来自路径、`stream` 来自方法名 —— 两样都**不从请求体里读**。
    #[test]
    fn model_and_stream_come_from_the_path_not_the_body() {
        let body = json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            // 请求体里这两格就算写了也不算数。
            "model": "别人写的", "stream": false,
        });
        let r = to_internal_request(&body, "agnes-3.0-flash", true).unwrap();
        assert_eq!(r.model, "agnes-3.0-flash");
        assert!(r.stream);
    }

    /// `generationConfig` 四格的映射（缺席时整块可以没有）。
    #[test]
    fn generation_config_maps_onto_the_upstream_names() {
        let r = internal(json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "generationConfig": {
                "maxOutputTokens": 128, "temperature": 0.2,
                "topP": 0.7, "stopSequences": ["</s>"],
            },
        }));
        assert_eq!(r.max_tokens, Some(128));
        assert_eq!(r.temperature, Some(0.2));
        assert_eq!(r.top_p, Some(0.7));
        assert_eq!(r.stop.as_deref(), Some(&["</s>".to_string()][..]));

        let bare = internal(json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
        }));
        assert_eq!(bare.max_tokens, None);
        assert_eq!(bare.top_p, None);
    }

    /// 🔴 ②/③ 档一律 400 点名；⚠️ **`candidateCount: 1` 必须放行**。
    #[test]
    fn undeliverable_params_are_named_but_candidate_count_one_passes() {
        let base = json!({ "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }] });

        let mut t = base.clone();
        t["tools"] = json!([]);
        assert_eq!(
            to_internal_request(&t, "m", false).unwrap_err(),
            RequestShapeError::UnsupportedParam {
                field: "tools".into(),
                why: WHY_NO_TOOL_CALLS.into()
            }
        );

        let mut tc = base.clone();
        tc["toolConfig"] = json!({});
        assert!(to_internal_request(&tc, "m", false).is_err());

        let mut k = base.clone();
        k["generationConfig"] = json!({ "topK": 40 });
        assert_eq!(
            to_internal_request(&k, "m", false).unwrap_err().to_string(),
            "不支持的生成参数: generationConfig.topK（上游的 OpenAI 兼容请求体里没有这一格）"
        );

        // ⚠️ 1 与不传逐字等价 —— 为它报 400 只是在骂人。
        for ok in [json!(1), json!(1.0)] {
            let mut c1 = base.clone();
            c1["generationConfig"] = json!({ "candidateCount": ok });
            assert!(to_internal_request(&c1, "m", false).is_ok(), "{ok}");
        }
        // >1 是转换不了的那一档：本网关只转第一条 candidate ⇒
        // 客户端付了 n 份的钱只拿得到一份。
        for bad in [json!(2), json!(0), json!(null), json!("1")] {
            let mut c2 = base.clone();
            c2["generationConfig"] = json!({ "candidateCount": bad });
            assert_eq!(
                to_internal_request(&c2, "m", false).unwrap_err(),
                RequestShapeError::UnsupportedParam {
                    field: "generationConfig.candidateCount".into(),
                    why: WHY_FIRST_CANDIDATE_ONLY.into()
                },
                "{bad}"
            );
        }
    }

    /// 🔴 **非文本 part 一律 400，绝不静默蒸发。**
    ///
    /// 回归对象（线上真打过）：带一张 8×8 红色 PNG 的 `inline_data` 打 `:generateContent`
    /// 回 **200**，正文逐字是 `NO_IMAGE_RECEIVED` —— 模型**完全没看到图**，
    /// 而客户端既无从察觉也无从排查。
    #[test]
    fn a_non_text_part_is_named_in_a_400_instead_of_evaporating() {
        for (part, expected) in [
            (json!({ "inlineData": { "data": "..." } }), "inlineData"),
            (json!({ "fileData": {} }), "fileData"),
            (json!({ "functionCall": {} }), "functionCall"),
            // ⚠️ 一个键都没有的空对象报 `unknown` —— 它确实不是本网关认识的任何一种。
            (json!({}), "unknown"),
        ] {
            let body = json!({ "contents": [{ "role": "user", "parts": [part] }] });
            assert_eq!(
                to_internal_request(&body, "m", false).unwrap_err(),
                RequestShapeError::UnsupportedContent {
                    block_type: expected.into()
                }
            );
        }
        // systemInstruction 那一侧同样拦。
        let body = json!({
            "systemInstruction": { "parts": [{ "inlineData": {} }] },
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
        });
        assert!(to_internal_request(&body, "m", false).is_err());
    }

    /// ⚠️ **块类型取「除 `text` 之外的第一个键」** —— part 没有 `type` 格，
    /// 报文里要说得出是哪一种块，只能报键名。
    #[test]
    fn the_reported_block_type_is_the_first_key_other_than_text() {
        let body = json!({
            "contents": [{ "parts": [{ "text": null, "inlineData": {} }] }],
        });
        assert_eq!(
            to_internal_request(&body, "m", false).unwrap_err(),
            RequestShapeError::UnsupportedContent {
                block_type: "inlineData".into()
            }
        );
    }

    /// 形状不对一律 400，不 panic。
    #[test]
    fn structurally_broken_bodies_are_400_never_a_panic() {
        for bad in [
            json!(null),
            json!([]),
            json!({}),
            json!({ "contents": [] }),
            json!({ "contents": ["not an object"] }),
            json!({ "contents": [{ "role": "user" }] }), // 漏 parts
            json!({ "contents": [{ "role": "user", "parts": "hi" }] }),
            json!({ "contents": [{ "parts": [] }], "generationConfig": 7 }),
        ] {
            assert!(to_internal_request(&bad, "m", false).is_err(), "{bad}");
        }
    }

    // ── 非流式响应 ─────────────────────────────────────────────────────────

    #[test]
    fn openai_response_is_rebuilt_into_gemini_candidates() {
        let v = to_gemini_response(
            &json!({
                "choices": [{ "message": { "content": "hello" }, "finish_reason": "length" }],
                "usage": { "prompt_tokens": 3, "completion_tokens": 4 },
            }),
            "agnes-2.0-flash",
        );
        assert_eq!(v["candidates"][0]["content"]["role"], "model");
        assert_eq!(v["candidates"][0]["content"]["parts"][0]["text"], "hello");
        assert_eq!(v["candidates"][0]["finishReason"], "MAX_TOKENS");
        assert_eq!(v["candidates"][0]["index"], 0);
        assert_eq!(v["modelVersion"], "agnes-2.0-flash");
        assert_eq!(
            v["usageMetadata"],
            json!({ "promptTokenCount": 3, "candidatesTokenCount": 4, "totalTokenCount": 7 })
        );
    }

    /// `finishReason` 全表；没见过的值走 `STOP`。
    #[test]
    fn the_finish_reason_table_is_complete_and_has_a_safe_default() {
        assert_eq!(finish_reason("stop"), "STOP");
        assert_eq!(finish_reason("length"), "MAX_TOKENS");
        assert_eq!(finish_reason("content_filter"), "SAFETY");
        assert_eq!(finish_reason("tool_calls"), "STOP");
    }

    // ── 模型清单 ───────────────────────────────────────────────────────────

    /// 🔴 **只有对话模型声明支持那两个方法；媒体模型给空数组，但仍然列出来。**
    ///
    /// 回归对象：① 把视频模型声明成可对话 ⇒ 照它渲染下拉框的客户端会列出来，
    /// 运维选中发一次对话 = 白烧一把 key + 一次全网关共享的限流额度；
    /// ② 把媒体模型从清单里删掉 ⇒ 做模型发现的客户端会认为「这个模型不存在」。
    #[test]
    fn only_chat_models_advertise_the_two_methods_and_media_models_stay_listed() {
        let v = model_list();
        let models = v["models"].as_array().unwrap();
        assert_eq!(models.len(), 12, "12 这个数是清单完整性的锚");
        assert_eq!(models[0]["name"], "models/agnes-2.0-flash");
        assert_eq!(models[0]["displayName"], "agnes-2.0-flash");
        assert_eq!(
            models[0]["supportedGenerationMethods"],
            json!(["generateContent", "streamGenerateContent"])
        );
        for m in &models[6..] {
            assert_eq!(
                m["supportedGenerationMethods"],
                json!([]),
                "{} 不是对话模型，它一个方法都不该声明",
                m["name"]
            );
        }
    }

    /// 清单里声明的那两个方法名，必须**就是**路径切分认的那两个。
    /// 否则「清单说能用、真打回 404」，而两边各写一份字面量时这件事迟早发生。
    #[test]
    fn the_advertised_methods_are_exactly_the_ones_the_router_accepts() {
        let v = model_list();
        for m in v["models"][0]["supportedGenerationMethods"]
            .as_array()
            .unwrap()
        {
            let path = format!("agnes-2.0-flash:{}", m.as_str().unwrap());
            assert!(split_model_and_method(&path).is_ok(), "{path}");
        }
    }

    // ── 流式 ───────────────────────────────────────────────────────────────

    /// 每个非空 delta 一帧，**末尾一个终帧**。
    #[tokio::test]
    async fn every_delta_becomes_a_frame_and_the_stream_ends_with_a_final_one() {
        let up = one_chunk(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n",
            "data: [DONE]\n\n",
        ));
        let got = frames(to_gemini_stream(up, "m".into())).await;
        assert_eq!(got.len(), 3);
        assert_eq!(got[0]["candidates"][0]["content"]["parts"][0]["text"], "he");
        assert_eq!(
            got[1]["candidates"][0]["content"]["parts"][0]["text"],
            "llo"
        );
        // 增量帧**不带** finishReason —— 带了就等于每一帧都在宣告结束。
        assert_eq!(got[0]["candidates"][0]["finishReason"], json!(null));
        assert_eq!(got[2]["candidates"][0]["finishReason"], "STOP");
    }

    /// 🔴 **终帧必须同时带 `finishReason` 与 `usageMetadata`。**
    ///
    /// 回归对象（实测，官方 `google-genai` 2.22.0 回放线上真吐的字节，零上游请求）：
    /// 从前循环结束后**什么都不补**，`last.candidates[0].finish_reason` 与
    /// `last.usage_metadata` **都是 `None`** ⇒ **一个被截断的半截回答和一个完整回答
    /// 在客户端看来逐字节不可区分。**
    ///
    /// 「上游没给」这条辩护是堵死的：同一网关 `/v1/chat/completions` 透传的流式末尾
    /// 真实字节里含 `finish_reason` 与 `usage`。
    #[tokio::test]
    async fn the_final_frame_carries_both_finish_reason_and_usage_metadata() {
        let up = one_chunk(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}],",
            "\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":6}}\n\n",
        ));
        let got = frames(to_gemini_stream(up, "m".into())).await;
        let last = got.last().unwrap();
        assert_eq!(last["candidates"][0]["finishReason"], "MAX_TOKENS");
        assert_eq!(last["candidates"][0]["content"]["parts"], json!([]));
        assert_eq!(
            last["usageMetadata"],
            json!({ "promptTokenCount": 5, "candidatesTokenCount": 6, "totalTokenCount": 11 })
        );
        assert_eq!(last["modelVersion"], "m");
    }

    /// 🔴 **断流：终帧的 `finishReason` 是 `OTHER`，且不报 token 数。**
    ///
    /// ⚠️ 刻意不发 `promptFeedback.blockReason`：那是一句关于**请求**的断言，
    /// 而这里发生的是**响应**中途断了 —— 用它等于替上游说一句它没说过的话。
    #[tokio::test]
    async fn a_broken_upstream_ends_with_other_and_no_token_counts() {
        let up = chunks_then_error(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"half\"}}],\"usage\":{\"prompt_tokens\":9}}\n\n",
        ]);
        let got = frames(to_gemini_stream(up, "m".into())).await;
        let last = got.last().unwrap();
        assert_eq!(last["candidates"][0]["finishReason"], "OTHER");
        assert_eq!(
            last["usageMetadata"],
            json!(null),
            "断流时那两个数是残缺的，报出去等于给下游一个看起来能用的假账"
        );
        assert_eq!(
            last.get("promptFeedback"),
            None,
            "promptFeedback 说的是请求被拦了，与响应断流不是一回事"
        );
        assert_eq!(got.len(), 2, "断流帧之后不许再补一个正常终帧");
    }

    /// 上游一个增量都没有时，仍然只有一个终帧（结构完整、正文为空）。
    #[tokio::test]
    async fn an_upstream_with_no_deltas_still_produces_the_final_frame() {
        let got = frames(to_gemini_stream(one_chunk("data: [DONE]\n\n"), "m".into())).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["candidates"][0]["finishReason"], "STOP");
    }
}
