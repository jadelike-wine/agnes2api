# 02 · HTTP 层与管理 API 契约（从 TypeScript 版抽取）

> 抽取自 `agnes2api` TS 版 `src/http/`（37 文件 / 9,400 行）+ `src/entry/node.ts`，
> 并交叉验证了五语言 `docs/*/API.md`。
> **本文是「Rust 版必须实现成什么样」的规格**，不是实现建议。凡是写「⚠️」的地方，
> 都是 TS 版在注释里明确标注过「改了会静默坏掉 / 曾经真的坏过」的性质。
>
> 口径约定：本文凡说「段数」，一律指 `/admin/api/` 之后按 `/` 切出来的段数。
> 本文不含任何真实凭据、域名、IP。

---

## 0. 一页速查

| 维度 | 结论 |
|---|---|
| Web 框架 | Hono 4.13.x（Rust 侧对标 axum 0.8） |
| 进程入口 | 只剩 `src/entry/node.ts` 一个（Worker 形态 P0 已砍） |
| 对外端口 | `PORT`，默认 `8080`；数据目录 `DATA_DIR`，默认 `/app/data` |
| 路由总数 | 协议/健康 **10 条** + `/admin/api/*` **33 条** + 静态 **3 条**（`/favicon.ico`、`/admin`、`/admin/*`） |
| 错误信封 | **三种并存**，见 §4，不许合并 |
| 两把钥匙 | `/v1*` 认 `GATEWAY_TOKEN` + 子密钥；`/admin` **只**认 `X-Admin-Key` |
| 全局中间件 | 4 条（其中 1 条条件挂载），顺序敏感，见 §2 |

---

## 1. 装配顺序与依赖注入

### 1.1 三层装配

```
entry/node.ts::main(env)
  └─ FileStorage(DATA_DIR)
  └─ http/wire.ts::buildApp(env, storage, { probeStorage: true })
       └─ http/app.ts::createApp(deps)  → Hono
  └─ startTendScheduler(...)            // 定时补池轮，与 HTTP 无关但共用 tendGate
  └─ serve({ fetch: app.fetch, port })
```

`buildApp` 返回 `{ app, configHolder, repo, tendGate }`。
后三样交出来**不是为了测试方便**，是三条结构性约束：

- `repo`：面板写完 key 后要 `invalidate()` **这一个**实例，另建一个等于空操作；
- `tendGate`：定时轮与面板「立即补池」必须共用**同一把**，各拿各的等于并发补池；
- `configHolder`：入口取 `registrar.tendIntervalMs` 建定时器，不再独立读一次存储。

### 1.2 `buildApp` 里建了什么（顺序有意义）

| # | 组件 | 要点 |
|---|---|---|
| 1 | `now = options.now ?? Date.now` | ⚠️ **这次装配里所有组件共用这一个时钟**，不许有第二个时间来源（存储 TTL 与 sink 的 `expiresAt` 用不同时钟会造出「生下来就过期」的键，实测踩过） |
| 2 | `storageHealth` + `watchStorage(storage, …)` | 包一层后所有写的成败自动反映到 `/health` |
| 3 | `shardId = randomUUID().slice(0,8)` | **每进程一次，终生不变**；事件 sink 与用量 sink **共用同一个** |
| 4 | `StoreLogger`（事件落库） | `onError` 走裸 `ConsoleLogger`，**不走 fan-out 后的 logger**（sink 自己坏了还往自己里写没意义） |
| 5 | `logger = multiLogger(consoleLogger, storeLogger)` | |
| 6 | `probeWritable()`（仅 `probeStorage: true`） | 只有真启动路径开 |
| 7 | `configHolder = createConfigHolder(...)` | **首次装载失败必须抛**（缺 `GATEWAY_TOKEN` 拒绝服务是不变量） |
| 8 | `repo = new KeyPoolRepo(watched, { cacheTtlMs, touchIntervalMs })` | ⚠️ 这两个旋钮**建 app 时读一次**，改了要重启；面板文案不许写「立即生效」 |
| 9 | `tendGate = createTendGate()` | |
| 10 | `usageSink`（**仅当 `usageStatsEnabled`**） | ⚠️ 关闭时必须是 `None`，**不是「建好再用 if 拦住写」**；生产一律 `budgetPerDay: null` |
| 11 | `apiKeyHolder`（**无条件建**） | 与 10 刻意不同：它在「没签发过子密钥」的部署上一次存储调用都不产生 |
| 12 | `createApp({...})` | |

### 1.3 `createApp` 的依赖注入语义（Rust 侧必须保留的两条）

**① `config` 必须是 getter，不能是拷贝值。**
`dispatchDeps()` 里写的是 `get config() { return deps.configHolder.current(); }`。
⚠️ **返回值不许被展开**（`{ ...dispatchDeps(deps) }` 会当场求值 getter，把配置永久冻结在建 app 那一刻 —— 那正是历史上「改完配置网关照旧按旧值跑」的成因）。
Rust 侧对等做法：路由 state 持有 `Arc<ConfigHolder>`，每次请求开头 `holder.current()` 取**一份快照**，单个请求内部保持一致。

**② `currentGatewayToken` 同样是 getter。**
「`ADMIN_TOKEN` 不得等于 `GATEWAY_TOKEN`」这条要在**每个管理请求**上复查，而 `gatewayToken` 运行中可被面板 / 手改 `store.json` 改掉。

### 1.4 `AppDeps` 的可选项与「缺席的后果」

TS 版把「没接线」与「功能关着」严格分成两类，Rust 侧必须保留这个区分：

| 依赖 | 缺席时 | 语义 |
|---|---|---|
| `registrar` | 3 条端点 **照常注册、照常鉴权**，回 `503 {reason:"not_wired"}` | 装配不全 |
| `config` | 5 条端点（get/put/validate/secrets.clear/reset）回 `503 not_wired` | 装配不全 |
| `apiKeys` | 6 条端点回 `503 not_wired`；**鉴权退化成「只认主口令」** | 装配不全（但鉴权退化是正常形态） |
| `usageSink` | 3 条用量端点 **照常 200**，如实回 `tier: "off"` | **正常且默认的部署形态**，回 503 会让面板把「没开统计」渲染成「后端坏了」 |
| `adminToken` 未设/不合规 | **整棵 `/admin` 树不注册** → 404 | 不泄漏「这里有个后台」 |

> 🔴 **路由表绝不许随运行时配置变化。** 「注册机没开」「Tier-2 没开」「没接线」一律是
> handler 里的一个状态码，不是「这条路由不存在」。TS 版的理由：枚举式鉴权矩阵会因为
> 夹具恰好关着某功能而让那几条端点从矩阵里静默消失。

---

## 2. 中间件链（顺序敏感，逐条）

`createApp` 里的注册顺序（★ = 顺序写错会**静默失效且不报错**）：

```
app.onError(...)          // 全局异常兜底
app.notFound(...)         // 404 / 405 兜底
★ app.use("*", configRefresh(configHolder))
★ app.use("*", logFlush(() => storeLogger.maybeFlush()))
★ app.use("*", usageFlush(() => usageSink.maybeFlush()))   // 仅 usageSink 存在时
★ app.use("*", 全局 nosniff)                                // 必须写在 next() 之后
  app.route("/", healthRoutes(...))
★ app.use("/v1/*",     auth(getGatewayToken, apiKeyAuth))
★ app.use("/v1beta/*", auth(getGatewayToken, apiKeyAuth))   // 两条共用同一份 wiring
  app.route("/", openai / anthropic / gemini / responses / media)
  app.route("/", adminRouter(...))                          // 内部第一行才是 adminAuth
```

⚠️ **Hono 实测**：把 `route` 写在 `use` 之前，中间件会静默失效（200 而不是 401）。
Rust/axum 用 `Router::layer` + 嵌套 router，等价风险在「layer 挂在哪一层 Router 上」。

### 2.1 `onError`

- `HTTPException` → 原样返回它自带的 `Response`；
- 其它异常 → **打一条 `http.unhandled_error` 事件**（`method` / `path` / `error`），
  再返回 `500 {"error":{"type":"internal_error","message":"网关内部错误"}}`。
- 🔴 **绝不回显 `err.message` 给客户端**（可能含上游 URL、栈帧）；
  🔴 **也绝不静默吞掉**（实测 14 次 500 之后事件流零记录，运维无从排障）。

### 2.2 `notFound`（**404 与 405 的分岔**）

Hono 不区分「路径不存在」与「方法不对」，两者都落进 `notFound`，所以要自己算：

1. 从**当前实际注册表**里取候选方法集（不写死常量表，否则 `app.on("QUERY", …)` 会静默漏报）；
2. 跳过请求方自己用的那个方法；
3. 对每个候选方法探一次路由匹配，**只认 `method !== "ALL"` 的命中项**
   （`ALL` 全是 `use()` 挂的中间件，拿它们当证据会让每一条 404 变成 405）；
4. 有命中 → `405` + `Allow: <排序后逗号分隔>`（RFC 9110 §15.5.6 的 MUST）；
   没有 → `404 {"error":{"type":"not_found","message":"没有这条路由"}}`。

⚠️ 404 **必须是 JSON 信封**，不是 Hono 默认的 `text/plain`：OpenAI 兼容客户端遇 4xx 会先 `response.json()`，拿到纯文本会抛解析异常而不是干净的 `NotFoundError`。

⚠️ `notFound` **不覆盖已命中 handler 的 404**：`/admin` 静态兜底的 404 由 `uiRoutes()` 自己返回（纯文本 + 全套面板安全头）。

### 2.3 `configRefresh`

```
if path != "/health" { holder.ensure_fresh().await }
next()
```
- 每请求刷到「不早于一个 TTL」。刷新次数是 **TTL 界定**的（默认 30 秒 ⇒ 2,880 次/天/副本），与请求数、与是否鉴权无关。
- 跑在鉴权**之前**是必须的：`/v1` 的鉴权口令就在配置里，顺序反不过来。
- ⚠️ `/health` 豁免：镜像 HEALTHCHECK 每 30 秒一次，不该变成每 30 秒一次存储读。

### 2.4 `logFlush` / `usageFlush`（两条同形）

```
next().await;
if path == "/health" { return }
let _ = flush().await;   // 永不让旁路影响响应
```
- 🔴 **必须在 `next()` 之后**：写在前面的话本次请求自己产生的事件还没进缓冲；
- 🔴 **必须 `await`**：不 await 的话 ① 落盘失败的诊断与引发它的请求对不上号；② 收到 `SIGTERM` 时最后一批事件直接丢；
- ⚠️ **`/health` 豁免对用量比对事件更要紧**：HEALTHCHECK 是一个从不产生任何用量的调用方，不该消费用量统计的写预算。
- ⚠️ **注册顺序 ≠ 落盘顺序**：中间件收尾段由内向外跑，注册得越晚收尾越早 ⇒ 今天是**用量先落、事件后落**。

### 2.5 全局 `nosniff`

```
next().await;
resp.headers.insert("x-content-type-options", "nosniff");
```
- 🔴 **必须写在 `next()` 之后。** 写在之前时只对框架自己构造的 JSON 响应生效，
  handler 直接返回裸 `Response` 的那一族（流式转发、dispatcher 错误、`uiRoutes` 的 200/301/304/404、兜底 404/405）**整条头被静默丢掉**。
- **刻意只加 nosniff，不加 CSP / X-Frame-Options**：后两条防的是「文档被渲染 / 被套 iframe」，而 `/v1/*` 与 `/health` 从不返回文档。面板那棵树的全套安全头在 `ui/serve.ts`。

---

## 3. 完整路由表

### 3.1 非 `/admin/api`（协议 + 健康 10 条，静态 3 条，外加 2 条 `use`）

