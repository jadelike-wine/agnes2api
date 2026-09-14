//! 从验证码邮件里抠出那六位数字。
//!
//! 移植自 TS 版 `src/core/registrar/code.ts`（160 行），规则真源是
//! `docs/porting/01-core-rules.md §10.9`。
//!
//! # 🔴 这个文件已经在同一个坑里栽过两次，别把上界当成「随便取的数」
//!
//! 三条正则里**每一个量词都必须有上界**，而且**上界必须逐段按真实数据论证** ——
//! 不许把一个数机械地套到所有段上。TS 那一侧的历史是：
//! 第一次漏了 `[^>]*` 没限；第二次把 200 直接套到 `[^>]` 上 ⇒ **真实模板取到错码**。
//!
//! ## 超界的后果**不是** `None`，而是一个「看起来合理的错码」
//!
//! 快路径失配之后会落到兜底的「全文第一个六位数」——
//! 正文里只要在验证码之前出现过**任意一个**六位数（订单号 / 工单号 / 活动 ID），
//! 就会被当成验证码提交上去：白烧一个补池名额、白烧一个临时邮箱，
//! Agnes 那边还会注册失败。**所以这些上界宁可留足余量，代价只是几十毫秒。**
//!
//! ## ⚠️ Rust 与 TS 的一处真实差异，但结论不变
//!
//! TS 那边这三条上界同时在挡**灾难性回溯**（规格里有实测表：1 MiB 输入
//! 单条模式要 215 秒同步 CPU，会把整个事件循环连同四条协议的转发一起堵死）。
//! Rust 的 `regex` crate 是**线性时间、无回溯**的，那张表在这里不成立。
//! **但三个上界照留**，理由与回溯无关：
//! ① 它们同时是上面那条「取到错码」的防线；
//! ② `MAX_BODY_LEN` / [`bounded_for_scan`] 的伪词边界问题在**任何引擎**下都成立。

use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// class 属性值 / 关键词到下一段数字之间允许的最大间隔。
///
/// 段内容是 **class 属性值**，真实模板几十字符；
/// 而 `[^\d]{0,200}` 那一段**本来就跨不过任何数字**（`padding:10px` 里的 `10` 就能截断它）。
const MAX_GAP: usize = 200;

/// 开标签里**剩余的全部属性**允许的最大长度。
///
/// 🔴 **刻意不复用 [`MAX_GAP`]** —— 这一段与它是完全不同的长度分布：
/// 真实营销模板一个 `style=` 就能到 200+，**实测两个「正常但不极端」的模板
/// 已经吃到 175 / 179 字符**。把 200 套到这里，就是 TS 那边栽的第二次。
const MAX_TAG_GAP: usize = 400;

/// `>` 与验证码数字之间允许的排版空白。
///
/// 真实 Agnes 模板是 `>143770<`（零空白），但**格式化过的**模板是换行 + 缩进 ——
/// 实测「换行 + 12 个空格」这种普通缩进在 `{0,8}` 下就会失配。
const MAX_WS: usize = 32;

/// 正文送进正则之前的长度上界。
///
/// 🔴 **是 1 MiB 而不是更小。** Agnes 的验证码邮件走 HTML 模板，一旦内嵌 base64 logo
///（动辄 100 KB+）且图片排在验证码元素之前，几十 KiB 的上限会被图片整个吃掉 ⇒
/// [`extract_code`] 恒返回 `None` ⇒ 轮询一路空转到超时 ⇒ **注册机 100% 静默失效**，
/// 而单测里的正文都是短字符串，**全绿**。
///
/// ⚠️ **往回调这个值的前提是所有量词都有上界。** 也别在这个文件里新增无界量词。
const MAX_BODY_LEN: usize = 1024 * 1024;

/// 邮件标题送进正则之前的长度上界。
const MAX_SUBJECT_LEN: usize = 1024;

/// CSS 十六进制颜色。先抹掉，免得 `#123456` 被当成验证码。
static HEX_COLOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#[0-9a-fA-F]{6}\b").expect("HEX_COLOR 是编译期常量正则"));

