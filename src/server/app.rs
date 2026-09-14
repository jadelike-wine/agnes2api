//! 路由装配。**中间件的挂载顺序本身是约束。**

use crate::server::errors::GatewayError;
use crate::server::middleware;
use crate::server::routes;
use crate::server::state::AppState;
use axum::Router;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};

/// 装配整棵路由树。
///
/// # 期望的**执行**顺序（由外向内）
///
/// ```text
/// 1. config_refresh   ← 必须在鉴权之前：/v1 的口令就在配置里
/// 2. log_flush        ← 必须在 next() 之后收尾
/// 3. usage_flush      ← 同上；且它比 log_flush 更内层 ⇒ **用量先落、事件后落**
/// 4. nosniff          ← 必须在响应返回的路上加，否则裸响应那一族整条头被丢掉
/// 5. gateway_auth     ← 只管 /v1* 与 /v1beta* 以及三条别名前缀
/// 6. 路由（/health、四协议、媒体、/admin/api）
/// ```
///
/// # ⚠️⚠️ 为什么下面的 `.layer()` 是**倒着**写的
///
/// axum 的 `Router::layer` 是**包裹**：`r.layer(a).layer(b)` == `b(a(r))`
/// ⇒ **最后加的那个在最外层、最先执行**。
/// 这与 Hono 的 `app.use()`（写的顺序就是执行顺序）**方向相反**。
/// 照抄 TS 的行序会得到一条方向相反的链，而它的失效方式是**静默的**。
///
/// ⇒ 下面按「期望执行顺序的**逆序**」写 `.layer()`，并由
/// `middleware_order_is_outermost_first` 那一格判据钉住实际顺序。
///
/// # 🔴 路由表绝不许随运行时配置变化
///
/// 「没接线」「功能没打开」一律是 handler 里的一个状态码，**不是「这条路由不存在」**：
/// - 没接线 → `503 {reason:"not_wired"}`；
/// - 功能关着（Tier-2）→ **`200` + `tier:"off"`**（回 503 会让面板把
///   「运维没打开统计」渲染成「后端坏了」）；
/// - 注册机开着但这份配置没装起来 → `409`。
///
/// **唯一的例外**是 `ADMIN_TOKEN` 未配置时整棵 `/admin` 树不注册（404）——
/// 那是装配期的一次性判定，目的是不泄漏「这里有个后台」。
pub fn build_router(state: AppState) -> Router {
    let has_usage_sink = state.usage_sink.is_some();
    let has_admin = state.admin_token.is_some();

    let mut app = Router::new()
        .merge(routes::health::router())
        .merge(protocol_router())
        .with_state(state.clone());

    if has_admin {
        app = app.merge(admin_router(state.clone()));
        // 🔴 **面板静态树与 `/favicon.ico` 必须进同一个 `if`。**
        // 「没配 `ADMIN_TOKEN` ⇒ 整棵 `/admin` 树不存在」是一条**不泄漏「这里有个
        // 后台」**的性质；把静态树挂在 `if` 外面会当面推翻它 —— 一个 200 的
        // `/admin` 就足以告诉对方这里有管理面。
        // 由 [`crate::webui`] 的 `without_an_admin_token_the_static_tree_is_absent_too` 钉着。
        //
        // ⚠️ 这几条路由**不经过 `admin_auth`**，这是对的：面板本身是一份公开的静态
        // 文档（HTML/CSS/JS，零机密），凭据在浏览器里、由 `/admin/api` 那棵树的闸验。
        // 反过来做（让 HTML 也过闸）会得到一个「拿不到登录页所以永远登不进去」的死锁。
        app = app.merge(crate::webui::router());
    }

    // 兜底：没命中任何路由 ⇒ **JSON 信封的 404**，不是 axum 默认的空体。
    //
    // ⚠️⚠️ **两条兜底挂在两个不同的挂点上，缺一条就漏一半。**
    // `Router::fallback` 只覆盖「**路径**没匹配上」；「路径匹配上了但**方法**不对」
    // 走的是 `Router::method_not_allowed_fallback`。只挂前者时 axum 自己回的 405
    // 是**空体**（连 content-type 都没有），而 405 在 `02 §4.1` 里明列为网关信封的用户。
    //
    // ⚠️ `method_not_allowed_fallback` 是**对当前注册表里每个 method router 逐个打补丁**，
    //    所以它必须写在**所有 `.route()` / `.merge()` / `.nest()` 之后** ——
    //    后注册的路由不会自动带上它。
    let app = app
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed);

    // 请求体上限。**它不是中间件链的一员**：只往请求扩展里塞一个数，
    // 提取器读它，执行顺序上什么都不改变 ⇒ 挂在最内层，不打乱下面那五条的编号。
    // 🔴 不挂的话吃的是 axum 的默认 2 MiB，而那是一条真的功能退化 ——
    //    理由与取值见 [`routes::MAX_REQUEST_BODY_BYTES`]。
    let mut app = app.layer(axum::extract::DefaultBodyLimit::max(
        routes::MAX_REQUEST_BODY_BYTES,
    ));

    // ── 由内向外 ────────────────────────────────────────────────────────────
    // 5
    app = app.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        middleware::auth::gateway_auth,
    ));
    // 4
    app = app.layer(axum::middleware::from_fn(middleware::nosniff::nosniff));
    // 3 —— 🔴 **sink 不存在时这条中间件根本不挂**：
    //        「关 = 零成本」是一条可观测性质，不是「建好再用 if 拦住写」。
    if has_usage_sink {
        app = app.layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::flush::usage_flush,
        ));
    }
    // 2
    app = app.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        middleware::flush::log_flush,
    ));
    // 1
    app.layer(axum::middleware::from_fn_with_state(
        state,
        middleware::config_refresh::config_refresh,
    ))
}

/// 四协议 + 媒体。
///
/// 🔴 **别名与裸路径走的是同一个 handler 函数**（下面每一对的第二行）。
/// 各写一个 handler 的后果是改一处漏一处，而现象是
/// 「换个 base_url 行为就不一样」—— 最难查的那一种。
fn protocol_router() -> Router<AppState> {
    Router::new()
        // ── OpenAI ──────────────────────────────────────────────────────────
        .route("/v1/models", get(routes::openai::models))
        .route("/openai/v1/models", get(routes::openai::models))
        .route(
            "/v1/chat/completions",
            post(routes::openai::chat_completions),
        )
        .route(
            "/openai/v1/chat/completions",
            post(routes::openai::chat_completions),
        )
        // ── Responses ───────────────────────────────────────────────────────
        .route("/v1/responses", post(routes::responses::responses))
        .route("/openai/v1/responses", post(routes::responses::responses))
        // ── Anthropic ───────────────────────────────────────────────────────
        // ⚠️ `/v1/messages/count_tokens` 与 `/v1/messages` 都是静态段，
        //    matchit 不会把前者吃进后者。**但这条性质必须有判据钉住**，
        //    否则将来换匹配器会复发（Hono 那边就是靠注册顺序保证的）。
        .route("/v1/messages", post(routes::anthropic::messages))
        .route("/claude/v1/messages", post(routes::anthropic::messages))
        .route(
            "/v1/messages/count_tokens",
            post(routes::anthropic::count_tokens),
        )
        .route(
            "/claude/v1/messages/count_tokens",
            post(routes::anthropic::count_tokens),
        )
        // ── Gemini ──────────────────────────────────────────────────────────
        .route("/v1beta/models", get(routes::gemini::models))
        .route("/gemini/v1beta/models", get(routes::gemini::models))
        .route("/v1beta/models/{*rest}", post(routes::gemini::generate))
        .route(
            "/gemini/v1beta/models/{*rest}",
            post(routes::gemini::generate),
        )
        // ── 媒体三条 ────────────────────────────────────────────────────────
        .route(
            "/v1/images/generations",
            post(routes::media::images_generations),
        )
        .route(
            "/openai/v1/images/generations",
            post(routes::media::images_generations),
        )
        .route("/v1/videos", post(routes::media::videos_create))
        .route("/openai/v1/videos", post(routes::media::videos_create))
        .route("/v1/videos/{id}", get(routes::media::videos_poll))
        .route("/openai/v1/videos/{id}", get(routes::media::videos_poll))
}

