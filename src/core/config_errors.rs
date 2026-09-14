//! 配置错误的**词表与类型**，外加「网关拒绝服务」那一档的专用错误。
//!
//! 它是一个**零依赖的叶子**：装载侧（`config_provenance`）与校验侧
//! （将来的 `core::admin::config_validate`）都从这里拿码，放在任一边都会形成
//! 模块环（TS 版实测踩过 `provenance → validate → provenance` 的运行期值循环）。

use std::collections::BTreeMap;
use std::fmt;

/// 一条逐字段错误。
///
/// `code` 是机器可读判别串，**面板靠它选五语言文案，不解析 `message`**
/// （本类型刻意**没有** `message` 字段 —— 文案在面板的字典里，后端只给码与参数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// 面板路径，例如 `maxStrikes` / `registrar.yyds.baseUrl`。
    pub field: String,
    pub code: ConfigErrorCode,
    /// 渲染文案要的参数（下界、两个冲突值……）。
    /// 🔴 **只放标量、永不放凭据**，与 `LogEntry.fields` 同一条纪律。
    pub params: BTreeMap<String, String>,
}

/// 全部配置错误码。**单一真源是下面这个数组，类型从它派生。**
///
/// 🔴 **为什么是「数组是真源、类型是派生」而不是反过来**（TS 版评审实测的教训）：
/// 手写一个枚举 + 在测试里放一份镜像数组，那条护栏是**假的** —— 镜像只保证
/// 「每一项都是合法的码」，**不保证每一个码都在镜像里**。加一个新码而不补五语言
/// 文案时全绿零信号，而后果是面板把裸码显示给运维。
///
/// ⇒ Rust 侧的落法：数组是真源，`ConfigErrorCode::ALL` 就是它；
/// 判据遍历 `ALL`，加一个码不补文案当场红。
///
/// ⚠️ 规格 §7.12 引的是「21 条」，那是旧数。**本轮一手数过 TS 原文件是 22 条**，
/// 以这里为准。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfigErrorCode {
    /// 请求体里有本表不认识的字段（拼错的字段名在宽松实现下是一次「保存成功、什么都没发生」）。
    UnknownField,
    /// 这个字段被环境变量锁定，写它不会生效 —— **拒绝而不是静默接受**。
    LockedByEnv,
    NotAnInteger,
    BelowMin,
    NotAString,
    NotABoolean,
    Empty,
    TooLong,
    NotAUrl,
    NotAChannel,
    /// 注册机开着却没选通道。
    ChannelRequired,
    DelayMinGtMax,
    /// 注册机开着、这条通道被选中，却没有凭据。
    ChannelCredentialsMissing,
    /// ⚠️ 下面三条**不是错误，是不拦人的通知**。它们进这张表只因为面板选文案走同一条路
    /// （这张表是「每个码都有五语言文案」那格判据的遍历源）。
    /// **不许因此把它们当 blocker 用**：blocker 的意思是「注册机本次不启动」，
    /// 这三条恰恰配着「注册机照常跑」。
    LegacyChannelKey,
    LegacyChannelEnv,
    LegacyFallbackIgnored,
    /// 两边都没有网关口令 ⇒ 冷启动会 fail-closed。
    GatewayTokenRequired,
    /// 凭据首尾带空白：HTTP 头值在传输层被 trim，客户端**永远送不出**这个值。
    WhitespacePadded,
    /// 凭据含送不出去的字符（非可打印 ASCII）。
    NotSendable,
    TooShort,
    /// 网关口令不得等于管理口令：中转口令是发给每一个下游用户的。
    SameAsAdminToken,
    /// **这份配置构造不出来，但说不出是哪一格。**
    /// 保留它是因为「说不出哪一格」这件事本身必须有一个如实的表达，
    /// **不编一个具体字段出来**；具体原因走事件板块。
    ConfigUnloadable,
}

