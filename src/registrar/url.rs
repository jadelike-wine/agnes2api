//! 注册机出站请求的**地址脱敏**与四个错误消息模板。
//!
//! 移植自 TS 版 `src/core/registrar/url.ts`（285 行），规则真源
//! `docs/porting/01-core-rules.md §10.10`。
//!
//! # 🔴 要防的**不是面板**
//!
//! `registrar.yyds.baseUrl` 标的是 `public`，`GET /admin/api/config` 今天就把它明文返给
//! **已鉴权的**管理员；事件板块同样在鉴权之后。
//! ⚠️ **「事件板块没鉴权」是假的，别把它写成理由。** 真正的理由有两条：
//!
//! - `GET /admin/api/events/download` 的**设计用途就是被导出** —— 粘进 issue、贴进聊天窗口。
//!   **导出的那一刻它就离开了鉴权边界。**
//! - 事件同时进容器 stdout，**那是另一个信任域**。
//!
//! # 脱敏的射程 —— 盲区在这里登记，文档里不许说成「已脱敏」
//!
//! 只挡三处：**userinfo（用户名 / 口令）、查询串、片段**。
//! 🔴 **凭据写在路径段里挡不住** —— 形如 `https://host/t/<token>/v1` 的地址会**原样**进日志。
//! **这是已知缺口，不是遗漏**：路径段里哪一段是凭据没有可靠判据，
//! 而「不要用关键词启发式去兜底」那条禁令逐字管着这种场合。

use url::Url;

/// 解析不开时的替代文本。
///
/// 🔴 **刻意不回落成原样输出**：解析不开的串照样可能含凭据 —— env 侧的
/// `YYDS_BASE_URL` / `MOEMAIL_BASE_URL` **只过一道非空检查，一道 URL 校验都不走**，
/// 运维粘错半截带口令的串是完全可能的。
/// 而**「它压根不是一个 URL」本身就是一条完整的诊断结论**。
pub const UNPARSEABLE_URL: &str = "<不是一个可解析的 URL>";

/// 回查发现输出里仍有凭据成分时，整段丢弃用的替代文本。
pub const UNSAFE_MESSAGE: &str = "<原始错误消息里仍有凭据成分，已整段丢弃>";

/// 解析并要求它**长得像一个我们真会去打的地址**。
///
/// # 🔴 一处跨语言移植的坑，实测抓到的
///
/// TS 那一侧「解析不开 ⇒ 整段替换」这条防线，建立在 **JS `new URL()` 的严格度**上。
/// Rust 的 `url` crate **宽松得多**：`user:pass@half-a-url` 在这里**解析成功** ——
/// 它把 `user:` 当成 scheme、`pass@half-a-url` 当成路径。
/// ⇒ 照抄 TS 的写法，这个串会**原封不动**穿过 [`redact_url`]，那个 `pass` 直接进日志。
///
/// ⚠️ **这不是「Rust 的 url crate 有问题」，是同一条规则在两个解析器下射程不同。**
/// 补法是把「解析得开」收紧成「解析得开**且**有 host **且** scheme 是 http/https」——
/// 后两条正是「我们真会拿它去发请求」的定义，没有 host 的串本来就打不出去。
fn parse_fetchable(raw: &str) -> Option<Url> {
    let u = Url::parse(raw).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    // `has_host()` 对 `https:///path` 这种也会是假。
    if !u.has_host() || u.host_str().is_some_and(str::is_empty) {
        return None;
    }
    Some(u)
}

