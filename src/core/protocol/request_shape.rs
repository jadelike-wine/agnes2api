//! 四条协议**共用**的请求体形状校验 + 🔴 **三档参数裁定表**（`request-shape.ts`，01 §6.1/§6.2）。
//!
//! # 它解决的实测缺陷（2026-09-10 新加坡 Docker 验收发现）
//!
//! TS 侧从前只有 `readJson()` 一道，而它只管 **JSON 语法**。语法过关之后请求体被
//! 无条件当成目标协议的类型直接送进转换函数（`readJson<T>` 只是**编译期**断言），于是：
//!
//! | 请求体 | 从前 | 现在 |
//! |---|---|---|
//! | `{"model":"…"}`（漏写 `messages`） | **500「网关内部错误」** | 400 说清哪个字段 |
//! | `[]` / `"hello"` / `null`（合法 JSON、非对象） | **500** | 400 |
//!
//! 🔴 **「漏写 messages」不是构造出来的畸形输入，是真实客户端很容易犯的错。**
//! 用户看到「网关内部错误」会以为网关挂了来报障，而不是自己少传了字段。
//!
//! # 为什么错误是**一个枚举**而不是三个类型
//!
//! TS 侧 `UnsupportedContentError` / `UnsupportedParamError` 都从 `InvalidRequestError`
//! 继承，于是**路由层只需要一个 catch**——少一个 catch 就少一处会漏（上一版就是只
//! catch 了内容块那一种，同一条路由上「漏写 messages」照样 500）。
//! Rust 侧的等价物是**一个枚举**：`Result<_, RequestShapeError>` 只有一种错误类型，
//! 路由层一个 `?` 就到位，**结构上不存在「漏接一种」这件事**。
//!
//! # 🔴 为什么必须 4xx 而不是 5xx
//!
//! **OpenAI 官方 SDK 对 5xx 默认自动重试（默认 2 次）**，于是一个**永远修不好**的
//! 客户端错误会被放大成 3 倍请求。上游那层 CF 约 2 次快请求就回 1015，
//! **而限流额度是整个网关共享的——一个客户端的 bug 能把所有人的通道打死。**

use serde_json::{Map, Value};
use std::fmt;

/// ③ 档的理由串：工具调用。**会逐字进 400 报文，是对外可见的东西。**
pub const WHY_NO_TOOL_CALLS: &str = "本网关不转换工具调用";
/// ② 档的理由串：上游那份请求体里根本没有这一格。同上，对外可见。
pub const WHY_NOT_IN_UPSTREAM: &str = "上游的 OpenAI 兼容请求体里没有这一格";
/// ③ 档的理由串：`candidateCount > 1`。同上，对外可见。
pub const WHY_FIRST_CANDIDATE_ONLY: &str = "本网关只转换第一条 candidate";