| 方法 | 路径 | 鉴权 | 说明 |
|---|---|---|---|
| GET | `/health` | 无 | 零存储 I/O |
| GET | `/v1/models` | 网关 | OpenAI 形状模型列表 |
| POST | `/v1/chat/completions` | 网关 | **原样透传**，不做结构转换 |
| POST | `/v1/responses` | 网关 | |
| POST | `/v1/messages` | 网关 | Anthropic |
| GET | `/v1beta/models` | 网关 | Gemini 形状 |
| POST | `/v1beta/models/:rest{.+}` | 网关 | Gemini，见 §3.2 |
| POST | `/v1/images/generations` | 网关 | 同步档 |
| POST | `/v1/videos` | 网关 | 建任务，同步档 |
| GET | `/v1/videos/:id` | 网关 | 轮询，**首字节档**（唯一的「非流式但用首字节档」例外） |
| GET | `/favicon.ico` | 无 | 由 `uiRoutes()` 注册，跟着 `/admin` 树一起存在/消失 |
| GET | `/admin`、`/admin/*` | 无 | 静态面板，见 §8 |

### 3.2 四协议路由的行为差异（Rust 侧逐条对齐）

| 协议 | 本地校验 | 超时档 | `expectJson` | Tier-2 token |
|---|---|---|---|---|
| OpenAI `/v1/chat/completions` | `model` 必须是字符串、`messages` 必须是数组 | `stream ? firstByte : sync` | **否**（原样搬运） | **恒 0**，见下 |
| Anthropic `/v1/messages` | `toInternalRequest` 抛 `InvalidRequestError` → 400 | 同上 | `!stream` | 从上游 `usage.prompt_tokens/completion_tokens` |
| Responses `/v1/responses` | 同上 | 同上 | `!stream` | 同上 |
| Gemini `/v1beta/models/{m}:{method}` | ① 路径必须含 `:`（否则 400）② **方法名白名单** | 同上 | `!stream` | 同上 |

- **OpenAI 的本地校验不是可选项**：TS 版从前一次校验都没有，`{"foo":1}` 被原样送上游由上游回 400；而上游那层 CF 约 2 次快请求就回 1015，限流额度**整个网关共享** ⇒ 一个客户端 bug 循环重试畸形请求几秒内能打死所有人。但 ⚠️ **只校验这三样，不做更严的形状检查**（上游接受哪些字段是它的事）。
- **OpenAI 的 token 恒 0 不是漏**：它是唯一不传 `expectJson` 的一条，网关从头到尾没有解析过响应体。这个缺口由 `GET /admin/api/capabilities` 的 `stats.tokensCoverage` 如实告诉面板。🔴 **别在这里补一次 `res.clone().json()`** —— 那是热路径上给每个请求加一次全量解析。
- **Gemini 方法名白名单**：模型名按**最后一个冒号**切（模型名本身可能含冒号）；
  `method` 只许 `generateContent` / `streamGenerateContent`，其余 → **404**（`not_found_error`），
  理由：请求体没错，是这条端点本网关没实现，真 Gemini 给的也是 NOT_FOUND。
  🔴 历史缺陷：不校验时 `:countTokens` 会落进 generateContent handler 真打一次上游对话（实测 200 + 一段真回答 + `candidatesTokenCount: 43`）。
- **流式响应**：OpenAI 只补 `content-type: text/event-stream; charset=utf-8`（用 `Headers(res.headers)` 追加，**不整体替换**，否则会丢 `cache-control`）；另三条协议自己构造流并设 `content-type` + `cache-control: no-cache`。
- **媒体路由的视频任务 id 校验**：`^[A-Za-z0-9_-]{1,128}$`，不合法回 400，报文里的形状串与「建任务那条端点的方法+路径」**都从真源常量派生，不手抄**。

### 3.3 `/admin/api/*`（**33 条**，按注册顺序）

> ★ = 注册顺序不许动（更窄的必须排在更宽的之前，否则被吃掉的那条恒不可达，
> 而它只会回一个**看起来合理的 400**）。

```
 0  use   /admin/api/*                      ← adminAuth（必须是子 app 内第一行）
 1  GET   /admin/api/session
 2  GET   /admin/api/keys
 3  GET   /admin/api/capabilities
 4  GET   /admin/api/overview
 5  GET   /admin/api/events
 6  GET   /admin/api/events/download
 7  POST  /admin/api/keys
 8★ POST  /admin/api/keys/bulk               ← 必须在 /keys/:id 之前
 9★ POST  /admin/api/keys/purge              ← 同上
10  GET   /admin/api/keys/:id/reveal
11  DELETE/admin/api/keys/:id
12  PATCH /admin/api/keys/:id
13  GET   /admin/api/keys/:id/usage
14  POST  /admin/api/keys/:id/verify
15  POST  /admin/api/registrar/tend
16  GET   /admin/api/registrar/status
17  POST  /admin/api/registrar/channels/:channel/test
18  GET   /admin/api/config
19  PUT   /admin/api/config
20  POST  /admin/api/config/validate
21  POST  /admin/api/config/secrets/clear
22  POST  /admin/api/config/reset
23  GET   /admin/api/models
24  GET   /admin/api/upstream/models
25  POST  /admin/api/models/:id/test
26★ GET   /admin/api/usage                   ← 必须在 /usage/:date 之前
27  GET   /admin/api/usage/:date
28  GET   /admin/api/apikeys
29  POST  /admin/api/apikeys
30★ POST  /admin/api/apikeys/purge           ← 必须在 /apikeys/:id 之前
31  GET   /admin/api/apikeys/:id/reveal
32  DELETE/admin/api/apikeys/:id
33  PATCH /admin/api/apikeys/:id
34  route /  ← uiRoutes()（静态兜底，**必须最后**）
```

🔴 **两条位置铁律**：
- 任何 `/admin/api/*` 端点都必须注册在 **`adminAuth` 之后、静态兜底之前**。
  挪到前面 = 一条免鉴权的管理端点；挪到后面 = **恒 404，拿着正确口令也是 404，没有任何报错**。
- ⚠️ **「更窄排在更宽之前」不管新加的是通配还是字面段**：实测追加
  `GET /admin/api/usage/summary` 到 `:date` 之后，它恒回
  `400 date 必须是 UTC 的 YYYY-MM-DD，收到的是：summary`。

（axum 的路由匹配优先级与 Hono 不同——axum 0.8 用 matchit，静态段天然优先于 `{param}`。
**Rust 侧这条风险本身会消失，但必须写测试把它钉住**，否则将来换匹配器会复发。）

---

## 4. 三种错误信封（**不许合并**）

### 4.1 网关信封（`src/http/errors.ts`）

```json
{ "error": { "type": "<string>", "message": "<string>" } }
```
- `content-type: application/json`，可带 `extraHeaders`（今天唯一用户：405 的 `Allow`）；
- 被五语言 API.md 逐字写死，是**对外契约**；
- 用户：四协议路由、兜底 404/405/500、**以及管理树里若干处**（见 4.4）。

### 4.2 管理信封（`src/http/admin/errors.ts`）

```json
{ "error": { "type": "…", "code": "<AdminErrorCode>", "message": "…", "params": { } } }
```
- ⚠️ **`code` 才是对外契约，`message` 不是**。面板靠 `code` 查五语言字典，**不许解析 message**；`message` 措辞可随时改。
- `params` 三条口径：① 一个汉字都不许有（会被插进 ja/ko 的句子）；② 永不放凭据、永不回显请求体里任何字段的**值**；③ **但确实会逐字回显调用方送来的字段名**（`unknown_field` 的 `params.fields` 就是多余键名的 `join`）—— 这是这条码的用途。
- `readAdminJson()` 畸形 JSON → `400 code:"bad_json"`。

**`AdminErrorCode` 全表（25 个）**：
```
admin_unavailable, admin_unauthorized, bad_json, body_not_an_object, unknown_field,
key_not_found, must_disable_first, not_a_boolean, note_not_a_string, note_too_long,
empty_patch, keys_not_a_string_array, too_many_import_keys, not_a_bulk_op,
ids_not_a_string_array, too_many_bulk_ids, apikey_not_found, name_not_a_string,
name_empty, name_too_long, expires_not_a_number, expires_in_the_past,
too_many_apikeys, version_not_a_number, stale_write, apikeys_unreadable
```
每个码带哪些 `params`（`ADMIN_ERROR_PARAMS`）：
`unknown_field→[fields]`、`not_a_boolean→[field]`、`note_too_long/too_many_import_keys/too_many_bulk_ids/name_too_long/too_many_apikeys→[max]`、`not_a_bulk_op→[ops]`、`stale_write→[expected, actual]`，其余为空。

### 4.3 转发失败信封（`src/core/dispatcher.ts`）

```json
{ "error": { "reason": "<FailReason>", "message": "…" } }
```
⚠️ **这一族用的是 `reason` 而不是 `type`**，与 4.1 不同，别搞混。
- `503`：`pool_empty` / `all_cooling` / `all_disabled` / `all_evicted` / `upstream_error`。
  `all_cooling` 带 `Retry-After`（秒）；`all_disabled` / `all_evicted` **一律不带**（不会自愈，带了等于让客户端每秒重试一把永远不会回来的 key）。
- `504`：`upstream_timeout`（同步档耗尽 `UPSTREAM_SYNC_TIMEOUT_MS` 总预算）。
- `502`：`upstream_bad_body`（上游 200 但响应体不是 JSON，只在 `expectJson` 的路由上）。

### 4.4 「handler 自拼」的混合体（**现状，如实登记**）

有一批管理端点不走前两种，而是自己拼一个「网关信封 + 顶层附加字段」：

```json
{ "error": { "type": "conflict", "message": "…" }, "reason": "…", "<其它顶层字段>": … }
```
- 用户：`notWired()`（config / apikeys / registrar 三处各一份）、`keysPurgeHandler` 的 409、`keyDeleteHandler` 的 409、`manualTendHandler` 的全部拒绝支、`channelTestHandler` 的全部拒绝支、`verify` / `model-test` / `upstream-models` 的 429。
- 🟡 **这是一个已知的不一致**：`GET /admin/api/keys/:id/usage` 的 404、
  `POST /admin/api/keys/purge` 的 400、`POST /admin/api/models/:id/test` 的 404、
  `POST /admin/api/keys/:id/verify` 的 404 与 400，全都走的是**不带 `code` 的网关信封**，
  而同一棵树上 `keys-write.ts` / `api-keys.ts` 的同类错误带 `code`。
  TS 版在 `keys-write.ts:606` 与 `config.ts:16` 两处注释里承认这是**刻意的**
  （理由：设置页 / 危险区不渲染那几条码，只渲染 message）。
  **Rust 侧建议**：保留现状以免破坏面板，但在 Rust 里用一个显式枚举把「哪条端点用哪种信封」编码进类型，别靠人记。

---

## 5. 鉴权

### 5.1 `/v1/*` 与 `/v1beta/*`（`middleware/auth.ts`）

**凭据提取顺序**（`extract()`）：
1. `Authorization` 头存在 → `^bearer\s+(.+)$`（**大小写不敏感**）取捕获组；
   不是 Bearer 格式 → 返回 trim 后的整个原值（非空时）；
2. 否则依次 `x-api-key` → `x-goog-api-key` → **URL 查询参数 `key`**；
3. 空串 / 缺失一律当「无凭据」。