/// 管理 API 的挂载前缀。**唯一的那一份。**
///
/// 🔴 `nest()` 与「不带尾段」那两条路由都从它取，判据也从它派生 ——
/// 手抄成字面量的那一刻，判据钉住的就只剩它自己那几个字了。
const ADMIN_API_PREFIX: &str = "/admin/api";

/// `/admin/api/*` 子树。
///
/// 🔴 **`admin_auth` 必须是子树内的第一道**，而 33 条方法级端点全部注册在它之后。
///
/// 闸是真的：未配 / 配错口令 → 整棵树不存在（404）；配了口令 → 先过闸再谈路由。
/// ⚠️ 子树自带 fallback **与自带的 405 兜底**，所以**闸跑在路由匹配之前** ——
/// 这与 Hono 的 `use("/admin/api/*")` 语义一致，也正是
/// 「拿着错口令探不出有哪些端点」的前提。
///
/// 🔴 **02 §3.3 那 33 条全部在这里注册，一条都不许缺**（实现深度分三档：
/// 完整实现 / 薄版 `tier:"off"` / 占位 `503 not_wired`，见 `admin/mod.rs` 文件头）。
/// 由 [`tests::every_one_of_the_thirty_three_admin_endpoints_is_really_registered`]
/// 逐条打真实路由树钉住 —— 在它之前，`session` / `upstream/models` / 三条 registrar
/// **少注册了 5 条而全仓一格判据都照不到**。
fn admin_router(state: AppState) -> Router {
    // 🔴 **闸只造一份，三个挂点共用。** 下面那两条「不带尾段」的写法挂在 `nest()`
    //    **外面**（理由见函数末尾），各自再过一次闸；两份 `from_fn_with_state` 也能跑，
    //    但那就是两处要一起改的东西了。
    let gate =
        axum::middleware::from_fn_with_state(state.clone(), middleware::admin_auth::admin_auth);
    // ⚠️ **`.layer()` 而不是 `.route_layer()`**：后者只作用在**匹配到的路由**上，
    //    于是 `/admin/api/不存在的端点` 会直接落到 fallback ——
    //    免鉴权地告诉对方「这条端点不存在」。用 `.layer()` 才能覆盖 fallback。
    let sub: Router = Router::new()
        // ── 登录探针（`admin::session`，02 §8.3）─────────────────────────────
        // 它是 02 §3.3 那张表的第 1 条，也是面板进站必打的第一条。
        .route(
            crate::admin::session::SESSION_SUBPATH,
            get(crate::admin::session::session),
        )
        // ── key 池管理组（`admin::keys`，02 §9.6~§9.14）──────────────────────
        // 🔴 路径一律从 `KeysEndpoint::subpath()` 取，**不许手抄字面量**：
        //    危险区那条的对外路径被五语言 DEPLOY.md 逐份写着，手抄的那份会漂。
        .route(
            crate::admin::keys::KeysEndpoint::List.subpath(),
            get(crate::admin::keys::list).post(crate::admin::keys::import),
        )
        .route(
            crate::admin::keys::KeysEndpoint::Bulk.subpath(),
            post(crate::admin::keys::bulk),
        )
        .route(
            crate::admin::keys::KeysEndpoint::Purge.subpath(),
            post(crate::admin::keys::purge),
        )
        .route(
            crate::admin::keys::KeysEndpoint::Reveal.subpath(),
            get(crate::admin::keys::reveal),
        )
        .route(
            crate::admin::keys::KeysEndpoint::Delete.subpath(),
            axum::routing::delete(crate::admin::keys::delete).patch(crate::admin::keys::patch),
        )
        .route(
            crate::admin::keys::KeysEndpoint::Usage.subpath(),
            get(crate::admin::keys::usage),
        )
        .route(
            crate::admin::keys::KeysEndpoint::Verify.subpath(),
            post(crate::admin::keys::verify),
        )
        // ── 注册机三条（`admin::registrar`，02 §9.15~§9.17）─────────────
        // 🔴 **路由表绝不许随运行时配置变化**：02 §1.4 的「路由表绝不许随运行时
        //    配置变化」。「没接线」是 handler 里的一个 `503 not_wired`，
        //    **不是「这条路由不存在」** —— 404 会把「这个部署没装注册机」
        //    说成「这个网关没有这个功能」，而五语言 API.md 逐条写着它们存在。
        .route(
            crate::admin::registrar::REGISTRAR_TEND_SUBPATH,
            post(crate::admin::registrar::tend),
        )
        .route(
            crate::admin::registrar::REGISTRAR_STATUS_SUBPATH,
            get(crate::admin::registrar::status),
        )
        .route(
            crate::admin::registrar::REGISTRAR_CHANNEL_TEST_SUBPATH,
            post(crate::admin::registrar::channel_test),
        )
        // ── 总览 / 能力 / 配置组（`admin::{overview,capabilities,config}`，
        //    02 §9.1 / §9.2 / §9.18~§9.22）────────────────────────────────────
        // 🔴 路径同样从各自的 `*_SUBPATH` 常量取，不许手抄字面量：危险区那条
        //    （`/config/reset`）的对外路径被五语言 DEPLOY.md 逐份写着。
        .route(
            crate::admin::capabilities::CAPABILITIES_SUBPATH,
            get(crate::admin::capabilities::capabilities),
        )
        .route(
            crate::admin::overview::OVERVIEW_SUBPATH,
            get(crate::admin::overview::overview),
        )
        .route(
            crate::admin::config::CONFIG_SUBPATH,
            get(crate::admin::config::get).put(crate::admin::config::put),
        )
        .route(
            crate::admin::config::CONFIG_VALIDATE_SUBPATH,
            post(crate::admin::config::validate),
        )
        .route(
            crate::admin::config::CONFIG_SECRETS_CLEAR_SUBPATH,
            post(crate::admin::config::clear),
        )
        .route(
            crate::admin::config::CONFIG_RESET_SUBPATH,
            post(crate::admin::config::reset),
        )
        // ── 用量 / 事件 / 模型组（`admin::{usage,events,models}`，
        //    02 §9.3 / §9.5 / §9.23 / §9.24 / §9.31 / §9.32）──────────────────
        // 🔴 路径同样从各自的 `*_SUBPATH` 常量取，不许手抄字面量。
        .route(
            crate::admin::events::EVENTS_SUBPATH,
            get(crate::admin::events::list),
        )
        .route(
            crate::admin::events::EVENTS_DOWNLOAD_SUBPATH,
            get(crate::admin::events::download),
        )
        .route(
            crate::admin::models::MODELS_SUBPATH,
            get(crate::admin::models::list),
        )
        .route(
            crate::admin::models::MODEL_TEST_SUBPATH,
            post(crate::admin::models::test),
        )
        // ⚠️ **`/upstream/models` 与上面那条 `/models` 是两个问题，别读混**：
        //    那一条回答「**本网关**支持哪些模型、怎么调」（零网络），
        //    这一条真打一次上游问「**这个账号**此刻有哪些模型」（02 §9.4）。
        .route(
            crate::admin::upstream_models::UPSTREAM_MODELS_SUBPATH,
            get(crate::admin::upstream_models::upstream_models),
        )
        // ★ **`/usage` 必须排在 `/usage/{date}` 之前。** axum 的 matchit 让静态段
        //   天然优先于 `{param}`，所以这条顺序在今天的匹配器下**不承重** ——
        //   ⚠️ 但实测过的失效形态是真的：Hono 下追加一条 `/usage/summary` 到
        //   `{date}` 之后，它恒回一个看起来很合理的
        //   `400 date 必须是 UTC 的 YYYY-MM-DD，收到的是：summary`。
        //   换匹配器就会复发，所以顺序照排 + 判据钉住。
        .route(
            crate::admin::usage::USAGE_SUBPATH,
            get(crate::admin::usage::summary),
        )
        .route(
            crate::admin::usage::USAGE_DATE_SUBPATH,
            get(crate::admin::usage::by_date),
        )
        // ── 对外 API 密钥组（`apikey::admin`，02 §9.25~§9.30）────────────────
        // ⚠️⚠️ **`/keys` 与 `/apikeys` 是两族，别读混**：上面那组是我们持有的上游
        //    凭据，这一组是我们签发的、别人拿来向我们证明身份的凭据。
        // 🔴 路径同样从 `ApiKeysEndpoint::subpath()` 取，不许手抄字面量。
        .route(
            crate::apikey::admin::ApiKeysEndpoint::List.subpath(),
            get(crate::apikey::admin::list).post(crate::apikey::admin::issue),
        )
        .route(
            crate::apikey::admin::ApiKeysEndpoint::Purge.subpath(),
            post(crate::apikey::admin::purge),
        )
        .route(
            crate::apikey::admin::ApiKeysEndpoint::Reveal.subpath(),
            get(crate::apikey::admin::reveal),
        )
        .route(
            crate::apikey::admin::ApiKeysEndpoint::Delete.subpath(),
            axum::routing::delete(crate::apikey::admin::delete).patch(crate::apikey::admin::patch),
        )
        // 🔴🔴 **「这条管理端点不存在」必须是一条真的路由，不能只靠 `.fallback()`。**
        //
        // 起因是 P2b：面板的静态树在 `/admin/{*file}` 上注册了一条通配路由。
        // matchit 让**静态段优先于通配**，所以 `/admin/api/capabilities` 这类**真存在**
        // 的端点照旧落进本子树；但 `/admin/api/压根不存在的名字` 在本子树里**不匹配
        // 任何路由**，于是它走的是 axum 的 fallback 机制 —— 而 **fallback 斗不过一条
        // 真路由**，它被 `/admin/{*file}` 抢走，变成面板那棵树的 `404 text/plain`。
        // 后果正是本文件下面两格判据钉着的那件事：**不带凭据时，注册过的路径（401）
        // 与不存在的路径（404）分得出来 ⇒ 整棵管理 API 的路径表可以被枚举出来。**
        // ⚠️ 它的失效方式是静默的：加静态树那一刻没有任何编译期或运行期信号。
        //
        // ⇒ 用 `any()` 把「本子树内任何没匹配上的路径 / 任何方法」收成一条**真路由**，
        //   它与下面那颗 `.layer()` 一起被闸包住 ⇒ 闸仍然跑在路由匹配之前。
        //
        // ⚠️ **这一条只覆盖「带尾段」的写法。** `{*rest}` 在 matchit 下要求至少一个
        //    字符，而 `nest("/admin/api", …)` 把内层 `/x` 映成 `/admin/api/x`、**没有**
        //    任何内层路径映得出 `/admin/api` 或 `/admin/api/`（`path_for_nested_route`
        //    把内层 `/` 特判成前缀本身，而本子树里没有 `/` 这条路由）⇒ 那两种
        //    **不带尾段**的写法只剩 `.fallback()` 接着，而 fallback 同样斗不过
        //    `/admin/{*file}`。处置在函数末尾那两条真路由。
        .route("/{*rest}", axum::routing::any(admin_not_found))
        .fallback(admin_not_found)
        // 🔴🔴 **405 那颗兜底必须在这里、在 `.layer()` 之前就挂上**，不能只靠
        //     `build_router()` 末尾那一次全局的 `.method_not_allowed_fallback()`。
        //
        // axum 0.8 的 `PathRouter::method_not_allowed_fallback` 对每个
        // `Endpoint::MethodRouter` 调 `MethodRouter::default_fallback(handler)`，
        // 而后者**只在 `fallback` 仍是 `Fallback::Default(_)` 时才替换**
        // （`method_routing.rs:665`）。`.layer()` 走的是 `Fallback::map`，
        // 保留的正是 `Fallback::Default(被包过的 route)` ⇒ 全局那一次会把管理子树上
        // **已经被闸包过**的兜底换成一颗**闸外**的 handler。
        // 后果不是「少一层装饰」：不带任何凭据用错误方法逐条打 `/admin/api/*`，
        // 靠 405 与 401 的差别就能**枚举出整棵管理 API 的路径表**。
        //
        // ⇒ 在这里显式挂一次，它于是是 `Fallback::BoxedHandler`，
        //    全局那一次见到它**会跳过**（那正是 `default_fallback` 的语义），
        //    而 `.layer()` 照样把它包进闸里。两个性质同时成立：
        //    闸外分不出端点清单，闸内照旧是 `02 §4.1` 要的那个 405 信封 + `Allow`。
        // ⚠️ 与全局那一次同理，它必须写在**本子树所有 `.route()` 之后**。
        .method_not_allowed_fallback(method_not_allowed)
        .layer(gate.clone())
        .with_state(state);

    // 🔴🔴 **`/admin/api` 与 `/admin/api/` 这两种「不带尾段」的写法也必须是真路由。**
    //
    // 起因与上面那条 `/{*rest}` 完全相同、只是漏在了那次处置之外（实测）：
    // 它们落进面板的 `/admin/{*file}`（`file` = `"api"` / `"api/"`），
    // `AdminAssets::get()` 未命中 ⇒ 拿到的是面板那套 `404 text/plain` + 全套面板安全头。
    // 后果有两条，都不是「少一层装饰」：
    //   ① **它们绕开了管理闸** —— 本该是闸内的 401，而 401 与 404 分得出来正是
    //      「整棵管理 API 的路径表可被枚举」的入口（与上面那条同一族边界）；
    //   ② 管理前缀底下的一块被静态树占着 —— 哪天嵌入树里真出现一个叫 `api` 的条目，
    //      这里就是一个 200。
    //
    // ⚠️ **只能挂在 `nest()` 外面。** `path_for_nested_route()` 把内层 `/` 特判成前缀
    //    本身（⇒ 能拿到 `/admin/api`），但**没有任何内层路径映得出 `/admin/api/`** ——
    //    前缀不带尾斜杠时它一律拼成 `前缀 + 内层路径`。两种写法分两处挂会让
    //    「改一处漏一处」重演，所以两条一起写在这里，共用同一份闸。
    let bare = || axum::routing::any(admin_not_found).layer(gate.clone());
    let with_trailing_slash = format!("{ADMIN_API_PREFIX}/");
    Router::new()
        .nest(ADMIN_API_PREFIX, sub)
        .route(ADMIN_API_PREFIX, bare())
        .route(&with_trailing_slash, bare())
}

