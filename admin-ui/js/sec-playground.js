/**
 * 「调试台」板块（kiro2api 那边叫 `modeltest`，裁定 P-6 改名 `playground`）——
 * **拿网关自己的对外接口真发一次请求**，四条对话协议 × chat / image / video 三种形态。
 *
 * ── 🔴🔴 三条安全性质，一个字都不许松 ─────────────────────────────────────
 *
 * 它们是本仓明确比 kiro2api 强的地方，也是用户那条规则第一句直接命中的：
 *
 * **① 两把钥匙分存、分路、分文件。**
 *    管理口令（`pure/storage-keys.mjs` 的 `KEY_STORE`）走 `x-admin-key` 打
 *    `/admin/api` 那棵树，实现在 `js/api.js`；**网关口令**（同一份文件里的
 *    `GW_KEY_STORE`）走各协议惯用的鉴权头打**对外**那棵树，实现只在本文件的
 *    `openGateway()` 里。
 *    ⚠️ **这两个键名在本文件里刻意只以常量名出现、不写字面量**：键名的单一真源是
 *    `pure/storage-keys.mjs`，而 `src/webui/mod.rs` 有一格判据钉着
 *    「除 `boot.js` 外任何面板文件都不许再写那个前缀的字面量」——**注释里也不行**。
 *    🔴 **本文件不许构造管理口令那个请求头，`js/api.js` 不许读网关口令那个存储键。**
 *    （措辞刻意说的是「构造 / 读」而不是「出现」：上面这两段散文本身就在提它们的
 *    名字，一条按字面量 grep 的纪律会把这段解释判成违规 —— 而那会让人把解释删掉，
 *    恰好删掉纪律里唯一说明理由的部分。真要上判据，钉的是**代码**里的请求头构造，
 *    不是注释里的专名。）
 *    两者的泄漏面完全不同：前者能改配置、能读全部密钥；后者是发给**每一个下游
 *    用户**的那把中转口令。合流一次，两边的爆炸半径就并成一个。
 *    ⚠️ 存储那一半在 `pure/storage-keys.mjs` 里写着：登出时两把一起清，
 *    而**光清存储不够** —— 本板块的模块变量在登出后仍然活着（登出不 reload 页面），
 *    所以 `onShow()` **每次都重新从存储读一遍**，见文件末尾。
 *
 * **② 发请求前做同源自查：目录被改过就一个字节都不发。**
 *    URL 是拿运行期的 `location.origin` + 目录给的路径模板拼的，正常情况下它恒同源；
 *    这一判把「同源」从一条**推导出来的**性质变成一条**被检查的**性质 ——
 *    一份被改过的协议目录（管理接口被穿透、或反代插了一手）足以把路径模板换成一条
 *    指向别处的地址，而那一刻送出去的是那把中转口令。
 *
 * **③ `redirect: "error"`。**
 *    同源自查只管**第一条 URL**。`fetch` 的 `redirect` 默认是 `follow`，同源那条路径
 *    回一个 302 指向别处时浏览器会跟过去，而自查早就做完了。
 *    ⚠️⚠️ **不能指望浏览器替我们剥掉那个头**：Fetch 规范里 CORS non-wildcard
 *    request-header name **只有 `authorization` 一个**，跨源重定向时只删它；而四条
 *    协议里 **Anthropic 用 `x-api-key`、Gemini 用 `x-goog-api-key`，这两个不会被删**，
 *    会原样跟着 302 送到新源。本项目对外那几条端点没有任何合法重定向 ⇒ 一次重定向
 *    在这里只可能是异常。
 *
 * ── 一条结构性纪律：**本文件不拼任何对外端点路径、不认识任何协议 id** ────────
 * 方法、路径模板、鉴权头名、流式怎么开、样例请求体、增量正文在哪一格 ——
 * 全部来自 `GET /admin/api/models` 那份协议目录（真源在 `src/admin/models.rs`）。
 * 在这里再写一份「四条协议长什么样」就是第二份真源，而漂的那一天没有任何东西会红。
 *
 * ⚠️ **板块内不许监听 `langchange`**：框架层 `apply(document)` 之后会重跑 `onShow()`。
 */
import { api } from "./api.js";
import { t } from "./i18n.js";
import { el, elI18n, toast } from "./ui.js";
// 🔴 键名的单一真源。**别在这里再声明一遍**：两处分叉的后果是「登出没清掉那一把」。
import { GW_KEY_STORE } from "./pure/storage-keys.mjs";

/* ============================================================
   常量（**只有面板这一侧的节奏，没有一条协议知识**）
   ============================================================ */

/** 视频轮询的节奏。三条上界同时生效，**先撞到哪条算哪条**。 */
const VIDEO_POLL_INTERVAL_MS = 5_000;
const VIDEO_POLL_MAX_ATTEMPTS = 60;
const VIDEO_POLL_MAX_MS = 300_000;

/** 多轮上下文的条数上限。超了就丢最早的那几条，**并且在屏幕上说出来**。 */
const TURNS_MAX = 20;

/** 三种形态。**顺序即分段控件的顺序。** */
const MODALITIES = ["chat", "image", "video"];
const MODALITY_KEY = {
  chat: "section.pg.modality.chat",
  image: "section.pg.modality.image",
  video: "section.pg.modality.video",
};

/** 一次失败的档位名 → 文案。**档位名绝不含口令、也绝不含响应正文。** */
const FAIL_KEY = {
  bad_url: "section.pg.err.badUrl",
  cross_origin: "section.pg.err.crossOrigin",
  transport_error: "section.pg.err.transport",
  stream_error: "section.pg.err.stream",
  build_failed: "section.pg.err.buildFailed",
  no_token: "section.pg.err.noToken",
  no_endpoint: "section.pg.err.noEndpoint",
  no_task_id: "section.pg.err.noTaskId",
  poll_timeout: "section.pg.err.pollTimeout",
};

/* ============================================================
   状态
   ============================================================ */

let nodes = null;

/** 协议目录。`null` = 还没读到 / 读不出来。 */
let catalog = null;
let catalogFailed = false;

const ui = {
  protocol: "",
  modality: "chat",
  model: "",
  stream: false,
};

