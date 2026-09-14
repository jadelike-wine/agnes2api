//! 取「客户端 IP」。**全仓只有一个消费者**：`admin.login_failed` 事件。
//!
//! 🔴 **它不参与鉴权、不参与限速、不参与任何授权判定。**
//! 这条射程限制是本模块存在的前提 —— 下面那些取舍之所以可以接受，正是因为
//! 最坏后果只是「面板上那条失败登录记录写着一个攻击者自己挑的字符串」。
//! 哪天有人想拿它做 IP 白名单或 per-IP 限流，**必须先重新论证下面三条**。

use axum::http::HeaderMap;

/// `CF-Connecting-IP`。
const CF_CONNECTING_IP: &str = "cf-connecting-ip";
/// `X-Forwarded-For`。
const X_FORWARDED_FOR: &str = "x-forwarded-for";

/// IPv6 形态校验的长度上界（`::ffff:255.255.255.255` 一类映射形态 + 区域 id 的余量）。
const IPV6_MAX_LEN: usize = 45;
/// 同上的下界（`::` 就是两个字符）。
const IPV6_MIN_LEN: usize = 2;

/// 取客户端 IP。拿不到时**如实 `None`**。
///
/// # 三条成立前提（🔴 必须与五语言 DEPLOY.md 同步，不是边角提醒）
///
/// **① `TRUST_PROXY` 之外一律 `None`，`CF-Connecting-IP` 也不例外。**
/// 「平台注入、不可伪造」只在请求**真的经过 Cloudflare** 时成立；直连暴露时
/// 客户端自己发一个就算数，而**直连正是 Docker 部署的默认形态**。
///
/// **② ⚠️⚠️ CF 优先于 XFF 这条排序，它的原理由已经没了。**
/// 原话是「Worker 形态下 CF 定义上就在前面」—— P0 砍掉 Worker 之后，
/// 它从「由形态保证的事实」降级成一个**有代价的取舍**：
/// - CF 真在前面（Docker + Cloudflare 代理，最常见）：边缘写入并**覆盖**同名头 ⇒ 伪造不了；
/// - CF **不在**前面（只挂自建 nginx / Caddy）：`CF-Connecting-IP` 没有任何人会覆盖，
///   攻击者自己带一个就会**优先于**反代写的 XFF 胜出。
///
/// ⇒ 🔴 **非 CF 部署必须在反代上把 `CF-Connecting-IP` 剥掉。**
/// 这句话今天是本函数排序成立的前提，不是可选建议。
///
/// **③ 两个头都过形态校验。** 目标是「这个字段不可能承载攻击者的任意文本」，
/// **不是**「它是一个合法 IP」。零校验时实测记下来的是
/// `ip="not-an-ip-at-all <img src=x>"` —— 那条字符串随后会被渲染进面板。
#[must_use]
pub fn client_ip(headers: &HeaderMap, trust_proxy: bool) -> Option<String> {
    // ① 门控。
    if !trust_proxy {
        return None;
    }
    // ② CF 优先。
    if let Some(ip) = header_ip(headers, CF_CONNECTING_IP) {
        return Some(ip);
    }
    // ③ XFF 兜底：取**第一段**（最靠近客户端的那一跳）。
    let raw = headers.get(X_FORWARDED_FOR)?.to_str().ok()?;
    ip_or_none(raw.split(',').next().unwrap_or(""))
}

fn header_ip(headers: &HeaderMap, name: &str) -> Option<String> {
    ip_or_none(headers.get(name)?.to_str().ok()?)
}

/// 形态校验：像 IP 才收下，否则 `None`。**绝不伪造 `"unknown"`**
/// —— 面板显示「—」是诚实的，`"unknown"` 会被当成一个真实来源。
fn ip_or_none(raw: &str) -> Option<String> {
    let v = raw.trim();
    if v.is_empty() {
        return None;
    }
    if is_ipv4(v) || is_ipv6_shaped(v) {
        return Some(v.to_string());
    }
    None
}

/// 严格点分十进制：4 段、每段 1~3 位十进制、值 ≤ 255、**不许前导零**
/// （`010.1.1.1` 在某些解析器里是八进制，两边理解不一致的字段不该被记下来）。
fn is_ipv4(v: &str) -> bool {
    let mut n = 0;
    for part in v.split('.') {
        n += 1;
        if n > 4 {
            return false;
        }
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        if part.len() > 1 && part.starts_with('0') {
            return false;
        }
        if part.parse::<u16>().unwrap_or(u16::MAX) > 255 {
            return false;
        }
    }
    n == 4
}