impl ConfigErrorCode {
    /// 🔴 **唯一真源。** 判据遍历它，面板字典对着它对账。
    pub const ALL: &'static [ConfigErrorCode] = &[
        Self::UnknownField,
        Self::LockedByEnv,
        Self::NotAnInteger,
        Self::BelowMin,
        Self::NotAString,
        Self::NotABoolean,
        Self::Empty,
        Self::TooLong,
        Self::NotAUrl,
        Self::NotAChannel,
        Self::ChannelRequired,
        Self::DelayMinGtMax,
        Self::ChannelCredentialsMissing,
        Self::LegacyChannelKey,
        Self::LegacyChannelEnv,
        Self::LegacyFallbackIgnored,
        Self::GatewayTokenRequired,
        Self::WhitespacePadded,
        Self::NotSendable,
        Self::TooShort,
        Self::SameAsAdminToken,
        Self::ConfigUnloadable,
    ];

    /// 对外线格式。🔴 **这些串是契约**（面板拿它查字典），一个字都不许改。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownField => "unknown_field",
            Self::LockedByEnv => "locked_by_env",
            Self::NotAnInteger => "not_an_integer",
            Self::BelowMin => "below_min",
            Self::NotAString => "not_a_string",
            Self::NotABoolean => "not_a_boolean",
            Self::Empty => "empty",
            Self::TooLong => "too_long",
            Self::NotAUrl => "not_a_url",
            Self::NotAChannel => "not_a_channel",
            Self::ChannelRequired => "channel_required",
            Self::DelayMinGtMax => "delay_min_gt_max",
            Self::ChannelCredentialsMissing => "channel_credentials_missing",
            Self::LegacyChannelKey => "legacy_channel_key",
            Self::LegacyChannelEnv => "legacy_channel_env",
            Self::LegacyFallbackIgnored => "legacy_fallback_ignored",
            Self::GatewayTokenRequired => "gateway_token_required",
            Self::WhitespacePadded => "whitespace_padded",
            Self::NotSendable => "not_sendable",
            Self::TooShort => "too_short",
            Self::SameAsAdminToken => "same_as_admin_token",
            Self::ConfigUnloadable => "config_unloadable",
        }
    }
}

impl fmt::Display for ConfigErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 「网关拒绝服务」这一档的专用错误。**它与「配置有问题」不是一回事。**
///
/// 逃出装配路径的**非** `ConfigRefusal` 错误，按定义就是代码 bug。
///
/// `message` 由抛点给，**逐字保留原文**：入口打的就是它，五语言部署文档的故障排查
/// 条目引的也是那句原文（「缺少 GATEWAY_TOKEN，网关无法启动」）。
///
/// 🔴 **它不进任何未鉴权响应体。** 未鉴权路径上回的 `reason` 一律是固定枚举串，
/// 永不由这里的 message 派生 —— 配置细节一个字节都不到未鉴权调用方。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigRefusal(pub String);

impl ConfigRefusal {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl fmt::Display for ConfigRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigRefusal {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// 码表是 22 条，且**期望值是手写字面量**。
    ///
    /// 不写 `ALL.len() == ALL.len()` 这种同义反复（04 §3.3 第 ⑥ 条假阳性）：
    /// 加一条码必须有人在这里表态，这正是这一格的用途。
    #[test]
    fn code_table_has_exactly_22_entries() {
        assert_eq!(ConfigErrorCode::ALL.len(), 22);
    }

    /// 线格式串两两不重复 —— 重复意味着面板字典里有一条永远选不到。
    #[test]
    fn wire_strings_are_unique() {
        let set: BTreeSet<&str> = ConfigErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(
            set.len(),
            ConfigErrorCode::ALL.len(),
            "有码的线格式串重复了"
        );
    }

    /// 抽查几条线格式串是手写字面量（防止有人把 `as_str` 改成从枚举名派生的
    /// snake_case 转换 —— 那会让改一个枚举名静默改掉对外契约）。
    #[test]
    fn wire_strings_are_pinned_literals() {
        assert_eq!(ConfigErrorCode::UnknownField.as_str(), "unknown_field");
        assert_eq!(
            ConfigErrorCode::GatewayTokenRequired.as_str(),
            "gateway_token_required"
        );
        assert_eq!(
            ConfigErrorCode::ConfigUnloadable.as_str(),
            "config_unloadable"
        );
    }

    /// `ConfigRefusal` 的 message **逐字保留**：入口打的就是它。
    #[test]
    fn refusal_message_is_verbatim() {
        let r = ConfigRefusal::new("缺少 GATEWAY_TOKEN，网关无法启动");
        assert_eq!(r.to_string(), "缺少 GATEWAY_TOKEN，网关无法启动");
    }
}