/// 一级：带 `verification` 类名的元素，**数字必须是该元素的直接文本**。
///
/// ⚠️ **这个严格性是判别器，不是缺陷**：`verification-title` 这类容器不含数字时匹配失败，
/// 引擎会自动前进到下一个候选容器（`verification-code`）。
/// **放宽成 `[\s\S]*?` 会让第一个开标签吞掉全文，后面的容器再无机会。**
static TAGGED_CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"class=["'][^"']{{0,{MAX_GAP}}}verification[^"']{{0,{MAX_GAP}}}["'][^>]{{0,{MAX_TAG_GAP}}}>\s{{0,{MAX_WS}}}(\d{{6}})\s{{0,{MAX_WS}}}<"#
    ))
    .expect("TAGGED_CODE 是编译期常量正则")
});

/// 二级：关键词锚定。跑在 `subject + " " + body` 上，大小写不敏感。
///
/// ⚠️ **两侧 `\b` 缺一不可**：`[^\d]` 吃不进数字但**吃得进字母**（`abc887766` 会误命中）；
/// 右边界则防止从时间戳 / 工单号这类长数字串里切出前六位。
static KEYWORD_ANCHORED_CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)(?:验证码|verification[\s-]?code|auth[\s-]?code)[^\d]{{0,{MAX_GAP}}}\b(\d{{6}})\b"
    ))
    .expect("KEYWORD_ANCHORED_CODE 是编译期常量正则")
});

/// 三级兜底：全文第一个六位数。定长量词，线性扫描。
static BARE_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{6})\b").expect("BARE_CODE 是编译期常量正则"));

/// 把字符串截到 `max` **字符**以内，供正则扫描。
///
/// # 🔴 截断点落在一串数字中间，会凭空造出一个伪词边界
///
/// 例如尾部是「订单号 1234567890」，切在第 6 位之后变成「订单号 123456<EOF>」——
/// 末尾就是词边界，于是 [`BARE_CODE`] **直接命中并返回一个错误的验证码**。
///
/// ⇒ 只要**真的发生了截断**，就把结尾那段数字**整个丢掉**（换成一个空格，
/// 免得把前后文粘到一起）。
///
/// ⚠️ **补一个字符不足以消掉这个伪边界** —— 空格前面照样是词边界，
/// **必须把残缺的数字本身去掉**。
///
/// ⚠️ **刻意不用 `\d+$` 这样的正则**：保留的前缀里若有长数字串而结尾不是数字，
/// 回溯引擎会退化成 O(n²)（TS 侧实测 128 KiB 要 35 秒）。这里从截断点往回走一次即可，
/// 线性；1 MiB 全数字的最坏输入 TS 侧实测约 5 ms。
/// Rust 的 `regex` 没有这个回溯问题，但**手写这一步仍然更直白**，且行为与 TS 逐字一致。
///
/// ⚠️ 按**字符**而不是字节截断：按字节切会切碎多字节字符，而正文是 HTML 模板、
/// 中文与 emoji 都可能出现。
fn bounded_for_scan(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    // 没截断就原样返回 —— 伪词边界只在真的切掉了东西时才会被造出来。
    if out.chars().count() == s.chars().count() {
        return out;
    }
    // 从尾部往回丢掉那一串残缺的数字。
    let mut dropped = false;
    while out.chars().next_back().is_some_and(|c| c.is_ascii_digit()) {
        out.pop();
        dropped = true;
    }
    // ⚠️ **只有真的丢了数字才补空格。** 那个空格的用途是「顶替被丢掉的那串数字，
    // 免得把前后文粘到一起」—— 没丢东西就没有要顶替的，凭空加一个空格是在改内容。
    // 截断本身也会在末尾造出词边界，但那只在被切断的是**数字串**时才危险：
    // 切在字母中间时 `\b(\d{6})\b` 根本没有可命中的形态。
    if dropped {
        out.push(' ');
    }
    out
}

/// 把邮件正文的各种形态规整成一个字符串。
///
/// ⚠️ **真机实测**：YYDS 的详情响应里 `html` 是**数组**（元素数 1）、`text` 是字符串。
/// TS 原版的模板插值对单元素数组**碰巧等价**，但多段 HTML 时会用**逗号**拼接 ——
/// 而**逗号会直接贴到数字上，影响 `\b` 边界判定**。
/// ⇒ 显式处理数组形态，并用**换行**拼接。
pub fn normalize_body(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|it| match it {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
    }
}

