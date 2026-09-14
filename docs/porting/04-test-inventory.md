# 04 · 判据资产盘点与重建裁定

> 对象：`agnes2api`（TypeScript，v0.4.0）的 `tests/` 目录。
> 用途：这份清单是 **P1~P3 的验收底稿**——Rust 版每完成一块，回来对着这里逐条销账。
> 口径：**只盘点、只裁定，不动源仓一个字节。**

---

## 0. 口径与方法（先说清数字是怎么来的，免得后人信一个假的总数）

- **文件数**：`tests/` 下 183 个文件 = **166 个 `*.test.ts`** + 16 个非用例文件
  （`tests/global-setup.ts` + `tests/helpers/` 下 15 个夹具）。
- **格数**：**5243 格**。这个数**不是 grep 数 `it(` 得来的**——直接 grep 只能数到 4318 格，
  因为大量用例走 `it.each` / 循环生成。本清单的 5243 来自
  `vitest list --reporter json`（vitest 自己报出来的收集结果，ground truth），
  再按 ` > ` 切出文件名统计。**期望侧与实际侧不是同一条推导链**，这是这个仓自己的纪律。
- **分类原则**：按**判据的性质**分，不按它将来落在哪一期。
  所以「面板资产生成流水线」归第 2 类（治理），但它的对象要等 P2 才存在——该标的地方都标了。

---

## 1. 分类计数表

| 类别 | 文件数 | 格数 | 占比（格） | 裁定 |
|---|---:|---:|---:|---|
| **① 业务规则判据**（网关该怎么行为） | **97** | **1974** | 37.7% | **必须在 Rust 里重建，一格不少** |
| **② 元判据 / 仓库治理判据** | **23** | **1758** | 33.5% | **全留**，但分三种落法（见 §4） |
| **③ TypeScript 特有的** | **0** | **≈0** | ~0% | 见 §5——**这一类实际上是空的**，别指望靠它减负 |
| **④ 前端纯函数 / DOM 判据**（P2） | **46** | **1511** | 28.8% | **全留**，随 P2 面板一起落 |
| 合计 | 166 | 5243 | 100% | |

> ⚠️ 第 ③ 类为 0 是一个**结论**，不是没看。理由在 §5，读完再决定要不要信。

按目录的对照（便于交叉核对）：

| 目录 | 文件 | 格 | 归类 |
|---|---:|---:|---|
| `tests/contract/` | 42 | 823 | 全部 ① |
| `tests/unit/`（根） | 50 | 2248 | 26 个 ①（445 格）／23 个 ②（1758 格）／1 个 ④（`i18n-dict` 56 格） |
| `tests/unit/admin/` | 14 | 330 | 全部 ① |
| `tests/unit/registrar/` | 15 | 376 | 全部 ① |
| `tests/ui/` | 25 | 990 | 全部 ④ |
| `tests/ui/dom/` | 20 | 465 | 全部 ④ |

另有 **16 个非用例文件必须一并重建**（它们不计入 5243，但少了任何一个，上面的格子都写不出来）：
`tests/global-setup.ts`（收集门禁，见 §4.3）、`tests/helpers/` 下的
`make-app.ts`、`fake-storage.ts`、`fake-fetcher.ts`、`fake-mailbox.ts`、`fake-dom.ts`、
`counting-storage.ts`、`recording-logger.ts`、`strip-comments.ts`、`css-decls.ts`、
`doc-glossary.ts`、`gate-tables.ts`、`internal-ref-placeholders.ts`、`readme-sections.ts`、
`registrar-grid.ts`、`ship-docs.ts`。

---

## 2. 第 ① 类逐文件清单（97 个文件 / 1974 格）

格式：`文件 [格数] — 它钉的是什么行为`。
标 🔴 的是**记录着真实事故**的文件（判定标准见 §3）。

### 2.A `tests/contract/` — 整条 app 起来的端到端契约（42 个 / 823 格）

