//! Desktop configuration editing, validation and atomic persistence.
use crate::{config::AppConfig, store::Store};
use anyhow::{Context, Result, ensure};
use eframe::egui::{self, RichText};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, value};

pub(crate) struct ConfigEditor {
    pub path: PathBuf,
    source: String,
    original: Option<String>,
    baseline: String,
    advanced: bool,
    error: Option<String>,
}

pub(crate) struct EditorOutcome {
    pub open: bool,
    pub applied: Option<(AppConfig, Store)>,
}

impl ConfigEditor {
    pub fn open(path: PathBuf) -> Result<Self> {
        let path = std::path::absolute(path)?;
        let original = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| format!("读取配置 {}", path.display())),
        };
        let source = original
            .clone()
            .unwrap_or(toml::to_string_pretty(&AppConfig::default())?);
        Ok(Self {
            path,
            baseline: source.clone(),
            source,
            original,
            advanced: false,
            error: None,
        })
    }

    fn save(&mut self, state_override: Option<&Path>) -> Result<(AppConfig, Store)> {
        let config: AppConfig = toml::from_str(&self.source).context("TOML 配置格式错误")?;
        config.validate()?;
        let parent = self.path.parent().context("配置目录不存在")?;
        // Load a temporary sibling so relative paths retain the destination file's meaning.
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(self.source.as_bytes())?;
        temp.as_file().sync_all()?;
        let mut resolved = AppConfig::load(Some(temp.path()))?;
        if let Some(path) = state_override {
            resolved.state_dir = path.to_owned();
        }
        let current = match std::fs::read_to_string(&self.path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        ensure!(
            current == self.original,
            "配置文件已被其他程序修改，请重新加载后编辑，避免覆盖外部改动。"
        );
        let store = Store::open(&resolved.state_dir)?;
        temp.persist(&self.path)
            .map_err(|e| e.error)
            .context("保存配置失败，原文件未替换")?;
        self.original = Some(self.source.clone());
        self.baseline = self.source.clone();
        Ok((resolved, store))
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        busy: bool,
        state_override: Option<&Path>,
    ) -> EditorOutcome {
        let mut outcome = EditorOutcome {
            open: true,
            applied: None,
        };
        let dirty = self.source != self.baseline;
        egui::Window::new("工作区设置").open(&mut outcome.open).default_width(740.0).default_height(590.0).resizable(true).collapsible(false).show(ctx, |ui| {
            ui.heading("配置分析工作区");
            ui.label(RichText::new(self.path.display().to_string()).small());
            ui.label("保存后用于新任务。恢复任务沿用它的历史配置。关闭窗口会丢弃未保存修改。");
            if let Some(path) = state_override { ui.label(format!("本次 --state 覆盖生效：{}（不写入文件）", path.display())); }
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.advanced, false, "常用设置");
                ui.selectable_value(&mut self.advanced, true, "完整 TOML");
                if ui.add_enabled(!dirty, egui::Button::new("打开配置…")).on_hover_text("先保存或放弃当前修改，再打开其他配置。").clicked()
                    && let Some(path) = rfd::FileDialog::new().add_filter("TOML", &["toml"]).pick_file()
                {
                    match Self::open(path) { Ok(editor) => *self = editor, Err(e) => self.error = Some(format!("{e:#}")) }
                }
                if ui.button("放弃修改 / 重新加载").clicked() {
                    match Self::open(self.path.clone()) { Ok(editor) => *self = editor, Err(e) => self.error = Some(format!("{e:#}")) }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().id_salt("settings-scroll").max_height((ctx.content_rect().height() - 280.0).clamp(180.0, 440.0)).show(ui, |ui| {
                if self.advanced {
                    ui.label("可编辑全部配置，包括角色模型、MCP、Ghidra 和工具限制。API key 本身不要写入文件。");
                    ui.add(egui::TextEdit::multiline(&mut self.source).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(22).code_editor());
                } else {
                    match (self.source.parse::<DocumentMut>(), toml::from_str::<AppConfig>(&self.source)) {
                        (Ok(mut doc), Ok(config)) => {
                            form(ui, &mut doc, &config);
                            self.source = doc.to_string();
                        }
                        _ => { ui.colored_label(egui::Color32::LIGHT_RED, "配置格式无法解析，请切换“完整 TOML”修正，或重新加载文件。"); }
                    }
                }
            });
            ui.separator();
            if let Some(error) = &self.error { ui.colored_label(egui::Color32::LIGHT_RED, error); }
            if busy { ui.label("分析或导出进行中。可以编辑，等待当前操作完成后再保存应用。"); }
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("保存并应用")).clicked() {
                    match self.save(state_override) {
                        Ok(applied) => { outcome.applied = Some(applied); self.error = None; }
                        Err(e) => self.error = Some(format!("{e:#}")),
                    }
                }
                if ui.button("校验配置").clicked() {
                    let result = toml::from_str::<AppConfig>(&self.source).map_err(anyhow::Error::from).and_then(|config| config.validate());
                    self.error = Some(match result { Ok(()) => "配置校验通过。".into(), Err(e) => format!("{e:#}") });
                }
                if self.source != self.baseline { ui.label("有未保存的修改"); }
            });
        });
        outcome
    }
}