/** 网关口令。🔴 **每次 `onShow()` 重新从存储读**，见文件头 ①。 */
let token = "";
/** 设置页那份 `{configured, hint}`；`null` = 还没读到 ⇒ 一致性自查那一档是「比不了」。 */
let tokenHint = null;

/**
 * 这一轮对话。每条是 `{ role, text, images }`（`images` 是 `data:` URL 数组）。
 * ⚠️ **助手那一条在流式时会被就地追加**，所以它必须是同一个对象引用。
 */
let turns = [];
/** 上一轮的元信息：状态码 / 延迟 / 收到几块 / 有几块读不出来。 */
let meta = { status: null, latencyMs: null, chunks: null, malformed: null };
/** 上一轮的失败档位（`FAIL_KEY` 的键）或 HTTP 非 2xx 的原文。 */
let failure = null;
/** 上一轮的响应体原文（非流式 / 媒体）。 */
let rawBody = null;
/** 这一轮拿到的媒体地址。 */
let mediaUrls = [];

let inFlight = false;
let abort = null;
/** 视频轮询进度：`null` = 这一轮没在轮询。 */
let poll = null;

/* ============================================================
   窄化（**读不出来一律 `null`，不是空数组**）
   ============================================================ */

function asObject(v) {
  return v !== null && typeof v === "object" && !Array.isArray(v) ? v : null;
}

function str(v) { return typeof v === "string" ? v : null; }

/**
 * 把目录响应窄化成调试台要的形状。
 * 🔴 **任何一条格式不对就整份判成读不出来**，不是把坏的那条跳过 ——
 * 跳过之后视频会少掉轮询那一条端点，而屏幕上看起来完全正常：
 * 运维建完任务永远等不到成片。
 */
function narrow(body) {
  const b = asObject(body);
  if (b === null) return null;
  const samplePrompt = str(b.samplePrompt);
  if (samplePrompt === null || samplePrompt === "") return null;
  if (!Array.isArray(b.protocols) || !Array.isArray(b.models) || !Array.isArray(b.media)) return null;

  const protocols = [];
  for (const item of b.protocols) {
    const p = asObject(item);
    if (p === null) return null;
    const one = {
      id: str(p.id), label: str(p.label), method: str(p.method),
      pathTemplate: str(p.pathTemplate), authHeader: str(p.authHeader),
      streamMode: str(p.streamMode), streamKey: str(p.streamKey),
      sampleBody: asObject(p.sampleBody),
      streamTextPath: Array.isArray(p.streamTextPath) ? p.streamTextPath.map(String) : null,
    };
    for (const k of ["id", "label", "method", "pathTemplate", "authHeader", "streamMode", "streamKey"]) {
      if (one[k] === null || one[k] === "") return null;
    }
    if (one.sampleBody === null || one.streamTextPath === null) return null;
    protocols.push(one);
  }

  const media = [];
  for (const item of b.media) {
    const e = asObject(item);
    if (e === null) return null;
    const one = {
      id: str(e.id), modality: str(e.modality), op: str(e.op), method: str(e.method),
      pathTemplate: str(e.pathTemplate), authHeader: str(e.authHeader),
      taskSlot: e.taskSlot === null ? null : str(e.taskSlot),
      sampleBody: e.sampleBody === null ? null : asObject(e.sampleBody),
    };
    for (const k of ["id", "modality", "op", "method", "pathTemplate", "authHeader"]) {
      if (one[k] === null || one[k] === "") return null;
    }
    // ⚠️ **双向都判**：轮询那条按定义没有请求体，生成那条必须有。
    //    单向的话「生成端点漏了请求体」会退化成一次发空 body 的请求。
    if (one.op === "poll" && one.sampleBody !== null) return null;
    if (one.op !== "poll" && one.sampleBody === null) return null;
    media.push(one);
  }

  const models = [];
  for (const item of b.models) {
    const m = asObject(item);
    if (m === null) return null;
    const id = str(m.id);
    const modality = str(m.modality);
    if (id === null || modality === null) return null;
    models.push({ id, modality, protocols: Array.isArray(m.protocols) ? m.protocols.map(String) : [] });
  }

  return { samplePrompt, protocols, media, models };
}

/* ============================================================
   请求构造（**知识全部来自目录**）
   ============================================================ */

/** 深拷 + 把每个字符串过一遍 `f`。**只动字符串，结构原样。** */
function mapStrings(v, f) {
  if (typeof v === "string") return f(v);
  if (Array.isArray(v)) return v.map((x) => mapStrings(x, f));
  const o = asObject(v);
  if (o === null) return v;
  const out = {};
  for (const [k, val] of Object.entries(o)) out[k] = mapStrings(val, f);
  return out;
}

/**
 * 把样例请求体里那句占位文本换成用户输入。
 *
 * 🔴 **占位文本跟着响应来**（`samplePrompt`），本文件不知道它是哪句话。
 * ⚠️ **恰好命中一次**才算数：零次说明目录与那一格分叉了（照发的话会**静默丢弃
 * 用户输入、恒发样例那句话**，面板上完全看不出来），多次说明这份样例有歧义。
 */
function withPrompt(proto, model, prompt, samplePrompt) {
  if (proto.sampleBody === null) return null;
  let hits = 0;
  const body = mapStrings(proto.sampleBody, (s) => {
    if (s !== samplePrompt) return s;
    hits++;
    return prompt;
  });
  if (hits !== 1) return null;
  // **模型名走请求体那一格**；那一格不存在说明这条协议把模型放在路径里。
  if (typeof proto.sampleBody.model === "string") {
    if (model === "") return null;
    body.model = model;
  } else if (!proto.pathTemplate.includes("{model}")) {
    // 两头都没有 ⇒ 选中的模型无处可去，而面板上明明摆着一个模型下拉框。
    return null;
  }
  return body;
}

/**
 * 一次对话请求需要的四样东西。构造不出来时 `null`。
 *
 * ⚠️ **流式怎么开是两条完全不同的路**：Gemini 换的是**路径**，其余三条换的是**请求体
 * 字段**。换错了的后果是**静默降级成非流式** —— 请求照样 200、内容照样对，只是一次性
 * 全回来了，而面板上没有任何东西会提到这件事。
 * ⇒ 表外的 `streamMode` 在**要开流式时**判失败，不是「当成 body 那一档」。
 * ⚠️ **口令为空时照常构造，由调用方拦**：把那一判塞进这里，会让「没填口令」与
 * 「口令填错了」在 UI 上变成同一个结果，而这两件事的处置完全不同。
 */