| # | 文件 | 格 | 它钉的是什么行为 |
|---:|---|---:|---|
| 1 | 🔴 `admin-apikeys-quota.test.ts` | 8 | 拿错口令打业务口时，「子密钥表不存在 / 读不出来」也必须当成一份合法快照按 TTL 缓存；1000 次错口令只许产生 **1 次**存储读。 |
| 2 | `admin-apikeys.test.ts` | 19 | 对外 API 密钥签发后真能用、停用/过期/删除后真用不了；明文只在签发那一次出现；主口令是逃生口；写侧走乐观并发 + 数量上限。 |
| 3 | 🔴 `admin-auth.test.ts` | 70 | 管理口令常数时间比较；口令形状校验不能只查空白与长度（否则会装出一棵进不去的面板）；未设 `ADMIN_TOKEN` 时整棵 `/admin` **404**，不泄漏「这里有个后台」。 |
| 4 | `admin-capabilities.test.ts` | 6 | `GET /admin/api/capabilities` 如实报出本部署开了哪些能力，面板据此隐藏板块。 |
| 5 | 🔴 `admin-config.test.ts` | 41 | 凭据只写不读；`FIELD_EXPOSURE` 双向哨兵（标 secret 的不许出现在响应里、标 public 的必须出现）；写入前校验；保存之后同一个进程立刻回读到新值。 |
| 6 | `admin-danger.test.ts` | 18 | 危险区两条端点（配置重置 / key 池清空）的鉴权与真实作用面。 |
| 7 | 🔴 `admin-events.test.ts` | 38 | 事件流读取与下载：游标契约、`limit` 截断要如实报 `truncated`、负数/未来 `after` 的处置、畸形条目不许打死端点、**响应返回前必须已经 flush**（fire-and-forget 会让刚发生的事读不到）。 |
| 8 | 🔴 `admin-keys-write.test.ts` | 58 | key 池写侧：批量导入上限、删除、`PATCH` 可改字段白名单、边界常量。 |
| 9 | `admin-keys.test.ts` | 15 | `GET /admin/api/keys` 的分页、默认/最大 size、明文不出现；`disabled` 字段缺席是常态。 |
| 10 | `admin-model-test.test.ts` | 20 | 「测模型」打哪条上游、用哪把 key、带哪个模型；只测对话模型；整轮共用一个探测闸所以互相挡；一个字节上游正文都不回。 |
| 11 | `admin-models.test.ts` | 6 | `GET /admin/api/models` 回本网关自己的模型目录。 |
| 12 | 🔴 `admin-overview.test.ts` | 12 | 概览：存储坏掉时字段降级成 `null` 而不是硬编码 0；响应里没有消费者的字段不许留（会漂）。 |
| 13 | 🔴 `admin-registrar.test.ts` | 51 | 注册机状态、通道连通性测试、立即补池的 `channel` 参数（「只用这一条通道」），以及这些在 `wire.ts` 上的**真装配**（不是照抄一遍装配去验装配）。 |
| 14 | `admin-session.test.ts` | 3 | `GET /admin/api/session` 的会话信息。 |
| 15 | 🔴 `admin-upstream-models.test.ts` | 18 | 拉上游模型清单：打哪条、用哪把；传输失败分「响应头没来 / 正文没来 / 根本连不上」**三档，不许混成一句**。 |
| 16 | 🔴 `admin-usage.test.ts` | 63 | 用量端点：查询参数契约、range 收敛、读扇出次数、**八种状态在响应字段上两两可分**、`note` 的优先级（畸形排在 `range_clamped` 之前）。 |
| 17 | 🔴 `admin-verify.test.ts` | 24 | 验活探针：验哪把、打哪条、**写零个存储字段**、明文 key 一个字节不回面板也不进事件、闸的粒度是 `verify:<id>`（两把不同的 key 不该互相挡）、自带超时。 |
| 18 | `anthropic.test.ts` | 6 | `POST /v1/messages` 端到端。 |
| 19 | `auth.test.ts` | 12 | 网关鉴权中间件，含空凭据 `auth('')` 的防御。 |
| 20 | 🔴 `config-degrade.test.ts` | 4 | **注册机装不起来时网关照常活着**——真机 74% 请求 500 那次事故的回归。 |
| 21 | `config-effective.test.ts` | 2 | 没设 `GATEWAY_TOKEN` 环境变量的部署，网关口令能被面板撤销，且 `dispatch()` 内部读到的配置同样跟着变。 |
| 22 | 🔴 `errors.test.ts` | 32 | 客户端畸形 JSON 一律 400 JSON 而不是 500 纯文本；合法 JSON 但结构不对必须 400 且**零上游调用**；未预期异常落进 JSON 错误信封**并留下一条事件**；无路由 404 是 JSON 信封、方法不对 405 + `Allow`。 |
| 23 | 🔴 `events-cursor-heal.test.ts` | 8 | 事件面板轮询在**持续**时钟偏移下的自愈稳态。 |
| 24 | `freshness.test.ts` | 10 | 配置与 key 池「刚改完多久能看见」必须是同一套语义——用**同一组断言**分别跑两者（面板上那个生效时间只有一份）。 |
| 25 | `gemini.test.ts` | 14 | `/v1beta/models`、`:generateContent`、`:streamGenerateContent` 端到端。 |
| 26 | `health.test.ts` | 5 | `GET /health`。 |
| 27 | `mailbox.test.ts` | 20 | 两家邮箱 provider 同语义不同线上格式；**删邮箱失败不向上传播**（stub 用 throw 而不是回状态码，否则断言测了个寂寞）。 |
| 28 | 🔴 `manual-tend.test.ts` | 21 | 手动补池四道护栏：跨副本单实例锁、手动冷却、每日写预算闸、端点自己 `await` 到底，以及鉴权与不可用状态。 |
| 29 | `media.test.ts` | 36 | 图片生成、视频两段式、`GET /v1/videos/{id}` 的上游 content-type 不许把本源变成同源文档、路径穿越防护。 |
| 30 | `openai.test.ts` | 5 | `GET /v1/models`、`POST /v1/chat/completions` 端到端。 |
| 31 | 🔴 `pool-delete-durability.test.ts` | 4 | **删除不许被另一个实例的陈旧快照写回撤销**——含「裸存储删除」这条今天唯一的吊销姿势（它不动 `pool:index`，一次陈旧写回就能让 key 复活并立刻重新可用）。 |
| 32 | 🔴 `pool-index-corrupt.test.ts` | 3 | `pool:index` 存了非 JSON 字节时 `all()` 与 `reconcileIndex()` **都不许抛**——否则每个转发请求 500，而被指定的修复者读同一个键同样抛，永远修不好。 |
| 33 | 🔴 `protocol-catalog.test.ts` | 11 | 协议目录与真实路由表对账，且目录里那些示例请求**真的调得通**。 |
| 34 | 🔴 `quota-panel.test.ts` | 7 | 面板轮询路径不许出现 `list()`，也不许产生额外的读；数的是**精确次数**，不是「有没有调用」。 |
| 35 | `registrar-backoff.test.ts` | 3 | 退避窗口内那一轮的行为。 |
| 36 | `responses.test.ts` | 7 | `POST /v1/responses` 端到端。 |
| 37 | 🔴 `security-headers.test.ts` | 10 | 全应用级 `nosniff`，**刻意覆盖非 `/admin` 路径**（只测那棵树的话，「把 nosniff 挪回 uiRoutes 里」这个回归不会被发现）。 |
| 38 | 🔴 `storage.test.ts` | 42 | Storage 端口契约：读写删、`list`、`put` 的第三参 `expiresAt`、**过期之后 `get` 与 `list` 都看不见**——对每种实现各跑一遍。 |
| 39 | 🔴 `stream-parity.test.ts` | 21 | **网关不整体缓冲**：第一个正文字必须在上游第二块之前到达；CRLF 换行下同样成立；协议目录的 `streamTextPath` 与真吐出去的字节对得上。 |
| 40 | 🔴 `ui-serve.test.ts` | 24 | 静态资源伺服、CSP（**写字面量不 import 常量**，否则删掉 `style-src` 两边一起变）、`/favicon.ico`、登录闸不泄漏运行时信息。 |
| 41 | 🔴 `usage-tier2.test.ts` | 33 | Tier-2 用量统计的接线（`USAGE_STATS_ENABLED` → `wire.ts`）、`UsageSink` 落盘契约、flush 间隔、按对外密钥归属。 |
| 42 | 🔴 `wiring.test.ts` | 13 | `buildApp` 装配、存储可写性探测、**`/health` 零存储 I/O**（`configRefresh` 与 `logFlush` 两处例外要写明）、管理端接线。 |