fn set(doc: &mut DocumentMut, section: &str, key: &str, item: toml_edit::Item) {
    if section.is_empty() {
        doc[key] = item;
    } else {
        doc[section][key] = item;
    }
}
fn remove(doc: &mut DocumentMut, section: &str, key: &str) {
    if section.is_empty() {
        doc.remove(key);
    } else if let Some(table) = doc
        .get_mut(section)
        .and_then(toml_edit::Item::as_table_like_mut)
    {
        table.remove(key);
    }
}
fn text(
    ui: &mut egui::Ui,
    doc: &mut DocumentMut,
    section: &str,
    key: &str,
    label: &str,
    current: &str,
    optional: bool,
) {
    let mut input = current.to_owned();
    ui.label(label);
    if ui
        .add(egui::TextEdit::singleline(&mut input).desired_width(f32::INFINITY))
        .changed()
    {
        if optional && input.trim().is_empty() {
            remove(doc, section, key);
        } else {
            set(doc, section, key, value(input));
        }
    }
}
fn number(
    ui: &mut egui::Ui,
    doc: &mut DocumentMut,
    section: &str,
    key: &str,
    label: &str,
    current: u64,
) {
    let mut input = current;
    ui.horizontal(|ui| {
        ui.label(label);
        if ui
            .add(
                egui::DragValue::new(&mut input)
                    .range(1..=i64::MAX as u64)
                    .speed(1.0),
            )
            .changed()
        {
            set(doc, section, key, value(input as i64));
        }
    });
}
fn choice(
    ui: &mut egui::Ui,
    doc: &mut DocumentMut,
    section: &str,
    key: &str,
    label: &str,
    current: &str,
    choices: &[(&str, &str)],
) {
    let mut selected = current.to_owned();
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt((section, key))
            .selected_text(
                choices
                    .iter()
                    .find(|(key, _)| *key == current)
                    .map_or(current, |(_, label)| *label),
            )
            .show_ui(ui, |ui| {
                for (key, label) in choices {
                    ui.selectable_value(&mut selected, (*key).to_owned(), *label);
                }
            });
    });
    if selected != current {
        set(doc, section, key, value(selected));
    }
}
fn form(ui: &mut egui::Ui, doc: &mut DocumentMut, config: &AppConfig) {
    egui::CollapsingHeader::new("模型与连接")
        .default_open(true)
        .show(ui, |ui| {
            let provider = serde_json::to_value(config.model.provider).unwrap();
            choice(
                ui,
                doc,
                "model",
                "provider",
                "接口类型",
                provider.as_str().unwrap(),
                &[
                    ("openai", "OpenAI 兼容 / Chat Completions"),
                    ("openai_responses", "OpenAI Responses"),
                    ("anthropic", "Anthropic"),
                    ("ollama", "Ollama"),
                ],
            );
            text(
                ui,
                doc,
                "model",
                "model",
                "主模型",
                &config.model.model,
                false,
            );
            text(
                ui,
                doc,
                "model",
                "base_url",
                "API 地址（留空使用默认）",
                config.model.base_url.as_deref().unwrap_or(""),
                true,
            );
            text(
                ui,
                doc,
                "model",
                "api_key_env",
                "API key 环境变量名（不是密钥内容）",
                config.model.api_key_env.as_deref().unwrap_or(""),
                true,
            );
            let key = config
                .model
                .api_key_env
                .as_deref()
                .unwrap_or(match config.model.provider {
                    crate::llm::Provider::Anthropic => "ANTHROPIC_API_KEY",
                    crate::llm::Provider::Ollama => "OLLAMA_API_KEY",
                    _ => "OPENAI_API_KEY",
                });
            ui.small(format!(
                "{}：{}",
                key,
                if crate::credentials::lookup(key).is_some() {
                    "已检测到凭据"
                } else {
                    "未检测到；请在 Windows 用户环境变量中设置"
                }
            ));
            text(
                ui,
                doc,
                "model",
                "role_model",
                "子 Agent 默认模型（留空继承主模型）",
                config.model.role_model.as_deref().unwrap_or(""),
                true,
            );
            if config
                .model
                .role_models
                .as_ref()
                .is_some_and(|m| !m.is_empty())
            {
                ui.small("文件存在单独的角色模型覆盖，优先于上面的默认值；可在完整 TOML 中编辑。");
            }
            number(
                ui,
                doc,
                "model",
                "max_tokens",
                "单次输出 Token 上限",
                config.model.max_tokens,
            );
            number(
                ui,
                doc,
                "model",
                "request_timeout_secs",
                "请求超时 / 秒",
                config.model.request_timeout_secs,
            );
        });
    egui::CollapsingHeader::new("预算与并发")
        .default_open(true)
        .show(ui, |ui| {
            number(
                ui,
                doc,
                "budget",
                "max_total_tokens",
                "任务总 Token 预算",
                config.budget.max_total_tokens,
            );
            number(
                ui,
                doc,
                "budget",
                "max_agents",
                "Agent 总数上限",
                config.budget.max_agents as u64,
            );
            number(
                ui,
                doc,
                "budget",
                "max_concurrent_agents",
                "同时运行 Agent 数",
                config.budget.max_concurrent_agents as u64,
            );
            number(
                ui,
                doc,
                "budget",
                "max_turns_per_agent",
                "每个 Agent 最大轮数",
                config.budget.max_turns_per_agent as u64,
            );
            number(
                ui,
                doc,
                "budget",
                "max_wall_seconds",
                "任务时限 / 秒",
                config.budget.max_wall_seconds,
            );
            let mut enabled = config.budget.max_usd.is_some();
            if ui.checkbox(&mut enabled, "限制任务费用（USD）").changed() {
                if enabled {
                    set(doc, "budget", "max_usd", value(10.0));
                } else {
                    remove(doc, "budget", "max_usd");
                }
            }
            for (section, key, label, current) in [
                (
                    "budget",
                    "max_usd",
                    "费用上限",
                    config.budget.max_usd.unwrap_or(10.0),
                ),
                (
                    "pricing",
                    "input_usd_per_million",
                    "输入价格 / 百万 Token",
                    config.pricing.input_usd_per_million,
                ),
                (
                    "pricing",
                    "output_usd_per_million",
                    "输出价格 / 百万 Token",
                    config.pricing.output_usd_per_million,
                ),
            ] {
                if key == "max_usd" && !enabled {
                    continue;
                }
                let mut input = current;
                ui.horizontal(|ui| {
                    ui.label(label);
                    if ui
                        .add(
                            egui::DragValue::new(&mut input)
                                .range(0.0..=1_000_000.0)
                                .speed(0.1),
                        )
                        .changed()
                    {
                        set(doc, section, key, value(input));
                    }
                });
            }
        });
    egui::CollapsingHeader::new("运行后端")
        .default_open(true)
        .show(ui, |ui| {
            let backend = serde_json::to_value(&config.runtime.backend).unwrap();
            choice(
                ui,
                doc,
                "runtime",
                "backend",
                "后端",
                backend.as_str().unwrap(),
                &[
                    ("disabled", "禁用执行"),
                    ("docker", "Docker"),
                    ("ssh", "SSH"),
                    ("trusted_local", "可信本地样本"),
                ],
            );
            match config.runtime.backend {
                crate::runtime::RuntimeBackend::Docker => {
                    text(
                        ui,
                        doc,
                        "runtime",
                        "docker_container",
                        "容器名称",
                        config.runtime.docker_container.as_deref().unwrap_or(""),
                        true,
                    );
                    text(
                        ui,
                        doc,
                        "runtime",
                        "docker_executable",
                        "Docker 程序",
                        &config.runtime.docker_executable,
                        false,
                    );
                }
                crate::runtime::RuntimeBackend::Ssh => text(
                    ui,
                    doc,
                    "runtime",
                    "ssh_host",
                    "SSH 主机（user@host）",
                    config.runtime.ssh_host.as_deref().unwrap_or(""),
                    true,
                ),
                crate::runtime::RuntimeBackend::TrustedLocal => {
                    ui.label("本地后端直接运行样本，仅用于可信测试文件。");
                    text(
                        ui,
                        doc,
                        "runtime",
                        "python",
                        "Python 程序",
                        &config.runtime.python,
                        false,
                    );
                    text(
                        ui,
                        doc,
                        "runtime",
                        "bridge_path",
                        "Worker bridge 路径",
                        &config.runtime.bridge_path.to_string_lossy(),
                        false,
                    );
                    text(
                        ui,
                        doc,
                        "runtime",
                        "state_dir",
                        "Worker 状态目录",
                        &config.runtime.state_dir.to_string_lossy(),
                        false,
                    );
                }
                _ => {}
            }
            if matches!(
                config.runtime.backend,
                crate::runtime::RuntimeBackend::Docker | crate::runtime::RuntimeBackend::Ssh
            ) {
                text(
                    ui,
                    doc,
                    "runtime",
                    "remote_bridge_path",
                    "远端 bridge 路径",
                    &config.runtime.remote_bridge_path,
                    false,
                );
                text(
                    ui,
                    doc,
                    "runtime",
                    "remote_state_dir",
                    "远端状态目录",
                    &config.runtime.remote_state_dir,
                    false,
                );
            }
            number(
                ui,
                doc,
                "runtime",
                "request_timeout_secs",
                "Worker 超时 / 秒（不能小于分析超时）",
                config.runtime.request_timeout_secs,
            );
        });
    egui::CollapsingHeader::new("存储与静态分析").show(ui, |ui| {
        text(
            ui,
            doc,
            "",
            "state_dir",
            "任务状态目录（相对配置文件）",
            &config.state_dir.to_string_lossy(),
            false,
        );
        text(
            ui,
            doc,
            "analysis",
            "ghidra_executable",
            "Ghidra 可执行文件（可选）",
            &config
                .analysis
                .ghidra_executable
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            true,
        );
        number(
            ui,
            doc,
            "analysis",
            "analysis_timeout_seconds",
            "分析超时 / 秒",
            config.analysis.analysis_timeout_seconds,
        );
        number(
            ui,
            doc,
            "analysis",
            "timeout_seconds",
            "分析调用总超时 / 秒",
            config.analysis.timeout_seconds,
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_forms_and_raw_editor_render_without_changing_the_file() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("settings.toml");
        let source = "# unchanged\nstate_dir = 'state'\n[model]\nmodel = 'fixture'\n";
        std::fs::write(&path, source)?;
        let mut editor = ConfigEditor::open(path.clone())?;
        let ctx = egui::Context::default();
        for advanced in [false, true] {
            editor.advanced = advanced;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(960.0, 640.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    let outcome = editor.show(ctx, true, None);
                    assert!(outcome.applied.is_none());
                },
            );
            assert!(!output.shapes.is_empty());
        }
        assert_eq!(editor.source, source);
        assert_eq!(std::fs::read_to_string(path)?, source);
        Ok(())
    }

    #[test]
    fn saves_comments_unknown_fields_and_relative_paths() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("settings.toml");
        let original = "# Keep this comment\nstate_dir = 'state'\ncustom_note = 'keep'\n[model]\nmodel = 'before'\n";
        std::fs::write(&path, original)?;
        let mut editor = ConfigEditor::open(path.clone())?;
        let mut doc = editor.source.parse::<DocumentMut>()?;
        set(&mut doc, "model", "model", value("after"));
        editor.source = doc.to_string();
        let (config, store) = editor.save(None)?;
        assert_eq!(config.model.model, "after");
        assert_eq!(
            store.root(),
            std::fs::canonicalize(dir.path().join("state"))?
        );
        let saved = std::fs::read_to_string(path)?;
        assert!(saved.contains("# Keep this comment"));
        assert!(saved.contains("custom_note = 'keep'"));
        assert!(saved.contains("state_dir = 'state'"));
        Ok(())
    }
    #[test]
    fn invalid_and_external_edits_do_not_overwrite_file() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "state_dir = 'state'\n")?;
        let mut editor = ConfigEditor::open(path.clone())?;
        editor.source = "[budget]\nmax_total_tokens = 0".into();
        assert!(editor.save(None).is_err());
        assert_eq!(std::fs::read_to_string(&path)?, "state_dir = 'state'\n");
        editor.source = "state_dir = 'state'\n".into();
        std::fs::write(&path, "# external edit\n")?;
        assert!(editor.save(None).is_err());
        assert_eq!(std::fs::read_to_string(path)?, "# external edit\n");
        Ok(())
    }
    #[test]
    fn state_override_is_not_persisted() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "state_dir = 'configured'\n")?;
        let mut editor = ConfigEditor::open(path.clone())?;
        let override_path = dir.path().join("override");
        let (config, store) = editor.save(Some(&override_path))?;
        assert_eq!(config.state_dir, override_path);
        assert_eq!(store.root(), std::fs::canonicalize(&override_path)?);
        assert_eq!(std::fs::read_to_string(path)?, "state_dir = 'configured'\n");
        Ok(())
    }
}