function buildRequest(proto, opts) {
  const stream = opts.stream === true;
  if (stream && proto.streamMode !== "path" && proto.streamMode !== "body") return null;
  const template = stream && proto.streamMode === "path" ? proto.streamKey : proto.pathTemplate;
  // **`split`/`join` 而不是 `replace`**：后者会把替换串里的 `$&` 之类当成引用展开。
  const path = template.split("{model}").join(opts.model);
  const body = withPrompt(proto, opts.model, opts.prompt, opts.samplePrompt);
  if (body === null) return null;
  if (stream && proto.streamMode === "body") body[proto.streamKey] = true;
  return { url: `${opts.origin}${path}`, method: proto.method, headerName: proto.authHeader, body };
}

/**
 * 这条协议的鉴权头该带什么值。**头的名字来自目录**，本函数只决定值要不要带 `Bearer `。
 * ⚠️ **它是本文件唯一收口令的函数**，返回值只许交给 `fetch` 的请求头，
 * **不许交给任何渲染代码**。
 */
function authHeaderValue(headerName, value) {
  return headerName === "authorization" ? `Bearer ${value}` : value;
}

/* ============================================================
   网关出口（🔴 全站第四个 `fetch`，见文件头 ①）
   ============================================================ */

class GatewayError extends Error {
  constructor(code) { super(code); this.name = "GatewayError"; this.code = code; }
}

/**
 * 发一次对外请求。**流式与非流式共用这一个函数，这不是省代码、是安全要求**：
 * 同源自查、`redirect:"error"`、`credentials:"omit"`、「口令只走请求头」四条全部长在
 * 这里。两条路各写一份的话，漏掉的那一份会把那把中转口令送去别处。
 *
 * ⚠️ **口令只走请求头，禁止查询参数。** 对外那棵树的鉴权确实也收查询参数（为 Gemini
 * 兼容），**但面板不继承它**：口令进 URL 会落进浏览器历史、Referer、反代与 CDN 的
 * 访问日志。这与 `js/api.js` 对管理口令的禁令是同一条理由。
 *
 * ⚠️ **失败一律抛 `GatewayError`，错误里只带档位名。** 别把它拼成「请求 X 失败，
 * 头是 Y」那种诊断串 —— 那正是口令漏进错误文案的路径。
 */
async function openGateway(req, signal) {
  let target = null;
  try {
    target = new URL(req.url);
  } catch (e) {
    throw new GatewayError("bad_url");
  }
  // 🔴 性质 ②：目录被改过 ⇒ **一个字节都不发**。
  if (nodes.origin === "" || target.origin !== nodes.origin) throw new GatewayError("cross_origin");

  const headers = {};
  headers[req.headerName] = authHeaderValue(req.headerName, token);
  if (req.body !== null) headers["content-type"] = "application/json";

  try {
    return await fetch(target.href, {
      method: req.method,
      headers,
      body: req.body === null ? undefined : JSON.stringify(req.body),
      // 本项目没有 Cookie 会话，带上只会扩大攻击面。
      credentials: "omit",
      // 🔴 性质 ③：同源自查只管第一条 URL，跟过去的重定向它管不着。
      redirect: "error",
      signal,
    });
  } catch (e) {
    // 断网 / 被取消 / CORS 挡下：**这一次没有拿到任何响应**，与上游的状态无关。
    throw new GatewayError("transport_error");
  }
}

/** 响应体当 JSON 读一遍。**读不出来是 `null`**，不是空对象（后者与「回了个空对象」同形）。 */
async function readJson(res) {
  try { return await res.json(); } catch (e) { return null; }
}

/* ============================================================
   SSE 分帧与增量取文（**都是协议无关的**）
   ============================================================ */

/** 一帧的边界 = **一个空行**，`\n\n` 与 `\r\n\r\n` 两种都认。 */
function frameEnd(buf) {
  const a = buf.indexOf("\n\n");
  const b = buf.indexOf("\r\n\r\n");
  if (a < 0 && b < 0) return null;
  if (b < 0 || (a >= 0 && a < b)) return { idx: a, len: 2 };
  return { idx: b, len: 4 };
}

/**
 * 把缓冲区切成一批 `data:` 负载。**不解析负载本身。**
 * `[DONE]` **就地收尾**：同一帧里它之后的 data 行、以及缓冲区里其后的整帧一律不再收。
 */
function sseFrames(buffer) {
  const payloads = [];
  let rest = buffer;
  let end = frameEnd(rest);
  while (end !== null) {
    const frame = rest.slice(0, end.idx);
    rest = rest.slice(end.idx + end.len);
    for (const line of frame.split("\n")) {
      // `event:` / `id:` / `retry:` / 注释行一律跳过：这里只要正文。
      if (!line.startsWith("data:")) continue;
      // `.trim()` 顺带吃掉 CRLF 下那个尾随的 `\r`。**别在别处再 trim 第二次**，
      // 那会把负载正文里合法的前后空白也吃掉。
      const payload = line.slice(5).trim();
      if (payload === "[DONE]") return { payloads, rest: "", done: true };
      if (payload !== "") payloads.push(payload);
    }
    end = frameEnd(rest);
  }
  return { payloads, rest, done: false };
}

/**
 * 一条 SSE 数据行里**这一块新增的回答文本**。
 *
 * ⚠️⚠️ **三态，不是两态**：
 * · `null` —— **这一行读不出来**（不是合法 JSON）⇒ 数进 `malformed` 并显示出来；
 * · `""`   —— 读得出来但**不带正文**（`message_start` / `content_block_start` …
 *             四条协议的流里都夹着这种行，它们完全正常）；
 * · 非空串 —— 正文。
 * 把后两档折叠成同一个空串就是撒谎：要么每条正常事件行都被数成畸形（那个计数就没有
 * 信息了），要么一条真的坏掉的数据被静默丢弃。
 *
 * ⚠️⚠️ **「正文在哪一格」不在本文件里，它来自目录**（`streamTextPath`）。
 * 四条协议的增量各不相同，把那张四行的对照表写进浏览器就是第二份真源。
 */
