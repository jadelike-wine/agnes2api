//! `/admin` 那棵静态树与 `/favicon.ico`。
//!
//! 面板的源码在仓根的 `admin-ui/`，由 `rust-embed` 在**编译期**整个烧进二进制
//! （裁定 P-11）。与被移植的 TS 版相比，这里换掉了一整条构建链：那边要跑
//! `scripts/build-ui.mjs` 生成一份与源同构的巨大 `.ts` 并**入仓**，还要靠一格判据
//! 守着「改了面板忘跑生成器」；`rust-embed` 编译期直接读原文件，
//! **「产物与源一字不差」在结构上不可能不成立**。
//!
//! 🔴🔴 **换掉那个生成器之后，它顺手做的三件事没有对象了，必须在这里补回来**
//! （各一格判据，见本文件的 `tests`）：
//!   ① **扩展名白名单** —— 尤其 🔴 **`svg` 不在内**：`image/svg+xml` 挂在 `/admin/` 下
//!      就是一份**可执行脚本的同源文档**，CSP 的 `script-src 'self'` 拦不住它。
//!   ② **零内联脚本** —— CSP 的 `script-src 'self'` 要求如此，按属性边界匹配。
//!   ③ **体积预算** —— 在 Rust 下这不再是平台约束（TS 版那 2 MiB 是 Worker 的脚本
//!      大小限制），纯粹是一条**早期预警线**：面板悄悄胖一倍时有人会看见。
//!
//! 🔴 **不做 kiro 的 SPA 回退。** kiro 自己的注释记着这个坑的下半段：无条件回退时
//! `assets/index-abc123.js` 这种明明不存在的资源也拿到 `200` + 一整篇 HTML，于是
//! **用 curl 核对某个构建产物是否上线时，不存在的文件也返回 200 —— 校验本身在骗人。**
//! 它把回退收窄成「不带扩展名的路径才回退」，而本仓的面板**压根没有 URL 路由**
//! （没有 hash、没有 history API，板块记忆走 localStorage）⇒ 回退没有服务对象，
//! 未命中一律 404。
//!
//! 🔴 **`/admin/` 不发 301**（裁定 P-12，登记为对 `02 §8.2` 的一处偏离）。
//! TS 版把 `/admin/` 301 到 `/admin`，理由是「一份内容一个规范 URL」；
//! 而 **301 会被浏览器近乎永久地缓存** —— 日后若改主意，老客户端手里会是一条
//! **拔不掉**的永久跳转。两条路径直接发同一份字节，代价只是同一份内容有两个
//! 缓存条目，而它们的 ETag 相同、内容相同。

use axum::Router;
use axum::body::Body;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "admin-ui"]
struct AdminAssets;

/// 面板的**全套安全响应头**。
///
/// ⚠️ **kiro2api 一条都没有**（U-19 / P-14：agnes 明确更强的一面，保留）。
///
/// - `style-src 'self'` **必须有**，漏了整份 CSS 会被拦掉 —— 一个很容易漏的坑。
/// - `script-src 'self'` 要求**零内联脚本**，由本文件的 `no_inline_scripts_anywhere` 拦。
/// - `frame-ancestors 'none'` 与 `X-Frame-Options: DENY` 两条都要：面板被 iframe
///   套住做点击劫持是真实风险，而老浏览器只认后者。
/// - 🔴 **`base-uri 'none'` 是 `index.html` 里不许出现 `<base>` 的直接原因**（P-7）：
///   有了它，`<base href="/admin/">` 会被浏览器**静默忽略**，相对路径解析到
///   `/css/base.css` ⇒ 整份 CSS 与 JS 全部 404。见 `index_html_has_no_base_tag_*`。
/// - `cache-control: no-cache` 不是「不缓存」，而是「可以存但每次必须回源验证」。
///   它**必须挂在 200/304/400/404 每一条路径上**，不能只挂在 200 那支 ——
///   一条没有 CSP 的 404 页面与一份没有 CSP 的文档在同一个源下是同一件事。
/// - 不加 CORS：面板同源，加了只会扩大攻击面。
///
/// ⚠️ **条数如实说：这里是 5 条**（`06 §7.5` 的行文写的是「六条」，而被移植的
/// `agnes2api/src/ui/serve.ts` 的 `SECURITY_HEADERS` 表里数出来就是这 5 条）。
/// 不为了凑数发明第 6 条。
const SECURITY_HEADERS: [(header::HeaderName, &str); 5] = [
    (
        header::CONTENT_SECURITY_POLICY,
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
         connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
    ),
    (header::X_FRAME_OPTIONS, "DENY"),
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (header::REFERRER_POLICY, "no-referrer"),
    (header::CACHE_CONTROL, "no-cache"),
];

/// 嵌入资源允许的扩展名。🔴 **`svg` 刻意不在内**，理由见文件头 ①。
///
/// ⚠️ `png` 是这一轮**新放进来**的（裁定 P-13）：TS 版禁一切二进制，理由是它那个
/// 生成器只会把文件当文本读；换 `rust-embed` 之后那个理由消失了。禁 `.svg` 的理由
/// 与生成器无关，仍然成立。
pub const ALLOWED_EXTENSIONS: &[&str] = &["html", "css", "js", "mjs", "json", "png"];

/// 嵌入资源的字节预算。
///
/// ⚠️ **这不再是平台约束**（TS 版那 2 MiB 是 Cloudflare Worker 的脚本大小上限），
/// 纯粹是一条**早期预警线**：面板悄悄胖一倍时有人会看见。
/// 与 TS 版同值，是为了让「面板变大了多少」这件事跨两个实现可比。
pub const ASSET_BUDGET_BYTES: usize = 2 * 1024 * 1024;

/// 面板品牌图。`/favicon.ico` 与 `/admin/logo.png` 是**同一份字节**。
const LOGO_PATH: &str = "logo.png";
/// 会话绝对上限：**12 小时**。面板那一份在 `admin-ui/js/pure/session.mjs`，
/// 由 `session_max_age_matches_the_panel` 那一格对账。
///
/// 🔴 它是**要写进部署文档、对运维承诺**的那一种数字，不是一个实现细节。
pub const SESSION_MAX_AGE_MS: u64 = 12 * 3_600_000;