### 2.B `tests/unit/`（根，业务那 26 个 / 445 格）

| # | 文件 | 格 | 它钉的是什么行为 |
|---:|---|---:|---|
| 43 | `anthropic.test.ts` | 22 | Anthropic ↔ 内部请求 / 响应 / 流 的双向转换（system 提到 messages 首位等）。 |
| 44 | `config-env-locked.test.ts` | 5 | `envLockedFields`：哪些字段被环境变量锁死（**空串也算设了**），面板据此置灰；清单是手写的十条，加字段必须在评审里被看见。 |
| 45 | `config-fatal-matrix.test.ts` | 3 | 注册机怎么坏都不影响「缺 `GATEWAY_TOKEN` 拒绝服务」这条 fatal。 |
| 46 | `config-holder.test.ts` | 11 | `ConfigHolder` 的 TTL 与刷新；存储里的非法值做**字段级降级**（面板一次误操作不得把网关砖掉）；`fixedConfigHolder`。 |
| 47 | `config-provenance.test.ts` | 16 | 四元组 env > 存储 > 内置默认值，逐字段说清谁赢了；凭据只写不读；`FIELD_EXPOSURE` 是「哪些字段是凭据」的唯一真源；`registrarFromEnv` 读到的 env 名全部进了锁定表。 |
| 48 | `config.test.ts` | 33 | `loadConfig` 的解析、数值下界、key 池两个旋钮。 |
| 49 | 🔴 `dispatcher.test.ts` | 52 | 调度核心：选 key、失败归因闭集、上游 content-type 不许变成可渲染的同源文档、上游错误体里的凭据片段要抹掉、**commit 抛错后仍要推进本请求内的视图**。 |
| 50 | `entry-fail-closed.test.ts` | 10 | 入口 fail-closed：缺 `GATEWAY_TOKEN` 与数值校验失败一律拒绝启动。 |
| 51 | `entry-node-tend-failure.test.ts` | 2 | `buildTendDeps` 装配失败不影响网关启动；启动时只读一次 config 键（防回归）。 |
| 52 | `entry-node.test.ts` | 8 | 留空的 `DATA_DIR` / `PORT` 视同「没设」；真实启动路径；**补池的存储级锁**（多副本共卷部署）。 |
| 53 | `errors.test.ts` | 9 | `classifyStatus` / `classifyThrown` 的归因闭集。 |
| 54 | `gemini.test.ts` | 18 | Gemini ↔ 内部 的双向转换、模型清单、流式。 |
| 55 | 🔴 `keypool-repo.test.ts` | 37 | 仓储层：`add`/`get`/`save`、**`all()` 热路径零 `list`**、「权威的空索引不许让手工导入的 key 隐身」、`pool:index` 存了非 JSON 字节时的行为。 |
| 56 | `keypool.test.ts` | 27 | `isAvailable` / `isDisabled` 的取值边界（含存储里存了字符串 `"true"` 这种脏值）、`selectKey` 选择策略、状态迁移。 |
| 57 | `logger-multi.test.ts` | 5 | 一个 sink 抛错不许影响别的 sink，更不许影响调用方。 |
| 58 | 🔴 `logger-store.test.ts` | 27 | 落存储的日志：写侧遇到畸形分片值时的两条真实缺陷、读侧畸形条目逐条丢掉**且丢了几条要数得出来**、`flush()` 绕过最小间隔闸。 |
| 59 | `logger.test.ts` | 13 | `ConsoleLogger` 的 logfmt 引用（调用方的数据撕不开字段结构）、`NULL_LOGGER`。 |
| 60 | 🔴 `pool-cache.test.ts` | 42 | isolate 级快照缓存；**写消除**（只有 `lastUsedAt` 变化时不落盘）以及它的合法性前提（`lastUsedAt` 不参与调度）；两个旋钮的默认值。 |
| 61 | `pool-index.test.ts` | 16 | `parsePoolIndex` / `idsFromKeyNames` / `sameIdSet` 与索引常量。 |
| 62 | 🔴 `quota-account.test.ts` | 13 | 配额账的可执行版本：**稳态与冷池是两笔账不许混**；`list` 与 `put` 归零、`get` 与请求数解耦；必须按**生产默认值**算而不是测试夹具的 0。 |
| 63 | `refreshable.test.ts` | 15 | `Refreshable` 原语（TTL、并发去重、失败语义）；假时钟必须递进而不是冻结。 |
| 64 | 🔴 `responses.test.ts` | 22 | OpenAI Responses ↔ 内部 的双向转换与**流式事件序列**（`flat()` 改回去这一格当场红）。 |
| 65 | `sse.test.ts` | 13 | `parseSseStream`（含 CRLF 下的增量）、`sseEvent`、`toSseStream`。 |
| 66 | `storage-file-ttl.test.ts` | 5 | `FileStorage` 的 TTL：过期键真的从 `store.json` 里**物理删掉**。 |
| 67 | `storage-health.test.ts` | 10 | `watchStorage` / `probeWritable`。 |
| 68 | 🔴 `tend-scheduler.test.ts` | 11 | `startTendScheduler` 的定时轮与异步链推进（假定时器必须走宏任务，微任务自旋会让变异后的测试挂死而不是变红）。 |

### 2.C `tests/unit/admin/`（14 个 / 330 格）

