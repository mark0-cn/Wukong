//! Human catalogs share the registry; translations never change model schemas.
use super::catalog::builtins;
use crate::i18n::Locale;
use anyhow::{Result, ensure};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Translation {
    source: String,
    text: String,
}

pub fn builtin_catalog_markdown() -> String {
    builtin_catalog_markdown_for_locale(Locale::EnUs).expect("English registry catalog")
}

pub fn builtin_catalog_markdown_for_locale(locale: Locale) -> Result<String> {
    let chinese = locale == Locale::ZhCn;
    let translations: BTreeMap<String, Translation> = if chinese {
        serde_json::from_str(include_str!("../../locales/zh-CN/tool-catalog.json"))?
    } else {
        BTreeMap::new()
    };
    let mut output = String::from(if chinese {
        "# 内置工具目录\n\n[English](tool-catalog.md) · 简体中文\n\n由 `wukong --locale zh-CN tool-catalog` 从工具注册表生成。不要手动编辑。\n\n原生协调工具 `task`、`message`、`wait` 由 Rig 提供；MCP 工具取决于配置。下表列出内置业务工具的角色边界及后端要求（保留技术标识）。实际模型列表还会根据环境预检过滤；执行仍校验目标、证据、会话归属和当前后端能力。中文说明仅用于文档，不改变模型协议。\n\n| 工具 | 可见角色 | 有副作用 | 后端要求 | 用途 |\n|---|---|---|---|---|\n"
    } else {
        "# Built-in tool catalog\n\nEnglish · [简体中文](tool-catalog.zh-CN.md)\n\nGenerated from the registry by `wukong tool-catalog`. Do not edit manually.\n\nRig provides native coordination tools `task`, `message`, `wait`; MCP tools depend on configuration. This table lists business tools, role boundaries and backend requirements. Actual model selections are also filtered by environment preflight; execution checks target identity, evidence, session ownership and current backend capabilities. Translated descriptions are documentation only and do not change the model protocol.\n\n| Tool | Visible roles | Side effects | Backend requirement | Purpose |\n|---|---|---|---|---|\n"
    });
    let mut specs = builtins()
        .iter()
        .filter(|spec| spec.model_visible)
        .collect::<Vec<_>>();
    specs.sort_by_key(|spec| spec.definition.name.as_str());
    if chinese {
        ensure!(
            translations.len() == specs.len(),
            "Tool translation set differs from the registry"
        );
    }
    for spec in specs {
        let name = spec.definition.name.as_str();
        let roles = if spec.root_only {
            "root"
        } else if spec.child_only {
            "analyst / verifier"
        } else if chinese {
            "全部"
        } else {
            "all"
        };
        let description = if chinese {
            let translation = translations
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("Missing tool translation: {name}"))?;
            ensure!(
                translation.source == spec.definition.description,
                "Stale tool translation: {name}"
            );
            ensure!(
                !translation.text.trim().is_empty(),
                "Empty tool translation: {name}"
            );
            &translation.text
        } else {
            &spec.definition.description
        };
        let side_effects = match (chinese, spec.side_effecting) {
            (true, true) => "是",
            (true, false) => "否",
            (false, true) => "yes",
            (false, false) => "no",
        };
        output.push_str(&format!(
            "| `{name}` | {roles} | {side_effects} | {} | {} |\n",
            spec.requirement.label(),
            description.replace('|', "\\|").replace('\n', " ")
        ));
    }
    Ok(output)
}
