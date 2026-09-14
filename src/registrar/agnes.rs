//! Agnes 站点的四步：要码 → 注册 → 登录 → 建 key。
//!
//! 移植自 TS 版 `src/core/registrar/agnes.ts`（195 行），规则真源
//! `docs/porting/01-core-rules.md §10.8`。
//!
//! ⚠️ **`platform_url` 与网关转发用的 `agnes_base_url` 不是同一个服务。**
//!
//! # 🔴 四步统一返回 `{ status, body }`，一步都不许直接把正文解析掉
//!
//! 直接解析的那一版**把正文本身吃掉了**，解析失败时只剩一个空值 ——
//! **而「上游回了什么」正是调用点唯一能拿去归因的东西**。
//!
//! 🔴 上一版这三步分别返回 `bool` / `Option<String>` / `Option<String>` ——
//! **状态码与正文在函数边界就被丢光了**，于是三条事件只写得出一句
//! 「Agnes 注册被拒」，**四种真因在面板上长得一模一样**。
//!
//! ⚠️ **正文在这里不截断、不脱敏**：那是调用点的职责
//!（**它才知道这一步有哪些凭据在场**）。**本模块只负责把证据完整交出去。**

use super::fetch::fetch_channel;
use super::mint::{AgnesSite, Reply};
use super::types::REGISTRAR_REQUEST_TIMEOUT_MS;
use crate::ports::fetcher::{Fetcher, OutboundRequest};
use async_trait::async_trait;
use bytes::Bytes;

/// 口令字母表。**去掉了易混字符 `l` `o` `O` `I` `0` `1`。**
const PASSWORD_ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// 随机口令：14 位 + **固定后缀 `Q!`**。
///
/// 🔴 **那个后缀不是装饰，它满足上游的复杂度要求**（至少一个大写 + 一个特殊字符）。
/// 去掉它的后果不是「口令弱一点」，而是**注册那一步被上游拒掉** ——
/// 而故障会显示成 `register_failed`，**没人看得出真因是口令不合格**。
///
/// **账号密码到此为止，不返回也不持久化。**
pub fn random_password(rand: &dyn Fn() -> f64) -> String {
    let mut s = String::with_capacity(16);
    for _ in 0..14 {
        let i = (rand() * PASSWORD_ALPHABET.len() as f64) as usize;
        s.push(PASSWORD_ALPHABET[i.min(PASSWORD_ALPHABET.len() - 1)] as char);
    }
    s.push_str("Q!");
    s
}

/// 令牌可能在的四个位置，**四处都要认**。
const TOKEN_PATHS: [(&str, &str); 4] = [
    ("data", "access_token"),
    ("data", "token"),
    ("", "access_token"),
    ("", "token"),
];
/// key 可能在的两个位置。
const KEY_PATHS: [(&str, &str); 2] = [("data", "key"), ("", "key")];

/// 从 `{...}` 或 `{"data": {...}}` 里按给定的几个位置挖一个字符串。
///
/// ⚠️ **非 JSON 正文（网关超时 / 维护页会以 200 返回）按「取不到」处理，不报错。**
fn pick(body: &str, paths: &[(&str, &str)]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    for (scope, name) in paths {
        let target = if scope.is_empty() {
            Some(&v)
        } else {
            v.get(scope)
        };
        if let Some(t) = target
            && let Some(s) = t.get(*name).and_then(serde_json::Value::as_str)
            && !s.is_empty()
        {
            return Some(s.to_string());
        }
    }
    None
}

/// 从一份正文里挖令牌。`None` 有两种形态，**靠 `status` 分**：
/// 非 2xx（上游拒了）与 2xx 却没认出令牌（上游改了字段名）。
pub fn pick_token(body: &str) -> Option<String> {
    pick(body, &TOKEN_PATHS)
}

/// 从一份正文里挖 key。
pub fn pick_key(body: &str) -> Option<String> {
    pick(body, &KEY_PATHS)
}

/// Agnes 站点的真实实现。
pub struct RealAgnes<'a> {
    pub fetcher: &'a dyn Fetcher,
    /// ⚠️ **与网关转发用的那个地址不是同一个服务。**
    pub platform_url: String,
}