| # | 文件 | 格 | 它钉的是什么行为 |
|---:|---|---:|---|
| 69 | `admin-errors.test.ts` | 13 | 管理接口错误码是**闭集**；码是码、文案在五语言字典里；面不许增长；网关业务口本期不做且不许半做。 |
| 70 | `api-keys.test.ts` | 30 | 对外密钥的掩码、摘要、过期与分档、`findByDigest`。 |
| 71 | 🔴 `config-validate.test.ts` | 54 | 逐字段校验；凭据缺席或空串 = 不改；跨字段规则与 `registrarFromEnv` 里每一处 `throw` 一一对应；`EDITABLE` 与 `FIELD_EXPOSURE` / `envLockedFields` **三表逐条对账**。 |
| 72 | `event-entry.test.ts` | 14 | `narrowEntries` 逐条窄化、`narrowShard` 整片窄化 + 丢弃计数。 |
| 73 | `event-ring.test.ts` | 39 | 事件环：`windowIndex`、`slotOf` 确定性且值域有界、`shardKey`/`candidateKeys` 可从时钟直接算出来（「索引无上限增长」根治的依据）。 |
| 74 | 🔴 `key-view.test.ts` | 26 | `KeyView` 永不含明文；`keyBucket` 与 `isAvailable` 等价；序号与返回顺序；**搜索绝不能变成明文预言机**。 |
| 75 | 🔴 `probe-guard.test.ts` | 26 | 出站探测两道闸；两条端点共用**单一真源**（源码级断言）。 |
| 76 | `protocol-catalog.test.ts` | 15 | 协议目录常量表自洽：`MODEL_CATALOG` 与 `MODELS` 一致、四协议 id 固定（期望值手写字面量，不写 `PROTOCOLS.length`）。 |
| 77 | `stats.test.ts` | 21 | `normalizeStats`；`maxStats` 基线只增不减；`withOutcome` 三条终态各记各的；写被消除时把计数攒起来。 |
| 78 | `tend-guard.test.ts` | 25 | 手动补池护栏的两个数字本身就是策略（独立钉死）；`dayIndex`/`dayEndsAt`；`narrowManualGuard`；`checkManualTend`。 |
| 79 | `tend-history.test.ts` | 16 | `TEND_HISTORY_SIZE`/`KEY` 常量即策略；环形追加；一轮结果 + 谁触发的；**存储里的东西一律不可信**。 |
| 80 | 🔴 `upstream-facts.test.ts` | 15 | 上游事实登记表本体、**依据强度**判据、真源锚判据（「这条上游事实未经核实」必须写出来）。 |
| 81 | 🔴 `upstream-models.test.ts` | 15 | `parseUpstreamModels`：读不出来是 `null`、坏记录逐条跳过；`diffAgainstCatalog` 两个方向都从 `MODEL_CATALOG` 现算。 |
| 82 | `usage-stats.test.ts` | 21 | Tier-2 的配额算术、合并、按对外密钥归属。 |

### 2.D `tests/unit/registrar/` — 注册机（15 个 / 376 格，P3 的底稿）

| # | 文件 | 格 | 它钉的是什么行为 |
|---:|---|---:|---|
| 83 | `agnes.test.ts` | 19 | 上游平台四步 API：`sendCode` / `register` / `login` / `createKey`。 |
| 84 | 🔴 `backoff.test.ts` | 13 | `inBackoff`/`retryAfterMs` 唯一判据是 `until` 与 `now` 比大小；`narrowBackoff` 读坏了当「没有退避」放行（**方向与台账刻意相反**，要写明理由）；`nextBackoff` 指数 + 封顶；`mergeBackoff` 一律先 get 再 merge（存储没有 CAS）。 |
| 85 | 🔴 `code.test.ts` | 29 | `extractCode`：从邮件 HTML / 文本里抠验证码，且避开 `#ffffff` / `#000000` 这类六位十六进制的误命中。 |
| 86 | `config-total.test.ts` | 15 | 注册机配置装载器**全函数化**：对抗性输入网格上一次都不抛。 |
| 87 | 🔴 `config.test.ts` | 50 | `registrarFromEnv`；存量主备旧键的迁移；`requireChannel`；五语言文档对轮级预算的表述必须有条件且与代码同步。 |
| 88 | 🔴 `domain-ledger-io.test.ts` | 28 | 域名台账：稳态下一次成功铸号**只打一次发码请求**；撞上限流整轮当场停手并记一个跨轮退避；已知能用的域名被上游拉黑后补池不许卡死；一轮之内域名轮着用、同一个域名最多学一跳。 |
| 89 | 🔴 `domain-ledger.test.ts` | 49 | `classifySendCode`：**同一个 400 有两种含义，正文是唯一线索**；`upstreamMessage` 抹掉已知凭据与 `sk-` 片段、上游正文进事件之前先抹掉邮箱地址；`bodyFieldNames` 在 2xx 却认不出目标字段时只说字段名。 |
| 90 | `log-prefix.test.ts` | 5 | 注册机日志的 `[registrar]` 前缀 + 稳定事件名（这是**对外文档承诺**）。 |
| 91 | `mailbox-moemail.test.ts` | 26 | 一家邮箱 provider：正常路径、2xx 但正文读不出来、`verifyCredentials`、`deleteMailbox` 两个方向的返回值。 |
| 92 | 🔴 `mailbox-yyds.test.ts` | 35 | 另一家邮箱 provider 的同一组判据（同语义、不同线上格式）。 |
| 93 | 🔴 `mint.test.ts` | 38 | `mintOne`：一次完整铸号的编排与每一步的失败分支；`sendCode` 回 `{status, body}` 而不只是状态码。 |
| 94 | 🔴 `scheduling-wiring.test.ts` | 17 | 两个入口确实会调到 `tendOnce`；**key 池索引对账接在「注册机是否启用」的判断之前**；Node 侧每轮重读配置（面板改完不必重启）；定时轮不带轮级墙钟预算。 |
| 95 | `scheduling.test.ts` | 4 | `buildTendDeps` 装配。⚠️ 原本还有一组「Worker scheduled 处理器」，P0 砍 CF 时已随被测对象一起删掉——**Rust 版不要凭空补回来**。 |
| 96 | 🔴 `tender.test.ts` | 33 | `tendOnce` 一轮补池的编排与收尾对账。 |
| 97 | 🔴 `url.test.ts` | 15 | `redactUrl` / `httpFailMessage` / `redactInMessage` / `transportFailMessage`——日志与事件里的脱敏。 |

---

## 3. 🔴 事故判据：花了真代价买来的那些

**先说一个不舒服的事实**：我按「实测 / 真机 / 上一版 / 复评发现 / 订正 / 线上」逐文件扫了一遍，
**166 个文件里 160 个命中**。也就是说——

> **「记录着真实事故的判据」不是一个子集，它几乎就是这套判据的全部。**
> 不要指望「先挑重点的搬，剩下的以后再说」——那个「剩下的」不存在。

下面只是**密度最高、失效代价最重**的那些，标出来是为了排期时别把它们排到最后。

### 3.1 真实生产事故的直接回归（最高优先级，P1 必须有）