/// 三级匹配。`subject` 只参与二级。
pub fn extract_code(subject: &str, body: &str) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let body = bounded_for_scan(body, MAX_BODY_LEN);
    let subject = bounded_for_scan(subject, MAX_SUBJECT_LEN);

    // 1. 先抹掉 CSS 十六进制颜色，免得 `#123456` 被当成验证码。
    //    换成空格而不是空串：直接删掉会把前后文粘成一个词。
    let body = HEX_COLOR.replace_all(&body, " ");

    // 2. 一级：只在 body 上跑，**不含 subject**。
    if let Some(c) = TAGGED_CODE.captures(&body) {
        return Some(c[1].to_string());
    }

    // 3. 二级：关键词锚定，跑在 subject + body 上。
    let joined = format!("{subject} {body}");
    if let Some(c) = KEYWORD_ANCHORED_CODE.captures(&joined) {
        return Some(c[1].to_string());
    }

    // 4. 兜底。
    BARE_CODE.captures(&body).map(|c| c[1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_body_yields_nothing_even_when_the_subject_has_six_digits() {
        // subject 只参与二级，而二级跑在 `subject + " " + body` 上 ——
        // 但 body 为空时整个函数在第一行就返回，**subject 里的数字一眼都不看**。
        assert_eq!(extract_code("验证码 123456", ""), None);
    }

    #[test]
    fn the_real_agnes_template_shape_is_matched_by_the_tagged_tier() {
        let body = r#"<div class="verification-code" style="font-size:32px">143770</div>"#;
        assert_eq!(extract_code("", body).as_deref(), Some("143770"));
    }

    /// 🔴 **这一格钉的是那条「严格性是判别器」的裁定。**
    ///
    /// 变异对象：把 `[^>]{0,400}>` 放宽成 `[\s\S]*?>` ——
    /// 第一个开标签会吞掉全文，`verification-code` 那个容器再无机会，
    /// 于是取到的是 `999999`（标题容器后面那个无关数字）而不是真码。
    #[test]
    fn a_title_container_without_digits_does_not_block_the_real_code_container() {
        let body = concat!(
            r#"<div class="verification-title">您的验证码</div>"#,
            r#"<div class="order">999999</div>"#,
            r#"<div class="verification-code">143770</div>"#,
        );
        // 一级在 title 容器上失配（它的直接文本不是六位数），引擎前进到下一个候选。
        assert_eq!(extract_code("", body).as_deref(), Some("143770"));
    }

    /// ⚠️ `MAX_TAG_GAP` 刻意不复用 `MAX_GAP` —— 这一格就是那条裁定的判据。
    ///
    /// 变异对象：把 `{0,400}` 改成 `{0,200}`。
    /// 真实营销模板一个 `style=` 就能到 200+，实测两个「正常但不极端」的模板
    /// 已经吃到 175/179 字符。
    #[test]
    fn a_tag_with_three_hundred_characters_of_attributes_still_matches() {
        let style = "x".repeat(300);
        let body = format!(r#"<div class="verification-code" data-x="{style}">143770</div>"#);
        assert_eq!(extract_code("", &body).as_deref(), Some("143770"));
    }

    /// ⚠️ `MAX_WS = 32`：格式化过的模板是换行 + 缩进，实测「换行 + 12 空格」
    /// 在 `{0,8}` 下就会失配。
    #[test]
    fn a_pretty_printed_template_with_newline_and_indent_still_matches() {
        let body = "<div class=\"verification-code\">\n            143770\n        </div>";
        assert_eq!(extract_code("", body).as_deref(), Some("143770"));
    }

    #[test]
    fn css_hex_colors_are_wiped_before_matching_so_they_cannot_pose_as_the_code() {
        // `#4f46e5` 排在真码之前；不抹掉的话兜底会取到 `4f46e5`… 它不是纯数字，
        // 但 `#123456` 是 —— 用它来钉。
        let body = r#"<p style="color:#123456">你的验证码是 143770</p>"#;
        assert_eq!(extract_code("", body).as_deref(), Some("143770"));
    }

    #[test]
    fn the_keyword_tier_reads_the_subject_too() {
        assert_eq!(
            extract_code("verification code 143770", "<p>hello</p>").as_deref(),
            Some("143770"),
        );
    }

    /// ⚠️ **左边的 `\b` 不能省**：`[^\d]` 吃不进数字但吃得进字母，
    /// 没有左边界时 `abc887766` 会被切出 `887766`。
    #[test]
    fn the_keyword_tier_refuses_digits_glued_to_letters() {
        // 关键词之后紧跟一个「字母粘数字」的串，再跟一个干净的六位数。
        let body = "验证码 abc887766 143770";
        // 左 `\b` 让 `887766` 不合格（它左边是字母 `c`），于是命中后面那个。
        assert_eq!(extract_code("", body).as_deref(), Some("143770"));
    }

    /// ⚠️ **右边的 `\b` 不能省**：没有它会从长数字串里切出前六位。
    #[test]
    fn the_keyword_tier_refuses_a_prefix_of_a_longer_number() {
        let body = "验证码 1234567890 然后 143770";
        assert_eq!(extract_code("", body).as_deref(), Some("143770"));
    }

    /// 🔴 **这一格钉的是那条「截断造出伪词边界」的裁定。**
    ///
    /// 变异对象：把 [`bounded_for_scan`] 里丢尾部数字那个循环删掉 ——
    /// 截断点落在「订单号 1234567890」中间，`\b(\d{6})\b` 会命中
    /// 一个**从来不存在的验证码** `123456`。
    #[test]
    fn a_truncation_landing_inside_a_number_does_not_invent_a_word_boundary() {
        let head = "x".repeat(20);
        let s = format!("{head}1234567890");
        // 切到第 26 个字符 ⇒ 正好落在 `123456` 之后。
        let cut = bounded_for_scan(&s, 26);
        assert!(
            !BARE_CODE.is_match(&cut),
            "截断把 `1234567890` 切成了 `123456`，凭空造出一个伪验证码：{cut:?}",
        );
        assert!(cut.ends_with(' '), "丢掉残缺数字之后要补一个空格：{cut:?}");
    }

    /// 反向控制：**没有真的截断**时一个字都不许动。
    #[test]
    fn a_string_shorter_than_the_bound_is_returned_untouched() {
        assert_eq!(bounded_for_scan("订单 1234567890", 999), "订单 1234567890");
    }

    /// ⚠️ 按**字符**截断，不是字节：按字节切会切碎多字节字符。
    #[test]
    fn truncation_counts_characters_not_bytes() {
        let s = "中文中文中文";
        assert_eq!(bounded_for_scan(s, 3), "中文中");
    }

    /// ⚠️ 真机实测：YYDS 的 `html` 是**数组**。
    #[test]
    fn an_html_array_body_is_joined_with_newlines_not_commas() {
        // 逗号拼接会直接贴到数字上，影响 `\b` 边界判定 —— 这正是原版模板插值的坑。
        let v = json!(["<p>验证码</p>", "143770"]);
        assert_eq!(normalize_body(Some(&v)), "<p>验证码</p>\n143770");
    }

    #[test]
    fn a_string_body_passes_through_and_null_becomes_empty() {
        assert_eq!(normalize_body(Some(&json!("abc"))), "abc");
        assert_eq!(normalize_body(Some(&Value::Null)), "");
        assert_eq!(normalize_body(None), "");
    }

    /// 🔴 **这一格钉的是 `MAX_BODY_LEN = 1 MiB` 那条裁定。**
    ///
    /// 变异对象：把上限调到几十 KiB —— 内嵌 base64 logo 会把额度整个吃掉，
    /// 验证码元素落在界外 ⇒ 恒返回 `None` ⇒ 注册机 100% 静默失效。
    #[test]
    fn a_base64_logo_ahead_of_the_code_does_not_push_it_out_of_range() {
        let logo = "A".repeat(200 * 1024); // 200 KiB，真实模板里常见的量级
        let body = format!(
            r#"<img src="data:image/png;base64,{logo}"><div class="verification-code">143770</div>"#
        );
        assert_eq!(extract_code("", &body).as_deref(), Some("143770"));
    }

    /// 兜底那一级：三级都走到了。
    #[test]
    fn the_bare_fallback_catches_a_plain_six_digit_number() {
        assert_eq!(
            extract_code("", "your code is 143770 ok").as_deref(),
            Some("143770")
        );
    }

    /// ⚠️ **兜底只在 body 上跑**（与一级同源），不在 `subject + body` 上 ——
    /// 否则标题里的任意六位数都会在正文没有码时被当成验证码。
    #[test]
    fn the_bare_fallback_does_not_read_the_subject() {
        assert_eq!(extract_code("订单 998877 已发货", "<p>没有码</p>"), None);
    }
}