/// 把一个地址脱敏成可以进日志的形态。**幂等**：对自己的输出再跑一次结果不变。
///
/// - userinfo 任一非空 ⇒ 两者清空，主机名前留 `***@` 这个**可见标记**。
///   ⚠️ **留标记而不是静默删掉**：运维如果真把凭据塞进了 baseUrl，**他需要知道我们抹了它**。
/// - 查询串非空 ⇒ 整段换成 `?<redacted>`（**不做逐参数白名单**）。
///   ⚠️ **留占位而不是直接删**：`…/domains?x=1` 与 `…/domains` 在日志里必须读得出区别。
/// - 片段非空 ⇒ 换成 `#<redacted>`。
///   ⚠️ **这条不是洁癖**：baseUrl 带 `#` 时，适配器里 `{baseUrl}/v1/domains` 追加的那段路径
///   **整个掉进 fragment**，实际打的是原来的 pathname —— 这个占位符是那条失败形态**唯一的线索**。
///
/// 解析不开（或不像个能打的 HTTP 地址，见 [`parse_fetchable`]）⇒ [`UNPARSEABLE_URL`]。
pub fn redact_url(raw: &str) -> String {
    let Some(mut u) = parse_fetchable(raw) else {
        return UNPARSEABLE_URL.to_string();
    };
    let had_userinfo = !u.username().is_empty() || u.password().is_some_and(|p| !p.is_empty());
    if had_userinfo {
        // 先清空，标记在下面按字符串拼 —— `url` crate 不允许把用户名设成 `***`
        // 之后还保证不被转义，直接拼更可控。
        let _ = u.set_username("");
        let _ = u.set_password(None);
    }
    let had_query = u.query().is_some_and(|q| !q.is_empty());
    if had_query {
        u.set_query(Some("<redacted>"));
    }
    let had_fragment = u.fragment().is_some_and(|f| !f.is_empty());
    if had_fragment {
        u.set_fragment(Some("<redacted>"));
    }
    let mut s = u.to_string();
    if had_query {
        // `set_query` 会把 `<` `>` 百分号转义，而这是给人看的占位符，不是要被解析的查询串。
        s = s.replace("?%3Credacted%3E", "?<redacted>");
    }
    if had_fragment {
        s = s.replace("#%3Credacted%3E", "#<redacted>");
    }
    if had_userinfo {
        // 在 scheme 之后、host 之前插入可见标记。
        if let Some(idx) = s.find("//") {
            s.insert_str(idx + 2, "***@");
        }
    }
    s
}

/// 一个地址里**属于凭据的那几段**。回查时用它判断输出干不干净。
///
/// `search[1..]` / `hash[1..]` 是去掉前导 `?` / `#` 之后的内容 ——
/// 那两个符号本身不是秘密，带上它们会让回查对「消息里恰好有个问号」误判。
fn url_secrets(raw: &str) -> Option<Vec<String>> {
    // 🔴 **必须与 [`redact_url`] 用同一道闸**。用两道不同的闸时会出现最坏的组合：
    // 回查认为「解析得开、凭据是这几段」，而 `redact_url` 认为「解析不开」并整段替换 ——
    // 于是 `msg.replace(raw, …)` 把原串换成了 `UNPARSEABLE_URL`，回查却拿着
    // 一份**从未出现在输出里**的凭据清单去查，查不到 ⇒ 放行。看起来两头都尽职了。
    let u = parse_fetchable(raw)?;
    let mut out = Vec::new();
    if !u.username().is_empty() {
        out.push(u.username().to_string());
    }
    if let Some(p) = u.password()
        && !p.is_empty()
    {
        out.push(p.to_string());
    }
    if let Some(q) = u.query()
        && !q.is_empty()
    {
        out.push(q.to_string());
    }
    if let Some(f) = u.fragment()
        && !f.is_empty()
    {
        out.push(f.to_string());
    }
    Some(out)
}

