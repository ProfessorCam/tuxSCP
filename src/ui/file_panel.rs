//! Generic file panel used for both local and remote panes.

use crate::models::FileEntry;
use egui::{Color32, RichText, ScrollArea, Sense, Ui};
// RichText is still used in the column header buttons below
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Name,
    Size,
    Modified,
    Permissions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

/// Payload carried during a panel-to-panel drag operation.
#[derive(Clone, Debug)]
pub struct DragPayload {
    /// true if dragged from the remote panel, false if from the local panel.
    pub from_remote: bool,
    /// The entries being dragged.
    pub entries: Vec<FileEntry>,
    /// The directory path of the source panel at drag time.
    pub source_path: String,
}

pub struct FilePanel {
    pub title: String,
    pub path: String,
    pub entries: Vec<FileEntry>,
    pub selected: HashSet<usize>,
    pub show_hidden: bool,
    pub sort_col: SortColumn,
    pub sort_order: SortOrder,
    path_edit: String,
    path_editing: bool,
    pub path_committed: Option<String>,
    pub double_clicked: Option<usize>, // index into entries
    pub context_menu_entry: Option<usize>,
    /// Set after a panel-to-panel drop lands on this panel.
    pub dropped_payload: Option<DragPayload>,
    /// Set when the OS drops files onto this panel (desktop drag-in).
    pub dropped_files: Vec<PathBuf>,
}

