//! Variable-height list: retained measurements and viewport-only layout.
use super::*;
#[derive(Default)]
pub(super) struct VirtualList {
    revision: u64,
    keys: Vec<u64>,
    width: f32,
    zoom: f32,
    heights: Vec<f32>,
    offsets: Vec<f32>,
}
impl VirtualList {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        revision: u64,
        keys: &[u64],
        mut row: impl FnMut(&mut egui::Ui, usize),
    ) -> usize {
        let count = keys.len();
        if self.revision != revision
            || self.heights.len() != count
            || (self.width - ui.available_width()).abs() > 1.0
            || self.zoom != ui.ctx().zoom_factor()
        {
            let resize = (self.width - ui.available_width()).abs() > 1.0
                || self.zoom != ui.ctx().zoom_factor();
            let previous: std::collections::HashMap<_, _> = self
                .keys
                .iter()
                .copied()
                .zip(self.heights.iter().copied())
                .collect();
            self.revision = revision;
            self.width = ui.available_width();
            self.zoom = ui.ctx().zoom_factor();
            self.heights = keys
                .iter()
                .map(|key| {
                    if resize {
                        120.0
                    } else {
                        previous.get(key).copied().unwrap_or(120.0)
                    }
                })
                .collect();
            self.keys = keys.to_vec();
            self.rebuild();
        }
        let origin = ui.next_widget_position().y;
        let low = (ui.clip_rect().min.y - origin - 200.0).max(0.0);
        let high = (ui.clip_rect().max.y - origin + 200.0).max(0.0);
        let start = self
            .offsets
            .partition_point(|y| *y < low)
            .saturating_sub(1)
            .min(count);
        let end = self
            .offsets
            .partition_point(|y| *y <= high)
            .min(count)
            .max(start);
        ui.add_space(self.offsets[start]);
        let mut changed = false;
        for i in start..end {
            let before = ui.next_widget_position().y;
            row(ui, i);
            let height = (ui.next_widget_position().y - before).max(1.0);
            if (self.heights[i] - height).abs() > 0.5 {
                self.heights[i] = height;
                changed = true;
            }
        }
        ui.add_space(self.offsets[count] - self.offsets[end]);
        if changed {
            self.rebuild();
            ui.ctx().request_repaint();
        }
        end - start
    }
    fn rebuild(&mut self) {
        self.offsets = Vec::with_capacity(self.heights.len() + 1);
        self.offsets.push(0.0);
        for height in &self.heights {
            self.offsets
                .push(self.offsets.last().copied().unwrap_or(0.0) + height);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ten_thousand_rows_only_render_visible_content() {
        let context = egui::Context::default();
        let mut layout = VirtualList::default();
        let mut drawn = 0;
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        drawn = layout.show(ui, 1, &(0..10_000).collect::<Vec<_>>(), |ui, i| {
                            ui.label(format!("row {i}"));
                            ui.add_space(100.0);
                        });
                    });
                });
            },
        );
        assert!(drawn > 0 && drawn < 20, "laid out {drawn} rows");
    }
}