/// **客户端把请求体写错了**——状态码类别是 4xx，不是 5xx。三种成员一律 400。
///
/// 三档参数裁定表（`request-shape.ts:116-145`，三条协议**共用这一张**）。
/// 判据是「**这一格在 OpenAI chat 请求体里有没有同名同义的位置**」（上游是 OpenAI 兼容体）：
///
/// | 档 | 处置 | 成员 |
/// |---|---|---|
/// | **① 透传** | 映射成本一行 | `temperature`；`top_p`（Gemini 的 `topP`）；`stop`（Anthropic 的 `stop_sequences`、Gemini 的 `stopSequences`） |
/// | **② 400** | 上游那份请求体里没有这一格 | Anthropic 的 `top_k`；Gemini 的 `generationConfig.topK` |
/// | **③ 400** | 传得过去，但响应会变成本网关转换不了的形状 | `tools` / `tool_choice` / `toolConfig`；Gemini 的 `candidateCount > 1` |
///
/// ⚠️ **① 档不做取值范围校验**：实测 `temperature: 99` / `top_p: 5` 上游照样 200。
/// **上游自己不判，网关这层替它判就是替上游说了一句它没说的话。**
///
/// ⚠️ **`candidateCount: 1` 放行**：它与不传逐字等价，为它报 400 只是在骂人。
///
/// ⚠️ **这张表是「今天做得到什么」，不是「客户端不该要什么」**：哪天真的实现了工具调用的
/// 双向转换，`tools` 就该从 ③ 挪到 ①，**而不是**在这里悄悄放行。
///
/// 🔴 **为什么必须 400 而不是静默丢掉**（2026-09-10 实测）：从前三条协议把
/// `tools` / `tool_choice` / `top_k` 一个字都不往下带，而同一台网关的
/// `/v1/chat/completions` 是整包透传。实测最难看的一幕：同一份带 `tools` +
/// `tool_choice:"required"` 的请求，走 OpenAI 那条上游返回**真的工具调用**，
/// 走 `/v1/messages` 返回 **200**、正文是模型自己吐的 `<tool_call><tool_call>…` 乱码、
/// `stop_reason` 还写着 `end_turn` ——**网关一句提示都没有**。
/// ⇒ **做不到就明说，别假装做到了。**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestShapeError {
    /// 基类那一档：形状不对（不是对象 / 缺字段 / 字段类型不对）。
    Invalid(String),
    /// 请求里出现了本网关无法映射到 OpenAI chat 格式的**内容块**。
    ///
    /// 内部规范格式只有纯文本，`image` / `inlineData` / `input_image` / `tool_use`
    /// 这些块无法无损转换。
    ///
    /// 🔴 **它从来就不是 Anthropic 专有的**：实测（2026-09-10）同一张 8×8 的 PNG 走
    /// `/v1beta/models/…:generateContent` 与 `/v1/responses` 都是 **HTTP 200 +
    /// 一段模型没看见图编出来的答案**（gemini 那条真打回来的正文逐字是
    /// `NO_IMAGE_RECEIVED`），而走 `/v1/messages` 是 400。
    /// **同一台网关对同一件事给三种行为**，而那两条给的正是被判定为「不可接受」的那一种。
    UnsupportedContent {
        /// 报文里点名的块类型。Gemini 的 part 没有 `type` 格，取的是「除 `text` 之外的第一个键」。
        block_type: String,
    },
    /// 请求里出现了本网关**无法如实转达给上游**的生成参数（② 档与 ③ 档共用这一个）。
    UnsupportedParam {
        /// 逐字点名是哪一格，客户端拿到 400 一眼就知道该删哪个字段。
        field: String,
        /// 为什么转达不了。取值见本模块三个 `WHY_*` 常量。
        why: String,
    },
}

impl fmt::Display for RequestShapeError {
    /// 🔴 这三句报文是**对外契约**（五语言 API.md 逐字写着），改它就是改对外可见的东西。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(msg) => f.write_str(msg),
            Self::UnsupportedContent { block_type } => {
                write!(f, "不支持的内容块类型: {block_type}（本网关仅支持 text）")
            }
            Self::UnsupportedParam { field, why } => {
                write!(f, "不支持的生成参数: {field}（{why}）")
            }
        }
    }
}

impl std::error::Error for RequestShapeError {}

/// 本模块统一的 `Result`。
pub type ShapeResult<T> = Result<T, RequestShapeError>;

/// 请求体必须是一个 **JSON 对象**。
///
/// `null` 与数组都要挡：TS 侧 `typeof null === "object"`，两者都能通过朴素判断然后在
/// 下一行属性访问上抛 `TypeError` ⇒ 500。Rust 侧不会抛，但**语义必须一致**：
/// `[]` 与 `null` 是 400，不是「当成空对象继续跑」。
pub fn require_object<'a>(req: &'a Value, what: &str) -> ShapeResult<&'a Map<String, Value>> {
    req.as_object()
        .ok_or_else(|| RequestShapeError::Invalid(format!("{what}必须是一个 JSON 对象")))
}

/// 某个字段必须是**非空数组**。
///
/// ⚠️ **空数组也拒**：`messages: []` 送到上游只会被上游以另一种措辞拒掉，
/// **白烧一次共享的限流额度**——本地能判的就别送出去。
///
/// 入参是 `Option<&Value>`（即 `o.get("messages")`），对齐 TS 的 `o.messages` 可能是
/// `undefined` 这一档：缺席与类型不对走同一句报文，因为对客户端来说是同一件事。
pub fn require_array<'a>(v: Option<&'a Value>, field: &str) -> ShapeResult<&'a Vec<Value>> {
    match v.and_then(Value::as_array) {
        Some(a) if !a.is_empty() => Ok(a),
        _ => Err(RequestShapeError::Invalid(format!(
            "{field} 必须是一个非空数组"
        ))),
    }
}

/// 某个字段必须是**非空字符串**。
pub fn require_string<'a>(v: Option<&'a Value>, field: &str) -> ShapeResult<&'a str> {
    match v.and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err(RequestShapeError::Invalid(format!(
            "{field} 必须是一个非空字符串"
        ))),
    }
}