| 判据 | 事故原文（摘） | Rust 版的落法 |
|---|---|---|
| `contract/config-degrade.test.ts` | 线上一次常规重新部署后 **81 次探测里 60 次返回 500（74%）**，`/health` 也在内。真因：存储里早就存成「注册机已启用 + 缺凭据」，旧实例缓存着旧配置照常跑，重新部署清掉旧实例 ⇒ 新实例一读配置就抛，全线 500。**这颗雷潜伏了很久才引爆。** | 「注册机子系统装配失败 ⇒ 网关主体照常起来并服务」必须是一条断言，不是一句注释。 |
| `contract/errors.test.ts` | 2026-09-10 容器验收实测：**五条路由全部返回 `500 Internal Server Error`（text/plain）**；另一处「14 次 500 之后事件流里一条记录都没有」，运维只能靠复现定位。 | 错误信封 + 「未预期异常必须留下一条可查的事件」两条一起搬。 |
| `contract/security-headers.test.ts` | 面板落地前整个网关**一条安全响应头都没有**（`/health` 的 `x-content-type-options` 实测是 `null`），而面板把管理口令放在 origin 作用域的存储里。 | 全应用级 `nosniff`，且断言**刻意打非 `/admin` 路径**。 |
| `contract/admin-apikeys-quota.test.ts` | 鉴权第二段「凭据 ≠ 主口令 ⇒ 查子密钥表」是**零凭据零限速**的路径：漏掉「null 也缓存」这一条，每个错口令请求就是一次存储读，当天配额当场打穿，**而功能测试会全绿**。 | 「1000 次错口令 = 1 次读」这种**精确计数**断言，不是「有没有调用」。 |
| `contract/quota-panel.test.ts` | 面板每 15 秒轮询一次，若走 `list()`，一个忘了关的浏览器标签 = 5760 次 list/天，**与网关转发共用同一个桶**。 | 同上，数精确次数。 |
| `contract/pool-index-corrupt.test.ts` | `pool:index` 存了非 JSON 字节 ⇒ `all()` 抛 ⇒ **每个转发请求 500**；而被指定的修复者 `reconcileIndex()` 读同一个键**同样抛** ⇒ 每 30 分钟徒劳挂一次，永远修不好，只能人工删键。 | Rust 里对应 serde 解析失败的**双路径**都要不 panic、不向上抛。 |
| `contract/pool-delete-durability.test.ts` | 删除被另一个实例的**陈旧快照写回**撤销 ⇒ 已吊销的 key 复活并立刻重新可用。含「裸存储删除」这条当时唯一的吊销姿势。 | 必须**跨实例**构造，同实例内 `delete()` 会 `invalidate()`，被测动作根本发生不了。 |
| `contract/admin-events.test.ts` | 把 `log-flush` 的 `await flush()` 改成 fire-and-forget，**这一格是全套 1400+ 里唯一变红的**。 | 「响应返回前已 flush」要单独成格。 |
| `contract/wiring.test.ts` | 判据只覆盖了半边：计数桩**只统计 `get`**，而注释承诺的是「零存储 I/O」，被违反的恰恰是写那一侧（`logFlush` 挂在 `*` 上）。 | 计数桩必须 `get`/`put`/`delete`/`list` 四种全数。 |

### 3.2 「注释里那句话是假的」——判据自身被打脸的记录

这一族不是产品 bug，是**判据本身骗了人**，价值同样不许丢：

- 🔴 `unit/source-guards.test.ts`（第 ② 类）：台账与多处派发都写着「`src/core` 零 IO，**有源码断言钉着**」——
  **那条断言根本不存在**。实测往 `src/core/` 加一处 `setTimeout` 加一处 `Math.random`，
  typecheck + 全套测试 + 九道 CI 门禁**全绿，零信号**。
  它的**第一版自己又犯了同一个错**：宣称覆盖「全部时间/随机/定时/网络/环境使用点」，
  复评拿四种最地道的写法一试就穿。最终裁定：**边界不再由散文宣称，而由 `COVERED` / `BLIND_SPOTS`
  两张可执行的表钉死**——每一种声称抓得住的写法有探针证明真抓得住，每一处已知盲区有探针证明确实抓不住。
- 🔴 `tests/global-setup.ts`（收集门禁）：第一版在文件头断言「收集门禁挡不住自己被取消收集，在 vitest 进程内无解」——
  **那句话是错的**，而且当时被写成了给后续任务的指令。起因是 `include` 里去掉 `tests/ui/**`，
  整条前端通道被关掉而**没有任何一条用例变红**（总数从 710 静静掉到 705）。
  更早还有一次**并集盲区**：两份配置时判定写成「被某一份收集到就算过」，于是其中一份的 include 被收窄
  对门禁完全不可见——`✅ 51 个文件 × 2 份配置，无漏收集` 与 `Test Files 2 passed` 同时为绿，
  **15 个契约测试静默消失**。
- 🔴 `unit/repo-front-door.test.ts`（第 ② 类）：上一版把五份社区文件写成一张**冻结的字面量表**，
  于是往 `.github/ISSUE_TEMPLATE/` 加第六份模板（那正是最会长文件的目录）、里面死链 + 查无此处的仓内路径 +
  不存在的命令三种错一起犯——**二十余格全绿**。
- 🔴 `unit/check-comment-refs.test.ts`（第 ② 类）：整份文件的 `@refs-ignore-file` 豁免**理由被改写过，旧那条是假的**
  （原文说「夹具字符串会被扫描器当成真注释」，扫描器改成逐字符扫之后这句不再成立）。**豁免仍需要，但理由换了一个。**
- 🔴 `contract/admin-usage.test.ts`：多处「上一版这里写 X，定向复评实测发现是 Y」——
  其中「优先级订正」那条（`range_clamped` 与畸形谁先）**上一版一格护栏都没有**。
- 🔴 `contract/admin-overview.test.ts`：上一版那条上界钉的是「比 TTL 大一个边缘缓存的量」，分工错了，已订正。
- 🔴 `contract/admin-upstream-models.test.ts`：评审提的修法（改回 `timeout` / `network_error`）**实测为假**，没采纳并写下了理由。

### 3.3 这套判据登记在案的「假阳性形态」（重建时的红线）

散在各文件头里，Rust 版要原样继承——**它们比任何一格断言都值钱**：