/// 由内容哈希生成强 ETag。资源在编译期嵌入，同一份构建里内容不变、哈希也不变。
fn etag_of(hash: &[u8], prefix: &str) -> String {
    let mut s = String::with_capacity(2 + prefix.len() + hash.len() * 2);
    s.push('"');
    s.push_str(prefix);
    for b in hash {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s.push('"');
    s
}

/// `If-None-Match` 强比较，且**只认完全相等**。
///
/// 刻意不解析 `W/` 弱前缀：判错的方向是「多发一次全量」，不是「回一个错内容的 304」。
fn matches_etag(inm: Option<&str>, etag: &str) -> bool {
    inm.is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
}

fn with_security_headers(mut res: Response) -> Response {
    let h = res.headers_mut();
    for (name, value) in SECURITY_HEADERS {
        h.insert(name, header::HeaderValue::from_static(value));
    }
    res
}

/// 一条 4xx。**同样带全套安全头**，理由见 `SECURITY_HEADERS`。
fn plain_error(status: StatusCode, body: &'static str) -> Response {
    with_security_headers(
        (
            status,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            body,
        )
            .into_response(),
    )
}

/// 带缓存协商地发一份字节。
///
/// 此前（TS 版之前的某一版）一个缓存头都不发。HTTP 规定这种情况下浏览器可以
/// **启发式缓存** —— 自己决定存多久 —— 于是发版后用户看到的仍是旧面板。
/// kiro 的注释记着它那边真实发生过的形态：**「后端已是 0.7.9，面板却显示 0.7.6，
/// 而服务端每个接口查出来都是对的」**，而这类现象最难查。
fn serve_bytes(bytes: Vec<u8>, mime: &str, etag: &str, inm: Option<&str>) -> Response {
    if matches_etag(inm, etag) {
        // 🔴 304 **不得带 body**，但仍要带 ETag（下一次条件请求还要用它）。
        return with_security_headers(
            (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response(),
        );
    }
    with_security_headers(
        (
            [(header::CONTENT_TYPE, mime), (header::ETAG, etag)],
            Body::from(bytes),
        )
            .into_response(),
    )
}

/// 取一份嵌入资源并连同它的强 ETag 一起发出去。
fn serve_embedded(
    path: &str,
    etag_prefix: &str,
    mime: Option<&str>,
    inm: Option<&str>,
) -> Response {
    // ⚠️ `AdminAssets::get` 本身就是**查表**（键是相对路径），路径穿越在结构上已经
    //    不成立。这一判是**额外**的一道，保留：它让「路径里带 `..`」这件事得到一个
    //    明确的 400，而不是与「这个文件不存在」混成同一个 404。
    if path.contains("..") {
        return plain_error(StatusCode::BAD_REQUEST, "Bad Request");
    }
    match AdminAssets::get(path) {
        Some(f) => {
            let etag = etag_of(&f.metadata.sha256_hash(), etag_prefix);
            let guessed;
            let mime = match mime {
                Some(m) => m,
                None => {
                    guessed = mime_guess::from_path(path).first_or_octet_stream();
                    guessed.as_ref()
                }
            };
            serve_bytes(f.data.into_owned(), mime, &etag, inm)
        }
        // 🔴 **404 是纯文本，不是 JSON 信封**：这棵树服务浏览器，不服务 SDK。
        //    正文固定，**不回显有哪些键**。
        None => plain_error(StatusCode::NOT_FOUND, "Not Found"),
    }
}

fn if_none_match(h: &HeaderMap) -> Option<String> {
    h.get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// 面板静态树 + `/favicon.ico`。
///
/// 🔴 **只在配了 `ADMIN_TOKEN` 时才被挂上**（`server/app.rs` 的 `if has_admin`）：
/// 没配管理口令的部署一个 HTML 文档都不发，也就没有标签页要贴图标，
/// 而「有没有这条路由」不该额外泄漏一份运行时事实。
///
/// ⚠️ **三条路由都落到同一个 handler，刻意不用 `.nest()`**：axum 0.8 的 nest 对挂载
/// 路径末尾斜杠敏感（`nest("/admin", ..)` 不匹配 `/admin/`，反之亦然），
/// 而 `/admin` 与 `/admin/` 是访问面板最自然的两种手输写法。
pub fn router() -> Router {
    Router::new()
        .route("/admin", get(index))
        .route("/admin/", get(index))
        .route("/admin/{*file}", get(asset))
        .route("/favicon.ico", get(favicon))
}

async fn index(h: HeaderMap) -> Response {
    serve_embedded(
        "index.html",
        "",
        Some("text/html; charset=utf-8"),
        if_none_match(&h).as_deref(),
    )
}

async fn asset(Path(file): Path<String>, h: HeaderMap) -> Response {
    serve_embedded(&file, "", None, if_none_match(&h).as_deref())
}

/// `GET /favicon.ico`。浏览器**不看页面里写了什么**也会来取这一条（新标签页、
/// 收藏夹、历史记录里的图标都走它）。
///
/// **`.ico` 的名字 + `image/png` 的类型**是刻意的组合：名字由浏览器的默认约定定死，
/// 类型必须说真话 —— 这串字节是货真价实的 PNG，而全局那条 `nosniff` 会禁止浏览器
/// 去猜，类型写错就是不显示。
///
/// 🔴 **ETag 带 `favicon-` 前缀，不许与 `/admin/logo.png` 共用校验子。**
/// 两条 URL 今天字节相同，但它们是两份资源；共用校验子之后，哪一天图标换了格式
/// （而 `logo.png` 没换），带着旧校验子来的浏览器会拿到一个 304，把错的字节当图渲染。
async fn favicon(h: HeaderMap) -> Response {
    serve_embedded(
        LOGO_PATH,
        "favicon-",
        Some("image/png"),
        if_none_match(&h).as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use std::collections::{BTreeMap, BTreeSet};
    use tower::ServiceExt;

    // ════════════════════════════════════════════════════════════════════
    // 取证小工具
    // ════════════════════════════════════════════════════════════════════

    async fn call(uri: &str, inm: Option<&str>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut req = Request::builder().uri(uri);
        if let Some(v) = inm {
            req = req.header(header::IF_NONE_MATCH, v);
        }
        let res = router()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let body = to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, headers, body)
    }

    fn head(h: &HeaderMap, name: header::HeaderName) -> String {
        h.get(name)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    }

    fn asset_text(path: &str) -> String {
        let f = AdminAssets::get(path).unwrap_or_else(|| panic!("嵌入树里没有 {path}"));
        String::from_utf8(f.data.into_owned()).unwrap()
    }

    /// 去掉 CSS / JS 的块注释。
    ///
    /// 🔴 **一切对源码做的扫描都必须先过这一步。** 本仓的注释里大量出现被讨论的
    /// 那些色值与 key（例如 base.css 文件头里就写着 `#94a3b8` 与 `#059669`），
    /// 不剥注释的扫描会把**说明文字**当成**规则**报上来 —— 那种假阳性会逼着下一个人
    /// 去删注释，正好删掉最该留的东西。
    fn strip_delimited(src: &str, open: &str, close: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let mut rest = src;
        loop {
            match rest.find(open) {
                Some(at) => {
                    out.push_str(&rest[..at]);
                    let after = at + open.len();
                    match rest[after..].find(close) {
                        Some(end) => rest = &rest[after + end + close.len()..],
                        // 没闭合：剩下的整段都算注释，丢掉。
                        None => break,
                    }
                }
                None => {
                    out.push_str(rest);
                    break;
                }
            }
        }
        out
    }

    /// 去掉 CSS / JS 的块注释。
    ///
    /// ⚠️ **按 `&str` 切片做，不许逐字节 `as char`**：本仓的注释与词条都是中文，
    /// 逐字节拼回去会按 Latin-1 解释多字节序列，把整份字典变成乱码 —— 而那种坏法
    /// 不会报错，只会让后面每一格判据都在一份假的源码上做断言。
    fn strip_block_comments(src: &str) -> String {
        strip_delimited(src, "/*", "*/")
    }

    /// 去掉 HTML 注释。
    ///
    /// 🔴 **这一步和剥 CSS 注释是同一条纪律，别省。** 本仓的 `index.html` 注释里
    /// 逐字写着被禁止的那些东西（`<base href="/admin/">`、`<script>` …）——
    /// 不剥注释的扫描会把**警告文字**当成**违规**报上来，而那种假阳性会逼着
    /// 下一个人去删注释，正好删掉最该留的东西。
    fn strip_html_comments(src: &str) -> String {
        strip_delimited(src, "<!--", "-->")
    }

    /// 面板里所有 CSS 的（文件名, 去注释后的正文）。
    fn css_files() -> Vec<(String, String)> {
        AdminAssets::iter()
            .filter(|p| p.ends_with(".css"))
            .map(|p| {
                let text = asset_text(&p);
                (p.to_string(), strip_block_comments(&text))
            })
            .collect()
    }

    /// 取一个 `{ … }` 规则块的正文（从 `selector` 出现处往后找第一对花括号）。
    fn rule_body<'a>(css: &'a str, selector: &str) -> &'a str {
        let at = css
            .find(selector)
            .unwrap_or_else(|| panic!("CSS 里找不到选择器 {selector}"));
        let open = css[at..].find('{').expect("选择器后面没有 {") + at;
        let close = css[open..].find('}').expect("规则块没有 }") + open;
        &css[open + 1..close]
    }

    /// 从一个 token 定义块里读一个变量的值。
    fn token_value(block: &str, name: &str) -> String {
        for line in block.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix(name)
                && let Some(v) = rest.strip_prefix(':')
            {
                return v.trim().trim_end_matches(';').trim().to_string();
            }
        }
        panic!("token 块里没有 {name}");
    }

    /// WCAG 2.x 的相对亮度。
    fn relative_luminance(hex: &str) -> f64 {
        let h = hex.trim_start_matches('#');
        let full = if h.len() == 3 {
            h.chars().flat_map(|c| [c, c]).collect::<String>()
        } else {
            h.to_string()
        };
        let ch = |i: usize| {
            let v = u8::from_str_radix(&full[i..i + 2], 16).unwrap() as f64 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(0) + 0.7152 * ch(2) + 0.0722 * ch(4)
    }

    /// WCAG 2.x 的对比度。
    fn contrast(a: &str, b: &str) -> f64 {
        let (x, y) = (relative_luminance(a), relative_luminance(b));
        let (hi, lo) = if x > y { (x, y) } else { (y, x) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// 把一个 token 取值（`#rrggbb` 或 `rgba(r, g, b, a)`）**压到 `over` 这张面上**，
    /// 得到屏幕上真正那一块实色。
    ///
    /// 🔴 **半透明 tint 不合成就算，算出来的数与屏幕上那张面毫无关系。**
    /// 面板里几乎每一颗徽章的底都是 tint（`--success-10` 是 `rgba(16,185,129,0.1)`），
    /// 直接拿 `rgba(…)` 的三个通道当背景色去算，得到的是一张**根本不存在**的面：
    /// `.status-badge` 那颗「运行中」按那种算法是 1.00:1（自己压自己），
    /// 而实际压在白底上合成出的是 `#e7f8f2`、实算 2.31:1 —— 数不同，结论也不同。
    fn composite(value: &str, over: &str) -> String {
        let v = value.trim();
        let Some(inner) = v.strip_prefix("rgba(").and_then(|s| s.strip_suffix(')')) else {
            return v.to_string();
        };
        let parts: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
        assert_eq!(parts.len(), 4, "rgba() 不是四个分量：{v}");
        let alpha: f64 = parts[3].parse().expect("rgba() 的 alpha 读不出来");
        let back = over.trim_start_matches('#');
        let mix = |i: usize| {
            let f: f64 = parts[i].parse().expect("rgba() 的通道读不出来");
            let b = u8::from_str_radix(&back[i * 2..i * 2 + 2], 16).expect("底色不是 hex") as f64;
            (f * alpha + b * (1.0 - alpha)).round() as u8
        };
        format!("#{:02x}{:02x}{:02x}", mix(0), mix(1), mix(2))
    }

    /// 一份 CSS 里逐条 `(选择器, 规则体)`。
    ///
    /// ⚠️ **`@media` 之类的 at-rule 只脱一层壳**（壳本身不是一条规则，它里面的才是）。
    /// ⚠️ **调用前必须先 `strip_block_comments`**：注释里写满了被讨论的选择器与色值。
    fn css_rules(css: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let b = css.as_bytes();
        let (mut i, mut sel_start) = (0usize, 0usize);
        while i < b.len() {
            match b[i] {
                b'{' => {
                    let sel = css[sel_start..i].trim();
                    if sel.starts_with('@') {
                        i += 1;
                        sel_start = i;
                        continue;
                    }
                    let Some(rel) = css[i..].find('}') else { break };
                    let close = i + rel;
                    out.push((
                        sel.split_whitespace().collect::<Vec<_>>().join(" "),
                        css[i + 1..close].to_string(),
                    ));
                    i = close + 1;
                    sel_start = i;
                }
                b'}' => {
                    i += 1;
                    sel_start = i;
                }
                _ => i += 1,
            }
        }
        out
    }

    /// 一条规则体里某个属性的值。
    ///
    /// ⚠️ **属性名要整个相等地比**：`contains("color:")` 会把 `background-color`
    /// 与 `border-color` 一起收进来 —— 那两个不是文字色，混进来之后整格判据
    /// 算的就是另一件事了。
    fn decl(body: &str, prop: &str) -> Option<String> {
        body.split(';').find_map(|part| {
            let (k, v) = part.split_once(':')?;
            (k.trim() == prop).then(|| v.trim().to_string())
        })
    }

    /// 一个属性值里第一个 `var(--x)` 的 token 名。
    fn var_name(value: &str) -> Option<String> {
        let at = value.find("var(--")?;
        let start = at + "var(".len();
        let end = start + value[start..].find(')')?;
        Some(value[start..end].trim().to_string())
    }

    /// 从 HTML / JS 里抠出所有形如 `attr="值"` 的取值。
    fn attr_values(src: &str, attr: &str) -> Vec<String> {
        let needle = format!("{attr}=\"");
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(at) = src[from..].find(&needle) {
            let start = from + at + needle.len();
            match src[start..].find('"') {
                Some(end) => {
                    out.push(src[start..start + end].to_string());
                    from = start + end;
                }
                None => break,
            }
        }
        out
    }

    // ════════════════════════════════════════════════════════════════════
    // A 组 · 静态投递
    // ════════════════════════════════════════════════════════════════════

    /// A-1 🔴 `/admin` 与 `/admin/` **都直接发内容，而且逐字节相同**。
    ///
    /// 回归对象：TS 版把 `/admin/` **301** 到 `/admin`（`02 §8.2` 记的就是那个行为）。
    /// 301 会被浏览器近乎永久地缓存 —— 日后改主意时，老客户端手里是一条**拔不掉**的
    /// 永久跳转。裁定 P-12：两条都直接发同一份字节。
    #[tokio::test]
    async fn both_admin_spellings_serve_the_very_same_html() {
        let (s1, h1, b1) = call("/admin", None).await;
        let (s2, h2, b2) = call("/admin/", None).await;
        assert_eq!(s1, StatusCode::OK, "/admin 应直接发内容，不是 301");
        assert_eq!(s2, StatusCode::OK, "/admin/ 应直接发内容，不是 301");
        assert!(head(&h1, header::CONTENT_TYPE).starts_with("text/html"));
        assert!(head(&h2, header::CONTENT_TYPE).starts_with("text/html"));
        assert_eq!(b1, b2, "两种写法必须是同一份字节");
        assert_eq!(
            head(&h1, header::ETAG),
            head(&h2, header::ETAG),
            "同一份内容必须是同一个校验子，否则两条 URL 各有一份会各自过期的缓存"
        );
    }

    /// A-2 命中的资源必须带**强 ETag** 与 `Cache-Control: no-cache`。
    ///
    /// 一个缓存头都不发时，HTTP 允许浏览器**启发式缓存** —— 自己决定存多久 ——
    /// 发版后用户看到的仍是旧面板，而服务端每个接口查出来都是对的，最难查。
    #[tokio::test]
    async fn assets_carry_a_strong_etag_and_must_revalidate() {
        let (status, h, _) = call("/admin/js/app.js", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(head(&h, header::CACHE_CONTROL), "no-cache");
        let etag = head(&h, header::ETAG);
        assert!(
            etag.starts_with('"') && etag.ends_with('"') && etag.len() > 2,
            "ETag 必须是强校验子（带引号、非空、无 W/ 前缀），实际是 {etag:?}"
        );
        assert!(!etag.starts_with("W/"), "弱校验子不足以支撑 304");
    }

    /// A-3 内容未变 ⇒ **304 且 body 为空**，但仍带 ETag 与 Cache-Control。
    #[tokio::test]
    async fn an_unchanged_asset_answers_304_with_no_body() {
        let (_, h, _) = call("/admin/css/base.css", None).await;
        let etag = head(&h, header::ETAG);
        let (status, h2, body) = call("/admin/css/base.css", Some(&etag)).await;
        assert_eq!(status, StatusCode::NOT_MODIFIED);
        assert!(body.is_empty(), "🔴 304 不得带 body");
        assert_eq!(head(&h2, header::ETAG), etag);
        assert_eq!(head(&h2, header::CACHE_CONTROL), "no-cache");
    }

    /// A-4 内容不同的两份资源**不得共用 ETag**，否则浏览器会拿着 A 的副本当 B 用。
    #[tokio::test]
    async fn different_assets_get_different_etags() {
        let (_, a, _) = call("/admin/js/app.js", None).await;
        let (_, b, _) = call("/admin/js/api.js", None).await;
        assert_ne!(head(&a, header::ETAG), head(&b, header::ETAG));
    }

    /// A-5 🔴 不存在的资源回 **404 `text/plain`**，**不是** JSON 信封，
    /// 更**不是** SPA 回退。
    ///
    /// 回归对象：kiro 的无条件 SPA 回退让 `js/does-not-exist.js` 也拿到 `200` +
    /// 一整篇 HTML —— 浏览器把 HTML 当 JS 解析报 `Unexpected token '<'`，
    /// 而更坏的是**用 curl 核对某个构建产物是否上线时，不存在的文件也返回 200，
    /// 校验本身在骗人**。本仓的面板没有 URL 路由，回退没有服务对象。
    #[tokio::test]
    async fn a_missing_asset_is_a_plain_404_not_an_spa_fallback() {
        for p in [
            "/admin/js/does-not-exist.js",
            "/admin/assets/index-deadbeef.css",
            // 不带扩展名的路径 —— kiro 会把这一支回退成 index.html，本仓不会。
            "/admin/some/spa/route",
        ] {
            let (status, h, body) = call(p, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{p} 应 404");
            assert!(
                head(&h, header::CONTENT_TYPE).starts_with("text/plain"),
                "{p} 的 404 必须是纯文本"
            );
            let text = String::from_utf8_lossy(&body);
            assert!(!text.contains('{'), "{p} 的 404 不该是 JSON 信封：{text}");
            assert!(!text.contains('<'), "{p} 的 404 不该是一整篇 HTML：{text}");
        }
    }

    /// A-6 路径里含 `..` ⇒ **400**，与「这个文件不存在」明确分开。
    #[tokio::test]
    async fn a_traversal_shaped_path_is_a_400() {
        let (status, h, _) = call("/admin/css/../../Cargo.toml", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(head(&h, header::CONTENT_TYPE).starts_with("text/plain"));
    }

    /// A-7 🔴 **200 / 304 / 400 / 404 四条路径都必须带全套安全头。**
    ///
    /// 漏在错误分支上等于在同一个源下放了一条没有 CSP 的页面。
    #[tokio::test]
    async fn every_status_path_carries_the_full_security_header_set() {
        let (_, h0, _) = call("/admin/css/base.css", None).await;
        let etag = head(&h0, header::ETAG);
        let cases: [(&str, Option<&str>, StatusCode); 4] = [
            ("/admin/css/base.css", None, StatusCode::OK),
            (
                "/admin/css/base.css",
                Some(etag.as_str()),
                StatusCode::NOT_MODIFIED,
            ),
            ("/admin/a/../b", None, StatusCode::BAD_REQUEST),
            ("/admin/nope.js", None, StatusCode::NOT_FOUND),
        ];
        for (uri, inm, want) in cases {
            let (status, h, _) = call(uri, inm).await;
            assert_eq!(status, want, "{uri}");
            for (name, value) in SECURITY_HEADERS {
                assert_eq!(
                    head(&h, name.clone()),
                    value,
                    "{uri} 在 {want} 这条路径上缺了 {name}"
                );
            }
        }
    }

    /// A-8 `/favicon.ico` 回 `image/png`、字节 = 嵌入的 `logo.png`，
    /// 🔴 **且它的 ETag 与任何别的资源都不同**。
    ///
    /// 共用校验子的后果很具体：哪天图标换了格式而 `logo.png` 没换，带着旧校验子来的
    /// 浏览器会拿到一个 304，把错的字节当图渲染。
    #[tokio::test]
    async fn favicon_serves_the_logo_bytes_under_its_own_validator() {
        let (status, h, body) = call("/favicon.ico", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(head(&h, header::CONTENT_TYPE), "image/png");
        let logo = AdminAssets::get(LOGO_PATH).unwrap().data.into_owned();
        assert_eq!(body, logo, "字节必须与 /admin/logo.png 完全相同");

        let fav_etag = head(&h, header::ETAG);
        let mut others = BTreeSet::new();
        for p in AdminAssets::iter() {
            let (_, hh, _) = call(&format!("/admin/{p}"), None).await;
            others.insert(head(&hh, header::ETAG));
        }
        assert!(
            !others.contains(&fav_etag),
            "🔴 /favicon.ico 的校验子 {fav_etag} 与某个 /admin/* 资源撞了"
        );
        // 而且它**确实**能自己 304（前缀没有把条件请求弄坏）。
        let (s2, _, b2) = call("/favicon.ico", Some(&fav_etag)).await;
        assert_eq!(s2, StatusCode::NOT_MODIFIED);
        assert!(b2.is_empty());
    }

    /// A-9 🔴 **没配 `ADMIN_TOKEN` 时 `/admin` 与 `/favicon.ico` 都 404。**
    ///
    /// 「不泄漏这里有个后台」是 P1 就立下的性质，P2b 不许破掉它 ——
    /// 静态树必须与 `/admin/api` 一起进同一个 `if has_admin`。
    #[tokio::test]
    async fn without_an_admin_token_the_static_tree_is_absent_too() {
        use crate::adapters::storage_test_doubles::MemoryStorage;
        use crate::server::test_support::{TestApp, state_with_storage};
        use std::sync::Arc;

        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let app = crate::server::app::build_router(state);
        for uri in ["/admin", "/admin/", "/admin/css/base.css", "/favicon.ico"] {
            let res = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri} 不该存在");
        }
    }

    /// A-9b 🔴 **静态树不许吞掉 `/admin/api` 那棵树的「端点不存在」。**
    ///
    /// 这一格是 P2b 引进来的那条 `/admin/{*file}` 通配路由的直接看守。
    /// matchit 让静态段优先，所以**真存在**的端点不受影响；但「不存在的管理端点」
    /// 在管理子树里不匹配任何路由，本来靠 axum 的 fallback 兜 —— 而 **fallback
    /// 斗不过一条真路由**。不处理的话它会被面板那棵树接走，变成 `404 text/plain`，
    /// 于是**不带凭据时 401 与 404 分得出来 ⇒ 整棵管理 API 的路径表可以被枚举**。
    /// 处置在 `server/app.rs` 的 `admin_router()`：把它收成一条被闸包住的真路由。
    #[tokio::test]
    async fn the_static_tree_does_not_swallow_unknown_admin_api_paths() {
        use crate::adapters::storage_test_doubles::MemoryStorage;
        use crate::server::test_support::{TestApp, state_with_storage};
        use std::sync::Arc;

        let TestApp { mut state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        state.admin_token = Some(Arc::new("admin-token-0123456789abcdef".to_string()));
        let app = crate::server::app::build_router(state);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/admin/api/there-is-no-such-endpoint")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "🔴 不带凭据打一条不存在的管理端点必须与打一条存在的端点长得一样（401）——\
             拿到 404 说明面板的通配路由把它接走了，而那正好泄漏出端点清单"
        );
        let ct = res
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        assert!(
            ct.contains("json"),
            "管理树的响应必须是 JSON 信封，拿到 {ct:?} 说明它被面板那棵树接走了"
        );
    }

    /// A-10 🔴 嵌入文件的扩展名全在白名单内，**尤其不许有 `.svg`**。
    ///
    /// 这一格补的是 TS 版 `scripts/build-ui.mjs` 那张扩展名白名单 —— 换成
    /// `rust-embed` 之后它没有对象了。`image/svg+xml` 挂在 `/admin/` 下就是一份
    /// **可执行脚本的同源文档**，而 CSP 的 `script-src 'self'` 恰好放行同源脚本。
    #[test]
    fn every_embedded_file_has_an_allowed_extension() {
        let mut seen = BTreeSet::new();
        for p in AdminAssets::iter() {
            let ext = std::path::Path::new(p.as_ref())
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            assert!(
                ALLOWED_EXTENSIONS.contains(&ext.as_str()),
                "🔴 {p} 的扩展名 .{ext} 不在白名单 {ALLOWED_EXTENSIONS:?} 里"
            );
            seen.insert(ext);
        }
        assert!(
            !seen.contains("svg"),
            "🔴 /admin/ 下不许有 .svg —— 它会以 image/svg+xml 成为一份同源可执行文档"
        );
        assert!(!ALLOWED_EXTENSIONS.contains(&"svg"), "白名单本身不许放 svg");
    }

    /// A-11 🔴 每一份 `.html` **零内联脚本**。
    ///
    /// CSP 的 `script-src 'self'` 要求如此。判据按**属性边界**匹配：
    /// `<script data-src="x">payload</script>` 这类假 `src` 伪装拦得住
    /// （`data-src=` 里没有前导空格 + `src=` 那个形状）。
    #[test]
    fn no_inline_scripts_anywhere() {
        for p in AdminAssets::iter().filter(|p| p.ends_with(".html")) {
            let html = strip_html_comments(&asset_text(&p));
            let mut from = 0;
            while let Some(at) = html[from..].find("<script") {
                let start = from + at;
                let end = start
                    + html[start..]
                        .find('>')
                        .unwrap_or_else(|| panic!("{p} 里有一个没闭合的 <script"));
                let attrs = &html[start + "<script".len()..end];
                assert!(
                    attrs.contains(" src="),
                    "🔴 {p} 里有一段内联脚本（标签属性区是 {attrs:?}）—— CSP 的 \
                     script-src 'self' 会把它整段拦掉，屏幕上是一张没有行为的页面"
                );
                from = end;
            }
        }
    }

    /// A-12 嵌入总字节 ≤ 预算。**早期预警线**，不是平台约束。
    #[test]
    fn the_embedded_tree_stays_within_budget() {
        let total: usize = AdminAssets::iter()
            .map(|p| AdminAssets::get(&p).unwrap().data.len())
            .sum();
        assert!(
            total <= ASSET_BUDGET_BYTES,
            "面板嵌入体积 {total} 字节，超过预警线 {ASSET_BUDGET_BYTES}"
        );
        // 反向控制：预算真的在比，而不是一句摆设。
        assert!(total > 0, "一个字节都没嵌进来，上面那一比是空的");
    }

    // ════════════════════════════════════════════════════════════════════
    // B 组 · HTML / CSS 结构
    // ════════════════════════════════════════════════════════════════════

    /// 九个板块。🔴 顺序与名字照裁定 P-6（`registrar` 排第 8，`settings` 恒在最后）。
    const SECTIONS: [&str; 9] = [
        "dashboard",
        "accounts",
        "models",
        "apikeys",
        "playground",
        "usage",
        "logs",
        "registrar",
        "settings",
    ];

    /// B-13 🔴 `index.html` **不含 `<base`**，且每个 `href` / `src` 要么以 `/admin/`
    /// 开头、要么是 `data:`、要么是绝对 URL。
    ///
    /// 这一格钉的是本轮最容易踩、且**长得最像环境问题**的一条（P-7 / 06 §7.4）：
    /// 照抄 kiro 的 `<base href="/admin/">` + 保留本仓的 CSP（含 `base-uri 'none'`）
    /// ⇒ `<base>` 被浏览器**静默忽略** ⇒ `css/base.css` 解析成 `/css/base.css`
    /// ⇒ 整份 CSS 与 JS 全部 404 ⇒ 屏幕上是一张裸 HTML。
    #[test]
    fn index_html_has_no_base_tag_and_only_absolute_refs() {
        let html = strip_html_comments(&asset_text("index.html"));
        assert!(
            !html.contains("<base"),
            "🔴 index.html 里出现了 <base> —— CSP 的 base-uri 'none' 会静默忽略它，\
             于是全部相对路径解析到站点根、整份 CSS/JS 404"
        );
        // CSP 自己也必须还带着那条指令，否则上面那一判就没有对象了。
        assert!(
            SECURITY_HEADERS[0].1.contains("base-uri 'none'"),
            "CSP 里的 base-uri 'none' 是上面那条禁令的理由，两者必须同进同退"
        );
        let mut refs = attr_values(&html, "href");
        refs.extend(attr_values(&html, "src"));
        assert!(!refs.is_empty());
        for r in refs {
            let ok = r.starts_with("/admin/")
                || r.starts_with("data:")
                || r.starts_with("https://")
                || r.starts_with("http://");
            assert!(
                ok,
                "🔴 index.html 里的引用 {r:?} 既不是 /admin/ 绝对前缀、也不是 data:、\
                 也不是绝对 URL —— 没有 <base> 兜着，它会解析到站点根"
            );
        }
    }

    /// B-14 `index.html` 里的 `data-section` 集合**恰好**等于九个规定值，顺序也一致。
    #[test]
    fn the_nine_sections_are_exactly_the_prescribed_ones() {
        let html = strip_html_comments(&asset_text("index.html"));
        let got = attr_values(&html, "data-section");
        assert_eq!(
            got,
            SECTIONS.to_vec(),
            "侧栏导航的板块集合/顺序与裁定 P-6 不符"
        );
        // `<section id="sec-*">` 那一侧同样逐一对上 —— 路由按 `sec-<name>` 找节点。
        let ids: Vec<String> = attr_values(&html, "id")
            .into_iter()
            .filter(|i| i.starts_with("sec-"))
            .map(|i| i["sec-".len()..].to_string())
            .collect();
        assert_eq!(ids, SECTIONS.to_vec(), "九个 <section id> 与导航对不上");
    }

    /// B-15 三方对齐（规格 §8.2 第 15 条）：`data-section` ↔ **嵌入树里那九份
    /// `js/sec-*.js`** ↔ 字典里的 `nav.*` 键。**只删两处**就会被这一格抓住。
    ///
    /// ✅ **一处已了结的临时替换，记在这里备查。** 写这一格的那一轮，九份 `sec-*.js`
    /// 还不存在，于是第三方被临时换成了 `js/app.js` 的 `SECTION_NAMES`，并在原注释里
    /// 逐字写下「等 `sec-*.js` 落地时改回来」。**板块落地之后那一步没人回来做**，
    /// 判据就那么顶着一条临时替换活了好几轮 —— 本轮兑现：第三方已经是那九份文件。
    /// ⚠️ **`app.js` 那一格没有还回去，是白捡的第四方**：它是路由的真源，多钉一处
    /// 不要钱；下面三个 `assert_eq!` 一个都不许合并。
    #[test]
    fn nine_sections_line_up_three_ways() {
        let html = strip_html_comments(&asset_text("index.html"));
        let html_names: BTreeSet<String> = attr_values(&html, "data-section").into_iter().collect();

        // 🔴 规格里那个「第三方」：**嵌入树里真的存在几份 `js/sec-*.js`**。
        //    多一份没挂进导航、或少一份只写了 HTML 与路由表，都在这里红。
        let file_names: BTreeSet<String> = AdminAssets::iter()
            .filter_map(|p| {
                p.strip_prefix("js/sec-")
                    .and_then(|s| s.strip_suffix(".js"))
                    .map(|s| s.to_string())
            })
            .collect();

        let app = strip_block_comments(&asset_text("js/app.js"));
        let at = app
            .find("const SECTION_NAMES = [")
            .expect("js/app.js 里找不到 SECTION_NAMES 这张表");
        let open = app[at..].find('[').unwrap() + at;
        let close = app[open..].find(']').expect("SECTION_NAMES 没闭合") + open;
        let app_names: BTreeSet<String> = app[open + 1..close]
            .split(',')
            .filter_map(|s| {
                let s = s.trim();
                (s.len() > 2 && s.starts_with('"') && s.ends_with('"'))
                    .then(|| s.trim_matches('"').to_string())
            })
            .collect();

        let dict = dictionary();
        let nav_names: BTreeSet<String> = dict
            .keys()
            .filter_map(|k| k.strip_prefix("nav.").map(|s| s.to_string()))
            // `nav.logout` 与 `nav.menu` 是壳层控件，不是板块。
            .filter(|s| s != "logout" && s != "menu")
            .collect();

        assert_eq!(
            html_names, file_names,
            "index.html 的 data-section 与嵌入树里的 js/sec-*.js 对不上"
        );
        assert_eq!(
            html_names, app_names,
            "index.html 与 js/app.js 的板块表不一致"
        );
        assert_eq!(
            html_names, nav_names,
            "index.html 与 i18n 字典的 nav.* 不一致"
        );
    }

    /// CSS 里**允许**出现的硬编码色值（`:root` / `[data-theme="dark"]` 两个 token 块之外）。
    ///
    /// 每一条都有理由，不是"历史遗留"：
    /// - `#fff` / `#0b1f17`：实心按钮上的前景字。深色那一档是 kiro 自己的 a11y 补丁
    ///   （深色主色 `#34d399` 偏亮，白字不足，改深色字达 AA）。
    /// - `#7c3aed` / `#9333ea` / `#2563eb` / `#16a34a` / `#06b6d4`：**跳出 emerald 的
    ///   语义色标** —— 绑定账号那一组的紫、三档模型名的色标、"今日"那一格的青。
    ///   它们的作用正是"与主色系不同"，做成 token 会引诱别处复用。
    /// - `#7d8590` 与八档日志级别色：`.logs-console` 是一块**亮色下也是深色**的面，
    ///   它上面的前景色与整套亮/暗 token 无关（换主题时这一块不变）。
    const HARDCODED_COLOR_ALLOWLIST: &[&str] = &[
        "#fff", "#0b1f17", "#7c3aed", "#9333ea", "#2563eb", "#16a34a", "#06b6d4", "#7d8590",
        "#ff7b72", "#ffb3ad", "#f0b849", "#e8cf9a", "#6cb6ff", "#8b949e", "#9aa4af", "#6e7681",
    ];

    /// 一份 CSS 里，`:root` 与 `[data-theme="dark"]` 那两个块**之外**的正文。
    fn css_outside_token_blocks(css: &str) -> String {
        let mut out = String::new();
        let mut rest = css;
        loop {
            let next = ["\n:root {", "\n[data-theme=\"dark\"] {"]
                .iter()
                .filter_map(|m| rest.find(m).map(|i| (i, m.len())))
                .min_by_key(|(i, _)| *i);
            match next {
                Some((at, mlen)) => {
                    out.push_str(&rest[..at]);
                    let body_start = at + mlen;
                    match rest[body_start..].find("\n}") {
                        Some(end) => rest = &rest[body_start + end + 2..],
                        None => break,
                    }
                }
                None => {
                    out.push_str(rest);
                    break;
                }
            }
        }
        out
    }

    /// B-16 🔴 CSS 里**零硬编码色值** —— 除两个 token 块与上面那张登记表之外。
    ///
    /// 硬编码一个色值的代价是它**不跟着主题走**：亮色下看着对，深色下是一块死斑，
    /// 而且没有任何东西会告诉你。
    #[test]
    fn css_has_no_unregistered_hardcoded_colors() {
        // **反向控制**：登记表里每一条都必须真的出现在某条规则里。
        // 过期条目的坏处不是多几个字节，是**它会在下一次有人写出同一个色值时
        // 静默放行** —— 而那时它已经没有当初那条理由了。
        let bodies: String = css_files()
            .into_iter()
            .map(|(_, c)| css_outside_token_blocks(&c))
            .collect::<Vec<_>>()
            .join("\n");
        for lit in HARDCODED_COLOR_ALLOWLIST {
            assert!(
                bodies.to_ascii_lowercase().contains(lit),
                "登记表里的 {lit} 在任何一条 CSS 规则里都不再出现 —— 过期条目要删掉"
            );
        }
        for (name, css) in css_files() {
            let body = css_outside_token_blocks(&css);
            let b = body.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'#' {
                    let start = i;
                    let mut j = i + 1;
                    while j < b.len() && (b[j] as char).is_ascii_hexdigit() {
                        j += 1;
                    }
                    let len = j - start - 1;
                    // ⚠️ **十六进制串后面必须紧跟一个非标识符字符**，否则会把
                    //    **id 选择器**当成色值：`#accountsFreshness` 的前三个字符
                    //    恰好都是十六进制数字，不加这一判它会被报成 `#acc`。
                    let terminated = j >= b.len()
                        || !(b[j] as char).is_ascii_alphanumeric() && b[j] != b'_' && b[j] != b'-';
                    if terminated && (3..=8).contains(&len) {
                        let lit = body[start..j].to_ascii_lowercase();
                        assert!(
                            HARDCODED_COLOR_ALLOWLIST.contains(&lit.as_str()),
                            "🔴 {name} 的规则里出现了未登记的硬编码色值 {lit} —— \
                             它不会跟着主题走。要么改成 var(--token)，要么连同理由\
                             登记进 HARDCODED_COLOR_ALLOWLIST"
                        );
                    }
                    i = j;
                } else {
                    i += 1;
                }
            }
        }
    }

    /// 亮色 `:root` 里定义过的全部 token 名。
    fn defined_tokens() -> BTreeSet<String> {
        let base = strip_block_comments(&asset_text("css/base.css"));
        rule_body(&base, "\n:root ")
            .lines()
            .filter_map(|l| {
                let l = l.trim();
                l.starts_with("--").then(|| {
                    l.split(':')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .trim_end_matches(';')
                        .to_string()
                })
            })
            .collect()
    }

    /// B-17 🔴 每一个被 `var(--x)` 引用的 token 都必须在亮色 `:root` 里定义过。
    ///
    /// ⚠️ **引一个不存在的名字是静默失效**：`border-radius: var(--radius)` 在
    /// `--radius` 不存在时得到的不是报错，是一个**直角** —— 而屏幕上没有任何东西
    /// 在提示这件事。kiro 就有一处（`var(--border, #e2e8f0)`，`--border` 它从没定义过），
    /// 靠行内兜底活着；本仓把**所有** `var()` 兜底都摘掉了，正因为兜底是这类缺陷的藏身处。
    #[test]
    fn every_referenced_token_is_defined() {
        let defined = defined_tokens();
        assert!(defined.contains("--primary"), "取 :root 的方式坏了");
        for (name, css) in css_files() {
            let mut from = 0;
            while let Some(at) = css[from..].find("var(--") {
                let start = from + at + "var(".len();
                let end = start
                    + css[start..]
                        .find([')', ','])
                        .expect("var( 没有闭合")
                        .to_owned();
                let token = css[start..end].trim().to_string();
                assert!(
                    defined.contains(&token),
                    "🔴 {name} 引用了未定义的 token {token} —— 它不会报错，只会静默\
                     退化（少一个圆角、少一层色），而屏幕上没有任何提示"
                );
                from = end;
            }
        }
    }

    /// 每一档语义的「正文字色 × 它自己那张 tint 面」。
    ///
    /// 🔴 **这张表存在的理由，就是上一版那格判据的失效方式。**
    /// 它当初只点名钉了 `--primary-strong` 与深色 `--text-secondary` 两个 token
    /// —— 而 `--info-text` / `--success-text` / `--danger-text` / `--warning-text`
    /// 这一族**整族在它的覆盖面之外**。同一个病换个名字就绕过去了，而绕过去的
    /// 那几个 token 恰恰是横幅、徽章、错误提示这些**最需要被读懂**的东西的字色。
    /// ⇒ 判据必须按**语义对**遍历，不许再按名字点名。
    const SEMANTIC_TEXT_PAIRS: &[(&str, &str)] = &[
        ("--info-text", "--info-bg"),
        ("--success-text", "--success-bg"),
        ("--danger-text", "--danger-bg"),
        // ⚠️ warning 这一档**没有 `--warning-bg`**，它的底是 `--warning-15`
        //    （`.status-badge.is-degraded` / `.badge-warning` 都用它）——
        //    也正是四条 tint 里最厚的那一条，所以是最难过的那一格。
        ("--warning-text", "--warning-15"),
        // P-3 自己那一族：`--primary-strong` 当正文时压的底是 `--primary-10`
        //（`.current-pill` / `.model-tag` / `.lang-menu button.active` …）。
        ("--primary-strong", "--primary-10"),
    ];

    /// B-18 🔴 P-3 / P-4 那两处偏离**真的过了 WCAG AA**；并且**每一档语义的正文色 ×
    /// 三张主题面 × 它自己的 tint 面**，两个主题都过 AA。
    ///
    /// 这一格同时是那两条裁定的取证：改回 kiro 的原值（`#059669` / `#94a3b8`）时，
    /// 报文里会打出 3.77 与 4.04，一眼看得出为什么不能照抄。
    ///
    /// ⚠️ **P-3 / P-4 两条具名断言保留着，别"顺手"并进下面那个循环。**
    /// 它们的报文里带着 kiro 原值的实算数 —— 那是这两条裁定**为什么存在**的取证，
    /// 循环打不出来。循环钉的是覆盖面，具名钉的是来历，两者缺一不可。
    #[test]
    fn the_two_deliberate_color_deviations_really_pass_aa() {
        let base = strip_block_comments(&asset_text("css/base.css"));
        let light = rule_body(&base, "\n:root ");
        let dark = rule_body(&base, "\n[data-theme=\"dark\"] ");

        let primary_strong = token_value(light, "--primary-strong");
        let r1 = contrast(&primary_strong, "#ffffff");
        assert!(
            r1 >= 4.5,
            "P-3：--primary-strong = {primary_strong} 配白字只有 {r1:.2}:1，\
             不过 WCAG 1.4.3 AA 的 4.5:1（kiro 的 #059669 实算 3.77:1，正是为它才有这个 token）"
        );

        let text_secondary = token_value(dark, "--text-secondary");
        let bg_tertiary = token_value(dark, "--bg-tertiary");
        let r2 = contrast(&text_secondary, &bg_tertiary);
        assert!(
            r2 >= 4.5,
            "P-4：深色 --text-secondary = {text_secondary} 画在 --bg-tertiary = \
             {bg_tertiary} 上只有 {r2:.2}:1（.data-table th 正是这一对）。\
             kiro 的 #94a3b8 实算 4.04:1 —— **kiro 自己不达标**"
        );

        // 装饰用的 `--primary` 只受 1.4.11 的 3:1 约束，那一档必须仍然过。
        // ⚠️ **这一条只管住「装饰那一档够不够 3:1」，管不住「它有没有被当正文用」**
        //    —— 后半句由 `every_body_text_color_passes_aa_on_its_own_background`
        //    逐条规则去钉。P-3 当初漏的正是后半句：约 26 处拿 `--primary` 当正文字色，
        //    而这一格从头到尾是绿的。
        let primary = token_value(light, "--primary");
        let r3 = contrast(&primary, "#ffffff");
        assert!(
            r3 >= 3.0,
            "--primary = {primary} 做装饰（边框/焦点环/状态点）时是 {r3:.2}:1，\
             连 1.4.11 的 3:1 都不到"
        );

        // ── 语义对的全覆盖遍历（P-3e）─────────────────────────────────────
        const SURFACES: [&str; 3] = ["--bg-primary", "--bg-secondary", "--bg-tertiary"];
        for (theme, block) in [("亮色", light), ("深色", dark)] {
            let card = token_value(block, "--bg-primary");
            for (fg_token, tint_token) in SEMANTIC_TEXT_PAIRS {
                let fg = token_value(block, fg_token);
                // ① 三张主题面：正文色可能落在卡片、内容底、以及 chip/表头那一层上。
                for surface in SURFACES {
                    let bg = token_value(block, surface);
                    let r = contrast(&fg, &bg);
                    assert!(
                        r >= 4.5,
                        "🔴 {theme} {fg_token} = {fg} 画在 {surface} = {bg} 上只有 \
                         {r:.2}:1，不过 1.4.3 AA 的 4.5:1。\
                         它是**正文字色**那一档（`*-text` / `-strong` 这个名字就是这个意思），\
                         不是装饰 —— 装饰那一族叫 --info/--success/--danger/--warning/--primary，\
                         值一律不动。修法：在**同一条色阶**上挪一档，别换色系"
                    );
                }
                // ② 它自己那张 tint 面。tint 是半透明的，**必须先合成到卡片面上**。
                let tint = token_value(block, tint_token);
                let face = composite(&tint, &card);
                let r = contrast(&fg, &face);
                assert!(
                    r >= 4.5,
                    "🔴 {theme} {fg_token} = {fg} 压在 {tint_token} = {tint} \
                     合成到卡片面 {card} 之后的 {face} 上只有 {r:.2}:1。\
                     徽章 / 横幅那一族正是这一对 —— 屏幕上看到的是 {face}，不是 {tint}"
                );
            }
        }
    }

    /// 逐条登记的**豁免**：`(文件, 选择器, 理由)`。
    ///
    /// 🔴 **这张表只许放「按 WCAG 本身就不在 1.4.3 管辖内」的那几条**，
    /// 不许放「这一处比较难改」。每一条的理由都写在这里，而且**下面那格判据会反过来
    /// 验它还在不在**（选择器从 CSS 里消失了就要删掉这一行）—— 过期的豁免比没有豁免
    /// 更坏：它会在下一次有人写出同名规则时**静默放行**，而那时当初那条理由已经不在了。
    const BODY_TEXT_AA_EXEMPT: &[(&str, &str, &str)] = &[
        (
            "css/base.css",
            ".badge-off",
            "停用态文字。WCAG 1.4.3 自己就把「禁用控件」排除在外，而且它已经有 \
             `text-decoration: line-through` 这条非颜色线索（U-16 在这一格是满足的）。\
             实算亮色 4.34:1 / 深色 4.04:1 —— 够不到 4.5，这是登记在案的，不是漏的",
        ),
        // ── 五格图标 chip + 两颗仪表盘图标：**非文字元素**。
        //    `js/sec-usage.js` / `js/sec-dashboard.js` 建它们时一律带 `aria-hidden="true"`，
        //    语义整条由旁边的 `.stat-info h3` / `.dash-info-value` 承担（那两族走
        //    `--text-primary`，本来就过 AA）⇒ 属于 1.4.11 写明排除的「纯装饰图形」。
        //    ⚠️ 亮色下 `.stat-icon.warning` 1.91:1 / `.stat-icon.success` 2.31:1
        //    **连 1.4.11 那档 3:1 都不到** —— 留着靠的是上面那条排除，不是达标。
        //    要真收这两格该压的是 tint，不是字色；那是另一次改动，不在本轮。
        (
            "css/base.css",
            ".stat-icon.primary",
            "aria-hidden 的纯装饰图标 chip",
        ),
        (
            "css/base.css",
            ".stat-icon.success",
            "aria-hidden 的纯装饰图标 chip",
        ),
        (
            "css/base.css",
            ".stat-icon.warning",
            "aria-hidden 的纯装饰图标 chip",
        ),
        (
            "css/base.css",
            ".stat-icon.danger",
            "aria-hidden 的纯装饰图标 chip",
        ),
        (
            "css/base.css",
            ".stat-icon.info",
            "aria-hidden 的纯装饰图标 chip",
        ),
        (
            "css/sections-stats.css",
            ".dash-info-icon",
            "aria-hidden 的纯装饰 svg，语义由 .dash-info-value 承担",
        ),
        (
            "css/sections-stats.css",
            ".dash-uptime-icon",
            "aria-hidden 的纯装饰 svg，语义由 .dash-uptime-value 承担",
        ),
        (
            "css/sections-stats.css",
            "#sec-usage .stat-icon",
            "aria-hidden 的纯装饰表情图标",
        ),
    ];

    /// 底**既不是规则自己声明的、也推不出来**的那几条：`(文件, 选择器, 真正的底)`。
    ///
    /// ⚠️ 判据取底的顺序是：**规则自己声明的 → 从基础选择器推出来的 → 这张表 →
    /// 落到卡片面 `--bg-primary`**。前两条覆盖了绝大多数，落到这张表的只剩
    /// 「底由一个**选择器上看不出父子关系**的祖先给」这一种。
    ///
    /// **唯一的一条是日志控制台**：`.log-msg` 的底来自 `.logs-console`
    /// （`background: var(--code-bg)`），而 `.log-msg` 的选择器与 `.logs-console`
    /// 之间没有任何字符串上的包含关系 —— 推不出来。
    /// 它还是一块**亮色主题下也是深色**的面，不登记的话判据会拿
    /// `--code-text: #d4d4d4` 去和白底比，算出 1.48:1 的**假警报**
    /// （真值是压在 `--code-bg: #1e1e1e` 上的 11.25:1）。
    const BODY_TEXT_INHERITED_SURFACE: &[(&str, &str, &str)] =
        &[("css/sections-tools.css", ".log-msg", "--code-bg")];

    /// 一条规则的底：**先看它自己声明的，再从「基础选择器」推**。
    ///
    /// 🔴 **不推的话，判据会在一整类规则上算得比实际宽松。**
    /// 面板里到处是这种形态：基础规则给底、变体规则只换字色 ——
    /// `.logs-status { background: var(--bg-tertiary) }` +
    /// `.logs-status[data-state="connected"] { color: … }`，
    /// `.accounts-batchbar { background: var(--primary-10) }` + `.accounts-batchbar .bb-count`。
    /// 一律按卡片面算的话，亮色下会把 `#ffffff` 当底（比真实的 `#f1f5f9` / tint 面更亮）
    /// ⇒ **算出来的比值偏高**，而偏高的方向正是会放过真缺陷的那一侧。
    ///
    /// 推法：在全部 CSS 里找**最长**的那个「本规则选择器是它的扩展」的基础选择器
    ///（扩展的形态是 `X[…]` / `X.y` / `X:y` / `X y` 四种），取它声明的底。
    /// 取最长是因为 `.a` 与 `.a .b` 同时存在时，更具体的那个才是真正紧挨着文字的那张面。
    fn rule_surface(
        surfaces: &BTreeMap<String, String>,
        selector: &str,
        body: &str,
    ) -> Option<String> {
        if let Some(own) = decl(body, "background-color")
            .or_else(|| decl(body, "background"))
            .as_deref()
            .and_then(var_name)
        {
            return Some(own);
        }
        surfaces
            .iter()
            .filter(|(base, _)| {
                selector != base.as_str()
                    && selector.starts_with(base.as_str())
                    && matches!(
                        selector.as_bytes().get(base.len()),
                        Some(b'[' | b'.' | b':' | b' ')
                    )
            })
            .max_by_key(|(base, _)| base.len())
            .map(|(_, token)| token.clone())
    }

    /// B-18b 🔴 **每一处正文级文字色 × 它自己的背景都过 WCAG 1.4.3 AA**，两个主题都算。
    ///
    /// ── 这一格补的是什么 ────────────────────────────────────────────────────
    ///
    /// 上面那格（以及 P-3 / P-3b/c/d 那一串裁定）钉的全是 **token 本身**：
    /// 「`--primary-strong` 配白字够不够 4.5」「`--primary` 做装饰够不够 3.0」。
    /// 🔴 **但「`--primary` 只做装饰」这半句从来没有人验过，而它不成立。**
    /// 实测：`.mt-meta-val` / `.bb-count` / `.us-n.us-credits` / `.model-tag` /
    /// `.current-pill` / `.dash-rotation-badge` 全都拿 `--primary` 当**正文字色**
    /// —— 判据那一格从头到尾是绿的，而裁定在约 110 处被违反。
    /// ⇒ 判据必须落到**规则**这一层：不是问「这个 token 好不好」，
    ///   而是问「**这一条规则画出来的字，在它自己的底上读不读得清**」。
    ///
    /// ── 怎么算的 ────────────────────────────────────────────────────────────
    ///
    /// · 前景 = 规则里 `color: var(--x)` 的那个 token，在两套主题块里各解析一次；
    /// · 背景 = 规则**自己**声明的 `background` / `background-color`，
    ///   没声明就落到卡片面 `--bg-primary`（面板里每个板块都装在 `.card` 里）。
    ///   半透明 tint **先合成到卡片面上**再算；
    /// · 门槛 = 1.4.3 的 4.5:1，**只有真的够得上大文本**（≥24px，或 ≥18.66px 且 ≥700）
    ///   才降到 3.0。字号读不出来（`em` 这类相对单位、或压根没声明）时**按严的那档算**。
    ///
    /// ⚠️ **门槛别按「反正也差不多大」放宽。** P-3 自己那段注释写着这条：
    /// 「别用『把按钮字号调大到 18px 就算大文本』绕过去 —— 那会连坐整套按钮的尺寸标度」。
    ///
    /// ⚠️ **只扫 `color: var(--token)`，不扫 `color: #hex`。**
    /// 那 16 个硬编码前景（实心按钮上的 `#fff`、`.logs-console` 那八档日志级别色）
    /// 是**按主题被层叠覆盖**的（`[data-theme="dark"] .btn-primary { color: #0b1f17 }`），
    /// 一个不解析层叠的字符串扫描算它们只会算错。它们由
    /// `css_has_no_unregistered_hardcoded_colors` 那张登记表另行管着。
    #[test]
    fn every_body_text_color_passes_aa_on_its_own_background() {
        let base = strip_block_comments(&asset_text("css/base.css"));
        let light = rule_body(&base, "\n:root ").to_string();
        let dark = rule_body(&base, "\n[data-theme=\"dark\"] ").to_string();

        // 两张登记表的**过期自检**（与 HARDCODED_COLOR_ALLOWLIST 同一条纪律）。
        for (file, selector, _) in BODY_TEXT_AA_EXEMPT
            .iter()
            .chain(BODY_TEXT_INHERITED_SURFACE.iter())
        {
            let css = strip_block_comments(&asset_text(file));
            assert!(
                css_rules(&css).iter().any(|(s, _)| s == selector),
                "🔴 登记表里的 {file} / {selector} 在 CSS 里已经不存在了 —— \
                 过期的登记项会在下一次有人写出同名规则时静默放行，要删掉"
            );
        }

        // 全部 CSS 里「选择器 → 它自己声明的底 token」。基础选择器的底就从这里推。
        let mut surfaces: BTreeMap<String, String> = BTreeMap::new();
        for (_, css) in css_files() {
            for (selector, body) in css_rules(&css) {
                if let Some(t) = decl(&body, "background-color")
                    .or_else(|| decl(&body, "background"))
                    .as_deref()
                    .and_then(var_name)
                {
                    surfaces.insert(selector, t);
                }
            }
        }

        let mut checked = 0usize;
        for (name, css) in css_files() {
            for (selector, body) in css_rules(&css) {
                let Some(fg_token) = decl(&body, "color").as_deref().and_then(var_name) else {
                    continue;
                };
                if BODY_TEXT_AA_EXEMPT
                    .iter()
                    .any(|(f, s, _)| *f == name && *s == selector)
                {
                    continue;
                }
                // 底：自己声明的 → 从基础选择器推出来的 → 登记表 → 卡片面。
                let surface_token = rule_surface(&surfaces, &selector, &body).or_else(|| {
                    BODY_TEXT_INHERITED_SURFACE
                        .iter()
                        .find(|(f, s, _)| *f == name && *s == selector)
                        .map(|(_, _, t)| t.to_string())
                });

                // 大文本那一档：**只有真的够大才降门槛**。
                let px = decl(&body, "font-size").and_then(|v| {
                    let v = v.trim().to_string();
                    if let Some(n) = v.strip_suffix("rem") {
                        n.trim().parse::<f64>().ok().map(|x| x * 16.0)
                    } else {
                        v.strip_suffix("px")
                            .and_then(|n| n.trim().parse::<f64>().ok())
                    }
                });
                let weight = decl(&body, "font-weight")
                    .map(|v| match v.trim() {
                        "bold" | "bolder" => 700,
                        other => other.parse::<u32>().unwrap_or(400),
                    })
                    .unwrap_or(400);
                let large = px.is_some_and(|p| p >= 24.0 || (p >= 18.66 && weight >= 700));
                let need = if large { 3.0 } else { 4.5 };

                for (theme, block) in [("亮色", &light), ("深色", &dark)] {
                    let card = token_value(block, "--bg-primary");
                    let fg = token_value(block, &fg_token);
                    let (bg_name, bg) = match &surface_token {
                        Some(t) => (t.clone(), composite(&token_value(block, t), &card)),
                        None => ("--bg-primary（继承自卡片面）".to_string(), card.clone()),
                    };
                    let r = contrast(&fg, &bg);
                    checked += 1;
                    assert!(
                        r >= need,
                        "🔴 {theme}：{name} 的 `{selector}` 拿 {fg_token} = {fg} 当字色，\
                         画在 {bg_name} = {bg} 上只有 {r:.2}:1，够不到 {need:.1}:1。\n   \
                         这是**正文**（字号 {px:?}px / 字重 {weight}），受 WCAG 1.4.3 管。\n   \
                         修法二选一：① 改指那一档的正文色 token\
                         （--primary-strong / --success-text / --danger-text / \
                         --warning-text / --info-text，**装饰色那一族值一律不动**）；\
                         ② 这一处真的是非文字元素 / 停用态，那就连同**理由**登记进 \
                         BODY_TEXT_AA_EXEMPT。\n   \
                         ⚠️ 不许改的是门槛本身，也不许换色系 —— 同一条色阶上下挪档就够了"
                    );
                }
            }
        }
        // 反向下限：扫描真的扫到东西了。
        // 🔴 **这一格判据最坏的坏法不是报错，是空跑** —— `css_rules()` 的括号配对
        // 一旦走偏（例如将来某条规则里出现了带花括号的 `content:`），它会安静地
        // 返回一张更短的表，上面那个循环一条都断不出来，而整套用例照绿。
        // 今天是 246 条规则 × 2 套主题 = 492 对；留出删规则的余量，但不留出空跑的余量。
        assert!(
            checked >= 450,
            "只算了 {checked} 对前景/背景（今天应当是 492）—— 规则遍历坏了，这一格正在空跑"
        );
    }

    /// B-19 🔴 三个"选中态"各至少有一条**非颜色**声明（WCAG 1.4.1）。
    ///
    /// kiro 的这三个类只有颜色差 —— 色觉障碍用户看不出哪一项是选中的。
    /// 这一格钉的是 agnes 系统性执行、而 kiro 没执行的那条纪律（U-16）。
    #[test]
    fn state_is_not_color_only() {
        let base = strip_block_comments(&asset_text("css/base.css"));
        let manage = strip_block_comments(&asset_text("css/sections-manage.css"));
        let registrar = strip_block_comments(&asset_text("css/sections-registrar.css"));
        for (what, css, selector) in [
            ("侧栏选中项", base.as_str(), ".nav-item.active"),
            ("分段按钮选中项", manage.as_str(), ".seg-btn.is-active"),
            ("TAB 选中项", manage.as_str(), ".login-tab.active"),
            (
                "注册机 TAB 选中项",
                registrar.as_str(),
                ".reg-tab[aria-selected=\"true\"]",
            ),
        ] {
            let body = rule_body(css, selector);
            assert!(
                body.contains("font-weight"),
                "🔴 {what}（{selector}）只有颜色差，没有任何非颜色线索 —— \
                 色觉障碍用户分不出它是选中的。规则体是：{body}"
            );
        }
    }

    /// B-20 五档断点（1200 / 1024 / 900 / 768 / 640）各至少命中一条规则。
    ///
    /// ⚠️ ≤900 那一档是**侧栏变 off-canvas 抽屉**（U-12 对标 kiro）。删掉它不会让
    /// 任何东西报错 —— 只会让窄屏上的侧栏恒占 240px，内容被挤成一条。
    #[test]
    fn all_five_breakpoints_are_used() {
        let all: String = css_files().into_iter().map(|(_, c)| c).collect();
        for px in [1200, 1024, 900, 768, 640] {
            let needle = format!("max-width: {px}px");
            assert!(
                all.contains(&needle),
                "断点 ≤{px}px 一条规则都没命中 —— 五档响应式标度缺了一档"
            );
        }
    }

    // ════════════════════════════════════════════════════════════════════
    // C 组 · i18n
    // ════════════════════════════════════════════════════════════════════

    const LANGS: [&str; 5] = ["zh-CN", "zh-TW", "en", "ja", "ko"];

    /// 词条表的**命名空间登记表**。
    ///
    /// 🔴🔴 **它已经不再是 `referenced_keys()` 那道扫描的取值依据了**，这一点是本轮
    /// 修掉的那个缺陷的核心，别再改回去。原来的形态是：扫描**只认**这张表里的前缀，
    /// 于是「新加一个命名空间而忘了往这里加一行」= 那一族 key 在扫描里**根本不存在**
    /// ⇒ i18n-4 与 i18n-5 两条双向门禁对它**同时失效**，而且**全绿**。
    /// 这不是假设：`dash.*`（43 条）与 `keys.*`（115 条）就这么整整两族躺在外面，
    /// 仪表盘整屏印着 `dash.title` / `dash.refresh` 这样的裸 key，判据一格都没红。
    ///
    /// ⇒ **白名单这个形态本身就是缺陷之源：它的失败方向是「放过」。**
    /// 今天扫描按 **key 形态**认（`looks_like_an_i18n_key`），失败方向翻了过来 ——
    /// 一个形态像 key 却不在字典里的字面量**一律红**，要么补词条、要么去
    /// `NON_I18N_KEY_LITERALS` 登记。新开一个命名空间不需要有人记得改任何一张表。
    ///
    /// 那这张表今天还留着干什么？它降级成一份**登记**，由下面那一格三个方向钉住
    /// （字典 → 表、表 → 字典、面板源码 → 表）。它不再能让任何东西变瞎，
    /// 最坏的失败只是「表过期了」，而那一格会当场说出来。
    const NAMESPACES: &[&str] = &[
        "nav.", "header.", "login.", "common.", "reveal.", "section.",
        // 仪表盘与账号管理这两个板块先落地，各自起了自己的前缀，没并进 `section.`。
        "dash.", "keys.",
    ];

    /// **形态像 i18n key、但不是 i18n key** 的字面量登记表。
    ///
    /// 这是上面那段「失败方向翻过来」的代价，也是它的全部代价：扫描按形态认之后，
    /// 少数不是词条的字符串会被认出来，必须在这里逐条登记。
    /// ⚠️ **登记的门槛就是这一条**：它必须真的不是词条 —— 不是「这条还没翻译」的挡箭牌。
    ///
    /// 今天这 17 条是同一类东西：**服务端 DTO 交出来的判定 key**，在
    /// `js/sec-apikeys.js` / `js/sec-logs.js` / `js/sec-models.js` / `js/sec-usage.js`
    /// 的别名表里做**左列**（右列才是本仓字典的 key）。
    /// 它们的真源在 Rust（`src/core/admin/view/*.rs`），与这张词条表是两套命名。
    ///
    /// ⚠️ 下面那一格会反过来查这张表**有没有过期条目**：登记了一个已经不出现在任何
    /// 面板源码里的字面量会红 —— 否则它会在将来某天悄悄豁免掉一个真的漏了的词条。
    const NON_I18N_KEY_LITERALS: &[&str] = &[
        // `js/sec-apikeys.js` 的 `VIEW_KEY` 左列（另有两处 `===` 比较用同一批串）。
        "ak.action.disable",
        "ak.action.enable",
        "ak.bucket.active",
        "ak.bucket.disabled",
        "ak.bucket.expired",
        "ak.empty",
        // `js/sec-logs.js` 的 `LEVEL_LABEL` 左列。
        "ev.level.debug",
        "ev.level.error",
        "ev.level.info",
        "ev.level.unknown",
        "ev.level.warn",
        // `js/sec-models.js` 的 `MODALITY_KEY` 左列。
        "models.modality.chat",
        "models.modality.image",
        "models.modality.video",
        // `js/sec-usage.js` 的 `API_KEY_LABEL` 左列。
        "usage.key.master",
        "usage.key.other",
        "usage.key.unattributed",
        // `js/sec-settings.js` 的**配置字段路径**（`GROUPS` 与 `FIELD_LABEL` 的左列）。
        // 与上面 17 条同类：**真源在 Rust**（`src/admin/config.rs::FIELDS`），
        // 是服务端 DTO 交出来的路径，不是词条 —— 词条是它们映射到的右列
        // （`section.set.field.registrar.*` / `section.set.field.channel.*`）。
        // ⚠️ 之所以只有注册机这 15 条要登记：别的字段路径是**单段**的
        //（`maxStrikes`），形态上就不像 key；注册机这一族是两三段点分的。
        "registrar.agnesPlatformUrl",
        "registrar.channel",
        "registrar.codeTimeoutMs",
        "registrar.enabled",
        "registrar.maxDomainAttempts",
        "registrar.mintBatch",
        "registrar.mintDelayMaxMs",
        "registrar.mintDelayMinMs",
        "registrar.moemail.apiKey",
        "registrar.moemail.baseUrl",
        "registrar.targetKeys",
        "registrar.tendIntervalMs",
        "registrar.tokenName",
        "registrar.yyds.apiKey",
        "registrar.yyds.baseUrl",
    ];

    /// 「未翻译泄漏」的豁免名单：某些词条在几种语言里**本来就该逐字相同**
    /// （纯符号、专名、`ADMIN_TOKEN` 这类标识符）。
    /// ⚠️ 本轮字典里**一条都不需要豁免**，所以它是空的；下面那格判据会顺带检查
    /// 表里没有过期条目（写了一个已经不存在的 key 会红）。
    const UNTRANSLATED_ALLOWLIST: &[&str] = &[];

    /// 🔴 裁定 P-10 的落点：把 `js/i18n-dict.js` 的文件体当一整块 JSON **严格解析**。
    ///
    /// 掐掉 `export const I18N =` 前缀与末尾的 `;`，中间必须是合法 JSON
    /// （键全带双引号、**无尾逗号、无注释**）。
    /// 浏览器那边照旧 `import`，零变化；而这里能把 agnes 那套 node 门禁搬进 `cargo test`。
    fn dictionary() -> BTreeMap<String, BTreeMap<String, String>> {
        // ⚠️ **先剥注释再找锚点**：本文件的文件头注释里逐字写着
        //    `export const I18N =`（它在解释这条裁定），直接 `find` 会锚到注释上。
        let src = strip_block_comments(&asset_text("js/i18n-dict.js"));
        const MARK: &str = "export const I18N =";
        let at = src
            .find(MARK)
            .expect("i18n-dict.js 里找不到 `export const I18N =` —— 文件体的形状变了");
        let body = src[at + MARK.len()..].trim().trim_end_matches(';').trim();
        serde_json::from_str(body).unwrap_or_else(|e| {
            panic!(
                "🔴 i18n 字典不是一整块合法 JSON：{e}\n\
                 常见原因：尾逗号、块里写了注释、用了单引号。\
                 裁定 P-10 要求这一块严格可解析，否则下面 7 格判据全部没有对象。"
            )
        })
    }

    /// 一个双引号字面量**长得像不像一条 i18n key**。
    ///
    /// 🔴 这就是取代命名空间白名单的那个判据，理由见 `NAMESPACES` 的文件注释：
    /// 白名单漏一行是**静默放过**，形态判断漏一条是**当场红**。
    ///
    /// 形态定得很窄，窄到面板整棵树上只有 175 个字面量命中（158 条真词条 +
    /// `NON_I18N_KEY_LITERALS` 那 17 条），**零个别的假阳性**：
    ///   · 首段以 ASCII 小写字母开头，整段只有 ASCII 字母与数字；
    ///   · 后面至少还有一段 `.xxx`，每段同样非空且只有 ASCII 字母数字。
    /// ⇒ `/admin/js/app.js`（有 `/`）、`text/html`（有 `/`）、`1.0`（数字开头）、
    ///   `Intl.NumberFormat`（大写开头）这些全都落在外面。
    fn looks_like_an_i18n_key(lit: &str) -> bool {
        let mut segments = lit.split('.');
        let Some(first) = segments.next() else {
            return false;
        };
        if !first.starts_with(|c: char| c.is_ascii_lowercase())
            || !first.chars().all(|c| c.is_ascii_alphanumeric())
        {
            return false;
        }
        let mut tail = 0usize;
        for s in segments {
            if s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric()) {
                return false;
            }
            tail += 1;
        }
        tail >= 1
    }

    /// 面板源码里出现过的全部 i18n key（**按 key 形态**扫双引号字面量）。
    ///
    /// ⚠️ **字典自己不参与扫描** —— 否则每个 key 都"被自己引用"了，
    /// 「未引用的 key 是硬错」那一格会永远绿。
    ///
    /// 🔴 **刻意不按调用形态扫**（`t("…")` / `elI18n(tag, "…")` / `labelKey: "…"` /
    /// `data-i18n="…"` / 一句 `return "keys.verify.ok";` …）：本仓的 key 进 `t()` 的
    /// 写法今天就有六七种，而按调用形态扫的那道闸**每多一种写法就要有人记得加一条**
    /// —— 那正好是白名单那个病换了个身子，而且它漏掉时同样不会红。
    fn referenced_keys() -> BTreeMap<String, Vec<(String, usize)>> {
        let mut out: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
        for p in AdminAssets::iter().filter(|p| {
            (p.ends_with(".js") || p.ends_with(".mjs") || p.ends_with(".html"))
                && !p.ends_with("i18n-dict.js")
        }) {
            let src = asset_text(&p);
            let b = src.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'"' {
                    let start = i + 1;
                    match src[start..].find('"') {
                        Some(len) => {
                            let lit = &src[start..start + len];
                            if looks_like_an_i18n_key(lit) && !NON_I18N_KEY_LITERALS.contains(&lit)
                            {
                                out.entry(lit.to_string())
                                    .or_default()
                                    .push((p.to_string(), start + len + 1));
                            }
                            i = start + len + 1;
                        }
                        None => break,
                    }
                } else {
                    i += 1;
                }
            }
        }
        out
    }

    fn placeholders(s: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut from = 0;
        while let Some(a) = s[from..].find('{') {
            let open = from + a;
            match s[open..].find('}') {
                Some(b) => {
                    let inner = &s[open + 1..open + b];
                    if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric()) {
                        out.insert(inner.to_string());
                    }
                    from = open + b + 1;
                }
                None => break,
            }
        }
        out
    }

    /// i18n-1 字典能被**严格**解析成 `key → {lang → text}`。
    #[test]
    fn the_dictionary_parses_as_strict_json() {
        let dict = dictionary();
        assert!(!dict.is_empty(), "字典是空的");
        // 反向控制：解析出来的确实是"key → 语言表"那个形状，不是一层空壳。
        let (k, row) = dict.iter().next().unwrap();
        assert!(row.contains_key("zh-CN"), "{k} 下面没有 zh-CN，形状不对");
    }

    /// i18n-2 每个 key **恰好**五种语言，且都非空。
    #[test]
    fn every_key_has_all_five_languages() {
        for (k, row) in dictionary() {
            let got: BTreeSet<&str> = row.keys().map(|s| s.as_str()).collect();
            let want: BTreeSet<&str> = LANGS.into_iter().collect();
            assert_eq!(got, want, "{k} 的语言集合不对");
            for (lang, text) in &row {
                assert!(!text.trim().is_empty(), "{k} 的 {lang} 是空的");
            }
        }
    }

    /// i18n-3 🔴 同一个 key 的 `{占位符}` 集合在五种语言之间必须一致。
    ///
    /// 不一致的后果是**只有某一种语言的用户**看到一个裸的 `{status}`，
    /// 而中文环境下的开发者永远看不见。
    #[test]
    fn placeholder_sets_agree_across_languages() {
        for (k, row) in dictionary() {
            let base = placeholders(&row["zh-CN"]);
            for lang in LANGS {
                let got = placeholders(&row[lang]);
                assert_eq!(
                    got, base,
                    "🔴 {k} 的插值占位符在 {lang} 与 zh-CN 之间不一致：{got:?} | {base:?}"
                );
            }
        }
    }

    /// i18n-4 🔴 字典里每个 key 都必须在面板源码里被引用过。**未引用是硬错，不是警告。**
    ///
    /// 一个没人用的词条既不会被看见、也不会被翻译审核，它只会在下一次改名时被漏掉，
    /// 然后以「面板上出现一个裸 key」的形态浮上来。
    #[test]
    fn every_dictionary_key_is_referenced() {
        let used = referenced_keys();
        let orphans: Vec<String> = dictionary()
            .keys()
            .filter(|k| !used.contains_key(*k))
            .cloned()
            .collect();
        assert!(
            orphans.is_empty(),
            "🔴 字典里这些 key 没有任何地方在用：{orphans:?}"
        );
    }

    /// i18n-5 🔴 面板源码里出现的每个 key 都必须在字典里。
    ///
    /// 缺了的后果是 `t()` **返回 key 本身** —— 面板上出现一个裸的 `nav.foo`，
    /// 而生产环境的控制台里一个字都没有（`DEBUG_STORE` 默认是关的）。
    #[test]
    fn every_referenced_key_is_in_the_dictionary() {
        let dict = dictionary();
        let missing: Vec<String> = referenced_keys()
            .into_iter()
            .filter(|(k, _)| !dict.contains_key(k))
            .map(|(k, sites)| format!("{k}（出现在 {:?}）", sites[0].0))
            .collect();
        assert!(
            missing.is_empty(),
            "🔴 这些 key 用了但字典里没有：{missing:?}"
        );
    }

    /// i18n-6 🔴 带 `{占位符}` 的 key **不许被当成裸标签用**。
    ///
    /// 判据：这种 key 每一次以字面量出现时，闭合引号后面紧跟的非空白字符必须是 `,`
    /// （= 真的传了参数 / 真的是对象字面量里的一项）。
    /// ⚠️ **这条是人工冒烟真抓出来的已上线缺陷**：面板上出现一个裸的 `{status}`。
    #[test]
    fn keys_with_placeholders_are_never_used_as_bare_labels() {
        let dict = dictionary();
        let sites = referenced_keys();
        for (k, row) in &dict {
            if placeholders(&row["zh-CN"]).is_empty() {
                continue;
            }
            let Some(occurrences) = sites.get(k) else {
                continue;
            };
            for (file, after) in occurrences {
                let src = asset_text(file);
                let next = src[*after..].chars().find(|c| !c.is_whitespace());
                assert_eq!(
                    next,
                    Some(','),
                    "🔴 {file} 里 {k:?} 后面跟的是 {next:?} 而不是 `,` —— \
                     它带占位符，当裸标签用会让面板上出现一个裸的 {{…}}"
                );
            }
        }
    }

    /// i18n-7 🔴 未翻译泄漏：`en` / `ja` / `ko` 三种语言不许与 `zh-CN` 逐字相同。
    ///
    /// ⚠️ **`zh-TW` 不在这一判里**：它与 `zh-CN` 之间确实存在大量合法的逐字相同
    /// （`API 管理`、`取消`…），把它算进来只会逼人去造无意义的差异。
    #[test]
    fn no_untranslated_leakage() {
        let dict = dictionary();
        for entry in UNTRANSLATED_ALLOWLIST {
            assert!(
                dict.contains_key(*entry),
                "豁免名单里的 {entry} 已经不是字典里的 key 了 —— 过期条目要删掉，\
                 否则名单会悄悄豁免掉一个将来重名的 key"
            );
        }
        for (k, row) in &dict {
            if UNTRANSLATED_ALLOWLIST.contains(&k.as_str()) {
                continue;
            }
            for lang in ["en", "ja", "ko"] {
                assert_ne!(
                    row[lang], row["zh-CN"],
                    "🔴 {k} 的 {lang} 与 zh-CN 逐字相同 —— 这是一条没翻译却看起来\
                     翻译过了的词条"
                );
            }
        }
    }

    /// i18n-8 命名空间登记表 + **三个方向的自检** + 豁免表的过期自检。
    ///
    /// 🔴 **这张表已经不是 `referenced_keys()` 的取值依据了**（见 `NAMESPACES` 的
    /// 注释：正是那个形态让 `dash.*` / `keys.*` 两族 158 条在全绿下漏了出去）。
    /// 它今天是一份**登记**，所以这一格钉的是「这份登记没有撒谎」：
    ///   · 正向：字典里每个 key 的命名空间都在表里；
    ///   · 反向：表里每条命名空间都至少命中一个真 key（防止留下过期条目，
    ///     让人误以为某个前缀"已经被守着了"）；
    ///   · 🔴 **第三向（本轮新加）：面板源码里每个被认成 key 的字面量，
    ///     它的命名空间也必须在表里。** 这一条在今天的结构下确实**被
    ///     i18n-5 + 正向传递地蕴含**，留着是因为它的失败报文指向的是
    ///     **登记本身过期了**，而不是「某条词条漏了」—— 两件事的处置不同。
    ///
    /// 最后一段查 `NON_I18N_KEY_LITERALS` 有没有过期条目：豁免表比登记表危险，
    /// 一条过期的豁免会在将来某天**静默**放掉一个真的漏了的词条。
    #[test]
    fn the_namespace_table_is_tight_in_both_directions() {
        let dict = dictionary();
        for k in dict.keys() {
            assert!(
                NAMESPACES.iter().any(|ns| k.starts_with(ns)),
                "🔴 {k} 的命名空间不在登记表 {NAMESPACES:?} 里 —— 登记过期了"
            );
        }
        for ns in NAMESPACES {
            assert!(
                dict.keys().any(|k| k.starts_with(ns)),
                "登记表里的命名空间 {ns} 一个真 key 都没命中 —— 过期条目要删掉"
            );
        }
        for k in referenced_keys().keys() {
            assert!(
                NAMESPACES.iter().any(|ns| k.starts_with(ns)),
                "🔴 面板源码里的 {k} 开了一个没登记的命名空间 —— 把它加进 \
                 NAMESPACES（扫描不靠这张表，所以这不是安全问题，是登记过期）"
            );
        }
        // 豁免表的过期自检。**按整棵树的原文查**，不走 `referenced_keys()`
        // —— 被豁免掉的字面量按定义不会出现在那份结果里。
        let all: String = AdminAssets::iter()
            .filter(|p| p.ends_with(".js") || p.ends_with(".mjs") || p.ends_with(".html"))
            .map(|p| asset_text(&p))
            .collect();
        for lit in NON_I18N_KEY_LITERALS {
            assert!(
                all.contains(&format!("\"{lit}\"")),
                "🔴 豁免表里的 {lit} 已经不在面板任何源码里了 —— 过期的豁免会在将来\
                 某天静默放掉一个真的漏了的词条，要删掉"
            );
            assert!(
                !dict.contains_key(*lit),
                "🔴 {lit} 既登记成「不是词条」又真的是一条词条 —— 两者只能取一个，\
                 否则这条词条永远不会被「未引用的 key 是硬错」那一格看见"
            );
        }
    }

    // ════════════════════════════════════════════════════════════════════
    // 额外四格 · 面板与 Rust 侧那几处**必然的拷贝**的对账
    //
    // 这四处拷贝都消不掉（经典脚本没有 import、`<meta>` 不解析 CSS 变量、
    // 前后端各跑各的运行时），所以只能靠判据把它们钉在一起。
    // ════════════════════════════════════════════════════════════════════

    /// `js/boot.js` 里那两个存储键字面量与 `pure/storage-keys.mjs` 逐字相同。
    ///
    /// ⚠️ **这处重复是结构性的**：`boot.js` 是全站唯一的经典脚本（`<head>` 里同步、
    /// 非 module，为的是防闪白），**经典脚本没有 `import`**。
    /// 这一格顺带钉住「除 `boot.js` 外任何面板文件都不许再写 `agnes2api_` 字面量」。
    #[test]
    fn boot_js_storage_key_literals_match_the_module() {
        let keys = asset_text("js/pure/storage-keys.mjs");
        let boot = asset_text("js/boot.js");
        for (konst, literal) in [
            ("THEME_STORE", "agnes2api_theme"),
            ("LANG_STORE", "agnes2api_lang"),
        ] {
            assert!(
                keys.contains(&format!("export const {konst} = \"{literal}\";")),
                "storage-keys.mjs 里的 {konst} 不再是 {literal}"
            );
            assert!(
                boot.contains(&format!("\"{literal}\"")),
                "boot.js 里那份必然的副本与模块对不上了（{konst}）"
            );
        }
        for p in AdminAssets::iter().filter(|p| p.ends_with(".js") || p.ends_with(".mjs")) {
            if p.ends_with("js/boot.js") || p.ends_with("pure/storage-keys.mjs") {
                continue;
            }
            assert!(
                !asset_text(&p).contains("agnes2api_"),
                "🔴 {p} 里出现了 agnes2api_ 存储键字面量 —— 键名的单一真源是 \
                 js/pure/storage-keys.mjs。两处分叉实测过：登录成功之后每个请求送空\
                 口令头 ⇒ 401 ⇒ 登出循环，而全套用例照绿"
            );
        }
    }

    /// `<meta name="theme-color">` 的三份拷贝与 `base.css` 的 `--bg-secondary` 同值。
    ///
    /// ⚠️ 拷贝消不掉：`theme-color` 的 content 是给浏览器 chrome 读的**字符串**，
    /// 不走 CSS 变量解析；也不能在切主题那一帧里 `getComputedStyle`（读到的可能是上一套值）。
    #[test]
    fn theme_color_matches_the_css_token() {
        let base = strip_block_comments(&asset_text("css/base.css"));
        let light = token_value(rule_body(&base, "\n:root "), "--bg-secondary");
        let dark = token_value(
            rule_body(&base, "\n[data-theme=\"dark\"] "),
            "--bg-secondary",
        );
        let theme_js = asset_text("js/theme.js");
        assert!(
            theme_js.contains(&format!("THEME_COLOR_LIGHT = \"{light}\"")),
            "js/theme.js 的亮色 theme-color 与 base.css 的 --bg-secondary（{light}）对不上"
        );
        assert!(
            theme_js.contains(&format!("THEME_COLOR_DARK = \"{dark}\"")),
            "js/theme.js 的深色 theme-color 与 base.css 的 --bg-secondary（{dark}）对不上"
        );
        let html = asset_text("index.html");
        assert!(
            html.contains(&format!("name=\"theme-color\" content=\"{light}\"")),
            "index.html 里那条初始 theme-color 与 base.css 的亮色 --bg-secondary（{light}）对不上"
        );
    }

    /// 面板的口令字符集与后端**逐码位等价**。
    ///
    /// 前端这一半必须有：后端只在启动时看得到自己的口令，看不到用户粘了什么；
    /// 送出去也没用，而且会在服务端留下一条无意义的登录失败记录。
    /// 🔴 **后端是真源**（`src/server/middleware/admin_auth.rs`），这里钉的是
    /// 「浏览器那一份没有偷偷收窄或放宽」。
    #[test]
    fn the_panel_charset_matches_the_backend_byte_for_byte() {
        let js = asset_text("js/pure/sendable.mjs");
        assert!(
            js.contains(r"/^[\x20-\x7e]*$/"),
            "🔴 js/pure/sendable.mjs 的字符集正则变了 —— 后端那条是 (0x20..=0x7e)，\
             两边分叉会让一部分口令在前端被拦住而后端本来收，或者反过来"
        );
        // 反向控制：后端那条区间也还在，不是这边单方面对着空气断言。
        let backend = include_str!("../server/middleware/admin_auth.rs");
        assert!(
            backend.contains("(0x20..=0x7e)"),
            "后端的 SENDABLE 区间变了，面板那一份跟着要改"
        );
    }

    /// 会话 12 小时绝对上限：面板与本仓的常量同值。
    ///
    /// 🔴 这个数字是**要写进部署文档、对运维承诺**的那一种：五语言 DEPLOY 逐份写着
    /// 「面板会在 12 小时后要求重新输入口令」。两处分叉等于文档在撒谎。
    #[test]
    fn session_max_age_matches_the_panel() {
        assert_eq!(SESSION_MAX_AGE_MS, 12 * 3_600_000);
        let js = asset_text("js/pure/session.mjs");
        assert!(
            js.contains("SESSION_MAX_AGE_MS = 12 * 3600000"),
            "面板的会话上限与 Rust 侧的 SESSION_MAX_AGE_MS（{SESSION_MAX_AGE_MS} ms）对不上"
        );
    }

    /// 面板的品牌图与 `docs/logo.png` **逐字节相同**（U-23）。
    ///
    /// 理由很朴素：**项目不该在 README 与面板上长着两副面孔。**
    /// 推荐的处置就是这一种（同一份文件 + 一格字节相等判据），而不是引一个 PNG
    /// 解码依赖去做降采样对账。
    #[test]
    fn the_panel_logo_is_the_same_file_as_the_docs_logo() {
        let panel = AdminAssets::get(LOGO_PATH).unwrap().data.into_owned();
        let docs = include_bytes!("../../docs/logo.png");
        assert_eq!(
            panel.as_slice(),
            docs.as_slice(),
            "🔴 admin-ui/logo.png 与 docs/logo.png 不是同一份字节 —— \
             项目会在 README 与面板上长着两副面孔"
        );
    }

    // ════════════════════════════════════════════════════════════════════
    // F 组 · 面板**行为**上那几条一直没人守的纪律
    //
    // 这一组和上面各组不同：它钉的不是「数据对不对」，而是**几条只写在注释里、
    // 靠下一个人读到才成立**的纪律。它们每一条都真的失效过一次，而失效时
    // 整套用例是绿的 —— 那正是「这条规矩没有判据」的定义。
    //
    // ⚠️ 形态与 `boot_js_storage_key_literals_match_the_module` 同源：
    // **对面板源码做字符串扫描**。字符串扫描钉不住语义，只钉得住形状；
    // 所以每一格都写清楚它到底钉住了什么、没钉住什么，
    // **钉不住的那几条一条判据都不写**（写一格假装钉住了的判据比没有更坏：
    // 它会让下一个人以为这件事已经有人管了）。
    // ════════════════════════════════════════════════════════════════════

    /// 去掉 JS 的**行注释**（`//` 到行尾）。
    ///
    /// 🔴 **这一步和剥块注释是同一条纪律，而且这一组判据里它是必需的。**
    /// 本仓的行注释逐字写着被讨论的那些代码形状 ——
    /// `js/app.js` 里写着「上一版这里第一行就是 `if (!res.ok) → offline`」，
    /// `js/sec-apikeys.js` 里写着「这里是 `!cap.plaintextRetrievable`」——
    /// 不剥行注释的扫描会把**在解释为什么不能那么写的那句话**当成**违规**报上来。
    /// 那种假阳性会逼着下一个人去删注释，正好删掉最该留的东西。
    ///
    /// ⚠️ **`://` 要放过**：URL 里的双斜杠不是注释（`index.html` 的 GitHub 链接、
    /// 各处 `https://` 都会中招）。切片只落在 ASCII 的 `/` 上，中文不会被切坏。
    fn strip_line_comments(src: &str) -> String {
        src.lines()
            .map(|line| {
                let b = line.as_bytes();
                for i in 0..b.len().saturating_sub(1) {
                    if b[i] == b'/' && b[i + 1] == b'/' && (i == 0 || b[i - 1] != b':') {
                        return &line[..i];
                    }
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 面板里全部 `.js` / `.mjs` 的（文件名, **块注释与行注释都剥掉后**的正文）。
    fn panel_scripts() -> Vec<(String, String)> {
        AdminAssets::iter()
            .filter(|p| p.ends_with(".js") || p.ends_with(".mjs"))
            .map(|p| {
                (
                    p.to_string(),
                    strip_line_comments(&strip_block_comments(&asset_text(&p))),
                )
            })
            .collect()
    }

    /// 一份面板脚本，块注释与行注释都剥掉。
    fn panel_script(path: &str) -> String {
        strip_line_comments(&strip_block_comments(&asset_text(path)))
    }

    /// JS 源码里所有写成字面量的 class 串：`class: "…"` 与 `setAttribute("class", "…")`。
    fn class_literals(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        for opener in ["class: \"", "\"class\", \""] {
            let mut from = 0;
            while let Some(at) = src[from..].find(opener) {
                let start = from + at + opener.len();
                match src[start..].find('"') {
                    Some(end) => {
                        out.push(src[start..start + end].to_string());
                        from = start + end;
                    }
                    None => break,
                }
            }
        }
        out
    }

    /// 一个 JS 函数的函数体在源码里的字节区间（按花括号配对取）。
    fn js_fn_range(src: &str, header: &str) -> std::ops::Range<usize> {
        let at = src
            .find(header)
            .unwrap_or_else(|| panic!("面板源码里找不到 `{header}` —— 判据钉的那个落点改名了"));
        let open = at + src[at..].find('{').expect("函数头后面没有 {");
        let mut depth = 0i32;
        for (i, ch) in src.as_bytes().iter().enumerate().skip(open) {
            match ch {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return open + 1..i;
                    }
                }
                _ => {}
            }
        }
        panic!("`{header}` 的函数体没有闭合");
    }

    /// 一个 JS 函数的函数体。
    fn js_fn_body(src: &str, header: &str) -> String {
        src[js_fn_range(src, header)].to_string()
    }

    /// F-1 🔴 「不知道 / 没有 / 约等于」这三个**标记类**必须独占一个元素。
    ///
    /// ── 它为什么必须有判据 ───────────────────────────────────────────────────
    ///
    /// `.usage-unknown`（EM DASH，这一块**没读出来**）与 `.usage-none`（EN DASH，
    /// 读成功了、真的没有样本）是两件完全不同的事，而它们**只由 `color` + `font-weight`
    /// 区分**。把标记类和宿主格子的类并到同一个元素上时：
    /// 两者特指度完全相同（都是单个类），而 `sections-stats.css` 在 `index.html` 里
    /// **后加载** ⇒ 宿主那条 `color: var(--text-primary)` / `font-weight: 700`
    /// 把标记那条整条覆盖掉 ⇒ 屏幕上两种状态**同色同粗**，只剩破折号长度这一点差别。
    /// 🔴 这条路径在 `js/sec-dashboard.js` 上**真的失效过**（三格全中），
    ///    而当时没有任何一格判据会红。
    ///
    /// ── 它钉住了什么、没钉住什么 ─────────────────────────────────────────────
    ///
    /// 钉住：**类串字面量里这三个类必须是独苗**。这正好挡住那次失效的形状
    /// （`setAttribute("class", "dash-stat-value usage-unknown")`）。
    /// 没钉住：`classList.add()` 分两次加上去的同一件事。那需要数据流分析，
    /// 字符串扫描做不到 —— 如实写在这里，别以为这一格全包了。
    ///
    /// ⚠️ **`.usage-incomplete` 不在这张表里，而且必须不在。**
    /// 它只声明 `text-decoration: underline dotted`，宿主的 `color` / `font-weight`
    /// 覆盖不掉它 ⇒ 它**就该**和宿主并在一个元素上
    /// （`js/sec-dashboard.js` 的 `"dash-stat-value usage-incomplete"` 是对的，不是漏的）。
    #[test]
    fn the_two_dash_markers_never_share_an_element_with_their_host() {
        const MARKER_ONLY: &[&str] = &["usage-unknown", "usage-none", "approx"];
        let mut seen = 0usize;
        for (name, src) in panel_scripts() {
            for literal in class_literals(&src) {
                let classes: Vec<&str> = literal.split_whitespace().collect();
                for marker in MARKER_ONLY {
                    if !classes.contains(marker) {
                        continue;
                    }
                    seen += 1;
                    assert_eq!(
                        classes.as_slice(),
                        [*marker],
                        "🔴 {name} 把标记类 `{marker}` 和别的类并到了同一个元素上：\
                         class=\"{literal}\"。\n   \
                         两者特指度相同、而板块 CSS 后加载 ⇒ 宿主的 color / font-weight \
                         会把标记那条整条盖掉，屏幕上「不知道」与「没有」**同色同粗**。\n   \
                         修法：照 `markText()` / `unknownCell()` 的形态，把破折号包进\
                         **它自己的 `<span>`** 再挂进宿主格子"
                    );
                }
            }
        }
        assert!(
            seen >= 7,
            "只扫到 {seen} 处标记类字面量 —— class 字面量的抽取坏了，这一格正在空跑"
        );
    }

    /// F-2 🔴 横幅必须是**两层**：出现 `banner-*` 的地方，前面必须紧邻一个 `banner`。
    ///
    /// `css/base.css` 把横幅拆成 `.banner`（padding / border / 4px 左色条 / radius /
    /// margin / 字号 —— **横幅之所以看起来是横幅，全在这一层**）与
    /// `.banner-warn|-info|-danger`（只有底色与左色条的色）。
    /// 只写后者时屏幕上得到的是一行**裸文字**：没有底、没有边、没有那条色条，
    /// 与它上下的普通段落长得一模一样 —— 而横幅承载的恰恰是「这一块读失败了」
    /// 这类必须一眼看见的话。
    /// 🔴 `js/sec-registrar.js` 的六处横幅**曾经全部漏掉 `.banner` 那一层**。
    ///
    /// ⚠️ 这一格**按整份源码扫**，不只扫 `class:` 字面量 —— 因为横幅类名还会出现在
    /// `js/sec-usage.js` 的 `BANNER_CLASS` 映射表和 `b.className = …` 这类赋值里。
    #[test]
    fn every_banner_class_carries_its_base_layer() {
        let mut seen = 0usize;
        for (name, src) in panel_scripts() {
            let mut from = 0;
            while let Some(at) = src[from..].find("banner-") {
                let at = from + at;
                seen += 1;
                let before = &src[..at];
                assert!(
                    before.ends_with("banner "),
                    "🔴 {name} 里的 `{}…` 前面不是 `banner ` —— 横幅漏了基础层。\n   \
                     `.banner-*` 只给底色，**padding / 边框 / 那条 4px 左色条全在 `.banner` 上**\
                     ⇒ 少了它，屏幕上是一行和正文没有任何区别的裸字。\n   \
                     正确形态逐字是 \"banner banner-danger\" 这样的两层",
                    &src[at..(at + 16).min(src.len())]
                );
                from = at + "banner-".len();
            }
        }
        assert!(
            seen >= 20,
            "只扫到 {seen} 处 banner- —— 扫描坏了，这一格正在空跑"
        );
    }

    /// F-3 🔴 登出**必须把九个板块的 DOM 整份拆掉，并且把 `__inited` 一起放倒**。
    ///
    /// ── 两件事，缺一不可 ─────────────────────────────────────────────────────
    ///
    /// ① **清 DOM**：运维在「API 管理」里点过「显示明文」之后，那把 `sk-…` 已经写进了
    ///    `<code class="reveal-text">`。`leave()` 只跑当前板块的 `onHide()`，而那几个
    ///    `onHide()` 清的都只是 JS 那份 `Map` ⇒ **明文原样躺在 `#sec-apikeys` 里**，
    ///    `display:none` 只是看不见，F12 一开就在屏幕上。
    ///    **登出的语义就是「让下一个人看不到」，这是它唯一的用处。**
    /// ② **放倒 `__inited`**：九个板块里有四个（models / playground / registrar /
    ///    apikeys）是在 `init()` 里建骨架、把子节点存进模块级 `nodes` 的。
    ///    只清 DOM 不重置这一格，它们下次进来会往一棵**已经脱离文档**的子树上渲染
    ///    —— 屏幕上是一张永远空白的板块，而控制台里一个字都没有。
    ///
    /// ── 它钉住了什么 ─────────────────────────────────────────────────────────
    ///
    /// 钉住：`leave()` 真的调了 `teardownSections()`；后者两件事都做，
    /// 而且**两个循环都是泛化的**（`SECTION_NAMES` / `loaded.values()`）——
    /// 泛化正是「新板块自动被覆盖」这半句的落点：判据不必认识将来那个板块的名字。
    /// 没钉住：`init()` 里建的骨架有没有别的模块级状态需要一起重置。
    #[test]
    fn logging_out_tears_down_every_section_and_resets_init() {
        let app = panel_script("js/app.js");

        let leave = js_fn_body(&app, "function leave(");
        assert!(
            leave.contains("teardownSections()"),
            "🔴 js/app.js 的 leave() 不再调 teardownSections() —— 登出只是盖了一张\
             登录闸上去，上一个人点开过的明文原样留在 DOM 里"
        );

        let teardown = js_fn_body(&app, "function teardownSections(");
        assert!(
            teardown.contains("for (const name of SECTION_NAMES)")
                && teardown.contains("textContent = \"\""),
            "🔴 teardownSections() 不再**泛化地**清空九个板块节点。\n   \
             这里必须是 `for (const name of SECTION_NAMES)` 那种走全表的形态：\
             逐个板块点名的写法会在下一个板块加进来时静默漏掉它，\
             而漏掉的表现是「明文留在 DOM 里」——屏幕上什么都看不出来。\n   \
             现在的函数体是：{teardown}"
        );
        assert!(
            teardown.contains("for (const sec of loaded.values())")
                && teardown.contains("__inited = false"),
            "🔴 teardownSections() 清了 DOM 却没把 `__inited` 一起放倒。\n   \
             models / playground / registrar / apikeys 四个板块是在 init() 里建骨架的：\
             只清 DOM 不重置这一格 ⇒ 它们下次进来往一棵**已经脱离文档**的子树上渲染，\
             屏幕上是一张永远空白的板块，控制台里一个字都没有。\n   \
             现在的函数体是：{teardown}"
        );
    }

    /// F-4 🔴 `/health` **非 2xx 不许短路掉读响应体那一步**。
    ///
    /// `src/server/routes/health.rs` 在存储不可写时发的是 **503 + `status:"degraded"`**，
    /// 而那个 503 是**故意**的（只有非 2xx，镜像内置的 HEALTHCHECK 才会把容器标成
    /// unhealthy）。面板这里上一版第一行就是 `if (!res.ok) → offline`
    /// ⇒ **唯一一条会产出 `degraded` 的真实路径被整条吃掉**，
    /// `paintHealth("degraded")` 与 `.is-degraded` 那一档成了永远走不到的死代码。
    ///
    /// 后果不是少一个颜色：典型场景是 Docker 绑定挂载的属主不对 ⇒ 运维看到顶栏写着
    /// 「探测失败」，于是去查网络、查反代、查端口，而真相是**存储写不进去**。
    ///
    /// ⚠️ 这一格钉的是**顺序**：`res.json()` 必须出现在第一个 `!res.ok` 之前。
    /// 顺序是这件事唯一可被字符串看见的形状，而它恰好就是那次回归的形状。
    #[test]
    fn the_health_probe_reads_the_body_before_it_looks_at_the_status() {
        let app = panel_script("js/app.js");
        let body = js_fn_body(&app, "async function refreshHealth(");
        let read_at = body
            .find("res.json()")
            .expect("🔴 refreshHealth() 里不再读响应体 —— degraded 那一档没有别的来源");
        let status_at = body
            .find("!res.ok")
            .expect("🔴 refreshHealth() 里不再看状态码 —— 真的探不到时会被当成「状态未知」");
        assert!(
            read_at < status_at,
            "🔴 js/app.js 的 refreshHealth() 又在读响应体**之前**先看 `!res.ok` 了。\n   \
             /health 用 **503 + status:\"degraded\"** 表达「服务在、存储写不进去」\
             （503 是故意的，HEALTHCHECK 靠它）。先看状态码 ⇒ 那条路径被整条吃掉，\
             `.is-degraded` 变成死代码，运维只会看到「探测失败」然后去查网络。\n   \
             顺序必须是：读 body → 先认它自己说的话 → 再落到状态码"
        );
    }

    /// F-5 🔴 `/capabilities` 那几格的 `null`（**还不知道**）不许被 `!` 当成 `false` 吃掉。
    ///
    /// ── 先说清楚这一格**没有**钉住什么 ───────────────────────────────────────
    ///
    /// 「`null` 是不是被当成结论用了」是**语义**问题，字符串扫描看不见。
    /// 而且这条纪律**本来就不是一刀切**：`js/sec-apikeys.js` 里两处 `cap.plaintextRetrievable`
    /// 的判据是**刻意相反**的，各自倒向安全的那一侧 ——
    ///   · `revealCell()` 用 `=== false`：`null` ⇒ **把按钮留着**（不知道就别替服务端下结论）；
    ///   · 新建 Key 那张弹窗用 `!cap.…`：`null` ⇒ **照样警告**（代价只是多复制一次，
    ///     反过来的代价是关掉这一屏之后明文可能真的没了）。
    /// ⇒ 一格「禁止出现 `!cap.`」的判据是**错的**，它会把上面第二条正确的写法判红。
    ///
    /// ── 那它钉住了什么 ───────────────────────────────────────────────────────
    ///
    /// 两件能被字符串看见、而且恰好是那次回归形状的事：
    /// ① `revealCell()` 这个**危险的那一侧**必须是 `=== false`，并且它的函数体里
    ///    **一个 `!cap.` 都不许有**。上一版这里逐字是 `if (!cap.plaintextRetrievable || …)`
    ///    ⇒ 一次 `/capabilities` 读失败让「显示明文 / 复制」两颗按钮**静默消失**，
    ///    运维看到的是一行没有任何操作、也没有任何解释的掩码。
    /// ② 全站其余的 `!cap.` 必须**逐处登记**。新写一处会红，而红的时候要求写的是
    ///    「为什么这一处 `null` 倒向警告是安全的那一侧」——把默认值从「随手写」改成「要解释」。
    #[test]
    fn the_capability_tristate_is_not_flattened_where_it_is_dangerous() {
        /// `(文件, 函数, 为什么这一处让 `null` 落进 `!` 那一支是安全的)`
        const NULL_MAY_WARN: &[(&str, &str, &str)] = &[(
            "js/sec-apikeys.js",
            "function showSecret(",
            "新建 Key 那张弹窗：null（还不知道能不能再取回）**照样警告**。\
             代价是运维多复制一次；反过来（不知道就不警告）的代价是关掉这一屏之后\
             明文可能真的没了。不确定时不做那个不可挽回的选择",
        )];

        let apikeys = panel_script("js/sec-apikeys.js");
        let reveal = js_fn_body(&apikeys, "function revealCell(");
        assert!(
            reveal.contains("cap.plaintextRetrievable === false"),
            "🔴 revealCell() 不再用 `=== false` 判「这个部署真的不留明文」。\n   \
             `null` 是「/capabilities 这一次没读出来」，**不是一条结论**；\
             它一旦落进这一支，一次读失败就会让「显示明文 / 复制」两颗按钮静默消失，\
             运维看到的是一行没有任何操作、也没有任何解释的掩码。\n   \
             现在的函数体是：{reveal}"
        );
        assert!(
            !reveal.contains("!cap."),
            "🔴 revealCell() 里出现了 `!cap.…` —— 这正是上一版那条 \
             `if (!cap.plaintextRetrievable || …)` 的形状：它把 `false`（真的不留明文）\
             与 `null`（还不知道）并成一档，而这两件事对运维的下一步完全不同"
        );

        // 其余的 `!cap.` 必须逐处登记，并且登记项不许过期。
        for (file, header, _) in NULL_MAY_WARN {
            let src = panel_script(file);
            assert!(
                js_fn_body(&src, header).contains("!cap."),
                "🔴 登记表里的 {file} / {header} 已经不再写 `!cap.` 了 —— \
                 过期的登记项会在将来静默放行一处真的把 null 吃掉的写法，要删掉"
            );
        }
        for (name, src) in panel_scripts() {
            // 这份文件里所有被登记过的函数体区间。
            let allowed: Vec<std::ops::Range<usize>> = NULL_MAY_WARN
                .iter()
                .filter(|(f, _, _)| *f == name)
                .map(|(_, header, _)| js_fn_range(&src, header))
                .collect();
            let mut from = 0;
            while let Some(at) = src[from..].find("!cap.") {
                let at = from + at;
                from = at + "!cap.".len();
                let registered = allowed.iter().any(|r| r.contains(&at));
                assert!(
                    registered,
                    "🔴 {name} 里出现了一处没登记的 `!cap.…`（{}）。\n   \
                     `cap.*` 是**三态**：true / false / null（这一次没读出来）。\
                     `!` 会把后两者并成一档，而它们对运维的下一步常常完全相反。\n   \
                     两条路：① 改成 `=== false`（让 null 落到「不知道」那一支）；\
                     ② 这一处 null 倒向警告确实是安全的那一侧 —— 那就连同**理由**\
                     登记进 NULL_MAY_WARN",
                    &src[at..(at + 32).min(src.len())]
                );
            }
        }
    }

    // ──────────────────────────────────────────────────────────────────
    // F-5 的三个扫描件：JS 标识符 / 调用实参区间 / Rust 与 JS 的字符串字面量
    // ──────────────────────────────────────────────────────────────────

    /// JS 标识符里允许出现的字节。
    fn is_js_ident_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
    }

    /// `ident` 作为**完整标识符**出现的所有字节位置。
    ///
    /// ⚠️ 两头都要看：`badgeClass` 不许匹配到 `myBadgeClass` / `badgeClassOf`。
    fn js_ident_positions(src: &str, ident: &str) -> Vec<usize> {
        let b = src.as_bytes();
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(rel) = src[from..].find(ident) {
            let at = from + rel;
            let end = at + ident.len();
            if (at == 0 || !is_js_ident_byte(b[at - 1]))
                && (end >= b.len() || !is_js_ident_byte(b[end]))
            {
                out.push(at);
            }
            from = end;
        }
        out
    }

    /// `callee(…)` 每一次调用**实参那一段**的字节区间。
    ///
    /// ⚠️ **括号配对时要跳过字符串字面量**：实参里出现一个 `"a (b"` 会让裸计数
    /// 从此错位，而错位的结果是把**真的没过翻译**的那几处当成过了 —— 判据一旦
    /// 算错方向，比没有判据更坏。
    fn js_call_arg_ranges(src: &str, callee: &str) -> Vec<std::ops::Range<usize>> {
        let b = src.as_bytes();
        let mut out = Vec::new();
        for at in js_ident_positions(src, callee) {
            let mut open = at + callee.len();
            while open < b.len() && b[open].is_ascii_whitespace() {
                open += 1;
            }
            if open >= b.len() || b[open] != b'(' {
                continue;
            }
            let (mut depth, mut quote, mut j) = (0i32, None::<u8>, open);
            while j < b.len() {
                let c = b[j];
                match quote {
                    Some(q) => {
                        if c == b'\\' {
                            j += 2;
                            continue;
                        }
                        if c == q {
                            quote = None;
                        }
                    }
                    None => match c {
                        b'"' | b'\'' | b'`' => quote = Some(c),
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                out.push(open + 1..j);
                                break;
                            }
                        }
                        _ => {}
                    },
                }
                j += 1;
            }
        }
        out
    }

    /// 一段源码里所有**双引号**字符串字面量的内容（认 `\` 转义）。
    fn double_quoted(src: &str) -> Vec<String> {
        let b = src.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] != b'"' {
                i += 1;
                continue;
            }
            let start = i + 1;
            let mut j = start;
            while j < b.len() {
                if b[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if b[j] == b'"' {
                    break;
                }
                j += 1;
            }
            if j >= b.len() {
                break;
            }
            out.push(src[start..j].to_string());
            i = j + 1;
        }
        out
    }

    /// Rust 源码里的双引号字面量。**先剥掉 `//` 行注释**（同 `strip_line_comments`
    /// 那条纪律：`view/` 那几份的文档注释里逐字写着被讨论的那些类名）。
    fn rust_string_literals(src: &str) -> Vec<String> {
        let stripped = src
            .lines()
            .map(|l| match l.find("//") {
                Some(i) if !l[..i].contains('"') => &l[..i],
                _ => l,
            })
            .collect::<Vec<_>>()
            .join("\n");
        double_quoted(&stripped)
    }

    /// CSS 里**真的有一条规则**的那些类名。
    fn css_class_names() -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for (_, css) in css_files() {
            for (sel, _) in css_rules(&css) {
                let b = sel.as_bytes();
                let mut i = 0;
                while i < b.len() {
                    if b[i] != b'.' {
                        i += 1;
                        continue;
                    }
                    let start = i + 1;
                    let mut j = start;
                    while j < b.len()
                        && (b[j].is_ascii_alphanumeric() || b[j] == b'-' || b[j] == b'_')
                    {
                        j += 1;
                    }
                    if j > start {
                        out.insert(sel[start..j].to_string());
                    }
                    i = j.max(start);
                }
            }
        }
        out
    }

    /// F-5 🔴 **后端 DTO 的徽章类名，必须过 `badgeClass()` 才允许上屏。**
    ///
    /// ── 它为什么必须有判据 ───────────────────────────────────────────────────
    ///
    /// `src/core/admin/view/` 有**四个**徽章类名发射器（`keys::badge_class` /
    /// `apikeys::ak_badge_class` / `events::level_badge_class` /
    /// `models::ProtocolBadge::badge_class`）。其中前三个发的 `badge-ok` /
    /// `badge-warn` 在 `css/base.css` 里**根本不存在**（那边是 `-success` /
    /// `-warning`）⇒ 原样挂上去的话 `.badge` 之外那一半是个**不存在的选择器**，
    /// N 档在屏幕上塌成同一片中性灰。那张别名表收在 `js/pure/badge-class.mjs`，
    /// **全站唯一一份**。
    ///
    /// 🔴 `js/sec-models.js` **真的漏过这一道**：`narrowCatalog()` 把 `bb.badgeClass`
    /// 原样塞进 `cls`，`badgeCell()` 再挂上去，中间一道翻译都没有。它当时不出事
    /// **只因为 `models.rs` 恰好发的是 `badge badge-success` / `badge badge-off`，
    /// 这两个 CSS 里真有** —— 那是巧合，不是保证：`models.rs` 哪天改口发 `badge-ok`
    /// （和另外三个发射器同一套写法），三档就在屏幕上同色，而 Rust 那几格
    /// 「N 档互不相同」的判据**只比字符串、照样全绿**。
    ///
    /// ── 它钉住了什么 ─────────────────────────────────────────────────────────
    ///
    /// **半 A（面板侧：翻了没有）**。面板脚本里每一个标识符 `badgeClass` 都必须落进
    /// 五种形状之一：别名模块的 `import`、`badgeClass(…)` 调用、对象字面量的**键**、
    /// **被 `badgeClass(…)` 括住的属性读**、以及 `typeof x.badgeClass !== "string"`
    /// 这一档窄化守卫（它的结果是 `"string"` 不是类名，上不了屏）。
    /// 🔴 钉的是**读出来的那一刻**，不是上屏的那一刻 —— 所以中间转手几次、
    /// 换几个名字（`cls` / `line.badgeClass`）都逃不掉：不存在「读到了却没翻」的路径。
    /// 解构（`const { badgeClass } = row`）与下标（`row["badgeClass"]`）落进兜底那一支，
    /// 同样会红。
    ///
    /// **半 B（发射面 × 别名表 × CSS：翻完之后屏幕上真有样式没有）**。
    /// `src/core/admin/view/` 能发出去的**每一个** `badge…` 字面量，过完那张
    /// （从 JS 里现读的）别名表之后，逐个类名都必须在 CSS 里**真的有一条规则**。
    /// 只有半 A 的话，后端发一个 `badge-frobnicate` 仍然会静默落到裸 `.badge`。
    ///
    /// ⚠️ 两半都是**两个方向**都钉的：半 A 要求四个消费方**一个不少**地出现
    /// （少一个 = 某个板块又开始原样挂了，或者扫描空跑了）；半 B 要求发射面的字面量集合
    /// 与登记表**逐字相等**，且别名表里**没有死条目**（key 必须真的有发射器在发）。
    ///
    /// ── 它**没有**钉住什么（如实写在这里，别以为这一格全包了）───────────────
    ///
    /// · **字段名叫别的。** 半 A 只认识 `badgeClass` 这一个字段名。哪天后端新开一个
    ///   `toneClass` 往面板发 CSS 类名，这一格**不会红** —— 那要数据流分析，
    ///   字符串扫描做不到。半 B 是这条的对冲（新发射器只要吐 `badge…` 字面量就会被逮住），
    ///   **但它换个前缀（`chip-…`）照样逃得掉**。
    /// · **`classList.add()` 分两次加上去**的同一件事 —— 与 F-1 那格的缺口同源。
    /// · 半 B 的发射面清单是 `include_str!` 的一张**写死的表**，靠与 `view/mod.rs` 的
    ///   `pub mod` 声明逐条比对来防漏（view/ 下新增一份文件会红）；
    ///   **但发射器搬出 `view/` 之后就扫不到了**。
    #[test]
    fn every_dto_badge_class_goes_through_the_alias_table() {
        const IDENT: &str = "badgeClass";
        const ALIAS_MODULE: &str = "js/pure/badge-class.mjs";
        const ALIAS_IMPORT: &str = "import { badgeClass } from \"./pure/badge-class.mjs\";";

        /// 面板侧消费方 ↔ `src/core/admin/view/` 的发射器，一一对应。
        const CONSUMERS: &[(&str, &str)] = &[
            ("js/sec-accounts.js", "view::keys::badge_class"),
            ("js/sec-apikeys.js", "view::apikeys::ak_badge_class"),
            ("js/sec-logs.js", "view::events::level_badge_class"),
            (
                "js/sec-models.js",
                "view::models::ProtocolBadge::badge_class",
            ),
        ];

        /// 发射面：`view/` 下逐份源码。清单与 `view/mod.rs` 的 `pub mod` 逐条比对。
        const VIEW_SOURCES: &[(&str, &str)] = &[
            ("apikeys", include_str!("../core/admin/view/apikeys.rs")),
            ("events", include_str!("../core/admin/view/events.rs")),
            ("keys", include_str!("../core/admin/view/keys.rs")),
            ("models", include_str!("../core/admin/view/models.rs")),
            ("reveal", include_str!("../core/admin/view/reveal.rs")),
            ("usage", include_str!("../core/admin/view/usage.rs")),
        ];

        /// 发射面登记表：后端**今天**能发出去的全部徽章类串。
        const EMITTED: &[&str] = &[
            "badge",
            "badge badge-danger",
            "badge badge-off",
            "badge badge-ok",
            "badge badge-success",
            "badge badge-warn",
        ];

        // ── 半 A：面板脚本里每一处 `badgeClass` 都必须是那五种形状之一 ──────────
        let mut wrapped: BTreeMap<String, usize> = BTreeMap::new();
        let mut guards = 0usize;
        for (name, src) in panel_scripts() {
            if name == ALIAS_MODULE {
                continue;
            }
            let calls = js_call_arg_ranges(&src, IDENT);
            for at in js_ident_positions(&src, IDENT) {
                let end = at + IDENT.len();
                let before = src[..at].trim_end();
                let after = src[end..].trim_start();

                // ① 别名模块的 import。
                let ls = src[..at].rfind('\n').map_or(0, |p| p + 1);
                let le = src[at..].find('\n').map_or(src.len(), |p| at + p);
                if src[ls..le].trim() == ALIAS_IMPORT {
                    continue;
                }
                // ② 调用它自己。
                if after.starts_with('(') {
                    continue;
                }
                // ③ 对象字面量的**键**（值那一半会作为独立的一处被扫到）。
                if after.starts_with(':') && (before.ends_with('{') || before.ends_with(',')) {
                    continue;
                }
                // ④/⑤ 属性读：要么被 `badgeClass(…)` 括着，要么是 `typeof` 窄化守卫。
                if before.ends_with('.') {
                    if calls.iter().any(|r| r.contains(&at)) {
                        *wrapped.entry(name.clone()).or_default() += 1;
                        continue;
                    }
                    let b = src.as_bytes();
                    let mut ps = at;
                    while ps > 0 && (is_js_ident_byte(b[ps - 1]) || b[ps - 1] == b'.') {
                        ps -= 1;
                    }
                    let head = src[..ps].trim_end();
                    let word = head.len() >= "typeof".len()
                        && head.ends_with("typeof")
                        && !head
                            .as_bytes()
                            .get(head.len() - "typeof".len() - 1)
                            .is_some_and(|c| is_js_ident_byte(*c));
                    let cmp = (after.starts_with("!==") || after.starts_with("==="))
                        && after[3..].trim_start().starts_with("\"string\"");
                    if word && cmp {
                        guards += 1;
                        continue;
                    }
                }

                panic!(
                    "🔴 {name} 里有一处 DTO 的 `badgeClass` **没过 `badgeClass()`** 就往下走了：\n   \
                     …{}…\n   \
                     后端四个发射器里有三个发的是 `badge-ok` / `badge-warn`，\
                     而 `css/base.css` 里**没有这两个类**（那边是 `-success` / `-warning`）\
                     ⇒ 原样上屏的话 `.badge` 之外那一半是个不存在的选择器，\
                     N 档在屏幕上塌成同一片中性灰，而 Rust 那几格「N 档互不相同」的判据\
                     只比字符串、照样全绿。\n   \
                     修法：`import {{ badgeClass }} from \"./pure/badge-class.mjs\";`，\
                     **在读出来的那一刻**就翻（不是等到上屏才翻 —— 那样中间转一次手就漏）",
                    &src[at.saturating_sub(48)..(end + 24).min(src.len())]
                );
            }
        }

        let want: BTreeSet<&str> = CONSUMERS.iter().map(|(f, _)| *f).collect();
        let got: BTreeSet<&str> = wrapped.keys().map(String::as_str).collect();
        assert_eq!(
            got, want,
            "🔴 「过了 `badgeClass()` 的消费方」这一组和登记表对不上。\n   \
             少了谁 = 那个板块要么又开始原样挂 DTO 的类名了、要么整个扫描空跑了；\
             多了谁 = 新开了一个消费方却没登记它对应的发射器。\n   \
             登记表（消费方 ↔ 发射器）：{CONSUMERS:?}"
        );
        assert!(
            wrapped.values().sum::<usize>() >= CONSUMERS.len() && guards >= 1,
            "只扫到 {} 处「翻过的属性读」/ {guards} 处 typeof 守卫 —— 扫描件坏了，这一格正在空跑",
            wrapped.values().sum::<usize>()
        );

        // ── 半 B：发射面 × 别名表 × CSS ────────────────────────────────────────
        let declared: BTreeSet<&str> = include_str!("../core/admin/view/mod.rs")
            .lines()
            .filter_map(|l| {
                l.trim()
                    .strip_prefix("pub mod ")
                    .and_then(|s| s.strip_suffix(';'))
            })
            .collect();
        let scanned: BTreeSet<&str> = VIEW_SOURCES.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            scanned, declared,
            "🔴 `view/mod.rs` 声明的模块与这一格 `include_str!` 的清单对不上 —— \
             新加的那一份**没有被扫**，它发什么类名没人管"
        );

        let mut emitted: BTreeSet<String> = BTreeSet::new();
        for (_, src) in VIEW_SOURCES {
            for lit in rust_string_literals(src) {
                if lit == "badge" || lit.starts_with("badge ") || lit.starts_with("badge-") {
                    emitted.insert(lit);
                }
            }
        }
        let registry: BTreeSet<String> = EMITTED.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(
            emitted, registry,
            "🔴 `src/core/admin/view/` 的徽章类名发射面变了。\n   \
             新增一档不是改这张表就完事：它必须**过得了下面那张别名表、并且在 CSS 里\
             真的有一条规则** —— 否则屏幕上它就是一片裸 `.badge` 灰"
        );

        // 别名表从 JS 里**现读**，两边不可能各写一份再漂开。
        let alias_src = panel_script(ALIAS_MODULE);
        let at = alias_src
            .find("const ALIAS = {")
            .expect("`js/pure/badge-class.mjs` 里找不到 `const ALIAS = {` —— 别名表改形状了");
        let end = alias_src[at..].find("};").expect("ALIAS 表没有闭合") + at;
        let flat = double_quoted(&alias_src[at..end]);
        assert!(
            !flat.is_empty() && flat.len().is_multiple_of(2),
            "ALIAS 表读出来是 {flat:?} —— 键值不成对，扫描件坏了"
        );
        let alias: BTreeMap<&str, &str> = flat
            .chunks_exact(2)
            .map(|p| (p[0].as_str(), p[1].as_str()))
            .collect();

        let css = css_class_names();
        let mut used: BTreeSet<&str> = BTreeSet::new();
        for dto in EMITTED {
            for raw in dto.split_whitespace() {
                if let Some(to) = alias.get(raw) {
                    used.insert(raw);
                    assert!(
                        css.contains(*to),
                        "🔴 别名表把 `{raw}` 翻成 `{to}`，而 CSS 里**没有** `.{to}` 这条规则"
                    );
                } else {
                    assert!(
                        css.contains(raw),
                        "🔴 后端发的 `{dto}` 里这个 `{raw}`：别名表不认、CSS 里也没有 `.{raw}` \
                         ⇒ 它上屏之后是个**不存在的选择器**，那一档与「中性灰」不可区分。\n   \
                         两条路：① 在 `js/pure/badge-class.mjs` 的 ALIAS 里给它一条别名；\
                         ② 在 CSS 里补一条真的规则"
                    );
                }
            }
        }
        let dead: Vec<&&str> = alias.keys().filter(|k| !used.contains(**k)).collect();
        assert!(
            dead.is_empty(),
            "🔴 别名表里有死条目 {dead:?} —— 已经没有任何发射器在发它们了。\n   \
             死条目的害处不是多占一行，是它让下一个人以为「这一档有人翻」，\
             而真正在发的那一档可能正没人翻"
        );
    }
}