**两段验证，顺序不可交换**：
```
① presented == gatewayToken（常数时间比较）
     → c.set("apiKeyId", "master"); next()      ← 零存储 I/O
② 否则（且 apiKeys 已接线）：
     holder.ensure_fresh().await                 ← 只有这里才碰存储，被 TTL 摊平
     table = holder.current()
     rec = find_by_digest(table.keys, sha256(presented))
     if rec 可用 → c.set("apiKeyId", rec.id); next()
     if rec 存在但不可用 → 打一条 apikey.rejected 事件（只记 id 与 bucket）
③ 全部落空 → 401
```
🔴 **①在前不是性能优化，它就是逃生口本身**：`apikeys` 键被写坏、读不出来、持有者第一次装载就失败时，①这一支一个字节都不受影响。反过来写在功能上等价，**在逃生口上是完全不同的东西**。

🔴 **没带凭据的请求在第一步就结束，一次存储读都不产生**（扫描器打 `/v1/chat/completions` 不带任何头是最常见形态，让它撬不动存储）。

**401 响应体**（注意：与 4.1 的 `errorResponse` 字段顺序相反，是 `message` 在前）：
```json
{ "error": { "message": "未授权：缺少或无效的凭据", "type": "unauthorized" } }
```
⚠️ **401 不区分**「没这把密钥」/「停用了」/「过期了」—— 那是给攻击者的枚举信号。区分只进事件日志。

**归属传递**：`apiKeyId` 经请求上下文（Hono 的 `ContextVariableMap`）往下传给四条协议路由做 Tier-2 归因。主口令归到保留伪桶 `"master"`；结构上不可达的缺席归到 `"unattributed"`（兜底在 `UsageSink.record()` 里，**不在路由上**）。
Rust 侧：axum 的 `Extension<ApiKeyId>` 或 request extensions。

### 5.2 `/admin/api/*`（`admin/auth.ts`）—— 🔴 **只认 `X-Admin-Key`**

这是本仓裁定过的一条：

- 🔴 **不接受 `Authorization: Bearer`**（两把钥匙严格隔离）；
- 🔴 **不接受 `?key=`**（口令进 URL 会落进浏览器历史、Referer、各级访问日志）——
  这条禁令**同时否掉了 EventSource**（它设不了请求头），所以面板的事件流只能是轮询，不能是 SSE；
- 比较用**常数时间**；
- 失败记 `admin.login_failed` 事件，字段 `{ ip, path(截断), hasHeader }`
  —— ⚠️ **绝不记 provided 本身**（猜错的口令常常只差一位，记下来就是攻击者字典的一部分）。

**`ADMIN_TOKEN` 四条硬规则，顺序有意义**：

| # | 规则 | reason | 何时检查 |
|---|---|---|---|
| ⓪ | 首尾不得有空白 | `whitespace_padded` | 装配期（→ 整棵树不注册，404） |
| ① | 只允许可打印 ASCII `0x20–0x7E` | `not_sendable` | 装配期 |
| ② | 长度 ≥ **24** | `too_short` | 装配期 |
| ③ | 不得等于当前生效的 `GATEWAY_TOKEN` | `same_as_gateway_token` | **每请求复查** → 503 |

- 顺序理由：空白最先（唯一在配置文件里看不见的）；字符集排在长度前（一串汉字口令同时触发两条时，报「长度不足」会把人引向加长它，加长之后照样送不出去）；相同性最后。
- ⚠️ **③ 刻意不在装配期拦**：它的另一个输入运行中会变，装配期拦会造成**分裂脑**——冲突期间冷启动的实例整棵树 404 且**改回去仍 404、必须重启**，而冲突前建好的实例只是 503、改回去立刻恢复。
- ③ 命中时返回 `503 adminErrorBody("service_unavailable","admin_unavailable","管理接口不可用")`，
  🔴 **响应体不说原因**（这个分支跑在验证凭据之前，说出「两把口令相同」等于告诉一个手里已有中转口令的人「管理口令就是你手上那把」），原因只进日志。
  🔴 **不许把这条 503 改成 401**：面板规则是「401 = 会话失效，清凭据踢回登录闸」，改成 401 会让运维被踢出去并被告知「口令无效」，而他手里那把是对的。
- 空串 `ADMIN_TOKEN` 必须落进「未配置」分支 —— 历史上出过一次实际的鉴权绕过，成因就是空串在 `??` 下不下坠。

**字符集三段的实测边界（Rust 侧照抄，别自己推）**：
- (a) 码点 > U+00FF、`NUL`/`LF`/`CR` → 浏览器 `Headers.set` 直接抛 TypeError，请求发不出去；
- (b) `0x01–0x08`、`0x0B`、`0x0C`、`0x0E–0x1F`、`0x7F`（29 个）→ 浏览器送得出去，HTTP 解析器判 400；
- (c) `TAB 0x09` 与整段 `0x80–0xFF` → **送得出去、也真的能用**，拦它们是稳健性取舍
  （TAB 是不可见字符；`0x80–0xFF` 是编码口径问题：env 按 UTF-8 解、头值按 Latin-1 解，RFC 9110 已把 obs-text 标为弃用）。
- **空格 `0x20` 是允许的**（passphrase 式口令有价值，且纯 ASCII 零编码歧义）。
- 🟡 **`ADMIN_TOKEN` 没有任何登录限速**，下限 24 是唯一的防线。TS 版承认「限速这件事没有做，不是做不了」（单进程本来做得起）。**Rust 版可以补上这一块，属于加分项，但那是一次独立的功能决定**（要定策略、五语言文案、考虑反代已在限速的部署）。

---

## 6. `client-ip.ts`（**只有一个消费者**）

```rust
fn client_ip(req, trust_proxy: bool) -> Option<String> {
    if !trust_proxy { return None }                      // ① 门控
    if let Some(ip) = ip_or_none(header("cf-connecting-ip")) { return Some(ip) }  // ② CF 优先
    ip_or_none(header("x-forwarded-for").split(',').next())                       // ③ XFF 兜底
}
```

**成立前提（必须写进 Rust 侧注释与五语言 DEPLOY.md）**：

- ① **`TRUST_PROXY=1` 之外一律 `None`，`CF-Connecting-IP` 也不例外。**
  它「平台注入不可伪造」只在**请求真的经过 Cloudflare**时成立；直连暴露时客户端自己发一个就算数，而**直连正是 Docker 部署的默认形态**。
- ② ⚠️⚠️ **CF 优先于 XFF 这条排序的原理由在 v0.4.0 已经没了**（原话是「Worker 形态下 CF 定义上就在前面」）。
  **排序没改，但它从「由形态保证的事实」降级成一个有代价的取舍**：
  - CF 真在前面时（Docker + Cloudflare 代理，最常见）：边缘写入并**覆盖**同名头 ⇒ 伪造不了；
  - CF **不在**前面时（只挂自建 nginx/Caddy）：`CF-Connecting-IP` 没有任何人会覆盖，攻击者自己带一个会**优先于**反代写的 XFF 胜出。
  ⇒ **后一种形态的处置是「在反代上把 `CF-Connecting-IP` 剥掉」，这条今天是排序成立的前提，不是边角提醒。**
- ③ 两个头都过**形态校验**（`ip_or_null`）：IPv4 严格点分；IPv6 只做**形态**校验
  `^[0-9a-f:.]{2,45}$` 且**必须含冒号**（不做完整 RFC 4291 解析——目标是「这个字段不可能承载攻击者的任意文本」，不是「它是一个合法 IPv6」）。
  零校验时实测记下来的是 `ip="not-an-ip-at-all <img src=x>"`。
- 拿不到时**如实 `None`，绝不伪造 `"unknown"`**：面板显示「—」是诚实的，`"unknown"` 会被当成一个真实来源。

**射程**：全仓只有 `admin.login_failed` 一个消费点，**不参与鉴权、不参与限速**。

---

## 7. 三个「持有者 / sink」

### 7.1 `ConfigHolder`（`config-holder.ts`）

| 项 | 值 |
|---|---|
| TTL | `CONFIG_TTL_MS = 30_000` |
| 接口 | `current()` 同步、**永不抛**；`ensure_fresh()` **永不抛**（失败保留上一份合法快照）；`invalidate()` |
| 首次装载 | **失败必须抛**（缺 `GATEWAY_TOKEN` 拒绝服务是不变量） |
| 降级豁免 | **只有 prime 那一次**允许「存储读不出来 → 退到 env + 默认值」；之后每次刷新都严格 |
| 传播上界 | **= TTL 本身 = 30 秒**，中间不再有任何一层缓存 |

⚠️ **上界就是 30 秒，别再加任何一项。** 上一版加过一个「KV 边缘缓存 60 秒」，那一层随 Worker 形态没了；那个数曾经进过 `overview.freshness`、`PUT /config` 的 `propagation`、面板文案与五语言文档，v0.4.0 已全部删干净。
⚠️ 别和 `POOL_CACHE_TTL_MS`（默认 60 秒，「别的副本判的冷却/剔除多久能看到」）搞混，**面板要把两个数都显示出来**。

### 7.2 `ApiKeyHolder`（`apikey-holder.ts`）

| 项 | 值 |
|---|---|
| 默认 TTL | `APIKEY_CACHE_TTL_MS = 300_000`（5 分钟） |
| 环境变量 | `APIKEY_CACHE_TTL_MS`，**空串 == 没设**；`0` 合法（关缓存，逃生口）；非法值装配期抛 `ConfigRefusal` |
| 刷新触发点 | ⚠️ **刻意不挂 `use("*")`**，只在鉴权第②段（凭据 ≠ 主口令）之后调 |
| 结构性的零 | 全部客户端都用主口令的部署 **每天 0 次读**——因为那条路径上根本没有调用点，不是因为某个 `if` |
| 读账 | 有子密钥客户端时：`86400 / TTL秒` 次/副本/天 ⇒ 默认 288 次 |
| 传播上界 | **= 5 分钟**：「面板上停用一把密钥，它最多还能再用约 5 分钟」——安全相关，面板 / ADMIN.md / DEPLOY.md **三处都要写具体数字，不许写「稍后生效」** |

**冷启动降级纪律**（与 ConfigHolder 同形）：
只有「还没有任何一份合法快照可退」的那一次才允许把读失败降级成「没有子密钥」。
⚠️ **冷启动那一档必须降级而不是抛**：抛出去的话 `Refreshable` 在 `everLoaded == false` 时刻意不推进计时 ⇒ **每个错口令请求一次存储读**，而那正是「一条零凭据零限速的路径变成撬动读配额的杠杆」。降级成「空表」之后它是一份**合法快照**，照样吃满一个 TTL。
（判据：1000 次错口令请求只产生 1 次 get。）

三档日志：`apikeys.unreadable`（读失败降级）、`apikeys.invalid`（结构不认，**绝不静默当空表**，原字节留在存储里没被动过、写路径会拒绝覆盖它）、`apikeys.reload_failed`。

### 7.3 `UsageSink`（`usage-sink.ts`，Tier-2）

| 项 | 值 |
|---|---|
| 开关 | `USAGE_STATS_ENABLED`（建 app 时读一次）；**关 = sink 根本不存在** |
| 落盘间隔 | `USAGE_FLUSH_INTERVAL_MS`，默认 `USAGE_FLUSH_MIN_INTERVAL_MS = 7_200_000`（2 小时）；空串 == 没设；非法值装配期抛 `ConfigRefusal` |
| 写预算 | 机制还在，但**生产装配一律传 `null`**（那道闸是为 KV 每日写配额存在的，文件存储没有） |
| 分片键 | `usage:<UTC日序号>:<槽位>`，`USAGE_SLOTS = 2`，槽位 = `hash(shardId) % SLOTS`，**构造时算一次、终生不变** |
| 保留 | `USAGE_DAY_RETAIN = 30` 天 |
| 键上界 | `byModel` 最多 `USAGE_MODEL_MAX_KEYS = 32` 个键；`byApiKey` 最多 `APIKEY_MAX + 1 = 201` |