1. 空遍历 / 空集合下整组断言全绿（路径从 `cwd` 解析、`readdir` 扫到 0 个文件）。
2. 只数调用次数而两条路径返回值相同 ⇒ 谁赢都通过（缓存类测试的头号假阳性）。
3. stub 用 `resolve(状态码)` 而不是 `throw` ⇒ 被测代码把 `try/catch` 整段删掉也照绿。
4. 冻结时钟下，TTL 类变异的结果是**挂起**而不是失败（要用递进假时钟 + 宏任务 flush）。
5. **测试覆盖的状态让被测的选择不可观测**（同实例 delete + save、计数桩只统计一半操作）。
6. **期望值从被测对象现算**（同义反复）——两边一起改、断言永远绿。
7. 测试**照抄一遍装配**去验证装配——照抄的那份永远验证不了原件。

---

## 4. 第 ② 类：元判据 / 仓库治理判据（23 个文件 / 1758 格）—— 裁定

### 4.1 裁定总纲

**全留。** 三条理由：

1. 用户的分期规则是「**kiro2api 没有的、或者比 kiro2api 更强的就保留**」。
   实测 kiro2api 的门禁面：`scripts/` 只有 1 个 `set-version.sh`，`tests/` 只有 5 个集成测试文件，
   CI 只有 fmt / clippy / cargo test / `node --test admin-ui-v2/tests/*.mjs` 四格。
   **agnes2api 这 1758 格治理判据在 kiro2api 里完全没有对应物，属于「更强」，按规则保留。**
2. 这一类里 **21/23 的被测对象根本不是 TypeScript**——是 `README.md`、`docs/<5 语言>/*.md`、
   `.env.example`、`docker-compose.yml`、`docker-entrypoint.sh`、`scripts/*.sh|mjs`、
   `.github/workflows/*.yml`、`docs/logo.png`、git 跟踪文件全集。**换语言这些对象一个都不会消失。**
3. 它们守的失效链是**沉默型**的（文档说了假话、门禁静默失效、公开仓门面死链）——
   这类失效不会有人报 bug，只会在用户那边悄悄发生。删掉它们等于把「零内置凭据」「文档五语言一致」
   这些对外承诺退回成一句注释，而这个仓已经四次栽在「注释里的断言被后人信任」上。

⚠️ **但有一条不许自欺**：CI 里必须继续有 Node。这在 Rust 仓不是妥协——kiro2api 的 CI 自己就装了
`setup-node@v4 + pnpm` 来构建前端并跑 `node --test`。agnes2api-rs 的 P2 面板同样要构建前端，
Node 反正要在，**这批门禁搭它的顺风车，增量成本接近零**。

### 4.2 逐文件裁定

落法分三种：
**【A 原样搬】**被测对象与语言无关，脚本与判据几乎逐行照搬（Node 侧）。
**【B 换对象重写】**判据的**意图**保留，被测对象换成 Rust 世界的等价物。
**【C 有对象才落】**对象要等 P2 / 发版流程定稿之后才存在，先登记不落。

| # | 文件 | 格 | 它钉的是什么 | 裁定 |
|---:|---|---:|---|---|
| 1 | 🔴 `docs-parity.test.ts` | **614** | 五语言 `DEPLOY.md` / `API.md` / `ADMIN.md` 的关键数字、表格、503 reason 全集、「这条上游事实未经核实」、视频任务标识字符集……逐份对等；**期望值一律从真源常量 import，不手抄第二份**。 | **A**（真源 import 那一半要改成从 Rust 侧导出常量清单，见 §6 风险 1） |
| 2 | 🔴 `source-guards.test.ts` | **228** | `src/core` 零 IO、注册机模块级零 throw、注册机一格存储都不碰、配错形状的 throw 一律是 `ConfigRefusal`……**用 `COVERED` / `BLIND_SPOTS` 两张探针表明写边界**。 | **B**（被测对象是 Rust 源码；Rust 有更好的手段——模块可见性 / `#![deny]` / clippy lint / 架构测试，但**「明写盲区」这套方法必须继承**） |
| 3 | 🔴 `check-comment-refs.test.ts` | 143 | 注释里写下的路径指向必须解析得开（规则 A/C/D）；守的是「注释里写下一句假断言」这件事——台账已记二十余次。 | **A**（扫描器要加 Rust 注释语法；`///` `//!` `/* */` 与字符串跳过逻辑） |
| 4 | 🔴 `check-i18n.test.ts` | 104 | i18n 门禁脚本自己的元测试：十条判据逐条、以及第 ①/⑧ 条的**已知边界** | **C**（P2 面板落地后） |
| 5 | 🔴 `docs-typography.test.ts` | 94 | 40 份出货文档的排版：alert 语义位置、`<details>` 数量与位置、表格只许出现在指定四节、全仓零 HTML `<table>`、**>1200 字符区间数的长度棘轮**。 | **A** |
| 6 | 🔴 `repo-front-door.test.ts` | 82 | 公开仓门面：社区文件**从磁盘现算不写死清单**、CI 徽章指向的 workflow 真存在、node 大版本在所有钉它的地方彼此相等、`SECURITY.md` 不许把仓库纪律说成运行时安全承诺。 | **A**（「node 大版本」换成「Rust toolchain / MSRV 在 `Dockerfile` / workflows / `Cargo.toml` 三处相等」） |
| 7 | 🔴 `docs-internal-refs.test.ts` | 82 | 44 份公开 markdown 里**一个内部标识符都没有**（阶段编号 / 任务号 / 发现号 / 评审名）；该红时红、不乱红两边都钉。 | **A** |
| 8 | 🔴 `source-internal-refs.test.ts` | 64 | git 全部跟踪文件（减二进制与两条排除）里的内部标识符**逐份点名，与登记在案的欠账相等**。 | **A** |
| 9 | 🔴 `ui-assets.test.ts` | 47 | 生成物与 `admin-ui/` 源**逐字节相同**、没有漂移、生成器的硬规则、违规输入 `exit 1`；资产清单只锁键集合（多一个文件 = 多一个公网端点）。 | **C→B**（Rust 侧换成 `rust-embed` / `include_dir` 的编译期嵌入，判据意图不变） |
| 10 | 🔴 `prepush-guard.test.ts` | 40 | `prepush.sh` 跑的门禁**是从 `ci.yml` 当场抽的**；抽取器认不出要吵不许静默放行；逐格跑完再汇总、红不许被吃掉、列位不许错开。 | **A**（门禁表换成 Rust 那几格） |
| 11 | 🔴 `smoke-guard.test.ts` | 37 | 冒烟脚本的判据不许被换成没有鉴别力的东西。 | **B**（脚本换成 Rust 版镜像的冒烟） |
| 12 | 🔴 `docs-deviations.test.ts` | 31 | 「刻意偏离名册」每条今天都真成立；名册自守（编号不重、三样齐全、机器验不了的那条如实标注）；README 项目结构树每条路径真的存在。 | **A** |
| 13 | 🔴 `check-png.test.ts` | 31 | 图片门禁：十一条负例各自红、阳性对照证明判据既不过紧也不同义反复、名册（路径→sha256）、与 `check-no-binary` 的咬合。 | **A** |
| 14 | 🔴 `scan-secrets.test.ts` | 29 | **零内置凭据**那道门禁：六条规则（含裸 IP）、`--history` 扫历史、以及脚本自己的注释。 | **A**（这条是硬约束的机器保障，优先级最高的治理判据之一） |
| 15 | 🔴 `ops-closure.test.ts` | 29 | 运维闭环：`DATA_DIR` 没落在挂载点上要喊出来、回滚有抓手（镜像 tag 可钉）、五份 `DEPLOY.md` 都写着「`docker compose restart` 不重读 `.env`」。 | **A** |
| 16 | 🔴 `env-example-parity.test.ts` | 28 | `.env.example` 与真源对齐、照它 `cp` 出来的 `.env` 真的能起来、与五语言文档对等、不回退。 | **A**（「真的能起来」那格换成起 Rust 二进制） |
| 17 | 🔴 `scripts-guard.test.ts` | 18 | 体积预算门禁、`tests/ui` 真的被收集了、**CI 的测试命令不带文件过滤器**（收集门禁分档的前提）、`check-no-binary` 在门禁列表里。 | **B**（`cargo test` 的收集语义不同，见 §4.3） |
| 18 | 🔴 `pretag-guard.test.ts` | 16 | 打 tag 前：tag 只许落在「远端 main 那个提交」且「远端记着它是绿的」。 | **A**（**用户记忆里踩过的坑**：修复落在 tag 之后 = 等于没发出去） |
| 19 | `check-no-binary.test.ts` | 14 | 不许把二进制文件混进仓。 | **A** |
| 20 | `readme-sections.test.ts` | 11 | README 章节标题常量表 + 反向控制（拿改坏的表喂同一个检查函数）。 | **A** |
| 21 | `docs-glossary.test.ts` | 8 | 五语言术语表：同一源词在同一语言的不同文档里必须用同一译词；判官带反向控制。 | **A** |
| 22 | `sha-refs.test.ts` | 7 | 跟踪文件里写下的提交 sha 引用必须真的存在。 | **A** |
| 23 | `version.test.ts` | 1 | `VERSION` / `package.json` / `src/version.ts` 三处一致。 | **B**（换成 `VERSION` / `Cargo.toml` / `Cargo.lock` / Rust 侧版本常量四处一致——kiro2api 的 `scripts/set-version.sh` 正是干这个的，可对标） |