function deltaText(proto, dataLine) {
  const path = proto.streamTextPath;
  if (path.length === 0) return null;
  let node;
  try { node = JSON.parse(dataLine); } catch (e) { return null; }
  for (const seg of path) {
    if (Array.isArray(node)) {
      // 数组只认「非负整数下标」：`Number("")` 给 0、`Number("1x")` 给 NaN，
      // 两者都得挡掉，否则一条写错的路径会静默地取到第 0 项。
      const i = /^[0-9]+$/.test(seg) ? Number(seg) : -1;
      if (i < 0 || i >= node.length) return "";
      node = node[i];
      continue;
    }
    const o = asObject(node);
    if (o === null) return "";
    node = o[seg];
  }
  // 走到底不是字符串 ⇒ 这一行不带正文（**不是畸形**：它 JSON 解得开）。
  return typeof node === "string" ? node : "";
}

/* ============================================================
   媒体结果
   ============================================================ */

/**
 * 从响应体里捞出媒体地址。**深度有上界**，来路不明的响应不许把主线程转进去。
 * 只认两种键（`url` / `uri`）与「值本身就是一条 `data:` / `http(s)` 串」。
 */
function collectMediaUrls(value, depth, out) {
  if (out.length >= 8 || depth > 5) return out;
  if (typeof value === "string") {
    if (value.startsWith("data:image/") || value.startsWith("http://") || value.startsWith("https://")) {
      if (!out.includes(value)) out.push(value);
    }
    return out;
  }
  if (Array.isArray(value)) {
    for (const v of value) collectMediaUrls(v, depth + 1, out);
    return out;
  }
  const o = asObject(value);
  if (o === null) return out;
  for (const [k, v] of Object.entries(o)) {
    if (typeof v === "string" && k !== "url" && k !== "uri") continue;
    collectMediaUrls(v, depth + 1, out);
  }
  return out;
}

/**
 * 这条地址能不能**直接内嵌**成一张图。
 *
 * 🔴 **只有 `data:` 能内嵌，远端地址一律只给链接。** 这不是保守，是面板自己的
 * CSP 决定的：`img-src 'self' data:` ⇒ 一张远端图会被浏览器**直接拦掉**，屏幕上留下
 * 一个破图框，而运维读到的是「网关没返回图」—— 一句假话。给链接反而是真的。
 */
function embeddable(url) { return url.startsWith("data:image/"); }

/* ============================================================
   渲染
   ============================================================ */

function protoById(id) {
  if (catalog === null) return null;
  return catalog.protocols.find((p) => p.id === id) ?? null;
}

/** 当前形态下可选的模型。**从目录里筛，不在这里认识任何 id。** */
function modelsForModality() {
  if (catalog === null) return [];
  if (ui.modality !== "chat") return catalog.models.filter((m) => m.modality === ui.modality);
  return catalog.models.filter((m) => m.modality === "chat" && (ui.protocol === "" || m.protocols.includes(ui.protocol)));
}

function mediaEndpoint(modality, op) {
  if (catalog === null) return null;
  return catalog.media.find((e) => e.modality === modality && e.op === op) ?? null;
}

function segGroup(options, current, onPick) {
  const wrap = el("div", { class: "seg-group" });
  for (const o of options) {
    const on = o.value === current;
    const b = el("button", {
      type: "button",
      class: on ? "seg-btn is-active" : "seg-btn",
      "aria-pressed": on ? "true" : "false",
    }, o.label);
    b.disabled = inFlight;
    b.addEventListener("click", () => { if (!on) onPick(o.value); });
    wrap.appendChild(b);
  }
  return wrap;
}