impl FilePanel {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            path: String::from("/"),
            entries: Vec::new(),
            selected: HashSet::new(),
            show_hidden: false,
            sort_col: SortColumn::Name,
            sort_order: SortOrder::Asc,
            path_edit: String::new(),
            path_editing: false,
            path_committed: None,
            double_clicked: None,
            context_menu_entry: None,
            dropped_payload: None,
            dropped_files: Vec::new(),
        }
    }

    pub fn set_path(&mut self, path: impl Into<String>) {
        self.path = path.into();
        self.selected.clear();
    }

    pub fn set_entries(&mut self, entries: Vec<FileEntry>) {
        self.entries = entries;
        self.selected.clear();
    }

    pub fn sorted_visible_indices(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                if e.name == ".." { return true; }
                if !self.show_hidden && e.is_hidden() { return false; }
                true
            })
            .collect();

        // ".." always first
        let dotdot_pos = indices.iter().position(|&i| self.entries[i].name == "..");
        if let Some(pos) = dotdot_pos {
            let idx = indices.remove(pos);
            indices.insert(0, idx);
        }

        // Split dirs / files
        let (mut dir_indices, mut file_indices): (Vec<_>, Vec<_>) = indices
            .into_iter()
            .filter(|&i| self.entries[i].name != "..")
            .partition(|&i| self.entries[i].is_dir);

        let cmp = |a: &usize, b: &usize| {
            let ea = &self.entries[*a];
            let eb = &self.entries[*b];
            let ord = match self.sort_col {
                SortColumn::Name => ea.name.to_lowercase().cmp(&eb.name.to_lowercase()),
                SortColumn::Size => ea.size.cmp(&eb.size),
                SortColumn::Modified => ea.modified.cmp(&eb.modified),
                SortColumn::Permissions => ea.permissions.cmp(&eb.permissions),
            };
            if self.sort_order == SortOrder::Desc { ord.reverse() } else { ord }
        };

        dir_indices.sort_by(cmp);
        file_indices.sort_by(cmp);

        // Find the ".." entry index (may be at any position)
        let dotdot_idx = self.entries.iter().position(|e| e.name == "..");
        let mut result = Vec::new();
        if let Some(di) = dotdot_idx {
            result.push(di);
        }
        result.extend(dir_indices);
        result.extend(file_indices);
        result
    }

    pub fn selected_entries(&self) -> Vec<&FileEntry> {
        self.selected.iter().filter_map(|&i| self.entries.get(i)).collect()
    }

    /// `is_remote` distinguishes which side this panel represents, so the drag
    /// payload can carry that information for the receiving panel.
    pub fn show(&mut self, ui: &mut Ui, enabled: bool, is_remote: bool) {
        self.double_clicked = None;
        self.path_committed = None;
        self.context_menu_entry = None;
        self.dropped_payload = None;
        self.dropped_files.clear();

        // ── OS-level file drops (desktop → app window) ────────────────────────
        // Only collect them; the caller decides what to do with them.
        let os_drops = ui.input(|i| i.raw.dropped_files.clone());
        for f in os_drops {
            if let Some(p) = f.path {
                self.dropped_files.push(p);
            }
        }

        // Capture the available rect now — used later as the drop-zone boundary.
        let panel_rect = ui.max_rect();

        // Path bar
        ui.horizontal(|ui| {
            ui.label(format!("{}:", self.title));
            if self.path_editing {
                let resp = ui.add_sized(
                    [ui.available_width() - 60.0, 22.0],
                    egui::TextEdit::singleline(&mut self.path_edit),
                );
                if resp.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.path_committed = Some(self.path_edit.clone());
                    self.path_editing = false;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.path_editing = false;
                }
            } else {
                let path_btn = ui.add(
                    egui::Button::new(
                        RichText::new(&self.path).monospace(),
                    )
                    .frame(false),
                );
                if path_btn.clicked() {
                    self.path_edit = self.path.clone();
                    self.path_editing = true;
                }
            }
        });

        ui.separator();

        // Column headers
        let col_widths = [0.45f32, 0.15, 0.25, 0.15]; // proportions
        let avail = ui.available_width();
        let widths = col_widths.map(|f| f * avail);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;

            let headers = [
                ("Name", SortColumn::Name),
                ("Size", SortColumn::Size),
                ("Modified", SortColumn::Modified),
                ("Perms", SortColumn::Permissions),
            ];

            for (i, (label, col)) in headers.iter().enumerate() {
                let is_sorted = self.sort_col == *col;
                let arrow = if is_sorted {
                    if self.sort_order == SortOrder::Asc { " ▲" } else { " ▼" }
                } else {
                    ""
                };
                let text = format!("{label}{arrow}");
                let header_btn = ui.add_sized(
                    [widths[i], 22.0],
                    egui::Button::new(RichText::new(text).strong()).frame(true),
                );
                if header_btn.clicked() {
                    if self.sort_col == *col {
                        self.sort_order = if self.sort_order == SortOrder::Asc {
                            SortOrder::Desc
                        } else {
                            SortOrder::Asc
                        };
                    } else {
                        self.sort_col = *col;
                        self.sort_order = SortOrder::Asc;
                    }
                }
            }
        });

        ui.separator();

        let visible = self.sorted_visible_indices();

        // Resolve fonts once, outside the scroll loop
        let font_body = egui::TextStyle::Body.resolve(ui.style());
        let font_mono = egui::TextStyle::Monospace.resolve(ui.style());
        let font_small = egui::FontId::new(11.0, font_mono.family.clone());

        // Check whether a cross-panel drag is currently in flight.
        let active_payload = egui::DragAndDrop::payload::<DragPayload>(ui.ctx());
        let foreign_drag_active = active_payload
            .as_ref()
            .map(|p| p.from_remote != is_remote)
            .unwrap_or(false);

        ScrollArea::vertical()
            .id_salt(format!("panel_{}", self.title))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for &idx in &visible {
                    let entry = &self.entries[idx];
                    let is_selected = self.selected.contains(&idx);
                    let is_dotdot = entry.name == "..";

                    let row_h = 22.0;
                    let avail_w = ui.available_width();

                    // ── Allocate the row rect FIRST so we get a reliable Response ──
                    // Use click_and_drag when the panel is interactive so rows can
                    // be used as drag sources.
                    let row_sense = if enabled { Sense::click_and_drag() } else { Sense::click() };
                    let (row_rect, row_resp) = ui.allocate_exact_size(
                        egui::vec2(avail_w, row_h),
                        row_sense,
                    );

                    if !ui.is_rect_visible(row_rect) { continue; }

                    // ── Background (painted before content so it's underneath) ──
                    {
                        let p = ui.painter();
                        if is_selected {
                            p.rect_filled(row_rect, 0.0, Color32::from_rgb(30, 80, 160));
                        } else if row_resp.hovered() && enabled {
                            p.rect_filled(row_rect, 0.0, Color32::from_gray(50));
                        } else if !enabled {
                            p.rect_filled(row_rect, 0.0, Color32::from_gray(35));
                        }
                    }

                    // ── Row content rendered with the painter ──
                    {
                        let widths = col_widths.map(|f| f * avail_w);
                        let cy = row_rect.center().y;
                        let p = ui.painter();

                        // Dim the row while it is being dragged
                        let is_being_dragged = is_selected
                            && egui::DragAndDrop::payload::<DragPayload>(ui.ctx())
                                .as_ref()
                                .map(|pl| pl.from_remote == is_remote)
                                .unwrap_or(false);

                        let name_color = if is_being_dragged {
                            Color32::from_rgba_premultiplied(100, 180, 255, 120)
                        } else if entry.is_dir {
                            Color32::from_rgb(100, 180, 255)
                        } else if entry.is_symlink {
                            Color32::from_rgb(200, 150, 255)
                        } else if !enabled {
                            Color32::from_gray(120)
                        } else {
                            Color32::from_gray(220)
                        };

                        // Col 0 — icon + name (clipped to column width)
                        let name_str = format!("{} {}", entry.icon(), entry.name);
                        let col0_clip = egui::Rect::from_min_size(
                            row_rect.min,
                            egui::vec2(widths[0] - 4.0, row_h),
                        );
                        p.with_clip_rect(col0_clip).text(
                            egui::pos2(row_rect.left() + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            name_str,
                            font_body.clone(),
                            name_color,
                        );

                        // Col 1 — size
                        let size_text = if is_dotdot { String::new() } else { entry.size_display() };
                        p.text(
                            egui::pos2(row_rect.left() + widths[0] + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            size_text,
                            font_mono.clone(),
                            Color32::from_gray(180),
                        );

                        // Col 2 — modified date
                        p.text(
                            egui::pos2(row_rect.left() + widths[0] + widths[1] + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            entry.modified_display(),
                            font_mono.clone(),
                            Color32::from_gray(160),
                        );

                        // Col 3 — permissions
                        p.text(
                            egui::pos2(row_rect.left() + widths[0] + widths[1] + widths[2] + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            entry.permissions_display(),
                            font_small.clone(),
                            Color32::from_gray(140),
                        );
                    }

                    // ── Click / double-click interactions ──
                    if row_resp.clicked() {
                        if !enabled { continue; }
                        if is_dotdot {
                            // Single click on ".." = go up (back button behaviour)
                            self.double_clicked = Some(idx);
                        } else if ui.input(|i| i.modifiers.ctrl) {
                            if self.selected.contains(&idx) {
                                self.selected.remove(&idx);
                            } else {
                                self.selected.insert(idx);
                            }
                        } else if ui.input(|i| i.modifiers.shift) {
                            if let Some(&last) = self.selected.iter().last() {
                                let (lo, hi) = if last < idx { (last, idx) } else { (idx, last) };
                                for i in lo..=hi {
                                    if visible.contains(&i) {
                                        self.selected.insert(i);
                                    }
                                }
                            } else {
                                self.selected.insert(idx);
                            }
                        } else {
                            self.selected.clear();
                            self.selected.insert(idx);
                        }
                    }

                    if row_resp.double_clicked() && enabled && !is_dotdot {
                        self.double_clicked = Some(idx);
                    }

                    // ── Drag source: initiate a panel-to-panel drag ───────────
                    if row_resp.drag_started() && enabled && !is_dotdot {
                        // If the dragged row isn't already selected, select it
                        // exclusively so the user can drag un-selected rows.
                        if !self.selected.contains(&idx) {
                            self.selected.clear();
                            self.selected.insert(idx);
                        }
                        let dragged: Vec<FileEntry> = self
                            .selected_entries()
                            .into_iter()
                            .filter(|e| e.name != "..")
                            .cloned()
                            .collect();
                        if !dragged.is_empty() {
                            egui::DragAndDrop::set_payload(
                                ui.ctx(),
                                DragPayload {
                                    from_remote: is_remote,
                                    entries: dragged,
                                    source_path: self.path.clone(),
                                },
                            );
                        }
                    }

                    row_resp.context_menu(|ui| {
                        self.context_menu_entry = Some(idx);
                        ui.close_menu();
                    });
                }

                // Click on empty area below entries to deselect
                let remaining = ui.available_rect_before_wrap();
                if ui.interact(remaining, ui.id().with("empty_area"), Sense::click()).clicked() {
                    self.selected.clear();
                }
            });

        // ── Drop zone: accept drops from the other panel ──────────────────────
        if enabled && foreign_drag_active {
            let is_hovered = ui.input(|i| {
                i.pointer
                    .hover_pos()
                    .map(|p| panel_rect.contains(p))
                    .unwrap_or(false)
            });

            if is_hovered {
                // Draw a green highlight border over the panel.
                ui.painter().rect_stroke(
                    panel_rect.shrink(2.0),
                    4.0,
                    egui::Stroke::new(2.5, Color32::from_rgb(50, 200, 80)),
                );

                // On pointer release, consume the payload.
                if ui.input(|i| i.pointer.any_released()) {
                    if let Some(payload) = egui::DragAndDrop::payload::<DragPayload>(ui.ctx()) {
                        self.dropped_payload = Some((*payload).clone());
                        egui::DragAndDrop::clear_payload(ui.ctx());
                    }
                }
            }
        }

        // ── Floating drag preview label ────────────────────────────────────────
        // Show the name(s) being dragged near the cursor while a same-side drag
        // is in progress (i.e. dragged from this panel).
        if let Some(payload) = egui::DragAndDrop::payload::<DragPayload>(ui.ctx()) {
            if payload.from_remote == is_remote {
                if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                    let label = if payload.entries.len() == 1 {
                        payload.entries[0].name.clone()
                    } else {
                        format!("{} items", payload.entries.len())
                    };
                    let offset = egui::vec2(14.0, -8.0);
                    ui.painter().text(
                        pos + offset,
                        egui::Align2::LEFT_TOP,
                        label,
                        egui::FontId::proportional(13.0),
                        Color32::from_rgba_premultiplied(220, 220, 220, 200),
                    );
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
            }
        }
    }
}