/// ②/③ 档：这一格**出现了就 400**。
///
/// ⚠️ **「出现」按 `contains_key` 判，不按「值不是 null」判**——对齐 TS 的
/// `o.tools !== undefined`：`{"tools": null}` 在 TS 里是 `null !== undefined` ⇒ 报错。
/// 换成「null 视同缺席」会让 `{"tools": null}` 这种（SDK 真的会发的）请求悄悄走到
/// ③ 档想拦住的那条路上去。
///
/// * `key` —— 请求体里的键名；
/// * `field` —— 报文里点名的那个名字（Gemini 要写成 `generationConfig.topK` 这种带路径的）。
pub fn reject_param(
    o: &Map<String, Value>,
    key: &str,
    field: &str,
    why: &'static str,
) -> ShapeResult<()> {
    if o.contains_key(key) {
        return Err(RequestShapeError::UnsupportedParam {
            field: field.to_string(),
            why: why.to_string(),
        });
    }
    Ok(())
}

// ── ① 档：可选的透传参数 ──────────────────────────────────────────────────────
//
// 🔴 **这里是一处对 TS 行为的刻意偏离，理由与代价都写在这。**
//
// TS 侧这三格是**原样搬运**（`temperature: req.temperature`），类型对不对完全不看，
// `JSON.stringify` 之后由上游去判。Rust 侧的 `InternalRequest` 是强类型的，
// 于是只剩两条路：
//
// * **静默丢掉**类型不对的值 —— 与本模块存在的全部理由（`UnsupportedContentError` /
//   `UnsupportedParamError` 都是为了消灭「静默丢掉」）正面冲突；
// * **400 点名** —— 采用这条。
//
// ⚠️ **这不是「取值范围校验」**（那一条明确禁止：`temperature: 99` 必须放行）。
// 这里只判「是不是一个数字」，`99` / `-1` / `1e9` 一律放行。
//
// ⚠️ **显式 `null` 视同缺席**：Anthropic / OpenAI 的官方 SDK 对没设的可选参数是**省略**
// 而不是发 `null`，发 `null` 的一律是手搓请求体的客户端，其意图就是「不设」。
// **这与上面 `reject_param` 的相反处置不矛盾**：那边 `null` 出现意味着客户端**打算**用
// 一个我们转不了的能力，这边 `null` 出现意味着客户端**打算不用**这一格。

/// ① 档：可选的浮点参数（`temperature` / `top_p`）。
pub fn opt_f64(o: &Map<String, Value>, key: &str) -> ShapeResult<Option<f64>> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .map(Some)
            .ok_or_else(|| RequestShapeError::Invalid(format!("{key} 必须是一个数字"))),
    }
}

/// ① 档：可选的非负整数参数（`max_tokens` / `max_output_tokens` / `maxOutputTokens`）。
///
/// `field` 是报文里点名的那个名字（三条协议各叫各的，报文要说客户端自己写的那个词）。
pub fn opt_u32(o: &Map<String, Value>, key: &str, field: &str) -> ShapeResult<Option<u32>> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| RequestShapeError::Invalid(format!("{field} 必须是一个非负整数"))),
    }
}