**内存累加器结构**：`Map<日序号, DayAcc>`，`DayAcc = { total, hours, byModel, byProtocol, byApiKey }`。
⚠️ **按 UTC 日分桶，不是一份全局累计**：全局累计 + 按天分的键 = **重复计数**，而重复计数在面板上长得完全正常。

⚠️⚠️ **四个 map 必须是无原型的**（`Object.create(null)`）：`byModel` 的键完全由客户端控制，
`byModel["__proto__"] = 桶` 不是加一个键、是去改原型 ⇒ 那条计数**彻底消失**；
`byModel["toString"] ?? empty()` 会摸到 `Function.prototype.toString`。
两种都让落盘的分片**自己和自己对不上**（`total ≠ Σ byModel`）。
**Rust 侧这条风险天然消失**（`HashMap<String, Bucket>` 没有原型），但「上界 + 溢出归并到一个桶」那一半必须照做。

**`record()` 的三条硬约束**：
1. **永不抛**（兜底在这里，不在四条路由上——路由那一侧的表达式在「sink 缺席就 return」**之前**求值，放在那里的任何一行都会在 Tier-2 关着时照样跑）。
   ⇒ 🔴 **四条路由里 `model` 字段刻意不做 `String()` 强转**，一个 `{"model":{"toString":1,"valueOf":1}}` 会让 `String()` 自己抛，把**关着统计的部署**也打成 500。
2. **可能抛的每一步都排在任何一次写入累加器之前**（否则会留下「`total` 加了而 `byModel` 没加」的半截状态）。
3. `onError(err, phase)` 的 `phase` 决定说什么，**两句话意思相反**：
   - `flush` → `storage.usage_flush_failed`：累加器**保留**，下次落盘会带上，数据没丢；
   - `record` → `usage.record_failed`：这一条计数**已经永久丢失**。
   写成**查表**而不是三元（加新 phase 时 `tsc` 先报错；三元的 else 会把新 phase 误报成旧的那条）。

**`maybe_flush()` 的四条**：
1. `dirty.is_empty()` 早退 —— 它保证的**不是「零写」**（那是循环本身给的），而是
   **空转的 flush 不许把 `lastFlushAt` 往前推**：否则面板轮询会在零流量期间不停刷落盘时钟 ⇒ 真来了第一条计数还要再等满 2 小时。
2. `lastFlushAt` 与 `budget` **必须在发起任何 `await` 之前推进**（并发请求的 flush 会双双停在写上，各写一遍同一个键、各扣一格预算）。**失败不回滚**（回滚 = 让正在故障的存储无限重试）。
3. **发起写之前先取一份浅拷快照 + 记版本号**；写完只清「我发起写的那一刻的那个版本」——`await` 期间新到的 `record()` 会让版本号变，这一天**继续留在 dirty 里**下一轮整份重写。
   ⚠️ 少了这一条，落盘挂起期间到达的那条计数**永久消失**，而 `status()` 会报 `pending: 0`。
4. 只有「该落的都落下去了」才清 `pending`（偏保守方向是安全的：尾巴报得比实际长可以，反过来不行）。

`status() -> { shardId, pending, pendingMs, budgetExhausted }`。
⚠️ `pendingMs` 数的是「距上一次落盘**尝试**多久」，不是「距上一次写**成功**多久」——
`pending > 0 && pendingMs ≈ 0` 读作「刚试过、没写成」，不是「刚写完」。

**`upstreamTokens(body)`**：从**上游那份 OpenAI 形状**取 `usage.prompt_tokens` / `usage.completion_tokens`。
⚠️ **别拿协议目录的 `usagePath` 往这里套**：那一格描述的是**客户端收到的**响应里 usage 在哪（Gemini 那条是 `usageMetadata`），而三条路由拿到的 `res.json()` **全部**是上游的 OpenAI 格式。套错的后果是「Gemini 的 token 恒 0，而面板上它长得和『这段时间没人用 Gemini』一模一样」。
一律窄化：`usage: null` / `"12"` / 负数 → 0，**绝不把 NaN 累进桶**。

---

## 8. 三条基础端点

### 8.1 `GET /health`（不鉴权，零存储 I/O）

```json
200 { "status": "ok",       "version": "0.4.0", "storage": { "writable": true } }
503 { "status": "degraded", "version": "0.4.0",
      "storage": { "writable": false, "detail": "数据目录不可写，key 池无法持久化。…详见容器日志" } }
```
- 🔴 **503 而不是 200**：镜像 HEALTHCHECK 按响应是否 ok 判定，只有这样容器才会被标 unhealthy。
  原实现里这种容器一直报 healthy，而每一次 API 调用都在失败。
- 🔴 **不回显底层异常**（`EACCES: … open '/app/data/...'`）：它是不鉴权端点。
- ⚠️ 它是**唯一**被 `configRefresh` / `logFlush` / `usageFlush` 三条中间件同时豁免的路径。

### 8.2 `/admin` 静态面板（`ui/serve.ts`，**免鉴权**）

- `GET /admin` 与 `GET /admin/*` → 查内嵌资源表 `UI_ASSETS`；
- `GET /admin/` → **301** 到 `/admin`；
- 命中且 `If-None-Match` 相等 → **304**；否则 200 + `etag` + 资源自己的 `content-type`；
- 未命中 → **404 纯文本 `"Not Found"`**（不是 JSON 信封，它服务浏览器不服务 SDK）；
- `GET /favicon.ico` → 从面板 HTML 里 `<link rel="icon" href="data:image/png;base64,…">` 抠出来的 PNG 字节（构建期抠一次，抠不到就**建应用时抛**）。

**全套安全头（每一条响应都带）**：
```
content-security-policy: default-src 'none'; script-src 'self'; style-src 'self';
                         img-src 'self' data:; connect-src 'self'; frame-ancestors 'none';
                         base-uri 'none'; form-action 'none'
x-frame-options: DENY
x-content-type-options: nosniff
referrer-policy: no-referrer
cache-control: no-cache
```
⚠️ 静态资源免鉴权（登录闸得先能打开），但**整棵树跟着 `/admin` 一起存在或消失**：没配 `ADMIN_TOKEN` 时这几行根本不执行。

### 8.3 `GET /admin/api/session`

```json
200 { "ok": true, "version": "0.4.0" }
```
登录探针。**不返回任何配置或池子信息**——它是登录闸背后的第一个端点，越薄越好。

---

## 9. 管理端点逐条契约

> 全部端点：`X-Admin-Key` 必须正确，否则 `401 {error:{type:"unauthorized",code:"admin_unauthorized",message:"未授权"}}`；
> 两把口令相同时 `503 {error:{type:"service_unavailable",code:"admin_unavailable",message:"管理接口不可用"}}`。
> 下面只列 200 / 业务错误。

### 9.1 `GET /admin/api/capabilities`（零存储读）

```json
{
  "version": "0.4.0",
  "runtime": { "name": "node" },
  "storage": { "backend": "file", "writable": true },
  "quota":   { "model": "file" },
  "process": { "metrics": true },
  "logs":    { "processLog": false },
  "stats": {
    "tier2Enabled": false,
    "flushIntervalMs": 7200000,
    "tokensCoverage": ["anthropic", "responses", "gemini"]
  },
  "apiKeys": {
    "wired": true,
    "max": 200,
    "nameMax": 64,
    "plaintextRetrievable": true,
    "cacheTtlMs": 300000,
    "defaultCacheTtlMs": 300000
  }
}
```
- ⚠️ **`runtime.colo` 已在 v0.4.0 删掉**（那是 Workers 给 Request 挂的非标准属性，Node 下恒 `null`）。
- 🔴 `stats.tier2Enabled` **必须来自「这个 app 到底建没建 sink」**，不许现读配置里的 `usageStatsEnabled`：那个开关是建 app 时读一次的，有人往存储里改成 true 之后现读会说 true 而这个副本根本没有 sink ⇒ 面板画一张空图表并把它当「这段时间没有流量」。
- 🔴 `stats.flushIntervalMs` 报的是**生效值**（运维调过之后必须是调过的那个）。
- 🔴 `stats.tokensCoverage` 由协议目录的 `usagePath !== null` 算出来，**前端不许自己写一份**；
  ⚠️ **只从这一个出口发**：两条用量端点**都不许带它**（同一份知识两个出口，前端就要面对「读哪一个」这个不该存在的问题）。
- `runtime.name` / `storage.backend` / `quota.model` / `process.metrics` 四格**今天各自只剩一个取值、且没有前端消费者**（面板的运行时格子读的是 overview 的），留着是范围取舍。

### 9.2 `GET /admin/api/overview`（**逐块降级**）

```json
{
  "version": "0.4.0",
  "serverTime": 1735689600000,
  "runtime": { "name": "node" },
  "process": { "pid":1, "rssBytes":52428800, "uptimeMs":3600000 } | null,
  "storage": { "backend":"file", "writable":true, "checkedAt":1735689600000 },
  "pool":  { "total":3, "fresh":2, "cooling":1, "evicted":0, "disabled":0 } | null,
  "poolStats": { "requests":42,"success":40,"failed":2,"clientErrors":0,"approximate":true } | null,
  "freshness": {
    "poolCacheTtlMs": 60000,
    "poolVisibilityUpperBoundMs": 60000,
    "poolTouchIntervalMs": 21600000,
    "configTtlMs": 30000,
    "configVisibilityUpperBoundMs": 30000
  },
  "config": {
    "registrarEnabled": true,
    "registrarBlocked": false,
    "channel": "moemail" | null,
    "targetKeys": 20,
    "envLocked": ["gatewayToken"],
    "degraded": false
  }
}
```
- 🔴 **某块失败就该块返回 `null`，绝不伪造 0**（产品不变式）。包 `block()` 的恰好是三处：`repo.all()`、`runtime.process()`、以及从 records 派生的 `poolStats`。
- ⚠️ **`config` 块刻意不包 `block()`**：`ConfigHolder.current()` 契约是「同步读、永不抛」，包一层会让它看起来像参与降级而那个分支**永远走不到**（评审用每个方法都真抛的存储实测过，`config` 仍是完整对象）。
- `poolStats` 只挑 4 个计数字段，**不整段展开 `sumStats()`**（那还带 `lastErrorAt` / `lastErrorKind`，概览不消费，挂没人看的字段迟早会漂）。
- `freshness` 的两条上界**就等于各自的 TTL，中间不许再加任何一项**。
- `config.registrarBlocked` = 「开着、但这份配置本次没装起来」。面板必须 `registrarEnabled && registrarBlocked` 一起判。
- `process` 为 `null` 今天只剩**一个**成因：取进程指标出错。🟡 面板仍把它渲染成「Serverless · 无常驻进程」，**那句文案现在偏了**，属于面板那一面的待办。

### 9.3 `GET /admin/api/models`（零存储读、零网络）

```json
{
  "protocols": [ { "id","label","method","pathTemplate","upstreamPath",
                   "authHeader","streamMode","streamKey","usagePath","streamTextPath",
                   "sampleBody" } ],
  "media":     [ { "id","modality","op","method","pathTemplate","upstreamPath",
                   "authHeader","taskSlot","sampleBody" } ],
  "models":    [ { "id","modality","protocols":[...],"endpoints":[{"method","path"}] } ],
  "samplePrompt": "ping"
}
```
四条协议：`openai` / `anthropic` / `responses` / `gemini`（`usagePath` 分别为 `null` / `["usage"]` / `["usage"]` / `["usageMetadata"]`）。
三条媒体端点：`image.generate` / `video.create` / `video.poll`（后者 `taskSlot: ":id"`）。
模型目录 12 个（6 chat + 3 image + 3 video）。
集成示例卡、Playground、模型表三处都从这里取数，**一个端点路径都不在前端硬编码**。