### 4.3 单独裁定：`tests/global-setup.ts`（收集门禁）

**留，但必须换形态，而且不许照抄结论。**

- TS 侧的失效链是「`include` glob 悄悄关掉一整条通道，而 CI 全绿」。
  **Rust 里这条链的一半自己消失了**：`tests/*.rs` 与 `src` 内 `#[cfg(test)]` 由 `cargo test` 自动发现，没有 glob 可改。
- **但另一半更隐蔽地存在**，必须接住：
  1. 一个测试模块没在父模块里 `mod xxx;` 声明 ⇒ 根本不编译，**零信号**；
  2. `#[cfg(feature = "...")]` / `#[ignore]` 把整块测试关掉 ⇒ `cargo test` 照样绿；
  3. `cargo test -p <crate>` 或 workspace 成员没进 `default-members` ⇒ 整个 crate 的测试不跑；
  4. CI 命令带了 `--test <name>` 或 filter ⇒ 与 TS 那条「CI 不带文件过滤器」是同一条前提。
- 落法：一条 build-time 或 CI-time 断言，**期望侧从磁盘扫 `.rs` 文件、实际侧取
  `cargo test -- --list` 的输出**，两侧不许互相推导——这条纪律逐字继承。

---

## 5. 第 ③ 类：TypeScript 特有的判据 —— **这一类是空的**

裁定：**0 个文件，格数近似 0。**

依据（我是这么查的，不是猜的）：

- 全仓 `@ts-expect-error` / `@ts-ignore` **只有 2 处**，都在 `unit/registrar/tender.test.ts`。
- `as any` / `as unknown as` 全仓 **不到 30 处**，逐处看过样本，用途**全部**是
  「**构造存储里 / 上游回来的脏数据**」，例如：

  ```ts
  isDisabled(rec({ disabled: "true" as unknown as boolean }))  // 期望 false
  mk({ disabled: "yes" as unknown as boolean })
  ```

  这钉的不是「TS 类型系统会不会挡住」，而是**「持久化数据里出现了本不该出现的形状时，网关怎么办」**——
  那是**纯业务规则**，属于第 ① 类。

⚠️ **这里有一个必须由主线裁的陷阱，别当成「TS 的事没了」**：

> TS 里 `disabled: "true"` 会被 `JSON.parse` 原样带进来，代码显式判 `=== true`，**记录仍然可用**。
> Rust 里 `serde` 遇到 `"true"` 给 `bool` 字段会**整条记录反序列化失败**——如果按默认写法，
> 结果是**这把 key 直接从池子里消失**。
>
> **这是行为分岔，不是等价移植。** 同一族问题还有：`admin-usage` 那些「畸形分片」、
> `event-entry` 的 `narrowEntries` 逐条窄化、`tend-history` 的「存储里的东西一律不可信」、
> `upstream-models` 的「坏记录逐条跳过」。
> Rust 版要么全线用 `#[serde(default)]` + `serde_json::Value` 两段式解析显式复刻窄化语义，
> 要么明确裁定「换成更严格的语义」并**逐条重写对应判据**。**两条路都行，但必须选一条并写下来。**

`stats.test.ts` 里已经有人踩过并留了注释：「这里**不再**写 `as unknown as KeyStats`：真实输入来自 `JSON.parse`」——
方向是对的，Rust 版要把这条纪律贯彻到全部窄化函数。

