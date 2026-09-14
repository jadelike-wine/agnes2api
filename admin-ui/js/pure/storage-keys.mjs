/**
 * 面板用到的**全部浏览器存储键名，单一真源**。
 *
 * ⚠️ **这个文件是一条踩过的坑的直接产物。** 在它出现之前（agnes TS 版），
 * `agnes2api_admin_key` 与 `agnes2api_admin_key_at` 这两个名字**各自在两个文件里
 * 被声明了两遍**（写入方 `js/app.js` 一份、读取方 `js/api.js` 一份），而两份之间
 * 没有任何东西把它们绑在一起。实测：只改写入方那一处 ⇒ 登录成功、进壳层，随后每个
 * 请求送出去的 `x-admin-key` 是空串 ⇒ 401 ⇒ 登出循环，**面板彻底不可用，而全套
 * 用例照绿**。
 *
 * ⚠️ **`js/boot.js` 抄不了这里，那是结构性的，不是疏漏**：它是全站唯一的经典脚本
 *（`<head>` 里同步、非 module，理由见该文件头），**经典脚本没有 `import`**。
 * 于是主题与语言这两个名字在 `boot.js` 里必然是字面量。这条缺口不靠自觉，靠
 * `src/webui/mod.rs` 的 `boot_js_storage_key_literals_match_the_module` 那一格兜住
 * —— 那一格也顺带钉住「除 `boot.js` 外任何文件都不许再写 `agnes2api_` 字面量」。
 *
 * 这个目录下的文件受三条硬规则约束：**顶层只许 `export`、禁 `import`、
 * 禁出现 `document` / `window`**（这样它们才可能在没有浏览器的地方被验）。
 */

/** 管理口令本身。**与下面那个时刻键必须一起写、一起清**，理由见 SAVED_AT_STORE。 */
export const KEY_STORE = "agnes2api_admin_key";

/**
 * 「口令是什么时候存下来的」。会话绝对上限靠它算年龄，判定本身在 `./session.mjs`。
 *
 * **两个键必须一起写、一起清**：只清口令不清时刻，下次登录前那个时刻还是旧的；
 * 只写口令不写时刻，`sessionExpired()` 拿不到时刻会把刚登录的会话判成过期
 *（方向是 fail closed，但用户会永远进不去）。
 */
export const SAVED_AT_STORE = "agnes2api_admin_key_at";

/**
 * 上次停留在哪个板块（刷新复原用）。
 *
 * ⚠️ **裁定 P-6 改过板块名**（`overview`→`dashboard`、`keys`→`accounts`、
 * `events`→`logs`）⇒ 老用户这个键里存的旧值**不再是合法板块名**。
 * `js/app.js` 的 `showSection()` 有「不认识的值回落到默认板块」那条兜底，**别改它**。
 */
export const SECTION_STORE = "agnes2api_section";

/** 主题。**`js/boot.js` 里有一份必然的字面量副本**，见文件头。 */
export const THEME_STORE = "agnes2api_theme";

/** 界面语言。同样在 `js/boot.js` 里有一份必然的字面量副本。 */
export const LANG_STORE = "agnes2api_lang";

/**
 * 开发期开关：为真时 `t()` 会把缺失的 key 打进控制台。
 * **面板自己从不写它**（只读），运维/开发者手工设，所以它没有「两处分叉」的风险；
 * 收进来只是为了让「除 boot.js 外不许再出现 `agnes2api_` 字面量」那条判据能收干净。
 */
export const DEBUG_STORE = "agnes2api_debug";

/**
 * **调试台（playground）用的网关口令**。
 *
 * ⚠️⚠️ **它与上面那把管理口令（`KEY_STORE`）是两把完全不同的钥匙，必须分开存。**
 * 管理口令走 `x-admin-key` 打 `/admin/api` 那棵树；网关口令是发给**每一个下游用户**
 * 的那把中转口令，走各协议惯用的鉴权头打对外那棵树。两者共用一个存储键的后果是
 * 「面板登录一次就等于把中转口令也换掉了」—— 而它们的泄漏面完全不同。
 *
 * ⚠️⚠️ **登出时必须与上面那两个键一起清掉**（`js/app.js` 的 `store("del")`）。
 * 理由不是洁癖，是**别让面板严格变差**：会话 12 小时上限那条把「localStorage 里放的
 * 是原始 `ADMIN_TOKEN`，无过期、无登出、产品内无撤销路径」当成设上限的**理由**；
 * 而这一把**比它还弱**（没有年龄上限、后端也没有撤销路径）。再往同一块存储里加一把
 * 更弱的、还不跟着登出清掉的，是严格变差。
 * 具体后果很朴素：共享 / 投屏的机器上，登出之后下一个人登录进来，调试台的口令框是
 * **预填好的**。
 * ⚠️ **光清存储不够**：调试台的模块变量在登出后仍然活着（登出不 reload 页面），
 * 所以那边必须在 `onShow()` 里**每次都重新从存储读一遍**。两处缺一不可。
 */
export const GW_KEY_STORE = "agnes2api_gw_key";