### 9.4 `GET /admin/api/upstream/models`（**真打一次上游**）

用 `selectKey(repo.all(), 0, now)` 挑一把 key 打 `GET {agnesBaseUrl}/models`，超时用 `upstreamTimeoutMs`。

| 情形 | 状态码 | 响应体 |
|---|---|---|
| 池里没可用 key | **200** | `{ ok:false, status:null, latencyMs:0, reason:"no_key", models:null }` |
| 护栏挡下 | 429 | `{ error:{type:"rate_limit_error",message}, reason:"probe_in_flight"\|"probe_cooldown" }` |
| 上游非 2xx | 200 | `{ ok:false, status, latencyMs, reason:"upstream_error", models:null }` |
| 读不全 / 不是 JSON / 形状不认 | 200 | `reason:"body_incomplete"` / `"bad_payload"` |
| 网络错 / 超时 | 200 | `reason:"network_error"` / `"timeout"`，`status:null` |
| 成功 | 200 | `{ ok:true, status, latencyMs, reason:null, models:{ ids, truncated, onlyUpstream, onlyCatalog } }` |

**零存储写**；读侧消费 `repo.all()` 那份进程内快照。

### 9.5 `POST /admin/api/models/:id/test`（**真打一次对话**）

- **不接受任何请求体**：空体放行；非法 JSON → 400；非对象 → 400；有任何键 → `400 "模型测试不接受任何参数，不认识的字段：…"`（不带 `code`）。
- `:id` 不在模型目录 → `404 "这个网关的模型目录里没有这个模型"`（不带 `code`）。
- 模型 `modality != "chat"` → `400 { error:{type:"invalid_request_error",message:"…"}, reason:"modality_not_testable" }`
  （测图片会真生成一张图，测视频会建任务并轮询，都会真花掉额度）。
- 池里没 key → **200** `{ ok:false, status:null, latencyMs:0, reason:"no_key" }`。
- 护栏挡下 → 429（同 9.4）；⚠️ **它的 `kind` 是常量 `"model-test"`**（整轮测试要互相挡，那才是扛上游边缘限流的闸）。
- 成功 → `{ ok, status, latencyMs, reason: ok ? null : "upstream_error" }`。
- 失败 → `{ ok:false, status:null, latencyMs, reason:"timeout"|"network_error" }`。
- 🔴 **只回状态码，不回正文。**

### 9.6 `GET /admin/api/keys`（零 `list()`、零额外读）

查询参数：`q`（模糊匹配 id / masked，大小写不敏感）、`bucket`（不是合法档位时**整个忽略**）、`page`（1 基，越界回落 1）、`size`（默认 `DEFAULT_SIZE = 20`，上限 `MAX_SIZE = 200`，非法值回落默认）。

```json
{
  "items": [ {
    "id":"…", "masked":"sk-aa…aaaa", "seq":1, "bucket":"fresh",
    "addedAt":…, "lastUsedAt": …|null, "cooldownUntil":0,
    "cooldownReason": null, "evictedReason": null, "strikes":0,
    "disabled": false, "note": null,
    "stats": { "requests":0,"success":0,"failed":0,"clientErrors":0,
               "lastErrorAt":null,"lastErrorKind":null }
  } ],
  "total": 1, "page": 1, "size": 20, "pages": 1,
  "counts": { "all":1, "fresh":1, "cooling":0, "evicted":0, "disabled":0 },
  "approximate": true,
  "generatedAt": 1735689600000
}
```
- `masked` = `key[0..5] + "…" + key[len-4..]`，长度 ≤ 10 时是 `"…"`。
- `seq` 按 `(addedAt, id)` 排序后的 1 基序号。
- 🔴 **投影永不含明文**；明文只走 `/reveal`。
- 🔴 **`counts` 永远按整池算**，不受筛选影响（拿筛完的集合去算就恒等于当前这一档的条数，另外三档全是 0）。
- `pages = max(1, ceil(total/size))`。

### 9.7 `POST /admin/api/keys`（导入）

请求体：`{ keys: string[], resetExisting?: boolean }`，**未知顶层字段一律 400 `unknown_field`**。
`keys` 非字符串数组 → `400 keys_not_a_string_array`；`len > MAX_IMPORT_KEYS = 200` → `400 too_many_import_keys {max:200}`。

```json
200 { "added": ["<id>"], "duplicated": ["<id>"], "invalid": [2], "reset": 0 }
```
- `invalid` 装的是**输入里的位置（1 基）**，不是明文。
- 🔴 **`reset` 与 `duplicated.length` 不是一个数**：本批新建的那把被粘第二遍也算重复，但它谈不上「被重置」。面板显示 `duplicated.length` 就是在撒谎。
- 事件：`key.added`（info，`added/duplicated/invalid` 计数）、`key.restored`（warn，仅 `reset > 0`）。

### 9.8 `POST /admin/api/keys/bulk`

请求体：`{ op: "disable"|"clearCooldown"|"delete", ids: string[] }`，未知字段 400。
`op` 不合法 → `400 not_a_bulk_op {ops:"disable / clearCooldown / delete"}`；`ids` 非字符串数组 → `400 ids_not_a_string_array`；`len > 200` → `400 too_many_bulk_ids {max:200}`。

```json
200 { "results": [ { "id":"…", "ok":true,  "reason": null },
                   { "id":"…", "ok":false, "reason": "not_found" },
                   { "id":"…", "ok":false, "reason": "must_disable_first" } ] }
```
**只有三个动作**，没有「批量启用」「批量解除剔除」——那两个是「让更多 key 重新上场」的动作，逐把点比一次点全部安全。
删除走 `deleteMany(toDelete)` 一次批量落盘。事件里 ids 超过 `BULK_EVENT_IDS_MAX = 20` 时记 `ids:null, idsOmitted:true`。

### 9.9 `POST /admin/api/keys/purge`（危险区）

请求体：`{ expect: <非负整数> }`，未知字段 400（**带 code**）；`expect` 不合法 → `400`（**不带 code**，走网关信封）。

| 情形 | 状态码 | 响应体 |
|---|---|---|
| 池大小对不上 | 409 | `{ error:{type:"conflict",message:"池子在你确认之前变了：你看到的是 N 把，现在是 M 把。一把都没有删，请刷新后重来"}, reason:"pool_size_changed", expected:N, actual:M }` |
| 成功 | 200 | `{ deleted: N, remaining: 0, expected: N }` |

🔴 **路径必须来自单一真源常量 `KEYS_PURGE_PATH = "/admin/api/keys/purge"`**，五语言 DEPLOY.md 的配额账逐份写着它，有测试从常量现算。
🔴 **`expect` 不许改成可选**：枚举式鉴权矩阵会拿正确口令把每条路由真打一遍，`purge` 打通了就是清掉一池 key——它安全的唯一理由就是必填 `expect` 让不带体的 POST 在解析那一步就 400。

### 9.10 `GET /admin/api/keys/:id/reveal`

```json
200 { "key": "<上游 key 明文>" }
404 { "error": { "type":"not_found", "code":"key_not_found", "message":"没有这把 key" } }
```
每次调用**必留痕**：`key.revealed`（warn，只记 id）。
🔴 **明文只走这条专门端点，不进列表响应**：列表是高频、无意识被调用的，塞进去等于每次轮询、每条被记下的响应体、每个中间层缓存里都带着全部凭据明文。

### 9.11 `DELETE /admin/api/keys/:id`

| 情形 | 状态码 | 响应体 |
|---|---|---|
| 不存在 | 404 | `{error:{type:"not_found",code:"key_not_found",message:"没有这把 key"}}` |
| 既没停用也没被剔除 | 409 | `{ error:{type:"conflict",code:"must_disable_first",message:"请先停用这把 key 再删除（删除不可撤销，而停用随时可以撤销）"}, reason:"must_disable_first" }` |
| 成功 | **204**（空体） | |

事件 `key.deleted`（warn，`{id, wasEvicted, wasDisabled}`）。

### 9.12 `PATCH /admin/api/keys/:id`

请求体键集 `PATCH_FIELDS = [disabled, note, clearCooldown, clearStrikes, unevict, clearStats]`，未知字段 400；**空对象 → `400 empty_patch`**。
- `disabled`：状态，`bool`；非 bool → `400 not_a_boolean {field}`；
- `note`：`string | null`；非串 → `400 note_not_a_string`；`len > MAX_NOTE_LENGTH = 200` → `400 note_too_long {max:200}`；
- 后四个是**动作而非状态**：传 `false` 等于没传。

```json
200 { "ok": true }
404 { "error": { "type":"not_found","code":"key_not_found","message":"没有这把 key" } }
```
事件：`disabled === true` → `key.disabled`（warn，文案必须说清「它仍然占着 targetKeys 的名额，不会触发补池」）；`disabled === false || clearCooldown || clearStrikes || unevict` → `key.restored`（info）。

### 9.13 `GET /admin/api/keys/:id/usage`（**Tier-1，与 Tier-2 完全无关**）

```json
200 { "id":"…",
      "stats": {"requests":…,"success":…,"failed":…,"clientErrors":…,
                "lastErrorAt":…|null,"lastErrorKind":…|null},
      "approximate": true, "generatedAt": … }
404 { "error": { "type":"not_found", "message":"没有这把 key" } }   ← 不带 code
```
走 `repo.all()`，**Tier-2 关着时照样可用**。

### 9.14 `POST /admin/api/keys/:id/verify`（**真打一次上游**）

- 不接受任何请求体（同 9.5，报文换成「验活不接受任何参数…」）；
- `:id` 不存在 → `404 "没有这把 key"`（**不带 code**）；
- 护栏 kind = `` `verify:${id}` ``（**逐把 key 一个槽位**），挡下 → 429；
- 用**指定那一把** key 打 `POST {agnesBaseUrl}/chat/completions`，请求体 = openai 协议的 `sample(MODEL_CATALOG[0].id)`，超时 `upstreamTimeoutMs`；拿到响应后**立刻 cancel 响应体**。

```json
200 { "ok": <2xx>, "status": <上游状态码>, "latencyMs": …, "reason": null }
200 { "ok": false, "status": null, "latencyMs": …, "reason": "timeout" | "network_error" }
```
🔴 **只回状态码，不回正文。** **一次存储写都不产生**（只读探针）。
🔴 **不许在 handler 里裸 `fetch`**：必须走注入的 `Fetcher` 端口，否则测试桩不掉、每跑一次就真打一次外网。

### 9.15 `POST /admin/api/registrar/tend`（「立即补池」）

请求体：**可选**。空体 = `{}`；非法 JSON → 400；非对象 → 400；未知字段 → 400（三者都是**不带 code** 的网关信封）。唯一允许的键是 `channel`。

**拒绝支一共七种，一种都不是「这条路由不存在」**（顺序即判定顺序）：

| # | 条件 | 状态码 | `reason` | 附加字段 |
|---|---|---|---|---|
| 1 | `registrar.enabled == false` | 409 | `registrar_disabled` | |
| 2 | `registrar.blocked == true` | 409 | `registrar_blocked` | |
| 3 | `channel` 不是 `moemail`/`yyds` | 400 | `unknown_channel` | |
| 4 | 该通道没配凭据 | 409 | `channel_not_configured` | `channel` |
| 5 | `wiring == null` | 503 | `not_wired` | （并打 `registrar.manual_tend_not_wired` 事件） |
| 6 | 进程内 gate 已被占 | 409 | `tend_in_flight` | |
| 7a | 手动冷却中 | 429 | `manual_cooldown` | `remaining, resetAt, retryAfterMs, cooldownUntil` |
| 7b | 今日次数用尽 | 429 | `write_budget_exhausted` | `remaining, resetAt, retryAfterMs` |
| 8 | 跨副本存储锁被占 | 409 | `locked` | `until` |