impl RealAgnes<'_> {
    fn req(&self, method: &str, path: &str, lang: &str, body: Option<Bytes>) -> OutboundRequest {
        OutboundRequest {
            method: method.into(),
            url: format!("{}{path}", self.platform_url.trim_end_matches('/')),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "application/json".into()),
                // ⚠️ 四步的 `x-user-language` **不是同一个值**（要码与建 key 是 `zh-CN`，
                // 注册与登录是 `zh`）—— 照抄上游实测到的形态，别「统一」它。
                ("x-user-language".into(), lang.into()),
            ],
            body,
            // 🔴 注册链上**每一个**请求都受这一个超时约束。见 `types` 里那段说明：
            // 一次挂起的请求就能把单轮拖过补池锁的 TTL ⇒ 两轮同时占邮箱名额。
            timeout_ms: REGISTRAR_REQUEST_TIMEOUT_MS,
        }
    }

    async fn call(&self, req: OutboundRequest, action: &str) -> Result<Reply, String> {
        // **唯一出口** —— 脱敏落在这里，不在各个调用点。
        let r = fetch_channel(self.fetcher, req, "agnes", action).await?;
        Ok(Reply {
            status: r.status,
            body: r.body,
        })
    }
}

#[async_trait]
impl AgnesSite for RealAgnes<'_> {
    async fn send_code(&self, address: &str) -> Result<Reply, String> {
        let path = format!(
            "/api/verification?email={}&purpose=register",
            urlencode(address)
        );
        self.call(self.req("GET", &path, "zh-CN", None), "要验证码")
            .await
    }

    async fn register(&self, address: &str, code: &str, password: &str) -> Result<Reply, String> {
        let body = serde_json::json!({
            "email": address,
            "password": password,
            // ⚠️ 上游要两遍口令，字段名是 `password_confirm`。
            "password_confirm": password,
            "code": code,
        });
        self.call(
            self.req("POST", "/api/user/register", "zh", Some(json_body(&body))),
            "注册",
        )
        .await
    }

    async fn login(&self, address: &str, password: &str) -> Result<Reply, String> {
        // ⚠️ 登录这一步的字段名是 `username` 而不是 `email`。
        let body = serde_json::json!({ "username": address, "password": password });
        self.call(
            self.req("POST", "/api/user/login", "zh", Some(json_body(&body))),
            "登录",
        )
        .await
    }

    async fn create_key(&self, token: &str, name: &str) -> Result<Reply, String> {
        let body = serde_json::json!({ "name": name });
        let mut req = self.req("POST", "/api/token", "zh-CN", Some(json_body(&body)));
        req.headers
            .push(("authorization".into(), format!("Bearer {token}")));
        self.call(req, "建 key").await
    }
}

fn json_body(v: &serde_json::Value) -> Bytes {
    Bytes::from(v.to_string())
}