function configPanel() {
  const card = el("div", { class: "card mt-config" });
  card.appendChild(elI18n("h3", "section.pg.cfg.title", { class: "mt-panel-title" }));

  // ── 形态 ────────────────────────────────────────────────────────────────
  const g1 = el("div", { class: "form-group" });
  g1.appendChild(elI18n("label", "section.pg.cfg.modality", { class: "form-label" }));
  g1.appendChild(segGroup(
    MODALITIES.map((m) => ({ value: m, label: t(MODALITY_KEY[m]) })),
    ui.modality,
    (v) => { ui.modality = v; ui.model = ""; render(); },
  ));
  card.appendChild(g1);

  // ── 协议（只有对话形态才有）────────────────────────────────────────────
  if (ui.modality === "chat") {
    const g2 = el("div", { class: "form-group" });
    g2.appendChild(elI18n("label", "section.pg.cfg.protocol", { class: "form-label" }));
    // 展示名走目录里的 `label`（协议的**专名**，刻意不进 i18n）。
    g2.appendChild(segGroup(
      catalog.protocols.map((p) => ({ value: p.id, label: p.label })),
      ui.protocol,
      (v) => { ui.protocol = v; ui.model = ""; render(); },
    ));
    card.appendChild(g2);
  }

  // ── 模型 ────────────────────────────────────────────────────────────────
  const g3 = el("div", { class: "form-group" });
  g3.appendChild(elI18n("label", "section.pg.cfg.model", { class: "form-label" }));
  const rows = modelsForModality();
  const sel = el("select", { class: "form-control" });
  if (rows.length === 0) {
    // ⚠️「这个形态下没有可选模型」是一句真话，**不画一个空下拉框假装能用**。
    sel.disabled = true;
    sel.appendChild(el("option", { value: "" }, t("section.pg.cfg.noModel")));
  } else {
    if (ui.model === "" || !rows.some((m) => m.id === ui.model)) ui.model = rows[0].id;
    for (const m of rows) sel.appendChild(el("option", { value: m.id }, m.id));
    sel.value = ui.model;
    sel.addEventListener("change", () => { ui.model = sel.value; });
  }
  sel.disabled = sel.disabled || inFlight;
  g3.appendChild(sel);
  card.appendChild(g3);

  // ── 流式（只有对话形态才有）────────────────────────────────────────────
  if (ui.modality === "chat") {
    const g4 = el("div", { class: "form-group" });
    const lab = el("label", { class: "mt-check" });
    const cb = el("input", { type: "checkbox" });
    cb.checked = ui.stream;
    cb.disabled = inFlight;
    cb.addEventListener("change", () => { ui.stream = cb.checked; });
    lab.appendChild(cb);
    lab.appendChild(elI18n("span", "section.pg.cfg.stream"));
    g4.appendChild(lab);
    card.appendChild(g4);
  }

  // ── 网关口令 ────────────────────────────────────────────────────────────
  const g5 = el("div", { class: "form-group" });
  g5.appendChild(elI18n("label", "section.pg.cfg.token", { class: "form-label" }));
  const row = el("div", { class: "mt-key-row" });
  const input = el("input", { type: "password", class: "form-control", autocomplete: "off", "data-i18n-ph": "section.pg.cfg.tokenPh" });
  input.setAttribute("placeholder", t("section.pg.cfg.tokenPh"));
  input.value = token;
  row.appendChild(input);
  const save = elI18n("button", "section.pg.cfg.save", { type: "button", class: "btn btn-outline" });
  save.addEventListener("click", () => {
    writeToken(input.value.trim());
    toast(t(token === "" ? "section.pg.token.cleared" : "section.pg.token.saved"), "ok");
    render();
  });
  row.appendChild(save);
  g5.appendChild(row);

  // ⚠️⚠️ **三态必须同时靠文字表达，颜色只是加强**（WCAG 1.4.1）。
  //    `unknown`（设置页那份 hint 读不到，**比不了**）绝不许并进 `mismatch`
  //    （比过了、不一样）—— 后者会让运维去改一把其实没错的口令。
  const hintState = tokenHintOf();
  const hint = el("div", {
    class: hintState === "mismatch" ? "form-hint mt-hint-warn" : "form-hint",
  });
  hint.appendChild(elI18n("span", {
    empty: "section.pg.token.empty",
    unknown: "section.pg.token.unknown",
    match: "section.pg.token.match",
    mismatch: "section.pg.token.mismatch",
  }[hintState]));
  g5.appendChild(hint);
  // 🔴 **这一句话是本板块的一条硬纪律**：面板拿不到明文网关口令（配置接口对凭据
  //    只回 `{configured, hint}`），别做「一键填入我的口令」——那需要在后端开一个
  //    明文回显口子。
  g5.appendChild(elI18n("p", "section.pg.token.note", { class: "mng-inline-note" }));
  card.appendChild(g5);

  // ── 提示词 ──────────────────────────────────────────────────────────────
  const g6 = el("div", { class: "form-group" });
  g6.appendChild(elI18n("label", "section.pg.cfg.prompt", { class: "form-label" }));
  const ta = el("textarea", { class: "form-control mt-prompt", "data-i18n-ph": "section.pg.cfg.promptPh" });
  ta.setAttribute("placeholder", t("section.pg.cfg.promptPh"));
  ta.value = nodes.prompt;
  ta.disabled = inFlight;
  ta.addEventListener("input", () => { nodes.prompt = ta.value; });
  g6.appendChild(ta);
  card.appendChild(g6);

  // ── 动作 ────────────────────────────────────────────────────────────────
  const actions = el("div", { class: "mt-actions" });
  const send = elI18n("button", "section.pg.send", { type: "button", class: "btn btn-primary" });
  send.disabled = inFlight || rows.length === 0;
  send.addEventListener("click", () => { void submit(); });
  actions.appendChild(send);
  if (inFlight) {
    const cancel = elI18n("button", "section.pg.cancel", { type: "button", class: "btn btn-outline" });
    cancel.addEventListener("click", () => { if (abort !== null) abort.abort(); });
    actions.appendChild(cancel);
  }
  const clear = elI18n("button", "section.pg.clear", { type: "button", class: "btn btn-outline" });
  clear.disabled = inFlight || turns.length === 0;
  clear.addEventListener("click", () => {
    turns = [];
    meta = { status: null, latencyMs: null, chunks: null, malformed: null };
    failure = null;
    rawBody = null;
    mediaUrls = [];
    render();
  });
  actions.appendChild(clear);
  card.appendChild(actions);
  return card;
}

function metaItem(labelKey, value) {
  const item = el("div", { class: "mt-meta-item" });
  item.appendChild(elI18n("span", labelKey, { class: "mt-meta-label" }));
  item.appendChild(el("span", { class: "mt-meta-val" }, value));
  return item;
}

