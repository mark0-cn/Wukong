//! Shared CommonMark presentation with bounded parsed-document caching.
use eframe::egui::{self, RichText};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};
use std::{
    collections::{HashMap, VecDeque},
    hash::{Hash, Hasher},
    sync::Arc,
};
#[derive(Clone, Debug)]
enum Kind {
    Paragraph,
    Heading(u8),
    Code(String),
    List(Option<u64>),
    Item,
    Quote,
    Table,
    Head,
    Row,
    Cell,
    Strong,
    Emphasis,
    Strike,
    Link(String),
    Other,
}
#[derive(Clone, Debug)]
enum Node {
    Group(Kind, Vec<Node>),
    Text(String),
    Code(String),
    Break,
    Rule,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<u64, (String, Arc<Vec<Node>>)>,
    order: VecDeque<u64>,
}
fn parse(text: &str) -> Vec<Node> {
    let mut stack = vec![(Kind::Other, Vec::new())];
    for event in Parser::new_ext(
        text,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    ) {
        match event {
            Event::Start(tag) => {
                let kind = match tag {
                    Tag::Paragraph => Kind::Paragraph,
                    Tag::Heading { level, .. } => Kind::Heading(level as u8),
                    Tag::CodeBlock(kind) => Kind::Code(match kind {
                        CodeBlockKind::Fenced(language) => language.into_string(),
                        _ => String::new(),
                    }),
                    Tag::List(start) => Kind::List(start),
                    Tag::Item => Kind::Item,
                    Tag::BlockQuote(_) => Kind::Quote,
                    Tag::Table(_) => Kind::Table,
                    Tag::TableHead => Kind::Head,
                    Tag::TableRow => Kind::Row,
                    Tag::TableCell => Kind::Cell,
                    Tag::Strong => Kind::Strong,
                    Tag::Emphasis => Kind::Emphasis,
                    Tag::Strikethrough => Kind::Strike,
                    Tag::Link { dest_url, .. } => Kind::Link(dest_url.into_string()),
                    _ => Kind::Other,
                };
                stack.push((kind, vec![]));
            }
            Event::End(_) => {
                if stack.len() > 1 {
                    let (kind, nodes) = stack.pop().unwrap();
                    stack.last_mut().unwrap().1.push(Node::Group(kind, nodes));
                }
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => stack
                .last_mut()
                .unwrap()
                .1
                .push(Node::Text(text.into_string())),
            Event::Code(text) => stack
                .last_mut()
                .unwrap()
                .1
                .push(Node::Code(text.into_string())),
            Event::SoftBreak => stack.last_mut().unwrap().1.push(Node::Text(" ".into())),
            Event::HardBreak => stack.last_mut().unwrap().1.push(Node::Break),
            Event::Rule => stack.last_mut().unwrap().1.push(Node::Rule),
            Event::TaskListMarker(done) => stack
                .last_mut()
                .unwrap()
                .1
                .push(Node::Text(if done { "☑ " } else { "☐ " }.into())),
            _ => {}
        }
    }
    stack.pop().unwrap().1
}
fn plain(nodes: &[Node]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Node::Group(_, nodes) => plain(nodes),
            Node::Text(text) | Node::Code(text) => text.clone(),
            Node::Break => "\n".into(),
            Node::Rule => String::new(),
        })
        .collect()
}
#[derive(Clone, Default)]
struct Style {
    strong: bool,
    italic: bool,
    strike: bool,
    size: Option<f32>,
    link: Option<String>,
}
fn inline(ui: &mut egui::Ui, nodes: &[Node], style: &Style) {
    for node in nodes {
        match node {
            Node::Group(kind, children) => {
                let mut style = style.clone();
                match kind {
                    Kind::Strong => style.strong = true,
                    Kind::Emphasis => style.italic = true,
                    Kind::Strike => style.strike = true,
                    Kind::Link(url) => style.link = Some(url.clone()),
                    _ => {}
                }
                inline(ui, children, &style);
            }
            Node::Text(text) | Node::Code(text) => {
                let mut rich = RichText::new(text);
                if style.strong {
                    rich = rich.strong();
                }
                if style.italic {
                    rich = rich.italics();
                }
                if style.strike {
                    rich = rich.strikethrough();
                }
                if let Some(size) = style.size {
                    rich = rich.size(size);
                }
                if matches!(node, Node::Code(_)) {
                    rich = rich.monospace();
                }
                if let Some(url) = &style.link {
                    if url.starts_with("https://") || url.starts_with("http://") {
                        ui.hyperlink_to(rich, url);
                    } else {
                        ui.add(egui::Label::new(rich).selectable(true))
                            .on_hover_text(url);
                    }
                } else {
                    ui.add(egui::Label::new(rich).wrap().selectable(true));
                }
            }
            Node::Break => {
                ui.end_row();
            }
            Node::Rule => {
                ui.separator();
            }
        }
    }
}
fn paragraph(ui: &mut egui::Ui, nodes: &[Node], style: Style) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        inline(ui, nodes, &style);
    });
}
fn blocks(ui: &mut egui::Ui, nodes: &[Node]) {
    for (index, node) in nodes.iter().enumerate() {
        ui.push_id(index, |ui| match node {
            Node::Group(Kind::Code(language), content) => {
                let text = plain(content);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(if language.is_empty() {
                            "代码"
                        } else {
                            language
                        });
                        if ui.small_button("复制代码").clicked() {
                            ui.ctx().copy_text(text.clone());
                        }
                    });
                    egui::ScrollArea::horizontal()
                        .id_salt("code")
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(text.trim_end_matches('\n')).monospace(),
                                )
                                .selectable(true),
                            );
                        });
                });
            }
            Node::Group(Kind::Heading(level), content) => paragraph(
                ui,
                content,
                Style {
                    strong: true,
                    size: Some((25.0 - *level as f32 * 1.5).max(16.0)),
                    ..Style::default()
                },
            ),
            Node::Group(Kind::List(start), items) => {
                for (i, item) in items.iter().enumerate() {
                    ui.horizontal_top(|ui| {
                        ui.label(
                            start
                                .map(|start| format!("{}.", start + i as u64))
                                .unwrap_or_else(|| "•".into()),
                        );
                        ui.vertical(|ui| {
                            if let Node::Group(_, content) = item {
                                blocks(ui, content);
                            }
                        });
                    });
                }
            }
            Node::Group(Kind::Quote, content) => {
                ui.indent("quote", |ui| {
                    blocks(ui, content);
                });
            }
            Node::Group(Kind::Table, content) => {
                let columns = content
                    .iter()
                    .filter_map(|node| {
                        if let Node::Group(Kind::Head, cells) = node {
                            Some(cells.len())
                        } else {
                            None
                        }
                    })
                    .next()
                    .unwrap_or(1)
                    .max(1);
                let width = (ui.available_width() / columns as f32 - 12.0).max(40.0);
                egui::Grid::new(ui.id().with("table"))
                    .striped(true)
                    .show(ui, |ui| {
                        for row in content {
                            if let Node::Group(kind, cells) = row {
                                for cell in cells {
                                    if let Node::Group(Kind::Cell, content) = cell {
                                        ui.vertical(|ui| {
                                            ui.set_max_width(width);
                                            paragraph(
                                                ui,
                                                content,
                                                Style {
                                                    strong: matches!(kind, Kind::Head),
                                                    ..Style::default()
                                                },
                                            );
                                        });
                                    }
                                }
                                ui.end_row();
                            }
                        }
                    });
            }
            Node::Group(Kind::Paragraph, content) => paragraph(ui, content, Style::default()),
            Node::Group(_, content) => blocks(ui, content),
            Node::Rule => {
                ui.separator();
            }
            _ => paragraph(ui, std::slice::from_ref(node), Style::default()),
        });
    }
}
pub(crate) fn code(ui: &mut egui::Ui, language: &str, text: &str) {
    blocks(
        ui,
        &[Node::Group(
            Kind::Code(language.into()),
            vec![Node::Text(text.into())],
        )],
    );
}
pub(crate) fn show(ui: &mut egui::Ui, text: &str) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let key = hasher.finish();
    let document = ui.ctx().data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<Arc<std::sync::Mutex<Cache>>>(egui::Id::new(
            "markdown-cache",
        ));
        let mut cache = cache.lock().unwrap();
        if let Some((source, document)) = cache.entries.get(&key)
            && source == text
        {
            return document.clone();
        }
        let document = Arc::new(parse(text));
        if cache.entries.len() >= 256
            && let Some(old) = cache.order.pop_front()
        {
            cache.entries.remove(&old);
        }
        cache.order.push_back(key);
        cache
            .entries
            .insert(key, (text.to_owned(), document.clone()));
        document
    });
    blocks(ui, &document);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rich_documents_render_at_narrow_and_wide_widths() {
        let ctx = egui::Context::default();
        for width in [320.0, 900.0] {
            let output=ctx.run(egui::RawInput { screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(width,900.0))),..Default::default() },|ctx| {
                egui::CentralPanel::default().show(ctx,|ui| show(ui,"# Header\n\n- **bold** and *italic* [link](https://example.com)\n\n| Name | Value |\n|---|---|\n| a | `1` |\n\n```rust\nlet example = 1;\n```\n"));
            });
            assert!(!output.shapes.is_empty());
        }
    }
    #[test]
    fn markdown_preserves_code_and_recognizes_tables_links_and_lists() {
        let nodes = parse(
            "# Title\n\n- **item** [link](https://example.com)\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```rust\nlet x = 1;\n```\n",
        );
        assert!(
            nodes
                .iter()
                .any(|node| matches!(node, Node::Group(Kind::Table, _)))
        );
        assert!(
            nodes
                .iter()
                .any(|node| matches!(node, Node::Group(Kind::List(_), _)))
        );
        assert!(nodes.iter().any(|node|matches!(node,Node::Group(Kind::Code(language),content) if language=="rust" && plain(content)=="let x = 1;\n")));
        assert!(format!("{nodes:?}").contains("https://example.com"));
    }
}