/// 把一条**运行时抛出来的**错误消息里的地址换成脱敏形态。
///
/// # 🔴 它守的洞是实测出来的，不是推断
///
/// baseUrl 带 userinfo 时，Node 的 undici 在**构造 Request 的那一步**就抛 `TypeError`，
/// 而它的 message 里带着**完整的原始 URL** —— TS 那一侧本机 Node v24.15.0 实测逐字：
/// `Request cannot be constructed from a URL that includes credentials: https://user:pass@…`
/// ⇒ **口令原样进事件板块、进容器 stdout**；适配器里 `if !ok` 那一支根本没跑到，
/// 脱敏被整个绕过。
///
/// ⚠️ Rust 这一侧 `reqwest` 的报错形态与 undici 不同，但**这个函数的立论与运行时无关**：
/// 只要「请求压根没发出去」这一支的错误消息是运行时给的，它就可能带着原始输入。
///
/// # 🔴 它凭什么敢说自己挡住了：**靠后置条件，不靠「把认识的形态替换掉」**
///
/// 替换完之后**再回头查一遍**，只要还有任何一段凭据留在输出里，**整段丢掉**。
/// ⇒ **「我们没想到的形态」的后果是少说一句话，不是多漏一把口令。**
/// 反过来写（只做替换、不做回查）时，一个被运行时重新编码过的 URL 就能整串漏出去，
/// **而门禁全绿**。
///
/// ⚠️ **另一半代价如实写着**：口令恰好是一个在别处也会自然出现的短串（`1`、`a=1`）时，
/// 回查会命中一句**本来无害**的消息并把它整段丢掉。
/// **这是刻意选的方向 —— 假阳性只让人少看到一句话，假阴性会把口令贴进 issue。**
///
/// ⚠️ **替换所有出现处而不是只换第一处**：undici 那句话里 URL 出现一次，
/// 别的运行时可能出现两次（「构造 X 失败」+「原始输入 X」）。
pub fn redact_in_message(msg: &str, raw_url: &str) -> String {
    // 解析不开 ⇒ **无条件整段丢弃**。连哪几段是凭据都判不出来，就没有任何依据说它干净。
    let Some(secrets) = url_secrets(raw_url) else {
        return UNSAFE_MESSAGE.to_string();
    };
    let out = msg.replace(raw_url, &redact_url(raw_url));
    if secrets.iter().any(|s| out.contains(s.as_str())) {
        return UNSAFE_MESSAGE.to_string();
    }
    out
}

/// 四个模板共用的形态：`<通道> <动作>失败: <原因> (<方法> <脱敏地址>)`。
///
/// ⚠️ **`method` 是参数而不是写死的 `GET`**：`create_mailbox` 打的是 POST，
/// **写死会让日志报一个它没发过的方法 —— 那是新的一句假话。**
fn compose(channel: &str, action: &str, reason: &str, method: &str, raw_url: &str) -> String {
    format!(
        "{channel} {action}失败: {reason} ({method} {})",
        redact_url(raw_url)
    )
}

/// 发得出去、上游回非 2xx。
///
/// 🔴 **它与 [`http_fail`] 必须是同一个工厂**：「消息里写着 404、属性上挂着 403」
/// 是这类改动最容易长出来的假话，**而它只会在两条路各产一半时出现**。
pub fn http_fail_message(
    channel: &str,
    action: &str,
    status: u16,
    method: &str,
    raw_url: &str,
) -> String {
    compose(channel, action, &format!("HTTP {status}"), method, raw_url)
}

/// 同上，但**另把状态码原样挂出来**给调用方分档用。
///
/// ⚠️⚠️ **绝不许用正则从消息里抠 `HTTP (\d+)`**：那是一张手写启发式，
/// 而消息模板是给人看的、会被翻译会被改写 —— **它一改，抠数字的那一头静默地开始答错，
/// 门禁全绿。** 状态码走这条结构化的路，消息走消息的路，两者同源。
pub fn http_fail(
    channel: &str,
    action: &str,
    status: u16,
    method: &str,
    raw_url: &str,
) -> (String, u16) {
    (
        http_fail_message(channel, action, status, method, raw_url),
        status,
    )
}

/// **请求压根没发出去**（URL 带 userinfo、DNS 失败、TLS 错误、超时中止）。
///
/// 🔴 **没有状态码，也不许伪造一个。** `0` / `502` 那种兜底值会让调用方把
/// **「没连上」读成「上游回了话」**。**没有就是没有。**
pub fn transport_fail_message(
    channel: &str,
    action: &str,
    cause: &str,
    method: &str,
    raw_url: &str,
) -> String {
    compose(
        channel,
        action,
        &redact_in_message(cause, raw_url),
        method,
        raw_url,
    )
}

