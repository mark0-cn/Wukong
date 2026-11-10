//! Presentation only: IDs, tool arguments and raw evidence never pass through translation.
use fluent_bundle::{FluentArgs, FluentResource, concurrent::FluentBundle};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::LazyLock};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum Locale {
    #[default]
    #[serde(rename = "en-US")]
    #[value(name = "en-US")]
    EnUs,
    #[serde(rename = "zh-CN")]
    #[value(name = "zh-CN")]
    ZhCn,
}
impl Locale {
    /// A bounded default, not language detection by an additional model call.
    /// Explicit output requests in the task remain authoritative in the prompt.
    pub fn for_task(task: &str, fallback: Self) -> Self {
        if task.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) {
            Self::ZhCn
        } else if task.chars().any(|c| c.is_ascii_alphabetic()) {
            Self::EnUs
        } else {
            fallback
        }
    }
    pub fn tag(self) -> &'static str {
        match self {
            Self::EnUs => "en-US",
            Self::ZhCn => "zh-CN",
        }
    }
    pub fn negotiate(tag: &str) -> Self {
        let tag = tag.replace('_', "-").to_ascii_lowercase();
        if tag == "zh"
            || tag.starts_with("zh-cn")
            || tag.starts_with("zh-hans")
            || tag.starts_with("zh-sg")
        {
            Self::ZhCn
        } else {
            Self::EnUs
        }
    }
    pub fn system() -> Self {
        Self::negotiate(&sys_locale::get_locale().unwrap_or_default())
    }
    pub fn text(self, code: &str) -> String {
        self.format(code, &BTreeMap::new())
    }
    pub fn has(self, code: &str) -> bool {
        match self {
            Self::EnUs => EN.has_message(code),
            Self::ZhCn => ZH.has_message(code),
        }
    }
    pub fn format(self, code: &str, args: &BTreeMap<String, serde_json::Value>) -> String {
        let bundle = match self {
            Self::EnUs => &*EN,
            Self::ZhCn => &*ZH,
        };
        let Some(pattern) = bundle.get_message(code).and_then(|m| m.value()) else {
            return if self != Self::EnUs {
                Self::EnUs.format(code, args)
            } else {
                code.into()
            };
        };
        let mut values = FluentArgs::new();
        for (key, value) in args {
            if let Some(number) = value.as_f64() {
                values.set(key, number);
            } else if let Some(text) = value.as_str() {
                values.set(key, text);
            }
        }
        let mut errors = Vec::new();
        bundle
            .format_pattern(pattern, Some(&values), &mut errors)
            .into_owned()
    }
}
fn bundle(locale: Locale, source: &str) -> FluentBundle<FluentResource> {
    let resource = FluentResource::try_new(source.into()).expect("validated translation resource");
    let mut bundle = FluentBundle::new_concurrent(vec![locale.tag().parse().unwrap()]);
    bundle.set_use_isolating(false);
    bundle
        .add_resource(resource)
        .expect("unique translation IDs");
    bundle
}
static EN: LazyLock<FluentBundle<FluentResource>> = LazyLock::new(|| {
    bundle(
        Locale::EnUs,
        include_str!(concat!(env!("OUT_DIR"), "/en-US.ftl")),
    )
});
static ZH: LazyLock<FluentBundle<FluentResource>> = LazyLock::new(|| {
    bundle(
        Locale::ZhCn,
        include_str!(concat!(env!("OUT_DIR"), "/zh-CN.ftl")),
    )
});

/// Transport a stable message and optional verbatim diagnostic, never a translated protocol key.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Message {
    pub code: String,
    #[serde(default)]
    pub args: BTreeMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
impl Message {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            ..Self::default()
        }
    }
    pub fn arg(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.args.insert(key.into(), value.into());
        self
    }
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
    pub fn render(&self, locale: Locale) -> String {
        let text = locale.format(&self.code, &self.args);
        match &self.detail {
            Some(detail) => format!("{text}\n{detail}"),
            None => text,
        }
    }
    pub fn from_error(error: anyhow::Error) -> Self {
        Self::from_error_ref(&error)
    }
    pub fn from_error_ref(error: &anyhow::Error) -> Self {
        if let Some(message) = error.downcast_ref::<Self>() {
            let mut message = message.clone();
            let details = error
                .chain()
                .filter(|cause| !cause.is::<Self>())
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            if !details.is_empty() {
                message.detail = Some(details.join(": "));
            }
            message
        } else {
            Self::new("error-operation").detail(format!("{error:#}"))
        }
    }
}
impl std::fmt::Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render(Locale::EnUs))
    }
}
impl std::error::Error for Message {}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum DisplayText {
    Raw(String),
    Localized(Message),
}
impl DisplayText {
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Raw(value) if value.is_empty())
    }
    pub fn render(&self, locale: Locale) -> String {
        match self {
            Self::Raw(value) => value.clone(),
            Self::Localized(message) => message.render(locale),
        }
    }
}
impl Default for DisplayText {
    fn default() -> Self {
        Self::Raw(String::new())
    }
}
impl From<String> for DisplayText {
    fn from(value: String) -> Self {
        Self::Raw(value)
    }
}
impl From<&str> for DisplayText {
    fn from(value: &str) -> Self {
        Self::Raw(value.into())
    }
}
impl From<Message> for DisplayText {
    fn from(value: Message) -> Self {
        Self::Localized(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn negotiation_and_diagnostics_preserve_identity() {
        assert_eq!(Locale::negotiate("zh_Hans_CN"), Locale::ZhCn);
        assert_eq!(Locale::negotiate("fr-FR"), Locale::EnUs);
        let message = Message::new("error-operation").detail("sha256=原始证据\n0x123");
        assert!(
            message
                .render(Locale::EnUs)
                .ends_with("sha256=原始证据\n0x123")
        );
        assert!(message.render(Locale::ZhCn).starts_with("操作未能完成"));
        let native = anyhow::anyhow!("raw cause: 0x42").context(Message::new("error-operation"));
        let native = Message::from_error(native);
        assert_eq!(native.code, "error-operation");
        assert_eq!(native.detail.as_deref(), Some("raw cause: 0x42"));
    }
}