function responsePanel() {
  const card = el("div", { class: "card mt-response-card" });
  const head = el("div", { class: "mt-response-head" });
  head.appendChild(elI18n("h3", "section.pg.resp.title", { class: "mt-panel-title" }));
  const m = el("div", { class: "mt-meta" });
  // 🔴 **没有那个数时画一根 EM DASH，绝不画 0**：0 毫秒是一句关于链路的话，
  //    「我们没有这个数」是一句关于我们自己的话。
  m.appendChild(metaItem("section.pg.resp.status", meta.status === null ? "—" : String(meta.status)));
  m.appendChild(metaItem("section.pg.resp.latency", meta.latencyMs === null ? "—" : `${meta.latencyMs} ms`));
  if (meta.chunks !== null) m.appendChild(metaItem("section.pg.resp.chunks", String(meta.chunks)));
  // 只在真有畸形块时才画这一格：恒显示一个 0 会让它变成背景噪音。
  if (meta.malformed !== null && meta.malformed > 0) {
    m.appendChild(metaItem("section.pg.resp.malformed", String(meta.malformed)));
  }
  head.appendChild(m);
  card.appendChild(head);

  const chat = el("div", { class: "mt-chat" });
  if (turns.length === 0 && failure === null) {
    const ph = el("div", { class: "mt-chat-placeholder" });
    ph.appendChild(elI18n("p", "section.pg.resp.placeholder"));
    chat.appendChild(ph);
  }
  for (const turn of turns) {
    const row = el("div", { class: `mt-msg mt-msg-${turn.role === "user" ? "user" : "assistant"}` });
    row.appendChild(el("div", { class: "mt-avatar" }, t(turn.role === "user" ? "section.pg.turn.user" : "section.pg.turn.assistant")));
    const bubble = el("div", { class: "mt-bubble" });
    // 🔴 **一切来自上游的正文一律 `textContent`**（`el()` 走的就是它）。
    bubble.appendChild(el("div", { class: "mt-bubble-text" }, turn.text));
    if (turn.streaming === true) bubble.appendChild(el("span", { class: "mt-typing-cursor" }));
    row.appendChild(bubble);
    chat.appendChild(row);
  }
  if (failure !== null) {
    const box = el("div", { class: "mt-error" });
    box.appendChild(elI18n("div", "section.pg.err.title", { class: "mt-error-title" }));
    // 上游那份错误正文**原样显示**（`textContent`），不改写、不拼接。
    box.appendChild(el("pre", { class: "mt-error-body" }, failure));
    chat.appendChild(box);
  }
  card.appendChild(chat);

  // ── 视频轮询进度 ────────────────────────────────────────────────────────
  if (poll !== null) {
    const line = el("div");
    line.appendChild(el("div", { class: "mng-inline-note" },
      t("section.pg.video.polling", { n: poll.attempt, max: VIDEO_POLL_MAX_ATTEMPTS })));
    const meter = el("div", { class: poll.attempt > VIDEO_POLL_MAX_ATTEMPTS / 2 ? "meter warm" : "meter" });
    const bar = el("span");
    bar.style.width = `${Math.min(100, Math.round((poll.attempt / VIDEO_POLL_MAX_ATTEMPTS) * 100))}%`;
    meter.appendChild(bar);
    line.appendChild(meter);
    card.appendChild(line);
  }

  // ── 媒体结果 ────────────────────────────────────────────────────────────
  if (mediaUrls.length > 0) {
    card.appendChild(elI18n("div", "section.pg.media.title", { class: "key-sub-h" }));
    for (const url of mediaUrls) {
      const row = el("div", { class: "key-line2" });
      if (embeddable(url)) {
        const img = el("img", { class: "mt-chat-img", alt: "", src: url });
        row.appendChild(img);
      } else {
        // ⚠️ 远端地址只给链接，理由在 `embeddable()` 上方（面板自己的 CSP 会拦图）。
        const a = el("a", { class: "kv-mono", href: url, target: "_blank", rel: "noopener noreferrer" }, url);
        row.appendChild(a);
        row.appendChild(elI18n("span", "section.pg.media.remote", { class: "cell-sub" }));
      }
      card.appendChild(row);
    }
  }

  // ── 响应体原文 ──────────────────────────────────────────────────────────
  if (rawBody !== null) {
    card.appendChild(elI18n("div", "section.pg.raw.title", { class: "key-sub-h" }));
    // 🔴 **`.mt-raw`，不是 `.logs-detail-body`。** 上一版借用后者是因为当时 CSS 那一面
    //    还没有这条规则，而**它没有任何高度上界** —— 一次几百 KB 的响应体会把整张卡
    //    拉成一条望不到头的长条，下面的一切（含「再发一次」）全被顶出视口。
    //    `.mt-raw`（06 §2.12 的表里点名的那条）给的是 `--code-bg` 深色面 +
    //    `max-height:320px` + `overflow:auto`：长响应体在自己那一框里滚，卡片不变形。
    card.appendChild(el("pre", { class: "mt-raw" }, rawBody));
  }
  return card;
}

function render() {
  if (nodes === null) return;
  const host = nodes.body;
  host.textContent = "";

  if (catalog === null) {
    // 🔴 目录读不出来 ⇒ **一颗按钮都不画**。画出来的话按下去只会发出零个请求，
    //    而屏幕上什么都不会变。
    const empty = el("div", { class: "empty-state" });
    // ⚠️ **「还在读」与「读不出来」不共用配色**：给一条还在进行中的提示套上危险色，
    //    等于每次进板块都先闪一次红。
    empty.appendChild(catalogFailed
      ? elI18n("span", "section.pg.unavailable", { class: "text-danger" })
      : elI18n("span", "section.pg.loading", { class: "text-muted" }));
    if (catalogFailed) {
      const retry = elI18n("button", "section.pg.retry", { type: "button", class: "btn btn-sm btn-outline" });
      retry.addEventListener("click", () => { void loadCatalog(); });
      empty.appendChild(retry);
    }
    host.appendChild(empty);
    return;
  }

  const layout = el("div", { class: "mt-layout" });
  layout.appendChild(configPanel());
  layout.appendChild(responsePanel());
  host.appendChild(layout);
}

/* ============================================================
   口令存取
   ============================================================ */

function readToken() {
  try { return localStorage.getItem(GW_KEY_STORE) || ""; } catch (e) { return ""; }
}

/**
 * 存下这把口令。**空串 = 清掉**，不是存一个空值：留着一个空串会让下次进来时
 * 「粘过但清空了」与「从来没粘过」分不开。
 */
function writeToken(value) {
  token = value;
  try {
    if (value === "") localStorage.removeItem(GW_KEY_STORE);
    else localStorage.setItem(GW_KEY_STORE, value);
  } catch (e) { /* 隐私模式：本次会话照常可用，刷新后要重新粘 */ }
}

/**
 * 粘进来的这把与设置页那个 `hint` 对不对得上。**一次即时自查**：粘错口令时不必
 * 等一次 401 才发现。
 *
 * ⚠️⚠️ **返回值只有档位名，绝不含口令的任何一个字节**：它会被渲染到屏幕上，
 * 把「你粘的是 …abcd，配置里是 …wxyz」画出去等于面板自己给出了一条口令末位的旁路。
 * ⚠️ **末几位取几位由 `hint` 自己的长度决定，不写死 4**：那条规则是后端的
 * （`src/admin/config.rs` 的 `hint_of`），写死就是在这里又抄了一次。
 */
function tokenHintOf() {
  if (token === "") return "empty";
  if (typeof tokenHint !== "string" || tokenHint === "") return "unknown";
  return token.slice(-tokenHint.length) === tokenHint ? "match" : "mismatch";
}

/* ============================================================
   网络
   ============================================================ */

async function loadCatalog() {
  try {
    const body = await api.get("/models");
    catalog = narrow(body);
    catalogFailed = catalog === null;
  } catch (e) {
    // ⚠️ **两个不同的失败落到同一档**：HTTP 失败与「读得回来但形状不对」在面板上
    //    都是「我们不知道这个网关长什么样」。
    catalog = null;
    catalogFailed = true;
  }
  // ⚠️ **`protocols` 为空要单独判一次，别写成 `catalog.protocols[0].id`。**
  //    `narrow()` 放行空数组（那是一份合法的目录：一个协议都没开的网关），
  //    而空数组下取 `[0].id` 会抛 TypeError —— 抛在这个 `await` 之后，
  //    `render()` 根本不会跑，屏幕永远停在「正在读取」那一句上，
  //    控制台之外没有任何迹象。
  if (catalog !== null && ui.protocol === "" && catalog.protocols.length > 0) {
    ui.protocol = catalog.protocols[0].id;
  }
  render();
}