成功（**注意：`200` 不是 `202`**）：
```json
200 {
  "started": true,
  "done": true,
  "trigger": "manual",
  "channel": "moemail" | null,
  "outcome": { "kind":"done",    "result": <TendResult>, "capped": <ManualTendCap>|null }
           | { "kind":"skipped", "reason":"disabled"|"blocked", "capped": null }
           | { "kind":"crashed", "error":"…", "capped": <ManualTendCap>|null },
  "remaining": 23, "resetAt": …, "cooldownUntil": …, "retryAfterMs": …
}
```
- 🔴 **`202 → 200` 是一次已发生的破坏性变更**：从前一收到点击就返回、整轮交给响应之后的后台任务，而平台会在响应结束后约 30 秒把它取消掉（3/3 复现）⇒ 补池历史、事件与锁的释放**全部落空**，面板上那一轮与「压根没点过」不可区分。`started: true` 保留给旧面板。
- 🔴 **`outcome.kind` 三种不许被读成同一种「失败」**。
- `remaining` **在成功那一支也照样给**（只在耗尽那一支给它，等于让运维毫不知情地撞上一堵墙）。
- **手动这一轮有自己那一族上限**：轮级预算 70 秒、最多铸 1 把、等码 60 秒、只试 1 个域名，**一律 `min()` 压顶而不是赋值**（运维把 `CODE_TIMEOUT_MS` 调到 30 秒时不该被「抬」回 60 秒）。真被压小时 `capped` 报出压前压后两组数并打 `registrar.manual_round_capped`。
- 存储写：护栏键 + 抢锁 + `tend:history` = **3 次 put**（健康的一轮**不写事件**）。
- `TEND_LOCK_TTL_MANUAL_MS = 180_000`；手动冷却 `MANUAL_TEND_COOLDOWN_MS = 600_000`；每日 `MANUAL_TENDS_PER_DAY = 24`。

### 9.16 `GET /admin/api/registrar/status`（**逐块降级**）

`wiring == null` → `503 reason:"not_wired"`。否则：
```json
{
  "serverTime": …,
  "enabled": true, "blocked": false, "channel": "moemail"|null,
  "channels": { "moemail": {"configured":true,"selected":true},
                "yyds":    {"configured":false,"selected":false} },
  "pool": { "target":20, "counted":18, "gap":2, "fresh":15, "mintBatch":1 } | null,
  "lockedUntil": <number>|null,
  "manual": { "used":1,"remaining":23,"perDay":24,"resetAt":…,
              "cooldownUntil":…|null,"retryAfterMs":…|null } | null,
  "domains": { "total":…|null,"ok":…,"blocked":…,"suspect":…,
               "unknown":…|null,"updatedAt":…|null,"size":…,"cap":… } | null,
  "backoff": { "until":…,"kind":"…","retryAfterMs":…,"since":…,"hits":… } | null,
  "history": { "entries": [ <TendRecord> ], "malformed": 0 } | null
}
```
🔴 **`counted` 刻意不叫 `available`**：判据是「占名额数」，被停用的与正在冷却的 key 都算在里面，而这两种恰恰都不能打上游。真正的可用数是并列的 `fresh`。**两者都住在 `pool` 对象里，不在顶层。**
`pool.mintBatch` 报的是 `min(配置值, MANUAL_MINT_BATCH=1)`。
`lockedUntil` / `backoff` 都只在「还没过期」时才给值，过期给 `null`。

`TendRecord` = `TendResult + { trigger: "cron"|"manual" }`；
`TendResult = { skipped, available, attempted, minted, mintedByChannel, failures:[{reason,channel}], at, primaryChannel, durationMs }`。
历史环大小 `TEND_HISTORY_SIZE = 50`，键 `tend:history`。

### 9.17 `POST /admin/api/registrar/channels/:channel/test`（**真打一次外网**）

**不收请求体**，通道名在路径里。

| 条件 | 状态码 | `reason` |
|---|---|---|
| `:channel` 不是 moemail/yyds | 400 | `unknown_channel` |
| 注册机没开 | 409 | `registrar_disabled`（+ `channel`） |
| 注册机开着但装不起来 | 409 | `registrar_blocked`（+ `channel`） |
| 通道没配凭据 | 409 | `channel_not_configured`（+ `channel`） |
| `wiring == null` | 503 | `not_wired`（+ `channel`） |
| 护栏（kind = `` `channel:${ch}` ``） | 429 | `probe_in_flight` / `probe_cooldown` |

执行两步：① `listDomains()` 数可用域名；② `verifyCredentials(domains[0])` **真验一次凭据**。

```json
200 { "ok": true,  "channel":"moemail", "domains": 3, "latencyMs": …,
      "credentials": "accepted" | "not_checked", "cleaned": true }
200 { "ok": false, "channel":"…", "reason": "not_attempted"|"credentials_rejected"
                              |"rate_limited"|"upstream_error", "latencyMs": …,
      "status": <上游状态码，拿不到时这一格不出现> }
409 { …, "reason": "registrar_disabled"|"registrar_blocked"|"channel_not_configured" }
```
- 一个域名都没读到时**不去验凭据**，`credentials` 如实回 `"not_checked"`（硬编一个域名就是伪造；没域名可用时补池本来就会失败）。
- ⚠️⚠️ **「这一步会不会消耗名额」由适配器决定，这一层不知道也不该知道**：YYDS 那条实现是「建一个再删掉」⇒ 每点一次消耗一个活跃邮箱名额；MoeMail 那条零名额。**这个差别不许被搬到 handler 里做分支**。`cleaned: false` 说明建出来了却没删掉，面板要让运维去事件板块找 `registrar.delete_mailbox_failed` / `registrar.mailbox_create_unparseable`。
- **失败的分档依据是这一次的状态码，不是通道名**：`401/403 → credentials_rejected`、`429 → rate_limited`、其余 → `upstream_error`。
- `not_attempted`（`probe_setup_failed`）是**独立的一档**：读本网关自己的配置就失败了，**一次上游请求都没发出去**。🔴 **不许并进任何一个上游档**——那是一句把本网关自己的故障说成上游故障的假话，处置方向正好相反。
- **一次存储写都不产生**。

### 9.18 `GET /admin/api/config`

`wiring == null` → `503 reason:"not_wired"`。
```json
{
  "fields":      { "<path>": { "stored": <原始值>, "env": "<原始串>"|null,
                               "effective": <生效值>, "lockedBy": "env:NAME"|null } } | null,
  "credentials": { "<path>": { "configured": true, "hint": "abcd"|null,
                               "lockedBy": "env:NAME"|null } } | null,
  "configDegraded": false,
  "loadBlocked":  [ { "field":"…","code":"…","params":{…} } ],
  "loadNotices":  [ … ],
  "editable":     [ "<EDITABLE_FIELDS 排序后全表>" ],
  "secrets":      [ "gatewayToken","registrar.moemail.apiKey","registrar.yyds.apiKey" ],
  "resetBlocked": [ … ],
  "propagation":  { "configTtlMs": 30000, "visibilityUpperBoundMs": 30000 }
}
```
- **公开字段与凭据字段分两个对象**：凭据只报「配没配 + 末 4 位」，**永不报值**；`hint` 在原值短于 5 位时是 `null`（给了就是给全，那就不叫 hint 了）。
- `fields` / `credentials` 同时为 `null` = **诊断视图**（存储里那份配置读得出来但构造不出合法配置），此时 `loadBlocked` 至少有一条（逐字段判据说不出是哪格时兜一条 `config_unloadable`，并打 `config.unloadable` 事件）。
- `loadBlocked`（注册机本次没跑起来）与 `loadNotices`（照常跑、但必须说出来的话，今天只有旧主备键那一族）是**平行的两格，处置完全不同**。
- `resetBlocked` = 「按下重置按钮之后会变成什么样」的预判。

**可编辑字段全表（`EDITABLE`，25 条）**：
```
gatewayToken(secret) agnesBaseUrl(url) upstreamTimeoutMs(int≥1) upstreamSyncTimeoutMs(int≥1)
maxStrikes(int≥1) cooldownRateLimitMs(int≥1) cooldownPaymentMs(int≥1) cooldownStrikeMs(int≥1)
poolCacheTtlMs(int≥0) poolTouchIntervalMs(int≥0)
registrar.enabled(bool) registrar.channel(channelOrNull) registrar.targetKeys(int≥1)
registrar.mintBatch(int≥1) registrar.tendIntervalMs(int≥1) registrar.codeTimeoutMs(int≥1)
registrar.mintDelayMinMs(int≥1) registrar.mintDelayMaxMs(int≥1) registrar.maxDomainAttempts(int≥1)
registrar.tokenName(text) registrar.agnesPlatformUrl(url)
registrar.yyds.baseUrl(url) registrar.yyds.apiKey(secret)
registrar.moemail.baseUrl(url) registrar.moemail.apiKey(secret)
```
（`poolCacheTtlMs` / `poolTouchIntervalMs` 的 `0` 是**逃生口，不是越界值**。）

**`ConfigErrorCode` 全表（22 个）**：
```
unknown_field, locked_by_env, not_an_integer, below_min, not_a_string, not_a_boolean,
empty, too_long, not_a_url, not_a_channel, channel_required, delay_min_gt_max,
channel_credentials_missing, legacy_channel_key, legacy_channel_env,
legacy_fallback_ignored, gateway_token_required, whitespace_padded, not_sendable,
too_short, same_as_admin_token, config_unloadable
```
`ConfigError = { field, code, params? }`，**面板靠 `code` 选五语言文案，不解析 message**。

**env 锁定映射（`ENV_LOCK_MAP`，27 条）**：见上表字段名 ↔ `GATEWAY_TOKEN / AGNES_BASE_URL / UPSTREAM_TIMEOUT_MS / UPSTREAM_SYNC_TIMEOUT_MS / MAX_STRIKES / COOLDOWN_* / POOL_* / USAGE_STATS_ENABLED / REGISTRAR_ENABLED / REGISTRAR_CHANNEL(+旧名 REGISTRAR_PRIMARY) / TARGET_KEYS / MINT_BATCH / TEND_INTERVAL_MS / CODE_TIMEOUT_MS / MINT_DELAY_MIN_MS / MINT_DELAY_MAX_MS / MAX_DOMAIN_ATTEMPTS / REGISTRAR_TOKEN_NAME / AGNES_PLATFORM_URL / YYDS_* / MOEMAIL_*`。
⚠️ `USAGE_STATS_ENABLED` 在 `ENV_LOCK_MAP` 里但**不在 `EDITABLE` 里**（存储里能改、面板不能改）。
⚠️ `ADMIN_TOKEN` / `TRUST_PROXY` / `RESET_CONFIG` / `USAGE_FLUSH_INTERVAL_MS` / `APIKEY_CACHE_TTL_MS` **是 env-only，不进配置面**——它们在存储里根本改不了，所以**没有 `stored` 那一格可以撒谎**。

### 9.19 `PUT /admin/api/config`

请求体：`{ patch: { "<path>": <value> } }`，顶层未知字段 → 400（不带 code）；`patch` 非对象 → 400。

- 校验不通过 → `400 { error:{type:"invalid_request_error",message:"配置校验没通过，本次一个字节都没有写入"}, errors:[ConfigError] }`。
  🔴 **纯函数校验、写之前跑、失败一个字节都不写。**
