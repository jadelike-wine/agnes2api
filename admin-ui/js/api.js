/**
 * 面板**登录之后**打管理接口的唯一网络出口。
 *
 * ⚠️ **"唯一"限定在"登录之后打 `/admin/api`"这一句上，别把它读成"全站唯一"。**
 * 本轮（P2b 外壳）全站一共 **3 个** `fetch` 调用点：本模块的 `raw()`、
 * `js/app.js` 的登录探针、`js/app.js` 的健康探针；调试台落地后会有第 4 个
 *（网关口令那把钥匙，**绝不许合流到本模块**）。
 * 逐条说清各自**为什么**绕开本模块：
 *   · **登录探针**：那一刻还没有会话可言 —— 没有 `expired()` 的前置（时刻键还没写）、
 *     不该走 `onUnauthorized`（401 在那里的意思是「口令不对」，不是「你掉线了」）、
 *     口令也还没进 `localStorage`（`readKey()` 读不到它）。
 *   · **健康探针**：`/health` 是**不鉴权**端点，走本模块会给它带上 `x-admin-key`
 *     —— 把口令送去一个不需要它的端点，等于凭空多一条外发路径。
 *   · **网关出口**（将来）：**它拿的是另一把钥匙**，送进本模块等于把两把钥匙合流，
 *     而两者的泄漏面完全不同。
 * 说准这件事很要紧：**本模块正是那个把「唯一出口」当成安全论证前提的地方**
 *（下面「口令只走请求头」那段），顺着一句假的全称句走，审计者会漏掉口令实际
 * 离开页面的其它路径。
 * ⚠️⚠️ **agnes TS 版为这件事栽过**：那边曾用一张手写的写法枚举表去数出口，而那张表
 * 数不出 `new WebSocket(` / 动态 `import(` / 把 fetch 先存进变量再调这三种；
 * 补齐之后又沿另一条轴漏了一族（扫描前会先抠掉模板串的字面文本，而**带花括号的
 * 插值会连同里面真会执行的代码一起消失** —— 往那种模板串里塞一个真 `fetch`，
 * 全套判据照绿）。⭐ 记一条形状：**收窄一句全称句时，先把它可能失效的轴列全，再逐条验。**
 *
 * **两把钥匙严格隔离**：本模块只发 `x-admin-key`、只打 `/admin/api/*`。
 * 网关口令与 `/v1` 是调试台的事，**不许在这里出现**。
 *
 * **401 = 会话失效；403 明确不当会话失效。**
 * 后者是照抄 kiro2api 用中文注释警告过的那个坑：它老写法把业务 403 当掉线，
 * 管理员被拒绝一次授权就被踢出后台并被告知「密钥无效」。
 */
import { sessionExpired } from "./pure/session.mjs";
// 键名的单一真源。**别在这里再声明一遍**：写入方是 `app.js`，两处分叉会让面板
// 登录成功之后每个请求都送空口令头（实测过：401 ⇒ 登出循环，而全套用例照绿）。
import { KEY_STORE, SAVED_AT_STORE } from "./pure/storage-keys.mjs";

const BASE = "/admin/api";

export class ApiError extends Error {
  constructor(status, message, body) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.body = body;
  }
}

let unauthorizedHandler = null;
/** 会话失效时的回调（清 key + 回登录闸）。只由 `app.js` 注册一次。 */
export function onUnauthorized(cb) { unauthorizedHandler = cb; }

function readKey() {
  try { return localStorage.getItem(KEY_STORE) || ""; } catch (e) { return ""; }
}

/**
 * 会话是不是已经到达绝对上限。判定在 `js/pure/session.mjs`，这里只喂参数。
 *
 * ⚠️ **为什么这一判必须在每个请求上做，而不是只在 `app.js` 启动那一次做。**
 * 面板是**常驻**的：日志在轮询、账号列表有自动刷新，运维把标签页开一整天是常态。
 * 只在模块加载那一次判的话，12 小时到点后这个标签页照样拿着那把口令继续打接口
 * —— 而部署文档逐字承诺「面板会在 12 小时后要求重新输入口令」，那就成了
 * **一句被测试保护起来、却在最常见的路径上不成立的承诺**。
 *
 * 放在**发请求之前**：过期就不发，与前端字符集拦截同一个道理 —— 送出去也没用，
 * 而且会在服务端留下一条无意义的登录失败记录。
 */
function expired() {
  try {
    return sessionExpired(Number(localStorage.getItem(SAVED_AT_STORE)), Date.now());
  } catch (e) {
    // 隐私模式下 localStorage 抛错：读不到时刻 ⇒ 与 sessionExpired 同一个方向，fail closed。
    return true;
  }
}

export async function raw(method, path, body, init) {
  // **会话绝对上限，每请求复查一次**（见 expired 的说明）。走的是与 401 完全相同的
  // 那条通路：`unauthorizedHandler()` 清会话 + 回登录闸，再抛同一个 ApiError ——
  // 调用方（各板块的轮询/刷新）已经会处理 401，不需要为过期再认一种新错误。
  if (expired()) {
    if (unauthorizedHandler) unauthorizedHandler();
    throw new ApiError(401, "session_expired", null);
  }
  // 🔴 **口令只走请求头。禁止 Cookie 会话、禁止 `?key=`**：口令进 URL 会落进浏览器
  // 历史、Referer、反代与 CDN 的访问日志。这条禁令同时否掉了 `EventSource`
  //（它设不了请求头）—— 所以实时日志在本轮走轮询，而不是照抄 kiro 的 `?api_key=`。
  const headers = { "x-admin-key": readKey() };
  if (body !== undefined) headers["content-type"] = "application/json";
  const res = await fetch(BASE + path, {
    method,
    headers: Object.assign(headers, (init && init.headers) || {}),
    body: body === undefined ? undefined : JSON.stringify(body),
    // 同源，且刻意不带凭据：本项目没有 Cookie 会话，带上只会扩大攻击面。
    credentials: "omit",
    signal: init && init.signal,
  });
  if (res.status === 401) {
    // **只有 401 清会话。** 403 是「这个操作被拒绝」，不是「你没登录」。
    if (unauthorizedHandler) unauthorizedHandler();
    throw new ApiError(401, "unauthorized", null);
  }
  return res;
}

async function json(method, path, body, init) {
  const res = await raw(method, path, body, init);
  let parsed = null;
  try { parsed = await res.json(); } catch (e) { parsed = null; }
  if (!res.ok) {
    const msg = parsed && parsed.error && parsed.error.message ? parsed.error.message : `http_${res.status}`;
    throw new ApiError(res.status, msg, parsed);
  }
  return parsed;
}

export const api = {
  get: (p, init) => json("GET", p, undefined, init),
  post: (p, b, init) => json("POST", p, b, init),
  put: (p, b, init) => json("PUT", p, b, init),
  // 账号池的行内动作（停用/启用/清冷却/解除剔除/备注）都是 `PATCH /admin/api/keys/:id`。
  patch: (p, b, init) => json("PATCH", p, b, init),
  del: (p, init) => json("DELETE", p, undefined, init),
  raw,
};