/**
 * 读一次配置里那份 `{configured, hint}`。**只为了口令一致性自查那一格。**
 * ⚠️ 代价如实登记：这是一次真实的存储读；所以它**只在这个板块第一次显示时拉一次**，
 * 之后不再拉（那份 hint 只有在设置页改过口令之后才会变，而那要重进面板）。
 */
async function loadTokenHint() {
  try {
    const body = await api.get("/config");
    const b = asObject(body);
    const creds = b === null ? null : asObject(b.credentials);
    const gw = creds === null ? null : asObject(creds.gatewayToken);
    tokenHint = gw === null ? null : str(gw.hint);
  } catch (e) {
    // 读不到就是「比不了」那一档，**不是「不一样」**。
    tokenHint = null;
  }
}

/* ============================================================
   发一轮
   ============================================================ */

function resetRound() {
  meta = { status: null, latencyMs: null, chunks: null, malformed: null };
  failure = null;
  rawBody = null;
  mediaUrls = [];
  poll = null;
}

function fail(code) {
  failure = t(FAIL_KEY[code] ?? "section.pg.err.transport");
}

/** 多轮上下文有上界。**超了要在屏幕上说出来**，不是静默丢。 */
function pushTurn(turn) {
  turns.push(turn);
  if (turns.length > TURNS_MAX) {
    turns = turns.slice(turns.length - TURNS_MAX);
    toast(t("section.pg.turnsTrimmed", { max: TURNS_MAX }), "warn");
  }
}

async function submit() {
  if (inFlight) return;
  if (catalog === null) return;
  const prompt = nodes.prompt.trim();
  if (prompt === "") { toast(t("section.pg.err.emptyPrompt"), "warn"); return; }
  // 🔴 **「还没粘口令」与「口令粘错了」必须分得开**：前者一个字节都不发。
  if (token === "") { resetRound(); fail("no_token"); render(); return; }

  resetRound();
  pushTurn({ role: "user", text: prompt });
  inFlight = true;
  abort = new AbortController();
  render();

  try {
    if (ui.modality === "chat") await runChat(prompt);
    else if (ui.modality === "image") await runImage(prompt);
    else await runVideo(prompt);
  } finally {
    inFlight = false;
    abort = null;
    poll = null;
    render();
  }
}

async function runChat(prompt) {
  const proto = protoById(ui.protocol);
  if (proto === null) { fail("build_failed"); return; }
  const req = buildRequest(proto, {
    model: ui.model, prompt, stream: ui.stream,
    origin: nodes.origin, samplePrompt: catalog.samplePrompt,
  });
  // 🔴 构造不出来 ⇒ **一个字节都不发**。静默降级成非流式、或静默丢掉用户输入，
  //    都是「请求照样 200 而面板上看不出任何异常」的那一类。
  if (req === null) { fail("build_failed"); return; }

  const startedAt = Date.now();
  let res = null;
  try {
    res = await openGateway(req, abort.signal);
  } catch (e) {
    fail(e instanceof GatewayError ? e.code : "transport_error");
    return;
  }
  meta = { ...meta, status: res.status, latencyMs: Date.now() - startedAt };

  // ⚠️ **上游不 ok 时不进读流那条路**：网关的错误响应是 JSON 不是 SSE，拿 SSE
  //    解析器去读它会得到零条负载 ⇒ 面板显示一次「什么都没发生」的成功流式。
  if (!ui.stream || !res.ok || res.body === null || res.body === undefined) {
    const body = await readJson(res);
    rawBody = body === null ? null : JSON.stringify(body, null, 2);
    if (!res.ok) {
      failure = rawBody === null ? t("section.pg.err.http", { status: res.status }) : rawBody;
      return;
    }
    // 非流式：正文在哪一格是协议的知识，本文件不认识 ⇒ **只画原文**，
    // 不假装从里面「猜」出一句回答。
    pushTurn({ role: "assistant", text: t("section.pg.resp.nonStream") });
    return;
  }

  const turn = { role: "assistant", text: "", streaming: true };
  pushTurn(turn);
  let chunks = 0;
  let malformed = 0;
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buf = "";
  try {
    for (;;) {
      const step = await reader.read();
      if (step.done) break;
      // **`{ stream: true }`**：一个多字节字符同样可能被拆在两个 chunk 之间。
      // 不带它的话中文 / emoji 会在切点上变成替换符，而那在 ASCII 假上游下一个字
      // 都看不出来。
      buf += decoder.decode(step.value, { stream: true });
      const found = sseFrames(buf);
      buf = found.rest;
      for (const p of found.payloads) {
        chunks++;
        const text = deltaText(proto, p);
        if (text === null) malformed++;
        else turn.text += text;
      }
      meta = { ...meta, chunks, malformed };
      render();
      if (found.done) break;
    }
    // 流可能不以完整的空行结尾（连接中断、反代截断）⇒ 把解码器里滞留的字节 flush
    // 出来，再把剩下的当成最后一帧处理，否则最后一个事件会被静默丢弃。
    // ⚠️ **这一句只在正常读完时才跑**：下面 `catch` 那条路上它根本不跑。
    buf += decoder.decode();
    for (const p of sseFrames(`${buf}\n\n`).payloads) {
      chunks++;
      const text = deltaText(proto, p);
      if (text === null) malformed++;
      else turn.text += text;
    }
  } catch (e) {
    // 读到一半断了。**已经交出去的那几块留着** —— 运维看到的半句话是真的发生过的，
    // 抹掉它比留着更不诚实；这里只在后面补一句「这条流没读完」。
    fail("stream_error");
  } finally {
    turn.streaming = false;
    meta = { ...meta, chunks, malformed };
    reader.cancel().catch(() => {});
  }
}