- 成功 → 200，响应体 = `GET` 的 `fields/credentials/configDegraded/loadBlocked/loadNotices` +
  `changed`（**生效值真的变了的公开字段**，排序）+ `credentialsChanged`（变动里属于凭据的那几条）+
  `appliedAt`（**服务器落盘那一刻，不是「已生效」的承诺**）+ `propagation`。
- 🔴 **凭据字段传空串一律是「不改」，不是清空**。清空只有 `secrets/clear` 一条路——把空串实现成清空，后果是运维保存一次设置页就抹掉了 `gatewayToken`，而热实例当场看不出任何异常。
- `lockedBy != null` 的字段被写 → `locked_by_env` **拒绝，不静默接受**。
- 成功后必须 `configHolder.invalidate()`。
- 事件 `config.updated`（warn，`{fields, changedEffective, secrets: <凭据变动条数>}`）。

### 9.20 `POST /admin/api/config/validate`（干跑）

请求体与 `PUT` **逐字相同**。不通过 → 同 400；通过 → `200 { ok:true, changed:[...] }`。**零写。**

### 9.21 `POST /admin/api/config/secrets/clear`

请求体：`{ path: "<SECRET_FIELDS 之一>" }`，其余字段 400；`path` 不是凭据 → 400（都不带 code）。

```json
200 {
  "cleared": "registrar.yyds.apiKey",
  "stillConfigured": false,      // env 里还有没有一份
  "gatewayTokenMissing": false,  // 清完之后 gateway_token_required 了吗
  "loadBlocked": [ … ],
  "fields": …, "credentials": …, "configDegraded": …,
  "resetBlocked": [ … ], "propagation": { … }
}
```
事件 `config.secret_cleared`，级别随 `blockedAfter.length` 在 warn / error 之间切，**文案按三档分岔**（清完装不起来 / 只是注册机停摆 / env 里还有一份 / 一切正常）——一句话糊四档是本仓反复裁过的形态。

### 9.22 `POST /admin/api/config/reset`（危险区）

请求体：`{ confirm: true }` **必须显式带**，其余字段 400，`confirm != true` → `400 "这一步不可撤销，必须显式带 confirm: true"`。
把存储里的 `config` 键整把写成 `{}`；响应体 = `fields/credentials/configDegraded/loadBlocked/changed/credentialsChanged/resetBlocked/appliedAt/propagation`。
`credentialsChanged` 算的是「重置前 `configured==true` 而重置后不再是」的那几条。
事件 `config.reset`。路径来自真源常量 `CONFIG_RESET_PATH`。

### 9.23 `GET /admin/api/usage`（Tier-2 区间）

查询参数 `from` / `to`：**epoch 毫秒非负安全整数**；非法 → `400`（不带 code）；`from > to` → 400。
默认 `to = now`，`from = to - 86400000`。晚于当前时刻的 `to` 夹到 now；早于保留期起点的 `from` 夹到起点；**任一被夹 ⇒ `range.clamped = true`**。

```json
{
  "tier": "tier2" | "off",
  "timezone": "UTC",
  "approximate": true,
  "generatedAt": …,
  "range": { "from": <fromDay 起点 ms>, "to": <toDay 终点 ms>, "clamped": false } | null,
  "days":  [ { "date": "2026-09-10", "total": <UsageBucket> } ] | null,
  "total": <UsageBucket> | null,
  "byApiKey": { "<id|master|unattributed>": <UsageBucket> } | null,
  "shards": 2 | null,
  "malformed": 0 | null,
  "pending": { "count": 0, "ms": 0, "budgetExhausted": false } | null,
  "note": <UsageNote> | null
}
```
`UsageBucket = { requests, success, errors, tokensIn, tokensOut, streamingRequests, latencySum, latencyCount }`。

**`UsageNote` 全表（9 个），🔴 不许混成同一句「没有数据」**：
`tier2_off`、`clock_unavailable`、`read_failed`、`range_clamped`、`date_out_of_retention`、`no_shards`、`all_malformed`、`partial_malformed`、`no_request_detail`。

判定优先级：`malformed>0 → (shards==0 ? all_malformed : partial_malformed)` → `clamped → range_clamped` → `shards==0 → no_shards` → `null`。

- 🔴 **Tier-2 关着照常回 200 + `tier:"off"`**（回 503 会让面板把「运维没打开统计」渲染成「后端坏了」）。
- 🔴 **本端点不带 `tokensCoverage`**（同一份知识只许一个出口，那个出口是 capabilities）。
- ⚠️ 最贵的是**读扇出**：30 天那一档一次请求发 `30 × USAGE_SLOTS = 60` 次 get，而每次 get 是整份 `store.json` 的读+解析。那一档**必须失败得诚实**（`read_failed`），不许半截。

### 9.24 `GET /admin/api/usage/{date}`（Tier-2 单日详情）

`date` **只认 UTC 的 `YYYY-MM-DD`**（且必须能往返），否则 `400`（不带 code）。
⚠️ **与区间端点的口径刻意不同**：那条只认 epoch 毫秒整数，这条只认日期串，**两者不通用**。

```json
{
  "tier":"tier2"|"off", "timezone":"UTC", "date":"2026-09-10",
  "approximate": true, "generatedAt": …,
  "hours":      { "00".."23": <UsageBucket> } | null,
  "byModel":    { "<model>": <UsageBucket> }  | null,
  "byProtocol": { "openai"|"anthropic"|"responses"|"gemini": <UsageBucket> } | null,
  "byApiKey":   { "<id>": <UsageBucket> }     | null,
  "shards": …|null, "malformed": …|null,
  "note": "tier2_off"|"clock_unavailable"|"date_out_of_retention"|"read_failed"|"no_request_detail"
}
```
成功那一支的 `note` 恒是 `no_request_detail`（本端点给的是聚合，没有逐请求明细）。

### 9.25 `GET /admin/api/apikeys`

`wiring == null` → `503 reason:"not_wired"`。
```json
{
  "unreadable": false,
  "version": 7,
  "keys": [ { "id","name","seq","masked","hint","bucket","disabled","createdAt","expiresAt" } ],
  "counts": { "all":1, "active":1, "disabled":0, "expired":0 },
  "max": 200,
  "cacheTtlMs": 300000
}
```
- `unreadable: true` ⇒ `version: null`、`keys: []`、`counts` 全 0。
  🔴 **它不等于「一把都没有」**，面板必须按这一格分支渲染。
- `bucket ∈ {active, disabled, expired}`；`seq` 按 `(createdAt, id)` 排序的 1 基序号。
- `masked` 由 `hint` 派生，**响应里没有明文**。
- `version` 是**乐观并发**的版本号，三条写端点都要原样带回来。

### 9.26 `POST /admin/api/apikeys`（签发）

请求体：`{ name: string, expiresAt?: number|null }`，未知字段 400。
- `name`：非串 → `name_not_a_string`；trim 后空 → `name_empty`；`> API_KEY_NAME_MAX = 64` → `name_too_long {max:64}`；
- `expiresAt`：非整数毫秒且非 null → `expires_not_a_number`；已过去 → `expires_in_the_past`（签发一把生下来就过期的密钥不是有意义的动作）；缺席 = `null`（不过期）；
- 表满（`APIKEY_MAX = 200`）→ `400 too_many_apikeys {max:200}`；
- 表结构不认 → `409 apikeys_unreadable`（**拒绝落盘，以免覆盖掉里面还留着的内容**）。

```json
201 { "secret": "sk-<32位十六进制>", "record": <ApiKeyView>, "version": <next> }
```
- 明文格式 `sk-` + 32 hex（**128 bit**，`crypto.getRandomValues`）。
  🔴 **绝不许改成 `Math.random()`**——可预测的伪随机等于没有密钥。
- `id` = 6 字节 hex，**独立随机，与密钥没有任何函数关系**（id 会进 URL 与事件日志）。
- 存储里同时存 `hash`（SHA-256 摘要，用于鉴权）与 `secret`（明文，用于 reveal）与 `hint`（末 4 位）。
- 成功后必须 `holder.invalidate()`。事件 `apikey.issued`（warn，只记 `{id, expiresAt}`）。

### 9.27 `GET /admin/api/apikeys/{id}/reveal`

```json
200 { "secret": "sk-…" }
200 { "secret": null, "reason": "issued_before_plaintext" }
404 { "error": { "type":"not_found","code":"apikey_not_found","message":"没有这把对外 API 密钥" } }
```
事件 `apikey.revealed`（warn，`{id, available}`）。
`capabilities.apiKeys.plaintextRetrievable` 说的是「**这个部署能不能取回明文**」，不是「每一把都取得回来」。

### 9.28 `PATCH /admin/api/apikeys/{id}`

请求体：`{ version: number（必填）, name?, disabled?, expiresAt? }`，未知字段 400；三个可选字段一个都没给 → `400 empty_patch`。
- `version` 非非负整数 → `400 version_not_a_number`；与当前表版本不符 → `409 stale_write {expected, actual}`，**一个字节都不写**；
- `expiresAt`：🔴 **显式传 `null` 是「改成不过期」，缺席才是「这次不动它」**。

```json
200 { "ok": true, "record": <ApiKeyView>, "version": <next> }
```
事件 `apikey.disabled` / `apikey.enabled`。停用文案必须写「别的实例最多还要一个缓存周期才看得见」。

### 9.29 `DELETE /admin/api/apikeys/{id}`

⚠️ **`version` 走查询参数**（`?version=7`），不是请求体。缺失/非法 → `400 version_not_a_number`；不匹配 → `409 stale_write`。
成功 → **204**（空体）。事件 `apikey.deleted`（warn，文案写「拿着它的客户端最多再过一个缓存周期就会开始 401」）。

### 9.30 `POST /admin/api/apikeys/purge`（危险区）

请求体：`{ version: number }`（**必填**，理由同 9.9：鉴权矩阵会真打一遍），其余字段 400；不匹配 → `409 stale_write`。
删掉全部**当前不可用**（停用 / 过期）的记录。
```json
200 { "deleted": 3, "remaining": 5, "version": <next> }
```
路径来自真源常量 `APIKEYS_PURGE_PATH`。

> ⚠️⚠️ **`/admin/api/keys` 与 `/admin/api/apikeys` 是两回事，别读混**：
> 前者是**我们持有的上游凭据**，后者是**我们签发的、别人拿来向我们证明身份的凭据**。
> 刻意不共用 `/admin/api/keys` 前缀，因为那条已经是发布过的公开契约。

### 9.31 `GET /admin/api/events`

查询参数：`after`（游标，**负数一律当缺失**）、`level`（不是合法级别时**整个忽略**）、`limit`（默认 `DEFAULT_LIMIT = 200`，上限 `MAX_LIMIT = 500`）。

```json
{
  "items": [ { "ts","level","event","msg","corr"?,"fields"? } ],
  "cursor": <number> | null,
  "shardId": "a1b2",
  "buffered": 0,
  "dropped": 0,
  "budgetExhausted": false,
  "truncated": false,
  "malformed": 0,
  "cursorAhead": false,
  "generatedAt": …
}
```
- 归并结果按 `ts` **降序**，`cursor = items[0].ts`。
  🔴 **`cursor` 只有两种合法值：有限数字，或 `null`；永远不许是「字段不存在」**——
  实测后果：游标就此冻结，稳态读吞吐 276,480 次/天 = 包线的 3.9 倍，且默认档位下不会自愈。