---

## 6. 第 ④ 类：前端纯函数 / DOM 判据（46 个文件 / 1511 格）—— P2

**全留**，但一格都不在 P1 落。这是 P2 的底稿，此处只做登记：

- `tests/ui/`（25 个 / 990 格）：纯函数层，直接 `import admin-ui/js/pure/*.mjs`，不碰运行时。
  重点：`api-session`(132)、`keys-write`(110)、`events`(106)、`registrar`(69)、`settings`(62)、
  `playground`(61)、`model-test`(49)、`playground-media`(47)、`usage`(44)、`models`(40)、`overview`(38)。
  其中三条是**跨端等价性**判据，P2 时是「前端与 Rust 后端说同一件事」的唯一保障：
  `sendable-parity`(24，口令字符集前后端逐码位等价)、`keys-write` 的 `isDeletable`
  （与后端 `deletable()` 是同一条判据的前端半身）、`session`(27，12 小时上限在五语言文档与代码里必须一致)。
  另有两条**架构绊线**：`no-hardcoded-endpoints`(9，前端不许硬编码网关端点)、
  `pure-boundary`(5，`sec-*.js` 不许重新声明 `pure/*.mjs` 已导出的函数名)。
- `tests/ui/dom/`（20 个 / 465 格）：DOM 层，跑在自建 `fake-dom.ts` 上。
  重点：`playground-section`(82)、`keys-actions`(66)、`registrar-section`(56)、`settings-save`(44)、
  `usage-section`(40)、`models-section`(26)、`apikeys-section`(22)。
  🔴 `fake-dom-parity`(7) 单独点名：**发货代码不许用到替身独有、真实 DOM 没有的成员**——
  没有这一格，整个 DOM 测试层可能在测一个不存在的浏览器。
- 🔴 `tests/unit/i18n-dict.test.ts`（56 格）归在这里：面板五语言字典的形状与覆盖。
  它的文件头写着一条实测：`scripts/check-i18n.mjs` 对 `pg.*` 的 41 个 key 里 **37 个**只报警告
  ⇒ 打错一个字母**三道 i18n 门禁一起沉默、面板五种语言全显示裸 key**，
  **那 41 个 key 的拼写全靠这一格**。P2 做面板 i18n 时，这是第一优先。

⚠️ 用户要求「面板 UI 完全对标 kiro2api」。kiro2api 的面板测试只有
`admin-ui-v2/tests/*.mjs`（`node --test` 跑）。**对标的是样式与功能，不是判据密度**——
这 1511 格里绝大多数钉的是**行为**（三态不许伪造 0、两句互相打脸的话不许同屏、
读不出来 ≠ 空清单），换一套皮不影响这些判据成立。P2 时逐条评估「这条判据在新 UI 上还指着谁」，
**不许整块丢**。

---

## 7. 给 P1 的可执行结论

1. **P1 的验收面 = §2.A + §2.B + §2.C 里与 HTTP / 四协议转发 / key 池调度 / 鉴权相关的部分**，
   粗算 **约 1400 格**（contract 823 全部 + unit 根 445 里除 registrar 相关外的大部分 + unit/admin 330 里
   与鉴权/密钥/配置相关的部分）。**§2.D 的 376 格属于 P3，P1 不碰但不许删。**
2. **先搬第 3.3 节那七条假阳性红线，再写第一格断言。** 顺序反了的话，搬过来的是绿色而不是保障。
3. **第 5 节那个 serde 分岔必须在写第一个 `struct` 之前裁掉**，它会影响每一个持久化类型的定义。
4. 第 ② 类的 A 类（原样搬）可以与 P1 并行——它们不依赖 Rust 代码写到哪一步，
   而 `scan-secrets` / `pretag-guard` / `repo-front-door` 这三条**越早接上越省事**。

---

## 8. 这份清单没弄清的、以及需要主线裁的

1. **`docs-parity` 那 614 格里「从真源常量 import 期望值」的那一半，在 Rust 里没有现成落法。**
   TS 侧靠 `import { MAX_IMPORT_KEYS } from "src/..."` 把文档里的数字接到代码常量上。
   Rust 侧要么导出一份 JSON 清单给 Node 侧门禁读（多一道生成物，要防漂移），
   要么把这批门禁改用 Rust 写（则要在 Rust 里重做 markdown 解析）。**两条路成本都不小，需要主线裁。**
2. **第 ② 类的 1758 格是按「文件性质」裁的，不是按格逐条看的。**
   `docs-parity`(614) 与 `source-guards`(228) 两个文件占了这一类的 48%，
   我读了它们的文件头与全部 `describe` 名，**没有逐格读完 842 格**。
   其中可能混着少量业务判据（比如 `docs-parity` 里对 `FAIL_REASONS` / `MODEL_CATALOG` 的对账
   其实同时是业务契约）。**这个风险我认，不装作已经查清。**
3. **`tests/helpers/` 的 15 个夹具我只读了被引用处，没有逐个读完实现。**
   其中 `make-app.ts`（测试夹具刻意把缓存与写消除关成 0，而 `quota-account` 必须用生产默认值）
   这类「夹具本身携带策略」的地方可能还有，重建时每个夹具都要单独确认它的默认值在说什么。
4. **kiro2api 的 978 处内联 `#[test]` 我只数了数量，没有逐个对照。**
   「哪些判据 kiro2api 已经有了、Rust 版可以直接抄形状」这件事**本轮没做**，
   需要单独一轮对照（尤其是 key 池调度、SSE 流式、鉴权三块，两边很可能已经各有一套）。
5. **格数口径在 Rust 下不可比。** vitest 的 5243 格里有大量 `it.each` 展开；
   Rust 的 `#[test]` 通常一格里断言多条。**不要用「Rust 侧有多少个 `#[test]`」去证明移植完成度**——
   销账要对着 §2 的**行为清单**逐条打勾，不是对着数字。
6. **`tests/ui/dom/` 依赖一个自建的 `fake-dom.ts`。** P2 时 Rust 仓里这一层要么继续用 Node
   （随 kiro2api 的 `node --test` 先例），要么换成真浏览器（Playwright）。
   **换成真浏览器会让 `fake-dom-parity` 那 7 格失去对象但同时让它不再必要**——这是 P2 的裁定点，本轮不裁。