/** 一次图片生成：**同步一段式**（建完就有结果）。 */
async function runImage(prompt) {
  const endpoint = mediaEndpoint("image", "generate");
  if (endpoint === null) { fail("no_endpoint"); return; }
  const out = await callMedia(endpoint, prompt, null);
  if (out === null) return;
  mediaUrls = collectMediaUrls(out.body, 0, []);
  if (mediaUrls.length === 0) pushTurn({ role: "assistant", text: t("section.pg.media.none") });
}

/**
 * 一次视频生成：🔴 **两段式**（建任务 + 轮询）。
 *
 * ⚠️ **三条上界同时生效，先撞到哪条算哪条**：次数、总时长、以及「这一轮被取消了」。
 * 少了任何一条，一个永远回 `processing` 的任务会让这个面板**无限轮询下去**
 * —— 而每一次轮询都是一次真实的外网请求。
 */
async function runVideo(prompt) {
  const create = mediaEndpoint("video", "generate");
  const query = mediaEndpoint("video", "poll");
  if (create === null || query === null || query.taskSlot === null) { fail("no_endpoint"); return; }

  const started = await callMedia(create, prompt, null);
  if (started === null) return;
  const taskId = taskIdOf(started.body);
  // 🔴 **拿不到任务标识就停在这里**：接着轮询一个猜出来的 id 只会打出一串 404，
  //    而屏幕上会显示「正在轮询」——那是一句假话。
  if (taskId === null) { fail("no_task_id"); return; }

  const deadline = Date.now() + VIDEO_POLL_MAX_MS;
  for (let attempt = 1; attempt <= VIDEO_POLL_MAX_ATTEMPTS; attempt++) {
    if (Date.now() > deadline) break;
    poll = { attempt };
    render();
    await sleep(VIDEO_POLL_INTERVAL_MS);
    if (abort === null || abort.signal.aborted) return;
    const step = await callMedia(query, prompt, taskId);
    if (step === null) return;
    const urls = collectMediaUrls(step.body, 0, []);
    if (urls.length > 0) {
      mediaUrls = urls;
      poll = null;
      return;
    }
  }
  poll = null;
  // ⚠️ **超时不是失败，是一句「我们不再等了」**：任务可能还在上游跑着。
  fail("poll_timeout");
}

/**
 * 打一次媒体端点。`taskId !== null` 时把它填进路径上那个占位符。
 * 返回 `null` = 这一次失败了（`failure` 已经写好）。
 */
async function callMedia(endpoint, prompt, taskId) {
  let path = endpoint.pathTemplate;
  if (endpoint.taskSlot !== null) {
    if (taskId === null) { fail("no_task_id"); return null; }
    path = path.split(endpoint.taskSlot).join(encodeURIComponent(taskId));
  }
  let body = null;
  if (endpoint.sampleBody !== null) {
    let hits = 0;
    body = mapStrings(endpoint.sampleBody, (s) => {
      if (s !== catalog.samplePrompt) return s;
      hits++;
      return prompt;
    });
    // 与对话那一半逐字同判据：**恰好一次**才算数。
    if (hits !== 1) { fail("build_failed"); return null; }
    if (typeof endpoint.sampleBody.model === "string") {
      if (ui.model === "") { fail("build_failed"); return null; }
      body.model = ui.model;
    }
  }
  const req = { url: `${nodes.origin}${path}`, method: endpoint.method, headerName: endpoint.authHeader, body };

  const startedAt = Date.now();
  let res = null;
  try {
    res = await openGateway(req, abort.signal);
  } catch (e) {
    fail(e instanceof GatewayError ? e.code : "transport_error");
    return null;
  }
  meta = { ...meta, status: res.status, latencyMs: Date.now() - startedAt };
  const parsed = await readJson(res);
  rawBody = parsed === null ? null : JSON.stringify(parsed, null, 2);
  if (!res.ok) {
    failure = rawBody === null ? t("section.pg.err.http", { status: res.status }) : rawBody;
    return null;
  }
  return { body: parsed };
}

/**
 * 从建任务那份响应里取任务标识。
 * 🔴 **取到之后还要过一道形状闸**：它会被拼进一条 URL 的路径段，而它来自上游。
 * 只认「非空、不含斜杠与问号、长度有上界」的那一族 —— `encodeURIComponent` 之外
 * 再加一道，因为一个空串或一个 `..` 在编码之后仍然是一条能打出去的路径。
 */
function taskIdOf(body) {
  const b = asObject(body);
  if (b === null) return null;
  const candidates = [b.id, b.taskId, b.task_id];
  const data = asObject(b.data);
  if (data !== null) candidates.push(data.id, data.taskId, data.task_id);
  for (const c of candidates) {
    if (typeof c !== "string") continue;
    if (c === "" || c.length > 256) continue;
    if (c.includes("/") || c.includes("?") || c.includes("#") || c.includes("..")) continue;
    return c;
  }
  return null;
}

function sleep(ms) {
  return new Promise((resolve) => { setTimeout(resolve, ms); });
}

/* ============================================================
   板块契约
   ============================================================ */

export const playgroundSection = {
  init(host) {
    host.textContent = "";
    const head = el("div", { class: "mt-head" });
    head.appendChild(elI18n("h2", "section.pg.title", { class: "section-title" }));
    host.appendChild(head);
    host.appendChild(elI18n("p", "section.pg.desc", { class: "mng-inline-note" }));
    const body = el("div");
    host.appendChild(body);
    // `location.origin` 是同源自查的参照物，只读一次（一次会话里它不会变）。
    nodes = { body, origin: location.origin, prompt: "" };
  },

  onShow() {
    // 🔴 **每次都重新从存储读一遍口令**：登出不 reload 页面，模块变量在登出之后
    //    仍然活着 ⇒ 光靠 `app.js` 清存储不够。共享 / 投屏的机器上，下一个人登录进来
    //    看到的口令框必须是空的。
    token = readToken();
    if (tokenHint === null) void loadTokenHint().then(render);
    if (catalog !== null) { render(); return; }
    render();
    void loadCatalog();
  },

  onHide() {
    // 在飞的那一轮作废：**它会真的取消掉外网请求**（`AbortController` 交给了 `fetch`）。
    if (abort !== null) { abort.abort(); abort = null; }
    inFlight = false;
    poll = null;
    // ⚠️ **口令不留在模块变量里**：切走之后没有任何理由继续持有它。
    token = "";
  },
};