- `shardId/buffered/dropped/budgetExhausted` 是**本实例**的自述状态（多副本下相邻两次轮询可能落到不同副本，带 `shardId` 面板才能说清「这句话说的是哪一个实例」）。
- `truncated` = 过滤/截断确实丢掉了本该出现的旧事件。
- `malformed` **恒为 0——「恒为 0」正是它的价值**：运维手改存储 / 值损坏时，它是**唯一**能把「面板少了几条事件」和「本来就没有事件」区分开的信号。**刻意不进黄条判据**（挂一个永远为假的分支上去没有意义），走轮询指示灯的 tooltip。
- `cursorAhead` = 游标所在窗口晚于本次请求的时钟（空结果不代表没有事件，是时钟纠纷）。
- ⚠️ **`items` 里的字段一律视为完全不可信**（含未鉴权请求的路径），这里**不做**转义或截断，前端一律 `textContent` 渲染。

事件环：`EVENT_WINDOW_RETAIN = 24` 个窗口 × `EVENT_SLOTS = 2` 槽位，候选键数**结构性有界**，不随部署年龄增长。

### 9.32 `GET /admin/api/events/download`

```
200 text/plain; charset=utf-8
content-disposition: attachment; filename="agnes2api-events.txt"
<逐行 JSON.stringify，不是一个 JSON 数组>
```
同样 clamp 到 `MAX_LIMIT = 500`。
⚠️ **刻意返回裸 `Response`**（不是框架的 `c.text()`）：它是「全局 nosniff 必须写在 `next()` 之后」那条性质在生产路由上的可观测锚点——本端点自己**不**设 nosniff，全靠那条全局中间件。

---

## 10. 出站探测护栏（`probe-guard.ts`）

```
PROBE_MIN_INTERVAL_MS = 3_000
tryAcquire(kind, now) -> Ok | Err{ reason: "probe_in_flight" | "probe_cooldown", message }
release(kind)
```
- **进程内、按 `kind` 分槽**，`kind` 三族：`` `verify:${id}` ``（逐把 key）、`` `channel:${ch}` ``（逐通道）、常量 `"model-test"` / `"upstream-models"`。
- 🔴 **一把 guard 交给全部四条探测端点**，在 `adminRouter` 里 `new` 一次。
  **刻意不做成可注入依赖**（`tendGate` 是那样的）：`tendGate` 要被定时轮与面板共用所以必须外部注入；探测护栏没有第二个持有者，做成可注入只会多出「两条端点各拿到一把」这种半装配形态。
- 全局约束：**「按一下就打上游的按钮，必须在同一个任务里连同护栏一起交付」**。

## 11. 补池锁（`tend-lock.ts`）—— **两把锁不是冗余**

| | `TendGate`（进程内） | `registrar_tend_lock`（存储） |
|---|---|---|
| 作用域 | 单进程：定时轮 vs 面板按钮 | 跨副本（多容器共卷） |
| 实现 | 一个 `inFlight` 布尔 + 一次性 `leave()` | `{ until }` 键，TTL 到期自然失效 |
| TTL | — | 定时轮 `SCHEDULED_ROUND_WALL_CLOCK_MS`；手动 `180_000` |
| 拒绝 | `409 tend_in_flight` | `409 locked` + `until` |

`acquireTendLock` 是「读 → 判过期 → 写」，**没有 CAS**（`FileStorage` 给不出）。
这一条必须如实写进文档：**丢更新的窗口消灭不了，只是被降级了。**

---

## 12. 交叉验证：文档 vs 实现的不一致（**本轮发现**）

以 `docs/zh-CN/API.md` 为准逐条核对（五语言同源，抽查 en 一致）：

| # | 位置 | 文档说 | 实现是 | 判定 |
|---|---|---|---|---|
| 1 | `GET /admin/api/capabilities` 响应样例 | `"runtime": { "name":"node", "colo": null }` | **没有 `colo` 这一格**（v0.4.0 删掉了，代码注释里明写） | 🔴 **文档过期**，Rust 版不许把 `colo` 加回来 |
| 2 | 同上 | 样例里**完全没有 `apiKeys` 块** | 实现有 6 个字段的 `apiKeys` 块，且面板靠它显隐整个板块 | 🔴 **文档漏字段** |
| 3 | 同上标题句 | 「**双运行时差异**的唯一出口」 | 只剩一种运行时，代码注释已认账「那个定位没了」 | 🟡 文案过期 |
| 4 | 同上 `flushIntervalMs: 60000` | 60 秒 | 后端常量是 `7_200_000`（2 小时） | 🟡 样例值误导（该格确实可配，但样例应给默认值） |
| 5 | `GET /admin/api/overview` 响应样例 | `config` 块只有 5 格 | 实现有 6 格，多 **`registrarBlocked`** | 🔴 **文档漏字段**，而面板必须两格一起判 |
| 6 | ~~同上 `pool`~~ | ~~`{ total, fresh, … }`~~ | **实现确实有 `total`**（`poolHealth()` 返回 `{ total: records.length, fresh, cooling, evicted, disabled }`） | ✅ **一致，本轮误判后已自查订正**，如实登记在此 |
| 7 | `GET /admin/api/keys` 响应样例 | `counts: { fresh, cooling, disabled, evicted }` | `bucketCounts()` **多一格 `all`** | 🔴 **文档漏字段**（同一份文档在 apikeys 那条里又写了 `all`，自相矛盾） |
| 8 | 同上 `items` 样例 | 7 格 | `KeyView` 有 **13 格**（缺 `addedAt/lastUsedAt/cooldownUntil/cooldownReason/evictedReason/stats`） | 🟡 样例刻意缩写，但读者会当全集 |
| 9 | `GET /admin/api/models` 响应样例 | `protocols` 5 格、`media` 3 格、`models` 2 格 | 实际 11 / 9 / 4 格 | 🟡 样例缩写 |
| 10 | 全局 | 文档未说明**同一棵管理树上存在两种错误信封**（带 `code` / 不带 `code`） | 实现确实混用（§4.4） | 🟡 **文档缺口**，而面板要靠 `code` 查字典 |

**1 / 2 / 5 / 7 四条已逐语言核验：五份 API.md（en / ja / ko / zh-TW / zh-CN）全部同错**
（`colo` 五份都在第 577 行；`apiKeys` 块、`registrarBlocked`、`counts.all` 五份都是 0 次命中）
—— 是系统性的文档漂移，不是某一份漏改。

> **处置建议**：P2（面板对标）之前，把 1/2/5/7 这四条在 Rust 版的新文档里修正；
> 8/9 保留缩写但加一句「样例非全集，字段全集见 §…」；10 用一张表把两族端点点名。

**另外登记两条「文档与代码注释互相矛盾」**（不影响线上行为，但会误导移植）：
- `client-ip.ts` 的排序理由：代码里已认账「原理由在 v0.4.0 没了，现在是一个有代价的取舍」，但五语言 DEPLOY.md 里那条「在反代上剥掉 `CF-Connecting-IP`」仍被写成边角提醒 —— **它今天是排序成立的前提**。
- `USAGE_FLUSH_INTERVAL_MS` 与 `APIKEY_CACHE_TTL_MS` 是 env-only、不进配置面，
  运维在设置页**看不到**它们当前是多少；只能经 `capabilities` 的 `stats.flushIntervalMs` / `apiKeys.cacheTtlMs` 间接看到。这条代价 TS 版明写了，Rust 版不许悄悄"改进"成可编辑（那会立刻掉进「四元组撒谎」形态）。

---

## 13. Rust（axum 0.8）移植要点

| TS 概念 | Rust 对应 | 注意 |
|---|---|---|
| `Hono` 子 app + `app.route("/", sub)` | `Router::merge` / `Router::nest` | 管理树用 `nest("/admin")` 时注意 `/favicon.ico` **不在** `/admin` 前缀下 |
| `app.use("*", mw)` | `Router::layer(middleware::from_fn(...))` | ⚠️ axum 的 layer 是**由外向内**包，顺序语义与 Hono 的 `use` 列表**相反**，必须重新推一遍「谁先跑 / 谁先收尾」 |
| `app.onError` | `HandleErrorLayer` / 自定义 `IntoResponse` 错误类型 | 必须同时打事件 + 返回固定文案 |
| `app.notFound` | `Router::fallback` | 405+`Allow` 需要自己算（axum 的 `MethodRouter` 其实会自动回 405，**但不带 `Allow` 头时要补**） |
| `c.set("apiKeyId", …)` | `req.extensions_mut().insert(ApiKeyId(..))` | 用 newtype，别用裸 `String` |
| `ConfigHolder` | `Arc<ConfigHolder>` 内含 `RwLock<Arc<GatewayConfig>>` + `AtomicU64` 时间戳 | `current()` 必须**同步、永不 panic** |
| `Refreshable` | 同上；`ensure_fresh` 用 `tokio::sync::Mutex` 防并发重载 | ⚠️ `everLoaded == false` 时**不推进计时**这条要照搬 |
| `errorResponse()` | 一个 `AppError` 枚举 + `IntoResponse` | **三种信封分三个类型**，别用一个带 `Option<code>` 的结构（TS 版明确写了「合成一个之后，顺手给网关那族也传一个码就只差一个参数」） |
| `constantTimeEqual` | `subtle::ConstantTimeEq` | |
| `crypto.getRandomValues` | `rand::rngs::OsRng` / `getrandom` | 🔴 绝不用 `rand::thread_rng` 的非 CSPRNG 变体 |
| `Object.create(null)` 那条 | `HashMap` 天然没有这个问题 | 但 **键数上界 + 溢出归并** 那一半必须照做 |
| 四条协议的 `recordUsage` 求值时机 | Rust 里参数是 eager 求值的，**同样会踩** | 把 `UsageOutcome` 的构造放进 `if let Some(sink)` **之内**，或让字段取值本身不可能 panic |

**建议的 crate 分层**（与 kiro2api 的形状对齐）：
```
src/http/
  app.rs            // createApp：路由装配 + 中间件顺序（本文 §2/§3 是它的规格）
  wire.rs           // buildApp：env + storage → 全套依赖
  errors.rs         // 网关信封 + dispatcher 信封
  client_ip.rs
  config_holder.rs  apikey_holder.rs  apikey_store.rs
  usage_sink.rs
  middleware/{auth.rs, config_refresh.rs, log_flush.rs, usage_flush.rs, nosniff.rs}
  routes/{health.rs, openai.rs, anthropic.rs, gemini.rs, responses.rs, media.rs}
  admin/
    mod.rs          // adminRouter：31 条端点的注册顺序（§3.3）
    auth.rs errors.rs probe_guard.rs tend_lock.rs
    handlers/{session,keys,keys_write,api_keys,capabilities,overview,models,
              upstream_models,model_test,events,config,usage,registrar,verify}.rs
```

---

## 14. 这一轮**没有**抽取的部分（交给别的批次）

| 内容 | 在哪 | 为什么不在本文 |
|---|---|---|
| `dispatch()` 的选 key / strike / 冷却 / 重试算法 | `src/core/dispatcher.ts`、`keypool.ts`、`keypool-repo.ts` | 那是「转发内核」批次；本文只抽它**对外的响应形状**（§4.3） |
| 四协议的请求/响应结构转换 | `src/core/protocol/*.ts` | 同上；本文只抽路由层的校验与超时档 |
| 注册机执行体（`tendOnce` / `mintOne` / 域名台账 / 退避） | `src/core/registrar/*` | P3 批次；本文只抽它**经 HTTP 暴露出来的三条端点** |
| 存储层 `FileStorage` 的写队列 / TTL / `store.json` 整份重写 | `src/adapters/storage-file.ts` | 存储批次；本文只引用它的性质（无 CAS、整份重写） |
| 面板前端（`admin-ui/`） | — | P2 批次，且已定「UI 完全对标 kiro2api」 |
| 五语言 i18n 键与文案 | `admin-ui/js/i18n` + `docs/*/` | P2 批次 |