/// IPv6 **只做形态校验**，不做完整的 RFC 4291 解析。
///
/// 判据：字符集 `[0-9a-f:.]`（大小写不敏感）、长度 2~45、**且必须含冒号**。
/// ⚠️ 「必须含冒号」那一条是承重的：没有它，`1.2.3.4.5` 这种坏 IPv4 会从这条路溜进来。
///
/// **刻意不做完整解析的理由**：完整解析要处理 `::` 压缩、IPv4 映射尾、区域 id，
/// 而本函数的目标只是把「任意文本」挡在外面。多写的那几十行只会多一族自己的边界 bug。
fn is_ipv6_shaped(v: &str) -> bool {
    let len = v.len();
    if !(IPV6_MIN_LEN..=IPV6_MAX_LEN).contains(&len) {
        return false;
    }
    if !v.contains(':') {
        return false;
    }
    v.bytes()
        .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hm(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    /// 🔴 **门控在最前面，`CF-Connecting-IP` 也不例外。**
    ///
    /// 回归对象：直连是 Docker 部署的默认形态，那时这个头是客户端自己写的。
    #[test]
    fn without_trust_proxy_even_the_cloudflare_header_is_ignored() {
        let h = hm(&[(CF_CONNECTING_IP, "203.0.113.7")]);
        assert_eq!(client_ip(&h, false), None);
        assert_eq!(client_ip(&h, true), Some("203.0.113.7".into()));
    }

    /// CF 头优先于 XFF。⚠️ 这条排序的前提写在函数文档里（非 CF 部署必须剥头）。
    #[test]
    fn the_cloudflare_header_wins_over_forwarded_for() {
        let h = hm(&[
            (CF_CONNECTING_IP, "203.0.113.7"),
            (X_FORWARDED_FOR, "198.51.100.9"),
        ]);
        assert_eq!(client_ip(&h, true), Some("203.0.113.7".into()));
    }

    /// XFF 取**第一段**，并且两侧空白被 trim 掉。
    #[test]
    fn forwarded_for_takes_the_first_hop() {
        let h = hm(&[(X_FORWARDED_FOR, " 198.51.100.9 , 10.0.0.1 ")]);
        assert_eq!(client_ip(&h, true), Some("198.51.100.9".into()));
    }

    /// 🔴 **形态不对就当没有，绝不把任意文本记下来。**
    ///
    /// 回归对象：实测记下来过 `ip="not-an-ip-at-all <img src=x>"`，
    /// 而这条字符串会被渲染进面板的失败登录列表。
    #[test]
    fn arbitrary_text_never_becomes_an_ip() {
        for bad in [
            "not-an-ip-at-all <img src=x>",
            "",
            "   ",
            "1.2.3",
            "1.2.3.4.5",
            "256.1.1.1",
            "010.1.1.1",
            "abc",
            "1.2.3.4 or 1=1",
        ] {
            let h = hm(&[(CF_CONNECTING_IP, bad)]);
            assert_eq!(client_ip(&h, true), None, "不该收下 {bad:?}");
        }
    }

    /// ⚠️ **坏头不许让好头失效**：CF 头形态不对时照常回落到 XFF。
    #[test]
    fn a_malformed_cloudflare_header_falls_through_to_forwarded_for() {
        let h = hm(&[
            (CF_CONNECTING_IP, "junk"),
            (X_FORWARDED_FOR, "198.51.100.9"),
        ]);
        assert_eq!(client_ip(&h, true), Some("198.51.100.9".into()));
    }

    /// IPv6 只做形态校验：像的收下、不像的挡住。
    #[test]
    fn ipv6_is_shape_checked_not_parsed() {
        assert!(is_ipv6_shaped("::1"));
        assert!(is_ipv6_shaped("2001:db8::8a2e:370:7334"));
        assert!(is_ipv6_shaped("::ffff:192.0.2.1"));
        // **必须含冒号** —— 少了这一条，坏 IPv4 会从这里溜进来。
        assert!(!is_ipv6_shaped("1.2.3.4.5"));
        assert!(!is_ipv6_shaped("2001:db8::<script>"));
        assert!(!is_ipv6_shaped(&"a:".repeat(40)), "超过 45 字符要挡住");
    }

    /// 拿不到就是 `None`，**不是 `"unknown"`**。
    #[test]
    fn absence_is_reported_as_absence() {
        assert_eq!(client_ip(&HeaderMap::new(), true), None);
    }
}