/// ① 档：可选的字符串数组（`stop_sequences` / `stopSequences`）。
///
/// ⚠️ **空数组放行**（不走 `require_array`）：`stop_sequences: []` 与不传等价，
/// 它不是「客户端写错了」，拦它属于替上游说话。
pub fn opt_string_array(
    o: &Map<String, Value>,
    key: &str,
    field: &str,
) -> ShapeResult<Option<Vec<String>>> {
    let Some(v) = o.get(key) else { return Ok(None) };
    if v.is_null() {
        return Ok(None);
    }
    let bad = || RequestShapeError::Invalid(format!("{field} 必须是一个字符串数组"));
    let arr = v.as_array().ok_or_else(bad)?;
    arr.iter()
        .map(|e| e.as_str().map(str::to_string).ok_or_else(bad))
        .collect::<ShapeResult<Vec<String>>>()
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── 三句对外报文 ────────────────────────────────────────────────────────

    /// 🔴 三句报文是**对外契约**（五语言 API.md 逐字写着），逐字钉住。
    ///
    /// 回归对象：有人顺手「改得通顺一点」，而下游正靠这句话里的字段名定位该删哪一格。
    #[test]
    fn the_three_messages_are_verbatim_contract() {
        assert_eq!(
            RequestShapeError::Invalid("model 必须是一个非空字符串".into()).to_string(),
            "model 必须是一个非空字符串"
        );
        assert_eq!(
            RequestShapeError::UnsupportedContent {
                block_type: "image".into()
            }
            .to_string(),
            "不支持的内容块类型: image（本网关仅支持 text）"
        );
        assert_eq!(
            RequestShapeError::UnsupportedParam {
                field: "tools".into(),
                why: WHY_NO_TOOL_CALLS.into()
            }
            .to_string(),
            "不支持的生成参数: tools（本网关不转换工具调用）"
        );
    }

    /// 三个 `WHY_*` 是对外可见的字面量。
    #[test]
    fn why_strings_are_stable_literals() {
        assert_eq!(WHY_NO_TOOL_CALLS, "本网关不转换工具调用");
        assert_eq!(WHY_NOT_IN_UPSTREAM, "上游的 OpenAI 兼容请求体里没有这一格");
        assert_eq!(WHY_FIRST_CANDIDATE_ONLY, "本网关只转换第一条 candidate");
    }

    // ── require_object ─────────────────────────────────────────────────────

    /// 🔴 `null` 与数组都必须挡住。
    ///
    /// 回归对象：TS 侧 `typeof null === "object"` 让这两种滑过朴素判断，
    /// 然后在下一行属性访问上抛裸 `TypeError` ⇒ 被兜成 **500「网关内部错误」**，
    /// 用户以为网关挂了来报障。
    #[test]
    fn null_and_array_are_not_objects() {
        for v in [
            json!(null),
            json!([]),
            json!([1, 2]),
            json!("hello"),
            json!(3),
        ] {
            let err = require_object(&v, "请求体").unwrap_err();
            assert_eq!(
                err,
                RequestShapeError::Invalid("请求体必须是一个 JSON 对象".into()),
                "{v} 不该被当成对象"
            );
        }
    }

    /// 空对象**是**合法对象 —— 缺字段由后面那两道守卫说话，不在这一道混着说。
    #[test]
    fn empty_object_passes_this_guard() {
        assert!(require_object(&json!({}), "请求体").is_ok());
    }

    // ── require_array ──────────────────────────────────────────────────────

    /// ⚠️ **空数组也拒**：`messages: []` 送上游只会被上游以另一种措辞拒掉，
    /// 白烧一次**整个网关共享**的限流额度。
    #[test]
    fn empty_array_is_rejected_so_we_never_burn_shared_quota_on_it() {
        let o = json!({ "messages": [] });
        let o = o.as_object().unwrap();
        assert_eq!(
            require_array(o.get("messages"), "messages").unwrap_err(),
            RequestShapeError::Invalid("messages 必须是一个非空数组".into())
        );
    }

    /// 缺席与类型不对走**同一句**报文 —— 对客户端来说是同一件事。
    #[test]
    fn missing_and_wrong_type_share_one_message() {
        let o = json!({ "messages": "hi" });
        let o = o.as_object().unwrap();
        let missing = require_array(None, "messages").unwrap_err();
        let wrong = require_array(o.get("messages"), "messages").unwrap_err();
        assert_eq!(missing, wrong);
    }

    // ── require_string ─────────────────────────────────────────────────────

    /// 空串拒、非字符串拒、非空字符串过。
    #[test]
    fn require_string_rejects_empty_and_non_strings() {
        let o = json!({ "a": "", "b": 1, "c": "x" });
        let o = o.as_object().unwrap();
        assert!(require_string(o.get("a"), "model").is_err());
        assert!(require_string(o.get("b"), "model").is_err());
        assert_eq!(require_string(o.get("c"), "model").unwrap(), "x");
        assert!(require_string(None, "model").is_err());
    }

    // ── reject_param ───────────────────────────────────────────────────────

    /// 🔴 **`{"tools": null}` 照样 400。**
    ///
    /// 回归对象：把这里改成「null 视同缺席」，`{"tools": null}` 就会悄悄走到
    /// ③ 档想拦住的那条路上 —— 而那条路的终点是「200 + 一段 `<tool_call>` 乱码」。
    #[test]
    fn an_explicit_null_still_counts_as_present() {
        let o = json!({ "tools": null });
        let o = o.as_object().unwrap();
        assert_eq!(
            reject_param(o, "tools", "tools", WHY_NO_TOOL_CALLS).unwrap_err(),
            RequestShapeError::UnsupportedParam {
                field: "tools".into(),
                why: WHY_NO_TOOL_CALLS.into()
            }
        );
    }

    /// 报文点的是 `field` 而不是 `key` —— Gemini 要说 `generationConfig.topK`，
    /// 因为客户端请求体里它就是嵌在那儿的。
    #[test]
    fn the_reported_field_can_differ_from_the_key() {
        let o = json!({ "topK": 40 });
        let o = o.as_object().unwrap();
        let err =
            reject_param(o, "topK", "generationConfig.topK", WHY_NOT_IN_UPSTREAM).unwrap_err();
        assert_eq!(
            err.to_string(),
            "不支持的生成参数: generationConfig.topK（上游的 OpenAI 兼容请求体里没有这一格）"
        );
    }

    /// 不出现就放行。
    #[test]
    fn absent_param_passes() {
        let o = json!({});
        assert!(reject_param(o.as_object().unwrap(), "tools", "tools", WHY_NO_TOOL_CALLS).is_ok());
    }

    // ── ① 档三个读取器 ─────────────────────────────────────────────────────

    /// ⚠️ **不做取值范围校验**：`99` / `-1` / `5` 一律放行。
    ///
    /// 回归对象：实测 `temperature: 99` / `top_p: 5` 上游照样 200。
    /// **上游自己不判，网关这层替它判就是替上游说了一句它没说的话。**
    #[test]
    fn out_of_range_numbers_pass_through_untouched() {
        let o = json!({ "temperature": 99, "top_p": 5, "t2": -1.5 });
        let o = o.as_object().unwrap();
        assert_eq!(opt_f64(o, "temperature").unwrap(), Some(99.0));
        assert_eq!(opt_f64(o, "top_p").unwrap(), Some(5.0));
        assert_eq!(opt_f64(o, "t2").unwrap(), Some(-1.5));
    }

    /// 类型不对**点名 400**，不静默丢掉。
    ///
    /// 这是本模块与 TS 侧唯一的行为差异（TS 原样搬运给上游），
    /// 两条路里选了「说出来」那条 —— 与整个模块的存在理由一致。
    #[test]
    fn wrong_typed_passthrough_params_are_named_not_dropped() {
        let o = json!({ "temperature": "hot", "max_tokens": -1, "stop_sequences": [1] });
        let o = o.as_object().unwrap();
        assert_eq!(
            opt_f64(o, "temperature").unwrap_err().to_string(),
            "temperature 必须是一个数字"
        );
        assert_eq!(
            opt_u32(o, "max_tokens", "max_tokens")
                .unwrap_err()
                .to_string(),
            "max_tokens 必须是一个非负整数"
        );
        assert_eq!(
            opt_string_array(o, "stop_sequences", "stop_sequences")
                .unwrap_err()
                .to_string(),
            "stop_sequences 必须是一个字符串数组"
        );
    }

    /// 显式 `null` 与缺席同义（三个读取器一致）。
    #[test]
    fn explicit_null_is_the_same_as_absent_for_passthrough_params() {
        let o = json!({ "temperature": null, "max_tokens": null, "stop_sequences": null });
        let o = o.as_object().unwrap();
        assert_eq!(opt_f64(o, "temperature").unwrap(), None);
        assert_eq!(opt_u32(o, "max_tokens", "max_tokens").unwrap(), None);
        assert_eq!(
            opt_string_array(o, "stop_sequences", "stop_sequences").unwrap(),
            None
        );
    }

    /// ⚠️ **空的 `stop_sequences` 放行**：它与不传等价，拦它属于替上游说话。
    #[test]
    fn an_empty_stop_list_is_allowed_unlike_an_empty_messages_list() {
        let o = json!({ "stop_sequences": [] });
        let o = o.as_object().unwrap();
        assert_eq!(
            opt_string_array(o, "stop_sequences", "stop_sequences").unwrap(),
            Some(vec![])
        );
    }

    /// `max_tokens` 越出 `u32` 时点名，不悄悄截断成一个别的数。
    #[test]
    fn oversized_max_tokens_is_named_not_truncated() {
        let o = json!({ "max_tokens": 5_000_000_000_u64 });
        let o = o.as_object().unwrap();
        assert!(opt_u32(o, "max_tokens", "max_tokens").is_err());
    }
}
