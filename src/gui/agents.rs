//! Parent-linked task tree and agent-scoped reading/composer state.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Active,
    Attention,
    Finished,
}
impl Filter {
    fn matches(self, agent: &AgentSummary, query: &str) -> bool {
        let status = match self {
            Self::All => true,
            Self::Active => !agent.status.terminal(),
            Self::Attention => matches!(
                agent.status,
                Status::Failed | Status::Crashed | Status::BudgetPaused
            ),
            Self::Finished => agent.status.terminal(),
        };
        status
            && (query.is_empty()
                || format!("{} {} {}", agent.name, agent.role, agent.task)
                    .to_lowercase()
                    .contains(query))
    }
}
#[derive(Default)]
pub(super) struct AgentTree {
    collapsed: BTreeSet<String>,
    filter: Filter,
    query: String,
}
struct TreeRow {
    index: usize,
    depth: usize,
    children: bool,
}
fn rows(
    agents: &[AgentSummary],
    collapsed: &BTreeSet<String>,
    filter: Filter,
    query: &str,
) -> Vec<TreeRow> {
    let ids: BTreeMap<_, _> = agents
        .iter()
        .enumerate()
        .map(|(i, a)| (a.id.as_str(), i))
        .collect();
    let mut children: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut roots = Vec::new();
    for (i, a) in agents.iter().enumerate() {
        if let Some(parent) = a
            .parent_id
            .as_deref()
            .and_then(|id| ids.get(id))
            .filter(|parent| **parent != i)
        {
            children.entry(*parent).or_default().push(i);
        } else {
            roots.push(i);
        }
    }
    let mut included: BTreeSet<_> = agents
        .iter()
        .enumerate()
        .filter(|(_, a)| filter.matches(a, query))
        .map(|(i, _)| i)
        .collect();
    // Preserve ancestors of matches, even when their own status/text does not match.
    for index in included.clone() {
        let mut next = index;
        let mut seen = BTreeSet::new();
        while seen.insert(next) {
            let Some(parent) = agents[next]
                .parent_id
                .as_deref()
                .and_then(|id| ids.get(id))
                .copied()
            else {
                break;
            };
            included.insert(parent);
            next = parent;
        }
    }
    struct Walk<'a> {
        agents: &'a [AgentSummary],
        children: &'a BTreeMap<usize, Vec<usize>>,
        included: &'a BTreeSet<usize>,
        collapsed: &'a BTreeSet<String>,
        seen: BTreeSet<usize>,
        rows: Vec<TreeRow>,
        searching: bool,
    }
    impl Walk<'_> {
        fn visit(&mut self, index: usize, depth: usize) {
            if !self.seen.insert(index) || !self.included.contains(&index) {
                return;
            }
            let children = self.children.get(&index).cloned().unwrap_or_default();
            self.rows.push(TreeRow {
                index,
                depth,
                children: !children.is_empty(),
            });
            if self.searching || !self.collapsed.contains(&self.agents[index].id) {
                for child in children {
                    self.visit(child, depth + 1);
                }
            }
        }
    }
    let mut walk = Walk {
        agents,
        children: &children,
        included: &included,
        collapsed,
        seen: BTreeSet::new(),
        rows: vec![],
        searching: filter != Filter::All || !query.is_empty(),
    };
    for index in roots {
        walk.visit(index, 0);
    }
    // Malformed cycles remain inspectable, without recursion loops. Collapsed descendants stay hidden.
    for index in 0..agents.len() {
        let mut current = index;
        let mut path = BTreeSet::new();
        while path.insert(current) {
            let Some(parent) = agents[current]
                .parent_id
                .as_deref()
                .and_then(|id| ids.get(id))
                .copied()
            else {
                break;
            };
            current = parent;
        }
        if path.contains(&current)
            && agents[current].parent_id.is_some()
            && !walk.seen.contains(&current)
        {
            walk.visit(current, 0);
        }
    }
    walk.rows
}
impl AgentTree {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        agents: &[AgentSummary],
        selected: Option<&str>,
    ) -> Option<String> {
        ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("搜索 Agent / 任务"));
        ui.horizontal_wrapped(|ui| {
            for (filter, label) in [
                (Filter::All, "全部"),
                (Filter::Active, "进行中"),
                (Filter::Attention, "需关注"),
                (Filter::Finished, "已结束"),
            ] {
                ui.selectable_value(&mut self.filter, filter, label);
            }
        });
        let visible = rows(
            agents,
            &self.collapsed,
            self.filter,
            &self.query.to_lowercase(),
        );
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .id_salt("agent-tree")
            .show(ui, |ui| {
                if visible.is_empty() {
                    ui.label("没有匹配的 Agent");
                }
                for row in visible {
                    let agent = &agents[row.index];
                    ui.push_id(&agent.id, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_space((row.depth.min(12) * 12) as f32);
                            if row.children {
                                if ui
                                    .small_button(if self.collapsed.contains(&agent.id) {
                                        "▶"
                                    } else {
                                        "▼"
                                    })
                                    .clicked()
                                {
                                    if !self.collapsed.remove(&agent.id) {
                                        self.collapsed.insert(agent.id.clone());
                                    }
                                }
                            } else {
                                ui.add_space(22.0);
                            }
                            if ui
                                .selectable_label(
                                    selected == Some(agent.id.as_str()),
                                    format!(
                                        "{}\n{} · {}",
                                        agent.name,
                                        agent.role,
                                        status_label(agent.status)
                                    ),
                                )
                                .clicked()
                            {
                                chosen = Some(agent.id.clone());
                            }
                        });
                    });
                }
            });
        chosen
    }
}
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ContextKey {
    pub root: PathBuf,
    pub run: String,
    pub agent: String,
}
#[derive(Default)]
pub(super) struct AgentContext {
    pub composer: Composer,
    pub chat: ChatView,
}
impl Workspace {
    pub(super) fn bind_recipient(&mut self) {
        let key = self
            .selection
            .selected
            .as_ref()
            .and_then(|id| self.data.runs.iter().find(|run| &run.id == id))
            .map(|run| ContextKey {
                root: self.store.root().to_owned(),
                run: run.id.clone(),
                agent: self
                    .selection
                    .agent
                    .clone()
                    .unwrap_or_else(|| run.root_id.clone()),
            });
        if key == self.context_key {
            return;
        }
        if let Some(previous) = self.context_key.take() {
            self.agent_contexts.insert(
                previous,
                AgentContext {
                    composer: std::mem::take(&mut self.composer),
                    chat: std::mem::take(&mut self.chat),
                },
            );
        } else {
            self.composer = Composer::default();
            self.chat = ChatView::default();
        }
        if let Some(key) = &key {
            if let Some(mut context) = self.agent_contexts.remove(key) {
                context.chat.chat_jump = false;
                self.composer = context.composer;
                self.chat = context.chat;
            }
        }
        self.context_key = key;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn siblings_stay_with_parents_and_filter_keeps_ancestors() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let coordinator = crate::coordinator::Coordinator::new(store.clone());
        let run = coordinator.create_run(
            &std::env::current_exe()?,
            "fixture",
            Budget::default(),
            serde_json::json!({}),
        )?;
        let root = crate::queries::AssessmentQueries::new(&store)
            .status(&run.id, 1)?
            .agents
            .remove(0);
        let mut a = root.clone();
        a.id = "a".into();
        a.parent_id = Some(root.id.clone());
        let mut b = a.clone();
        b.id = "b".into();
        let mut child = a.clone();
        child.id = "child".into();
        child.name = "needle".into();
        child.parent_id = Some(a.id.clone());
        let agents = vec![root, a, b, child];
        let order = rows(&agents, &BTreeSet::new(), Filter::All, "")
            .into_iter()
            .map(|row| row.index)
            .collect::<Vec<_>>();
        assert_eq!(order, vec![0, 1, 3, 2]);
        let filtered = rows(&agents, &BTreeSet::new(), Filter::All, "needle");
        assert_eq!(
            filtered.iter().map(|row| row.index).collect::<Vec<_>>(),
            vec![0, 1, 3]
        );
        let collapsed = BTreeSet::from([agents[1].id.clone()]);
        assert_eq!(
            rows(&agents, &collapsed, Filter::All, "")
                .iter()
                .map(|row| row.index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        Ok(())
    }
}