/// 管理树内「这条端点不存在」。
///
/// ⚠️ **用的是网关信封（不带 `code`）而不是管理信封**，理由是硬的：
/// [`AdminErrorCode`] 是一个**闭集**，里面没有「端点不存在」这一格，
/// 而 🔴 **那个面不许增长**（面板按 `code` 查五语言字典，多一个码就是多一处要翻译、
/// 要对账的东西）。借用 `admin_unavailable` 更糟 —— 那个码对面板的含义是
/// 「两把口令相同」，会把一次拼错的 URL 渲染成一次配置事故。
///
/// ⚠️ 这一支同时是本轮的**交付边界**：33 条管理端点归管理面那一轮，
/// 在它们落地之前，闸后面是空的。
async fn admin_not_found() -> Response {
    GatewayError::not_found().into_response()
}

/// 404 兜底。
///
/// ⚠️ **必须是 JSON 信封**：OpenAI 兼容客户端遇 4xx 会先 `response.json()`，
/// 拿到纯文本（axum 默认）会抛解析异常，而不是一个干净的 `NotFoundError`。
async fn not_found() -> Response {
    GatewayError::not_found().into_response()
}

/// 405 兜底。
///
/// ⚠️ **必须是 JSON 信封**，理由与 404 那条逐字相同：OpenAI 兼容客户端遇 4xx 先
/// `response.json()`，拿到 axum 默认的**空体**会抛解析异常 ——
/// 一次「方法写错了」在客户端侧显示成「客户端内部错误」。
///
/// 🔴 **`Allow` 传 `None`，让 axum 按它的实际注册表填**（`02 §2.2` 第 1 步：
/// 「从当前实际注册表里取候选方法集，不写死常量表」）。
/// axum 只在响应**还没有** `Allow` 时才填，所以这里一个空头值都不许塞。
async fn method_not_allowed() -> Response {
    GatewayError::method_not_allowed(None).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::server::routes::timeout_probe::{
        FIRST_BYTE_BUDGET_MS, SYNC_BUDGET_MS, state_with_timeout_spy,
    };
    use crate::server::test_support::{
        TEST_GATEWAY_TOKEN, TestApp, UpstreamReply, state_with_storage,
    };
    use axum::body::{Body, to_bytes};
    use axum::http::{Method as HttpMethod, Request, StatusCode};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    const ADMIN: &str = "admin-token-0123456789abcdef";

    async fn app() -> Router {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        build_router(state)
    }

    async fn app_with_admin() -> Router {
        let TestApp { mut state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        build_router(state)
    }

    async fn call(app: &Router, req: Request<Body>) -> (StatusCode, Vec<(String, String)>, String) {
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, headers, String::from_utf8_lossy(&b).into_owned())
    }

    fn authed(method: HttpMethod, uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {TEST_GATEWAY_TOKEN}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    /// 兜底 404 是 **JSON 信封**。
    #[tokio::test]
    async fn unknown_routes_get_a_json_envelope() {
        let (status, _, body) = call(
            &app().await,
            Request::builder().uri("/nope").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            json!({ "error": { "type": "not_found", "message": "没有这条路由" } })
        );
    }

    /// 🔴 **方法不对 ⇒ 405 + `Allow` + JSON 信封**（`02 §2.2`、`§4.1`、`§6.1 #22`）。
    ///
    /// 回归对象（实测）：`build_router` 只挂 `.fallback(not_found)` 时，
    /// `GET /v1/chat/completions` 拿到的是 `405` + `allow: POST` 但**响应体为空、
    /// 连 `content-type` 都没有** —— axum 的 `Router::fallback` 只覆盖
    /// 「路径没匹配上」，方法不匹配要 `Router::method_not_allowed_fallback`。
    /// 于是 [`GatewayError::method_not_allowed`] 定义了、单测过、**从未接线**。
    ///
    /// ⚠️ 后果与兜底 404 是同一条：OpenAI 兼容客户端遇 4xx 先 `response.json()`，
    /// 拿到空体会抛解析异常 —— 一次「方法写错了」在客户端侧显示成「客户端内部错误」。
    ///
    /// 🔴 `Allow` 必须**照旧在**（RFC 9110 §15.5.6 是 MUST），且它来自
    /// **实际注册表**而不是一张手抄的方法表。
    #[tokio::test]
    async fn a_wrong_method_gets_a_json_405_that_still_carries_allow() {
        let app = app().await;
        for (method, uri, allow) in [
            (HttpMethod::GET, "/v1/chat/completions", "POST"),
            (HttpMethod::DELETE, "/v1/messages", "POST"),
            (HttpMethod::POST, "/v1/models", "GET,HEAD"),
            (HttpMethod::POST, "/v1/videos/task123", "GET,HEAD"),
            // 闸外的那一条也一样（405 不是鉴权闸的产物）。
            (HttpMethod::POST, "/health", "GET,HEAD"),
        ] {
            let (status, headers, body) = call(&app, authed(method.clone(), uri, "")).await;
            assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{method} {uri}");
            assert_eq!(
                serde_json::from_str::<Value>(&body).unwrap_or(Value::Null),
                json!({ "error": { "type": "method_not_allowed", "message": "这条路径不接受该方法" } }),
                "{method} {uri} 的 405 必须是 JSON 信封，实际：{body:?}"
            );
            let of = |name: &str| {
                headers
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.clone())
            };
            assert_eq!(
                of("content-type").as_deref(),
                Some("application/json"),
                "{method} {uri}"
            );
            assert_eq!(of("allow").as_deref(), Some(allow), "{method} {uri}");
        }
    }

    /// 🔴 **一份 3 MB 的请求体不许被网关自己拒掉。**
    ///
    /// 回归对象（实测）：全仓零 `DefaultBodyLimit` 而所有协议路由都用 `Bytes` 提取
    /// ⇒ 走 axum 的 `into_limited_body()`，默认上限 **2 MiB**。
    /// 于是网关**凭空新增了一条上游本来接受的请求被自己拒掉**的路径 ——
    /// 透传的 `/v1/chat/completions` 里放一张 base64 图片或一段长对话历史轻松过 2 MB，
    /// 而 Hono 版没有这道闸，同一个请求会被原样转给上游。
    ///
    /// 这一格观察的是「真进了转发内核」（空池 503），不是「不等于 413」——
    /// 后者会把一次 400 / 401 也算过。
    #[tokio::test]
    async fn a_three_megabyte_request_is_not_refused_by_the_gateway_itself() {
        let app = app().await;
        let body = format!(
            r#"{{"model":"m","messages":[{{"role":"user","content":"{}"}}]}}"#,
            "a".repeat(3 * 1024 * 1024)
        );
        let (status, _, seen) = call(
            &app,
            authed(HttpMethod::POST, "/v1/chat/completions", &body),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "3 MB 的请求体该一路走到转发内核，实际 {status}：{seen}"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&seen).unwrap()["error"]["reason"],
            "pool_empty"
        );
    }

    /// 🔴 **超上限的请求体回的是 413 + JSON 信封，不是纯文本。**
    ///
    /// 回归对象（实测）：axum 默认的 413 是
    /// `content-type: text/plain` + `"Failed to buffer the request body: length limit exceeded"`,
    /// 正是 `02 §2.2` 与 [`GatewayError::bad_json`] 的论证明确禁止的形状 ——
    /// 客户端 `response.json()` 当场抛 `JSONDecodeError`。
    #[tokio::test]
    async fn an_oversized_request_is_refused_with_a_json_envelope_not_plain_text() {
        let app = app().await;
        let body = "a".repeat(routes::MAX_REQUEST_BODY_BYTES + 1);
        let (status, headers, seen) = call(
            &app,
            authed(HttpMethod::POST, "/v1/chat/completions", &body),
        )
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "content-type" && v == "application/json"),
            "413 的 content-type 必须是 JSON：{headers:?}"
        );
        let v: Value = serde_json::from_str(&seen).expect("413 必须是合法 JSON");
        assert_eq!(v["error"]["type"], "invalid_request_error");
        // 报文要把上限说出来，否则运维只能靠试。
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains(&routes::MAX_REQUEST_BODY_BYTES.to_string()),
            "报文里要有那个数：{v}"
        );
    }

    /// ⚠️ **路径不存在仍然是 404，不许被 405 那条兜底吃掉。**
    ///
    /// 两条兜底挂在 axum 的不同挂点上，接错了会让「没有这条路由」变成
    /// 「这条路由不接受该方法」——那是在骗客户端。
    #[tokio::test]
    async fn wiring_the_405_fallback_does_not_turn_404_into_405() {
        let app = app().await;
        for uri in ["/nope", "/v1/there-is-no-such-thing"] {
            let (status, _, body) = call(&app, authed(HttpMethod::POST, uri, "{}")).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(
                serde_json::from_str::<Value>(&body).unwrap()["error"]["type"],
                "not_found",
                "{uri}"
            );
        }
    }

    /// 🔴 **全应用级 nosniff，刻意覆盖非 `/admin` 路径。**
    #[tokio::test]
    async fn every_response_carries_nosniff() {
        let app = app().await;
        for uri in ["/health", "/nope", "/v1/models"] {
            let (_, headers, _) = call(
                &app,
                Request::builder().uri(uri).body(Body::empty()).unwrap(),
            )
            .await;
            assert!(
                headers
                    .iter()
                    .any(|(k, v)| k == "x-content-type-options" && v == "nosniff"),
                "{uri} 少了 nosniff"
            );
        }
    }

    /// 🔴 **中间件的执行顺序：`config_refresh` 在最外、`gateway_auth` 在最内。**
    ///
    /// 观察点是**可执行的**而不是靠读代码：
    /// 打一条未鉴权的 `/v1/*` 会拿到 401（说明 auth 跑了），
    /// 而这次请求**照样**触发了配置刷新（说明 config_refresh 跑在它之前而不是之后）。
    /// 顺序反过来的话，401 会在配置刷新之前返回 ⇒ 存储读数为 0。
    #[tokio::test]
    async fn middleware_order_is_outermost_first() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let TestApp { state, .. } = state_with_storage(counting).await;
        // 让配置「过期」，这样 config_refresh 真跑起来时会产生一次**可数的**存储读。
        // 不这么做的话它在 TTL 内直接返回，这一格就退化成「什么都没测」。
        state.config.invalidate();
        let app = build_router(state);

        let before = counts.snapshot().0;
        let (status, _, _) = call(
            &app,
            Request::builder()
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            counts.snapshot().0 > before,
            "config_refresh 必须跑在 gateway_auth **之前** —— 否则 /v1 的口令读的是上一个 TTL 的值"
        );
    }

    /// 🔴 **别名与裸路径走同一个 handler。**
    ///
    /// 判据观察的是**行为**：两条路径的响应体逐字节相同。
    /// （只断言「两条都是 200」是抓不住「各写了一个 handler」的。）
    #[tokio::test]
    async fn alias_prefixes_hit_the_very_same_handler() {
        let app = app().await;
        for (bare, alias) in [
            ("/v1/models", "/openai/v1/models"),
            ("/v1beta/models", "/gemini/v1beta/models"),
        ] {
            let (s1, _, b1) = call(&app, authed(HttpMethod::GET, bare, "")).await;
            let (s2, _, b2) = call(&app, authed(HttpMethod::GET, alias, "")).await;
            assert_eq!(s1, StatusCode::OK);
            assert_eq!(s1, s2);
            assert_eq!(b1, b2, "{bare} 与 {alias} 必须是同一个 handler");
        }
        // POST 那一族用「本地校验的报文」当指纹（它不需要上游）。
        let (s1, _, b1) = call(
            &app,
            authed(
                HttpMethod::POST,
                "/v1/messages/count_tokens",
                r#"{"bad":1}"#,
            ),
        )
        .await;
        let (s2, _, b2) = call(
            &app,
            authed(
                HttpMethod::POST,
                "/claude/v1/messages/count_tokens",
                r#"{"bad":1}"#,
            ),
        )
        .await;
        assert_eq!(s1, s2);
        assert_eq!(b1, b2);
    }

    /// 🔴🔴 **别名前缀族与裸路径发出的超时预算逐条相同** —— 22 行，一行一条路径。
    ///
    /// # 为什么另起一格，而不是把上面那格扩到打 dispatch 的那几对
    ///
    /// [`alias_prefixes_hit_the_very_same_handler`] 的观测量是**响应体逐字节相同**，
    /// 而它对超时档**结构性地看不见**：两个只差一个 `TimeoutProfile` 的 handler
    /// 产出的响应体是**一模一样的**。把它扩到打 dispatch 的那几对也照样漏 ——
    /// 漏的不是覆盖面，是观测量选错了。
    /// ⇒ 要挡住「把某条别名挂到第二个 handler 上（而那个 handler 选了另一档）」，
    /// 观测量只能是**这条路径真正发给上游的 `timeout_ms`**。
    ///
    /// # 现有的两格为什么都堵不住这个变异
    ///
    /// - [`alias_prefixes_hit_the_very_same_handler`]：只覆盖三对，
    ///   且**一条都不打 dispatch**（`/v1/models`、`/v1beta/models`、`count_tokens`）；
    /// - [`the_route_table_does_not_shrink_when_features_are_off`]：14 条别名都在列，
    ///   但粒度只到 `503 pool_empty` —— **换个档位的 handler 照样给 503**。
    ///
    /// # 这一格为什么不是「别名 == 裸路径」而是「各自等于那个字面量」
    ///
    /// 只断言两边相等的话，**两边一起改错**就测不出来了（改一个 handler
    /// 同时服务两条路径正是本仓的装配形态）。⇒ 每一行钉的是**绝对值**。
    ///
    /// ⚠️ 期望值**手写字面量**，见 `routes::timeout_probe` 里那两个常量的说明。
    /// ⚠️ **每一行都要求恰好一次上游调用**（`only_budget()` 自带）⇒
    ///    「这条别名压根没走到转发内核」也会在这里红，而不是悄悄算过。
    #[tokio::test]
    async fn every_alias_prefix_sends_the_same_timeout_budget_as_its_bare_path() {
        const CHAT: &str = r#"{"model":"m","messages":[{"role":"user","content":"x"}]}"#;
        const CHAT_S: &str =
            r#"{"model":"m","messages":[{"role":"user","content":"x"}],"stream":true}"#;
        const RESP: &str = r#"{"model":"m","input":"x"}"#;
        const RESP_S: &str = r#"{"model":"m","input":"x","stream":true}"#;
        const GEN: &str = r#"{"contents":[{"parts":[{"text":"x"}]}]}"#;
        const MEDIA: &str = r#"{"model":"m","prompt":"p"}"#;

        // 🔴 每一对的两行必须**紧挨着**：别名与裸路径的期望值一眼可比。
        for (method, uri, body, expected) in [
            // ── OpenAI：非流式 ⇒ 同步档，流式 ⇒ 首字节档 ──────────────────
            (
                HttpMethod::POST,
                "/v1/chat/completions",
                CHAT,
                SYNC_BUDGET_MS,
            ),
            (
                HttpMethod::POST,
                "/openai/v1/chat/completions",
                CHAT,
                SYNC_BUDGET_MS,
            ),
            (
                HttpMethod::POST,
                "/v1/chat/completions",
                CHAT_S,
                FIRST_BYTE_BUDGET_MS,
            ),
            (
                HttpMethod::POST,
                "/openai/v1/chat/completions",
                CHAT_S,
                FIRST_BYTE_BUDGET_MS,
            ),
            // ── Responses ────────────────────────────────────────────────────
            (HttpMethod::POST, "/v1/responses", RESP, SYNC_BUDGET_MS),
            (
                HttpMethod::POST,
                "/openai/v1/responses",
                RESP,
                SYNC_BUDGET_MS,
            ),
            (
                HttpMethod::POST,
                "/v1/responses",
                RESP_S,
                FIRST_BYTE_BUDGET_MS,
            ),
            (
                HttpMethod::POST,
                "/openai/v1/responses",
                RESP_S,
                FIRST_BYTE_BUDGET_MS,
            ),
            // ── Anthropic ────────────────────────────────────────────────────
            (HttpMethod::POST, "/v1/messages", CHAT, SYNC_BUDGET_MS),
            (
                HttpMethod::POST,
                "/claude/v1/messages",
                CHAT,
                SYNC_BUDGET_MS,
            ),
            (
                HttpMethod::POST,
                "/v1/messages",
                CHAT_S,
                FIRST_BYTE_BUDGET_MS,
            ),
            (
                HttpMethod::POST,
                "/claude/v1/messages",
                CHAT_S,
                FIRST_BYTE_BUDGET_MS,
            ),
            // ── Gemini：⚠️ 流不流式来自**路径里的方法名**，不在请求体里 ─────
            (
                HttpMethod::POST,
                "/v1beta/models/m:generateContent",
                GEN,
                SYNC_BUDGET_MS,
            ),
            (
                HttpMethod::POST,
                "/gemini/v1beta/models/m:generateContent",
                GEN,
                SYNC_BUDGET_MS,
            ),
            (
                HttpMethod::POST,
                "/v1beta/models/m:streamGenerateContent",
                GEN,
                FIRST_BYTE_BUDGET_MS,
            ),
            (
                HttpMethod::POST,
                "/gemini/v1beta/models/m:streamGenerateContent",
                GEN,
                FIRST_BYTE_BUDGET_MS,
            ),
            // ── 媒体三条：前两条同步档 ───────────────────────────────────────
            (
                HttpMethod::POST,
                "/v1/images/generations",
                MEDIA,
                SYNC_BUDGET_MS,
            ),
            (
                HttpMethod::POST,
                "/openai/v1/images/generations",
                MEDIA,
                SYNC_BUDGET_MS,
            ),
            (HttpMethod::POST, "/v1/videos", MEDIA, SYNC_BUDGET_MS),
            (HttpMethod::POST, "/openai/v1/videos", MEDIA, SYNC_BUDGET_MS),
            // ── 🔴 轮询是**唯一的例外**：非流式但用首字节档，别名也一样 ──────
            (
                HttpMethod::GET,
                "/v1/videos/task123",
                "",
                FIRST_BYTE_BUDGET_MS,
            ),
            (
                HttpMethod::GET,
                "/openai/v1/videos/task123",
                "",
                FIRST_BYTE_BUDGET_MS,
            ),
        ] {
            // 每一行一套新夹具：`only_budget()` 要求「恰好一次上游调用」。
            let (TestApp { state, .. }, spy) = state_with_timeout_spy(vec![UpstreamReply::json(
                json!({ "id": "chatcmpl-1", "choices": [] }),
            )])
            .await;
            let app = build_router(state);
            let (status, _, seen) = call(&app, authed(method.clone(), uri, body)).await;
            assert_eq!(status, StatusCode::OK, "{method} {uri} 没走到上游：{seen}");
            assert_eq!(
                spy.only_budget(),
                expected,
                "{method} {uri} 发出去的超时预算不对"
            );
        }
    }

    /// ⚠️ **`/v1/messages/count_tokens` 不许被 `/v1/messages` 吃掉。**
    ///
    /// axum 0.8 用 matchit，静态段天然优先 —— **这条风险在 Rust 侧本身会消失，
    /// 但必须写判据钉住**，否则将来换匹配器会复发。
    #[tokio::test]
    async fn count_tokens_is_not_swallowed_by_the_messages_route() {
        let app = app().await;
        let (status, _, body) = call(
            &app,
            authed(
                HttpMethod::POST,
                "/v1/messages/count_tokens",
                r#"{"model":"m","messages":[{"role":"user","content":"hello"}]}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // `count_tokens` 的形状，**不是** `/v1/messages` 的（后者会去打上游而失败）。
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            json!({ "input_tokens": 1 })
        );
    }

    /// 🔴 **没配 `ADMIN_TOKEN` ⇒ 整棵 `/admin` 树不存在（404），不泄漏「这里有个后台」。**
    #[tokio::test]
    async fn without_an_admin_token_the_whole_admin_tree_is_absent() {
        let (status, _, body) = call(
            &app().await,
            Request::builder()
                .uri("/admin/api/session")
                .header("x-admin-key", ADMIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        // 而且它与「任何一条不存在的路径」**长得一模一样**。
        let (_, _, other) = call(
            &app().await,
            Request::builder()
                .uri("/definitely-nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(body, other);
    }

    /// 配了口令之后，`/admin/api/*` **先过闸再谈路由**：错口令拿到 401 而不是 404。
    ///
    /// 🔴 这正是「拿着错口令探不出本网关有哪些管理端点」的前提。
    #[tokio::test]
    async fn the_admin_gate_runs_before_route_matching() {
        let app = app_with_admin().await;
        let (status, _, body) = call(
            &app,
            Request::builder()
                .uri("/admin/api/whatever-does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["error"]["code"],
            "admin_unauthorized"
        );
        // 而拿着**正确**口令打同一条不存在的路径，才会看到 404 ——
        // 这个先后关系就是「探不出端点清单」的全部内容。
        let (status, _, _) = call(
            &app,
            Request::builder()
                .uri("/admin/api/whatever-does-not-exist")
                .header("x-admin-key", ADMIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// 🔴🔴 **管理前缀底下的写法，一种都不许被面板静态树咬走。**
    ///
    /// # 回归对象（实测，本轮）
    ///
    /// P2b 那条 `/admin/{*file}` 通配路由把 `/admin/api` 与 `/admin/api/` 整个接走了
    /// （`file` = `"api"` / `"api/"`，`AdminAssets::get()` 未命中 ⇒ 面板那套
    /// `404 text/plain` + **全套面板安全头**）。上一轮只把**带尾段**的写法
    /// （`/admin/api/<名字>`）收成了一条闸内的真路由，这两种**不带尾段**的写法漏在外面
    /// —— 而它们本该是闸内的 401。
    ///
    /// # 这一格钉的是**那一整类**，不是那两个字面量
    ///
    /// 路径全部从 [`ADMIN_API_PREFIX`] 现拼，且观测量有两条：
    /// - ① **静态树的指纹**：面板的 `content-security-policy`。管理树的响应一条都不带它
    ///   ⇒ 但凡某种写法被静态树接走（不论它回 404、400 还是哪天真命中一份资源变成 200），
    ///   这一判都会红；
    /// - ② 与一条**真注册过**的端点在闸外**逐字节分不出来** ——
    ///   那正是 [`a_wrong_method_without_credentials_cannot_enumerate_the_admin_tree`]
    ///   守着的同一条边界：只要两者能分出来，整棵管理 API 的路径表就能被枚举。
    ///
    /// ⚠️ **反向控制必须有**：同样这几种写法挂在管理前缀**之外**的 `/admin` 路径下时，
    /// 照旧由静态树回答（带 CSP）。少了它，①那一判在「静态树整个没挂上」时也会绿。
    #[tokio::test]
    async fn no_spelling_under_the_admin_prefix_is_answered_by_the_static_tree() {
        // 🔴 「不带尾段」那两种（`""` 与 `"/"`）是本轮的回归对象，排在最前面。
        //    其余几种是同一类的邻居：多一个斜杠、带扩展名（会引诱人去查嵌入资源表）、
        //    以及两种穿越形状（裸的与百分号编码的）。
        const SPELLINGS: [&str; 9] = [
            "",
            "/",
            "//",
            "/x",
            "/x/y",
            "/index.html",
            "/css/base.css",
            "/../css/base.css",
            "/%2e%2e/css/base.css",
        ];
        const CSP: &str = "content-security-policy";

        let app = app_with_admin().await;
        let bare = |uri: &str| {
            Request::builder()
                .uri(uri.to_string())
                .body(Body::empty())
                .unwrap()
        };

        // 参照系：一条**真注册过**的管理端点在不带凭据时的样子。
        let reference = call(&app, bare(&format!("{ADMIN_API_PREFIX}/session"))).await;
        assert_eq!(
            reference.0,
            StatusCode::UNAUTHORIZED,
            "参照系自己就不对，这一格会退化成空测"
        );

        for tail in SPELLINGS {
            let uri = format!("{ADMIN_API_PREFIX}{tail}");
            let (status, headers, body) = call(&app, bare(&uri)).await;
            assert!(
                !headers.iter().any(|(k, _)| k == CSP),
                "🔴 {uri} 带着面板那套安全头 ⇒ 它被静态树接走了，管理子树的边界被咬掉一块：\
                 {status} {headers:?}"
            );
            assert_eq!(
                (status, &body),
                (reference.0, &reference.2),
                "🔴 {uri} 与一条真端点在**没有凭据**时分得出来 ⇒ 管理 API 的路径表可枚举"
            );
        }

        // 反向控制：管理前缀**之外**的 `/admin` 路径照旧归静态树。
        for tail in SPELLINGS {
            let uri = format!("/admin/not-the-api{tail}");
            let (status, headers, _) = call(&app, bare(&uri)).await;
            assert!(
                headers.iter().any(|(k, _)| k == CSP),
                "{uri} 本该由面板静态树回答（带全套安全头）—— 它不带，说明上面那条\
                 指纹判没有对象了：{status} {headers:?}"
            );
        }

        // 拿着**正确**口令时，这两种写法是闸**内**那条「没有这条路由」的网关信封，
        // 而不是面板那份纯文本 404。
        for tail in ["", "/"] {
            let uri = format!("{ADMIN_API_PREFIX}{tail}");
            let (status, headers, body) = call(
                &app,
                Request::builder()
                    .uri(&uri)
                    .header("x-admin-key", ADMIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(
                serde_json::from_str::<Value>(&body).unwrap_or(Value::Null),
                json!({ "error": { "type": "not_found", "message": "没有这条路由" } }),
                "{uri} 必须是闸内那条网关信封，实际：{body:?}"
            );
            assert!(
                headers
                    .iter()
                    .any(|(k, v)| k == "content-type" && v == "application/json"),
                "{uri}：{headers:?}"
            );
        }
    }

    /// 02 §3.3 那张表：**33 条方法级管理端点，一条都不许缺。**
    ///
    /// 🔴 这是 `admin/mod.rs` 那条铁律（「33 条全部注册，实现深度分三档」）**唯一**的
    /// 机器形态。在它之前，`session` / `upstream/models` / 三条 registrar 共 **5 条
    /// 压根没注册**，而全仓一格判据都照不到 —— 拿着正确口令打它们只会拿到一个
    /// 与「路径拼错了」一模一样的 404。
    ///
    /// # 观测量：**不是「不等于 404」，是「不等于那条兜底 404」**
    ///
    /// 好几条端点的 404 是 handler 说的正经话（「目录里没有这个模型」「没有这把 key」），
    /// 断 `assert_ne!(404)` 会逼着判据去给每条端点造一份能成功的夹具 —— 那是另一族判据的活。
    /// 这一格只回答**一个**问题：**这条路径注册了没有**。
    /// ⇒ 拦的是那条兜底信封本身（`GatewayError::not_found()` 的逐字报文），
    ///   外加 405（**方法**注册错了也是没注册）与 401（口令没被接受 ⇒ 整格会变成空测）。
    #[tokio::test]
    async fn every_one_of_the_thirty_three_admin_endpoints_is_really_registered() {
        // 🔴 **手写字面量，不从注册代码派生**：从 `admin_router()` 派生就是同义反复
        //   （少注册一条，派生出来的清单也跟着少一条，判据永远绿）。
        //   这张表的真源是 02 §3.3，改它必须有人回去读那一节。
        const TABLE: [(&str, &str); 33] = [
            ("GET", "/admin/api/session"),
            ("GET", "/admin/api/keys"),
            ("GET", "/admin/api/capabilities"),
            ("GET", "/admin/api/overview"),
            ("GET", "/admin/api/events"),
            ("GET", "/admin/api/events/download"),
            ("POST", "/admin/api/keys"),
            ("POST", "/admin/api/keys/bulk"),
            ("POST", "/admin/api/keys/purge"),
            ("GET", "/admin/api/keys/deadbeef/reveal"),
            ("DELETE", "/admin/api/keys/deadbeef"),
            ("PATCH", "/admin/api/keys/deadbeef"),
            ("GET", "/admin/api/keys/deadbeef/usage"),
            ("POST", "/admin/api/keys/deadbeef/verify"),
            ("POST", "/admin/api/registrar/tend"),
            ("GET", "/admin/api/registrar/status"),
            ("POST", "/admin/api/registrar/channels/moemail/test"),
            ("GET", "/admin/api/config"),
            ("PUT", "/admin/api/config"),
            ("POST", "/admin/api/config/validate"),
            ("POST", "/admin/api/config/secrets/clear"),
            ("POST", "/admin/api/config/reset"),
            ("GET", "/admin/api/models"),
            ("GET", "/admin/api/upstream/models"),
            ("POST", "/admin/api/models/no-such-model/test"),
            ("GET", "/admin/api/usage"),
            ("GET", "/admin/api/usage/2026-01-01"),
            ("GET", "/admin/api/apikeys"),
            ("POST", "/admin/api/apikeys"),
            ("POST", "/admin/api/apikeys/purge"),
            ("GET", "/admin/api/apikeys/deadbeef/reveal"),
            ("DELETE", "/admin/api/apikeys/deadbeef"),
            ("PATCH", "/admin/api/apikeys/deadbeef"),
        ];

        let app = app_with_admin().await;
        // 那条兜底的**逐字**形状（`admin_not_found` / `not_found` 同一个信封）。
        let route_missing = json!({ "error": { "type": "not_found", "message": "没有这条路由" } });

        let mut absent = Vec::new();
        for (method, uri) in TABLE {
            let (status, _, body) = call(
                &app,
                Request::builder()
                    .method(HttpMethod::from_bytes(method.as_bytes()).unwrap())
                    .uri(uri)
                    .header("x-admin-key", ADMIN)
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_ne!(
                status,
                StatusCode::UNAUTHORIZED,
                "{method} {uri}：口令没被接受，这一格会退化成空测"
            );
            assert_ne!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {uri}：路径在、**方法**没注册"
            );
            if serde_json::from_str::<Value>(&body).ok() == Some(route_missing.clone()) {
                absent.push(format!("{method} {uri} -> {status}"));
            }
        }
        assert!(
            absent.is_empty(),
            "02 §3.3 的 33 条里这几条**压根没注册**（拿着正确口令也是兜底 404）：\n{}",
            absent.join("\n")
        );
    }

    /// 🔴🔴 **不带凭据的「方法不对」不许把管理端点清单漏出去。**
    ///
    /// # 回归对象（实测）
    ///
    /// `admin_router()` 用 `.layer(admin_auth)` 把整棵子树（含 `.fallback()`）包在闸里，
    /// 但 `build_router()` 在 `merge` 之后又执行了
    /// `.method_not_allowed_fallback(method_not_allowed)` —— 它对**当前注册表里每个**
    /// `MethodRouter` 调 `default_fallback()`，而那一个只在 `fallback` 还是
    /// `Fallback::Default(_)` 时替换；`.layer()` 经 `Fallback::map` 保留的**正是**
    /// `Fallback::Default(被包过的 route)`。
    /// ⇒ 那颗**闸外**的 405 handler 把管理子树上被包过的兜底整个换掉了，于是
    /// **不带任何凭据**、用错误方法逐条打 `/admin/api/*`，靠 405（这条路径在）与
    /// 401（这条路径不在）的差别就能把整棵管理 API 的路径表**枚举出来**。
    ///
    /// # 判据的观测量：**两种请求的响应逐字节相同**
    ///
    /// 只断言「是 401」是不够的 —— 要断的是「**分不出来**」。
    /// ⚠️ 逐字节比才抓得住「状态码一样但信封里多了一格 `allow` / 多了一句别的话」。
    #[tokio::test]
    async fn a_wrong_method_without_credentials_cannot_enumerate_the_admin_tree() {
        let app = app_with_admin().await;
        // 每一对的第二格都是**真的注册过**的路径，只是方法不对。
        for (method, uri) in [
            (HttpMethod::POST, "/admin/api/capabilities"),
            (HttpMethod::GET, "/admin/api/keys/purge"),
            (HttpMethod::DELETE, "/admin/api/config"),
            (HttpMethod::PUT, "/admin/api/events"),
        ] {
            let hit = call(
                &app,
                Request::builder()
                    .method(method.clone())
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(hit.0, StatusCode::UNAUTHORIZED, "{method} {uri}");
            assert_eq!(
                serde_json::from_str::<Value>(&hit.2).unwrap()["error"]["code"],
                "admin_unauthorized",
                "{method} {uri}"
            );

            // 同一个方法打一条**压根不存在**的管理路径。
            let miss = call(
                &app,
                Request::builder()
                    .method(method.clone())
                    .uri("/admin/api/there-is-no-such-endpoint")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(
                (hit.0, hit.2),
                (miss.0, miss.2),
                "{method} {uri}：注册过的路径与不存在的路径在**没有凭据**时必须分不出来"
            );
        }
    }

    /// 🔴 **上一格修完之后，405 那个信封必须照旧在**（`02 §4.1`、`§6.1 #22`）。
    ///
    /// 两个性质要**同时**成立：闸外分不出端点清单（上一格），
    /// 而拿着**正确**口令时，方法写错照样拿到网关信封的 405 + `Allow`。
    /// ⚠️ 只修上一格、把管理子树的 405 退化成 404 或 401 也能让它绿 ——
    /// 所以这一格钉的是**带凭据**那一侧。
    #[tokio::test]
    async fn an_authorized_wrong_method_still_gets_the_json_405_envelope() {
        let app = app_with_admin().await;
        for (method, uri, allow) in [
            (HttpMethod::POST, "/admin/api/capabilities", "GET,HEAD"),
            (HttpMethod::GET, "/admin/api/keys/purge", "POST"),
            (HttpMethod::DELETE, "/admin/api/config", "GET,HEAD,PUT"),
        ] {
            let (status, headers, body) = call(
                &app,
                Request::builder()
                    .method(method.clone())
                    .uri(uri)
                    .header("x-admin-key", ADMIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{method} {uri}");
            assert_eq!(
                serde_json::from_str::<Value>(&body).unwrap_or(Value::Null),
                json!({ "error": { "type": "method_not_allowed", "message": "这条路径不接受该方法" } }),
                "{method} {uri} 的 405 必须是网关信封，实际：{body:?}"
            );
            let of = |name: &str| {
                headers
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.clone())
            };
            // 🔴 `Allow` 来自 axum 的**实际注册表**，不是一张手抄的方法表。
            assert_eq!(of("allow").as_deref(), Some(allow), "{method} {uri}");
            assert_eq!(
                of("content-type").as_deref(),
                Some("application/json"),
                "{method} {uri}"
            );
        }
    }

    /// ⚠️ **失败登录事件里的 `path` 必须是完整路径。**
    ///
    /// 这一格**必须打真实的嵌套路由**：`nest("/admin/api", ...)` 会把前缀从
    /// `req.uri().path()` 里剥掉，只在中间件自己的小 Router 上测是观察不到的
    /// （那正是已登记的假阳性形态第 ⑤ 条：测试搭的环境让被测的选择不可观测）。
    #[tokio::test]
    async fn a_failed_admin_login_records_the_full_path() {
        let TestApp {
            mut state,
            recorded,
            ..
        } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let app = build_router(state);
        call(
            &app,
            Request::builder()
                .uri("/admin/api/session")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        let events = recorded.entries();
        let e = events
            .iter()
            .find(|e| e.event == "admin.login_failed")
            .expect("失败登录必须留下一条事件");
        assert_eq!(e.fields["path"], json!("/admin/api/session"));
    }

    /// 🔴 **`/admin/api/*` 不认 `Authorization: Bearer`，也不认 `?key=`。**
    #[tokio::test]
    async fn the_admin_tree_refuses_bearer_and_query_credentials() {
        let app = app_with_admin().await;
        for req in [
            Request::builder()
                .uri("/admin/api/session")
                .header("authorization", format!("Bearer {ADMIN}"))
                .body(Body::empty())
                .unwrap(),
            Request::builder()
                .uri(format!("/admin/api/session?key={ADMIN}"))
                .body(Body::empty())
                .unwrap(),
        ] {
            assert_eq!(call(&app, req).await.0, StatusCode::UNAUTHORIZED);
        }
    }

    /// 🔴 **`/health` 不在鉴权闸射程内，且零存储 I/O。**
    #[tokio::test]
    async fn health_is_public_and_silent() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let TestApp { state, .. } = state_with_storage(counting).await;
        let app = build_router(state);
        let before = counts.snapshot();
        for _ in 0..10 {
            let (status, _, _) = call(
                &app,
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
        assert_eq!(counts.snapshot(), before, "/health 必须零存储 I/O");
    }

    /// 🔴 **路由表不随运行时配置变化**：Tier-2 关着、注册机没接线，
    /// 协议路由一条都不许消失。
    ///
    /// ⚠️ 这一格**逐条覆盖 `protocol_router()` 里注册的每一条路由，别名前缀族在内**。
    /// 从前只查三条 POST（`/v1/messages` 与 `/v1/images/generations` 不在列，
    /// 别名一条都不在列）⇒ 摘掉其中任何一条这格都不红。
    ///
    /// ⚠️ 打上游那一族断言的是**具体的 503 + `pool_empty`**，不是 `assert_ne!(404)`：
    /// 后者把「本地校验挡下的 400」「鉴权挡下的 401」一并算过 ——
    /// 而 `/v1/responses` 配 `{"model":"m","messages":[]}` 恰恰是一次 400
    /// （那条协议要的是 `input` 不是 `messages`），于是旧写法在它上面**从来没测到路由**。
    #[tokio::test]
    async fn the_route_table_does_not_shrink_when_features_are_off() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        assert!(state.usage_sink.is_none() && !state.registrar_wired());
        let app = build_router(state);

        // ① 零上游的那几条：200。
        for uri in [
            "/v1/models",
            "/openai/v1/models",
            "/v1beta/models",
            "/gemini/v1beta/models",
        ] {
            assert_eq!(
                call(&app, authed(HttpMethod::GET, uri, "")).await.0,
                StatusCode::OK,
                "{uri} 不该因为别的功能关着而消失"
            );
        }
        for uri in [
            "/v1/messages/count_tokens",
            "/claude/v1/messages/count_tokens",
        ] {
            assert_eq!(
                call(
                    &app,
                    authed(
                        HttpMethod::POST,
                        uri,
                        r#"{"model":"m","messages":[{"role":"user","content":"x"}]}"#
                    )
                )
                .await
                .0,
                StatusCode::OK,
                "{uri} 不该因为别的功能关着而消失"
            );
        }

        // ② 要打上游的那一族：池子是空的 ⇒ **必须走到 503 `pool_empty`**。
        //    到得了这个状态码就说明路由在、闸过了、本地校验过了、真进了转发内核。
        // ⚠️ 每一份都必须是**本地校验真能过**的请求体（非空 `messages` / `input` /
        //    `contents`），否则这一格观察到的是 400 而不是路由是否还在。
        const CHAT: &str = r#"{"model":"m","messages":[{"role":"user","content":"x"}]}"#;
        const RESP: &str = r#"{"model":"m","input":"x"}"#;
        const GEN: &str = r#"{"contents":[{"parts":[{"text":"x"}]}]}"#;
        const MEDIA: &str = r#"{"model":"m","prompt":"p"}"#;
        for (method, uri, body) in [
            (HttpMethod::POST, "/v1/chat/completions", CHAT),
            (HttpMethod::POST, "/openai/v1/chat/completions", CHAT),
            (HttpMethod::POST, "/v1/responses", RESP),
            (HttpMethod::POST, "/openai/v1/responses", RESP),
            (HttpMethod::POST, "/v1/messages", CHAT),
            (HttpMethod::POST, "/claude/v1/messages", CHAT),
            (HttpMethod::POST, "/v1beta/models/m:generateContent", GEN),
            (
                HttpMethod::POST,
                "/gemini/v1beta/models/m:generateContent",
                GEN,
            ),
            (HttpMethod::POST, "/v1/images/generations", MEDIA),
            (HttpMethod::POST, "/openai/v1/images/generations", MEDIA),
            (HttpMethod::POST, "/v1/videos", MEDIA),
            (HttpMethod::POST, "/openai/v1/videos", MEDIA),
            (HttpMethod::GET, "/v1/videos/task123", ""),
            (HttpMethod::GET, "/openai/v1/videos/task123", ""),
        ] {
            let (status, _, body) = call(&app, authed(method, uri, body)).await;
            assert_eq!(
                status,
                StatusCode::SERVICE_UNAVAILABLE,
                "{uri} 该走到转发内核（空池 503），实际 {status}：{body}"
            );
            assert_eq!(
                serde_json::from_str::<Value>(&body).unwrap()["error"]["reason"],
                "pool_empty",
                "{uri}"
            );
        }
    }
}