/// **上游答了 2xx、正文却读不出来。**
///
/// 🔴 **刻意不挂 `status: 200`**：上游确实回了 2xx，但挂上去会让消费方把它读成
/// 「上游回了话、这是它的裁决」，**而这一档真正要说的是正文读不出来**。
pub fn body_fail_message(
    channel: &str,
    action: &str,
    cause: &str,
    method: &str,
    raw_url: &str,
) -> String {
    compose(
        channel,
        action,
        &format!("响应正文不是 JSON: {}", redact_in_message(cause, raw_url)),
        method,
        raw_url,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn userinfo_is_wiped_but_leaves_a_visible_marker() {
        let out = redact_url("https://user:pass@example.com/v1/domains");
        assert!(!out.contains("user"), "用户名还在：{out}");
        assert!(!out.contains("pass"), "口令还在：{out}");
        assert!(
            out.contains("***@"),
            "抹掉了却没留标记 —— 运维需要知道我们抹了它：{out}",
        );
        assert!(
            out.contains("example.com/v1/domains"),
            "把地址本身也弄丢了：{out}"
        );
    }

    /// ⚠️ 占位而不是删掉：`…/domains?x=1` 与 `…/domains` 在日志里必须读得出区别。
    #[test]
    fn a_query_string_becomes_a_placeholder_not_nothing() {
        let out = redact_url("https://example.com/v1/domains?token=secret&x=1");
        assert!(!out.contains("secret"), "查询串里的凭据还在：{out}");
        assert_eq!(out, "https://example.com/v1/domains?<redacted>");
    }

    /// ⚠️ 片段那条不是洁癖：baseUrl 带 `#` 时，追加的路径整个掉进 fragment，
    /// 实际打的是原来的 pathname —— 这个占位符是那条失败形态**唯一的线索**。
    #[test]
    fn a_fragment_becomes_a_placeholder() {
        let out = redact_url("https://example.com/v1#frag");
        assert_eq!(out, "https://example.com/v1#<redacted>");
    }

    /// 🔴 幂等：对自己的输出再跑一次结果不变。
    #[test]
    fn redacting_twice_changes_nothing() {
        for raw in [
            "https://user:pass@example.com/a?b=c#d",
            "https://example.com/a",
            "not a url at all",
        ] {
            let once = redact_url(raw);
            assert_eq!(redact_url(&once), once, "不幂等：{raw} → {once}");
        }
    }

    /// 🔴 解析不开**不回落原串** —— 解析不开的串照样可能含凭据。
    ///
    /// ⚠️ **这一格抓到过一个真缺陷，别把它读成「例行检查」。**
    /// 照抄 TS 的写法（`Url::parse(raw).ok()?`）时它是红的：Rust 的 `url` crate
    /// 比 JS 的 `new URL()` **宽松得多**，`user:pass@half-a-url` 在这里解析**成功**
    ///（scheme = `user`，path = `pass@half-a-url`）⇒ 整个串原封不动穿过 `redact_url`，
    /// 那个 `pass` 直接进日志。修法见 [`parse_fetchable`]。
    #[test]
    fn an_unparseable_string_is_replaced_wholesale_not_echoed() {
        let out = redact_url("user:pass@half-a-url");
        assert_eq!(out, UNPARSEABLE_URL);
        assert!(!out.contains("pass"));
    }

    /// 🔴 上面那条坑的**一整类**，逐形态钉住。
    ///
    /// 变异对象：把 [`parse_fetchable`] 退回成裸 `Url::parse(raw).ok()`。
    /// 这几个串在 Rust 下全都「解析得开」，于是凭据原样穿出去。
    #[test]
    fn strings_that_rust_parses_but_we_would_never_fetch_are_all_treated_as_unparseable() {
        for raw in [
            "user:pass@half-a-url",    // scheme=user，JS 会抛，Rust 不抛
            "mailto:someone:pw@x.com", // 解析得开，但我们不会拿它发 HTTP
            "file:///etc/passwd",      // 有 scheme 没 host
            "ftp://u:pw@h.example/x",  // 有 host，但不是 http/https
            "half-a-url",              // 连 scheme 都没有
        ] {
            assert_eq!(
                redact_url(raw),
                UNPARSEABLE_URL,
                "🔴 这个串穿过了脱敏：{raw}",
            );
            assert!(
                !redact_url(raw).contains("pw") && !redact_url(raw).contains("pass"),
                "🔴 凭据漏出去了：{raw}",
            );
        }
    }

    /// 反向控制：**真正能打的地址**不许被这道新闸误伤。
    ///
    /// 没有这一格的话，把 [`parse_fetchable`] 写成「永远返回 `None`」也能让上面那格绿 ——
    /// 而那会让每一条日志都变成 `<不是一个可解析的 URL>`，脱敏「完美」、诊断价值归零。
    #[test]
    fn real_http_urls_are_not_swallowed_by_the_new_gate() {
        for raw in [
            "https://h.example/v1/domains",
            "http://h.example:8080/v1",
            "https://h.example",
        ] {
            assert_ne!(redact_url(raw), UNPARSEABLE_URL, "误伤了一个真地址：{raw}");
        }
    }

    /// 🔴 [`url_secrets`] 与 [`redact_url`] **必须共用同一道闸**。
    ///
    /// 两道不同的闸会凑出最坏的组合：回查认为「解析得开、凭据是这几段」，
    /// 而 `redact_url` 认为「解析不开」并把原串整段换掉 ⇒ 回查拿着一份
    /// **从未出现在输出里**的凭据清单去查，查不到 ⇒ **放行**。两头看起来都尽职了。
    #[test]
    fn the_post_check_and_the_redactor_agree_on_what_counts_as_a_url() {
        // 这个串在裸 `Url::parse` 下解析得开、在我们这道闸下不算数。
        let raw = "user:pass@half-a-url";
        // `redact_in_message` 必须走「解析不开 ⇒ 无条件整段丢弃」那一支。
        assert_eq!(redact_in_message("boom pass", raw), UNSAFE_MESSAGE);
    }

    /// 没有任何秘密成分时一个字都不许动。
    #[test]
    fn a_clean_url_passes_through_untouched() {
        assert_eq!(
            redact_url("https://example.com/v1/domains"),
            "https://example.com/v1/domains",
        );
    }

    /// 🔴 这一格钉的是那条实测出来的洞：运行时在**构造请求那一步**就抛，
    /// 消息里带着完整原始 URL。
    #[test]
    fn a_runtime_error_carrying_the_raw_url_gets_it_redacted() {
        let url = "https://user:pass@mail.example.com/v1";
        let msg =
            format!("Request cannot be constructed from a URL that includes credentials: {url}");
        let out = redact_in_message(&msg, url);
        assert!(!out.contains("pass"), "口令漏出去了：{out}");
        assert!(out.contains("***@"), "{out}");
    }

    /// ⚠️ 替换**所有**出现处，不是只换第一处。
    #[test]
    fn every_occurrence_is_replaced_not_just_the_first() {
        let url = "https://u:topsecret@h.example/v1";
        let msg = format!("构造 {url} 失败；原始输入 {url}");
        let out = redact_in_message(&msg, url);
        assert!(!out.contains("topsecret"), "第二处漏了：{out}");
    }

    /// 🔴 **后置条件那一格**：替换没覆盖到（例如运行时重新编码过）时整段丢弃。
    ///
    /// 变异对象：把 `redact_in_message` 里那道回查删掉 ——
    /// 这条消息里的口令会原样留在输出里，而门禁全绿。
    #[test]
    fn a_re_encoded_url_that_escapes_replacement_is_dropped_wholesale() {
        let url = "https://u:topsecret@h.example/v1";
        // 运行时把它重新编码过 ⇒ 字面替换对不上，但口令仍在
        let msg = "failed: https://u:topsecret@h.example:443/v1 (normalized)";
        let out = redact_in_message(msg, url);
        assert_eq!(out, UNSAFE_MESSAGE, "回查没拦住：{out}");
    }

    /// 🔴 解析不开 ⇒ **无条件**整段丢弃（连哪几段是凭据都判不出来）。
    #[test]
    fn an_unparseable_url_makes_the_whole_message_unsafe() {
        assert_eq!(
            redact_in_message("一句无害的话", "not-a-url"),
            UNSAFE_MESSAGE
        );
    }

    /// ⚠️ 如实钉住那一半代价：口令恰好是个常见短串时，一句无害的消息会被整段丢掉。
    /// **这是刻意选的方向**，不是缺陷 —— 假阳性只让人少看到一句话。
    #[test]
    fn a_short_password_that_also_occurs_innocently_costs_us_the_whole_message() {
        let url = "https://u:1@h.example/v1";
        let out = redact_in_message("超时 1 秒", url);
        assert_eq!(out, UNSAFE_MESSAGE);
    }

    /// 🔴 `http_fail` 与 `http_fail_message` 同源 ——
    /// 「消息里写着 404、属性上挂着 403」只会在两条路各产一半时出现。
    #[test]
    fn the_status_on_the_error_always_matches_the_status_in_the_message() {
        for status in [400u16, 403, 404, 429, 500] {
            let (msg, code) =
                http_fail("yyds", "取域名列表", status, "GET", "https://h.example/v1");
            assert_eq!(code, status);
            assert!(
                msg.contains(&format!("HTTP {status}")),
                "消息与属性对不上：{msg} / {code}",
            );
            assert_eq!(
                msg,
                http_fail_message("yyds", "取域名列表", status, "GET", "https://h.example/v1")
            );
        }
    }

    /// ⚠️ `method` 是参数：写死 `GET` 会让日志报一个它没发过的方法。
    #[test]
    fn the_method_in_the_message_is_the_one_that_was_actually_used() {
        let m = http_fail_message("moemail", "建邮箱", 500, "POST", "https://h.example/api");
        assert!(m.contains("(POST https://h.example/api)"), "{m}");
    }

    /// 🔴 传输失败与正文失败**都没有状态码，也不许伪造一个**。
    #[test]
    fn transport_and_body_failures_carry_no_status_at_all() {
        let t = transport_fail_message(
            "yyds",
            "取域名列表",
            "dns error",
            "GET",
            "https://h.example/v1",
        );
        let b = body_fail_message(
            "yyds",
            "取域名列表",
            "unexpected eof",
            "GET",
            "https://h.example/v1",
        );
        for m in [&t, &b] {
            assert!(!m.contains("HTTP "), "凭空长出了一个状态码：{m}");
            assert!(
                !m.contains(" 0 ") && !m.contains("502"),
                "伪造了兜底状态码：{m}"
            );
        }
        assert!(b.contains("响应正文不是 JSON"), "{b}");
    }

    /// 四个模板形态一致：`<通道> <动作>失败: <原因> (<方法> <脱敏地址>)`。
    #[test]
    fn all_four_templates_share_one_shape() {
        let url = "https://user:pass@h.example/v1?t=x";
        let all = [
            http_fail_message("yyds", "取域名列表", 404, "GET", url),
            http_fail("yyds", "取域名列表", 404, "GET", url).0,
            transport_fail_message("yyds", "取域名列表", "boom", "GET", url),
            body_fail_message("yyds", "取域名列表", "boom", "GET", url),
        ];
        for m in &all {
            assert!(m.starts_with("yyds 取域名列表失败: "), "形态不一致：{m}");
            assert!(m.contains("(GET "), "形态不一致：{m}");
            assert!(!m.contains("pass"), "🔴 凭据漏进消息了：{m}");
            assert!(!m.contains("t=x"), "🔴 查询串漏进消息了：{m}");
        }
    }
}