/// 只转义会出现在邮箱地址里、且在查询串里有歧义的那几个字符。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::fetcher::{BodyStream, FetchError, OutboundResponse};
    use std::sync::Mutex;

    struct Recorder {
        seen: Mutex<Vec<OutboundRequest>>,
        reply: Mutex<(u16, String)>,
    }
    #[async_trait]
    impl Fetcher for Recorder {
        async fn send(&self, req: OutboundRequest) -> Result<OutboundResponse, FetchError> {
            let (status, body) = self.reply.lock().unwrap().clone();
            self.seen.lock().unwrap().push(req);
            let s: BodyStream = Box::pin(futures_util::stream::iter(vec![Ok(Bytes::from(body))]));
            Ok(OutboundResponse {
                status,
                headers: vec![],
                body: s,
            })
        }
    }
    fn rec() -> Recorder {
        Recorder {
            seen: Mutex::new(Vec::new()),
            reply: Mutex::new((200, "{}".into())),
        }
    }
    fn site(r: &Recorder) -> RealAgnes<'_> {
        RealAgnes {
            fetcher: r,
            platform_url: "https://p.test/".into(),
        }
    }
    fn header(req: &OutboundRequest, name: &str) -> Option<String> {
        req.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }
    fn body_of(req: &OutboundRequest) -> serde_json::Value {
        serde_json::from_slice(req.body.as_ref().unwrap()).unwrap()
    }

    // ── 四步各自的形状 ────────────────────────────────────────────────

    #[tokio::test]
    async fn the_verification_step_encodes_the_address_into_the_query() {
        let r = rec();
        site(&r).send_code("u0+tag@a.test").await.unwrap();
        let seen = r.seen.lock().unwrap();
        assert_eq!(seen[0].method, "GET");
        assert_eq!(
            seen[0].url, "https://p.test/api/verification?email=u0%2Btag%40a.test&purpose=register",
            "🔴 `+` 与 `@` 没转义的话，上游收到的是另一个地址 —— \
             验证码会发去一个我们收不到的邮箱，表现成「等码超时」",
        );
        assert_eq!(
            header(&seen[0], "x-user-language").as_deref(),
            Some("zh-CN")
        );
    }

    #[tokio::test]
    async fn the_register_step_sends_the_password_twice_under_the_expected_names() {
        let r = rec();
        site(&r)
            .register("u0@a.test", "143770", "pw")
            .await
            .unwrap();
        let seen = r.seen.lock().unwrap();
        assert_eq!(seen[0].url, "https://p.test/api/user/register");
        let b = body_of(&seen[0]);
        assert_eq!(b["email"], "u0@a.test");
        assert_eq!(b["code"], "143770");
        assert_eq!(b["password"], "pw");
        assert_eq!(
            b["password_confirm"], "pw",
            "上游要两遍口令，少一遍会被判成参数缺失",
        );
        assert_eq!(header(&seen[0], "x-user-language").as_deref(), Some("zh"));
    }

    /// ⚠️ 登录这一步的字段名是 `username` 而不是 `email`。
    #[tokio::test]
    async fn the_login_step_uses_username_not_email() {
        let r = rec();
        site(&r).login("u0@a.test", "pw").await.unwrap();
        let b = body_of(&r.seen.lock().unwrap()[0]);
        assert_eq!(b["username"], "u0@a.test");
        assert!(b.get("email").is_none(), "写成 email 的话上游收不到用户名");
    }

    #[tokio::test]
    async fn the_create_key_step_carries_the_bearer_token() {
        let r = rec();
        site(&r).create_key("tk-1", "auto").await.unwrap();
        let seen = r.seen.lock().unwrap();
        assert_eq!(seen[0].url, "https://p.test/api/token");
        assert_eq!(
            header(&seen[0], "authorization").as_deref(),
            Some("Bearer tk-1"),
        );
        assert_eq!(body_of(&seen[0])["name"], "auto");
        assert_eq!(
            header(&seen[0], "x-user-language").as_deref(),
            Some("zh-CN")
        );
    }

    /// 🔴 **每一步都受同一个请求超时约束。**
    ///
    /// 一次挂起的请求就能把单轮拖过补池锁的 TTL ⇒ 下一轮在上一轮还活着时开跑
    /// ⇒ **两轮同时占邮箱名额**；容器这时被重启，正在铸的那个邮箱**就漏了**。
    #[tokio::test]
    async fn every_step_carries_the_request_timeout() {
        let r = rec();
        let s = site(&r);
        s.send_code("u0@a.test").await.unwrap();
        s.register("u0@a.test", "1", "p").await.unwrap();
        s.login("u0@a.test", "p").await.unwrap();
        s.create_key("t", "auto").await.unwrap();
        for req in r.seen.lock().unwrap().iter() {
            assert_eq!(
                req.timeout_ms, REGISTRAR_REQUEST_TIMEOUT_MS,
                "{} 这一步没带超时",
                req.url,
            );
        }
    }

    /// 🔴 **四步统一把状态码与正文完整交出去** —— 一步都不许把正文解析掉。
    #[tokio::test]
    async fn a_rejected_step_still_hands_back_the_status_and_the_whole_body() {
        let r = rec();
        *r.reply.lock().unwrap() = (422, r#"{"msg":"验证码不对"}"#.into());
        let out = site(&r).register("u0@a.test", "000000", "p").await.unwrap();
        assert_eq!(out.status, 422, "状态码在函数边界被丢掉了");
        assert!(
            out.body.contains("验证码不对"),
            "🔴 正文被吃掉了 —— 而「上游回了什么」正是调用点唯一能拿去归因的东西",
        );
    }

    /// 末尾斜杠不许拼出双斜杠。
    #[tokio::test]
    async fn a_trailing_slash_on_the_platform_url_does_not_produce_a_double_slash() {
        let r = rec();
        RealAgnes {
            fetcher: &r,
            platform_url: "https://p.test///".into(),
        }
        .login("u0@a.test", "p")
        .await
        .unwrap();
        assert_eq!(
            r.seen.lock().unwrap()[0].url,
            "https://p.test/api/user/login"
        );
    }

    // ── 令牌的四个位置 ────────────────────────────────────────────────

    /// **`login` 的 token 在四个可能位置之一，四处都要认。**
    #[test]
    fn the_token_is_recognised_in_all_four_places() {
        for (body, why) in [
            (r#"{"data":{"access_token":"tk"}}"#, "data.access_token"),
            (r#"{"data":{"token":"tk"}}"#, "data.token"),
            (r#"{"access_token":"tk"}"#, "顶层 access_token"),
            (r#"{"token":"tk"}"#, "顶层 token"),
        ] {
            assert_eq!(pick_token(body).as_deref(), Some("tk"), "认不出 {why}");
        }
    }

    /// ⚠️ **非 JSON 正文（网关超时 / 维护页会以 200 返回）按「取不到」处理，不报错。**
    #[test]
    fn a_maintenance_page_returned_with_two_hundred_is_treated_as_no_token() {
        assert_eq!(pick_token("<html>502 Bad Gateway</html>"), None);
        assert_eq!(pick_token(""), None);
        assert_eq!(pick_token("null"), None);
    }

    /// 空串不算有效令牌。
    #[test]
    fn an_empty_token_string_does_not_count() {
        assert_eq!(pick_token(r#"{"token":""}"#), None);
        // 但同一份正文里别处有真的就该认出来。
        assert_eq!(
            pick_token(r#"{"token":"","data":{"token":"tk"}}"#).as_deref(),
            Some("tk"),
        );
    }

    #[test]
    fn the_key_is_recognised_in_both_places() {
        assert_eq!(
            pick_key(r#"{"data":{"key":"sk-1"}}"#).as_deref(),
            Some("sk-1")
        );
        assert_eq!(pick_key(r#"{"key":"sk-1"}"#).as_deref(), Some("sk-1"));
        assert_eq!(pick_key(r#"{"data":{"secret_id":"x"}}"#), None);
    }

    // ── 口令 ──────────────────────────────────────────────────────────

    /// 🔴 **那个 `Q!` 后缀不是装饰。**
    ///
    /// 变异对象：把 `s.push_str("Q!")` 删掉 —— 口令里一个大写字母、一个特殊字符都没有，
    /// **上游会拒掉注册**，而故障显示成 `register_failed`，
    /// **没人看得出真因是口令不合格**。
    #[test]
    fn the_password_always_satisfies_the_upstream_complexity_rule() {
        for r in [0.0f64, 0.3, 0.7, 0.999] {
            let p = random_password(&|| r);
            assert!(p.ends_with("Q!"), "少了复杂度后缀：{p}");
            assert!(p.chars().any(|c| c.is_ascii_uppercase()), "没有大写：{p}");
            assert!(
                p.chars().any(|c| !c.is_ascii_alphanumeric()),
                "没有特殊字符：{p}"
            );
            assert_eq!(p.chars().count(), 16, "长度不对：{p}");
        }
    }

    /// **去掉了易混字符** —— 口令要被人从日志/面板里读出来抄一遍的场合虽然没有，
    /// 但这条与上游的字母表保持一致，省掉一类「看起来对却抄错」的排查。
    #[test]
    fn the_alphabet_has_no_easily_confused_characters() {
        for bad in ['l', 'o', 'O', 'I', '0', '1'] {
            assert!(
                !PASSWORD_ALPHABET.contains(&(bad as u8)),
                "字母表里还有易混字符 {bad}",
            );
        }
    }

    /// `rand` 回 1.0（边界）时不许越界 panic。
    #[test]
    fn a_rand_at_the_upper_boundary_does_not_panic() {
        assert_eq!(random_password(&|| 1.0).chars().count(), 16);
    }

    // ── urlencode ─────────────────────────────────────────────────────

    #[test]
    fn the_encoder_leaves_unreserved_characters_alone_and_escapes_the_rest() {
        assert_eq!(urlencode("u0-x_y.z~"), "u0-x_y.z~");
        assert_eq!(urlencode("a+b@c.test"), "a%2Bb%40c.test");
        assert_eq!(urlencode("&=?#"), "%26%3D%3F%23");
        // 多字节按字节转义。
        assert_eq!(urlencode("中"), "%E4%B8%AD");
    }
}
