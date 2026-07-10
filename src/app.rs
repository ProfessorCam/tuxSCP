use crate::models::{ConnectionParams, FileEntry, Transfer, TransferDirection, TransferStatus};
use crate::ui::{
    connect_dialog::ConnectDialog,
    file_panel::FilePanel,
    session_manager::SessionManager,
    transfer_panel,
    toolbar,
};
use crate::worker::{WorkerCmd, WorkerEvent, WorkerHandle};
use egui::{Color32, Context, TopBottomPanel, CentralPanel, RichText};
use std::path::PathBuf;

/// A transfer the user asked for, before it is dispatched to the worker. Kept
/// as an intermediate so a batch can be checked for overwrite conflicts first.
#[derive(Clone)]
struct Planned {
    direction: TransferDirection,
    local_path: PathBuf,
    remote_path: String,
    size: u64,
    is_dir: bool,
    /// True if a file/dir already exists at the destination.
    conflict: bool,
}

/// Shown when a queued batch would overwrite existing files.
struct OverwritePrompt {
    planned: Vec<Planned>,
}

// ── Per-tab dialogs ───────────────────────────────────────────────────────────

struct RenameDialog {
    old_path: String,
    old_name: String,
    new_name: String,
}

struct MkdirDialog {
    parent: String,
    name: String,
}

struct DeleteConfirm {
    paths: Vec<String>,
    names: Vec<String>,
    is_dirs: Vec<bool>,
}

struct ChmodDialog {
    path: String,
    mode_str: String,
}

struct SaveSessionPrompt {
    name: String,
    params: ConnectionParams,
}

// ── ConnectionTab — one SFTP session ─────────────────────────────────────────

struct ConnectionTab {
    id: usize,
    /// Display name shown on the tab strip
    label: String,
    worker: WorkerHandle,
    connected: bool,
    connecting: bool,
    connection_info: String,
    last_params: Option<ConnectionParams>,
    pending_save_prompt: Option<ConnectionParams>,
    local_panel: FilePanel,
    remote_panel: FilePanel,
    transfers: Vec<Transfer>,
    status_message: String,
    // Per-tab transient dialogs
    rename_dialog: Option<RenameDialog>,
    mkdir_dialog: Option<MkdirDialog>,
    delete_confirm: Option<DeleteConfirm>,
    chmod_dialog: Option<ChmodDialog>,
    overwrite_prompt: Option<OverwritePrompt>,
}

impl ConnectionTab {
    fn new(id: usize) -> Self {
        let local_path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let mut local_panel = FilePanel::new("Local");
        local_panel.set_path(local_path.to_string_lossy().to_string());
        refresh_local(&mut local_panel);

        let mut remote_panel = FilePanel::new("Remote");
        remote_panel.set_path("/");

        Self {
            id,
            label: String::from("New Connection"),
            worker: WorkerHandle::spawn(),
            connected: false,
            connecting: false,
            connection_info: String::new(),
            last_params: None,
            pending_save_prompt: None,
            local_panel,
            remote_panel,
            transfers: Vec::new(),
            status_message: String::from("Not connected."),
            rename_dialog: None,
            mkdir_dialog: None,
            delete_confirm: None,
            chmod_dialog: None,
            overwrite_prompt: None,
        }
    }

    fn start_connect(&mut self, params: &ConnectionParams) {
        self.last_params = Some(params.clone());
        self.connecting = true;
        self.label = format!("{}@{}", params.username, params.host);
        self.status_message = format!("Connecting to {}…", params.host);
        self.worker.send(WorkerCmd::Connect(params.clone()));
    }

    /// Drain and apply all pending worker events.
    /// Returns true if something changed that requires a UI repaint.
    fn process_events(&mut self, connect_dialog: &mut ConnectDialog) -> bool {
        let events = self.worker.drain_events();
        let changed = !events.is_empty();
        for event in events {
            match event {
                WorkerEvent::Connected { host, username, home_dir, listing } => {
                    self.connected = true;
                    self.connecting = false;
                    self.connection_info = format!("{username}@{host}");
                    self.label = format!("{username}@{host}");
                    self.status_message = format!("Connected — {home_dir}");
                    self.remote_panel.set_path(home_dir);
                    self.remote_panel.set_entries(listing);
                    // Close the connect dialog and clear any previous error
                    connect_dialog.open = false;
                    connect_dialog.connecting = false;
                    connect_dialog.error = None;
                    // Prompt user to save session (unless we already have these params saved)
                    self.pending_save_prompt = self.last_params.clone();
                }

                WorkerEvent::ConnectionFailed(e) => {
                    self.connected = false;
                    self.connecting = false;
                    self.label = String::from("New Connection");
                    self.status_message = format!("Connection failed: {e}");
                    connect_dialog.connecting = false;
                    connect_dialog.error = Some(format!("Connection failed: {e}"));
                }

                WorkerEvent::Disconnected => {
                    self.connected = false;
                    self.connecting = false;
                    self.connection_info.clear();
                    self.label = String::from("New Connection");
                    self.status_message = String::from("Disconnected.");
                    self.remote_panel.set_entries(Vec::new());
                    self.remote_panel.set_path("/");
                }

                WorkerEvent::DirListing { path, entries } => {
                    self.remote_panel.set_path(path.clone());
                    self.remote_panel.set_entries(entries);
                    self.status_message = format!(
                        "{} — {} items",
                        path,
                        self.remote_panel.entries.len()
                    );
                }

                WorkerEvent::DirError { path, error } => {
                    self.status_message = format!("Cannot list {path}: {error}");
                }

                WorkerEvent::TransferProgress { id, transferred, total, speed_bps } => {
                    if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                        t.transferred_bytes = transferred;
                        t.total_bytes = total;
                        t.speed_bps = speed_bps;
                        t.status = TransferStatus::InProgress;
                    }
                }

                WorkerEvent::TransferComplete { id } => {
                    if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                        t.status = TransferStatus::Completed;
                        t.transferred_bytes = t.total_bytes;
                        t.speed_bps = 0.0;
                        self.status_message = format!("Transfer complete: {}", t.filename);
                        match t.direction {
                            crate::models::TransferDirection::Download => {
                                refresh_local(&mut self.local_panel);
                            }
                            crate::models::TransferDirection::Upload => {
                                let path = self.remote_panel.path.clone();
                                self.worker.send(WorkerCmd::ListDir(path));
                            }
                        }
                    }
                }

                WorkerEvent::TransferFailed { id, error } => {
                    if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                        // A user-cancelled transfer surfaces as an error from the
                        // copy loop — show it as Cancelled, not Failed.
                        if error.to_lowercase().contains("cancel") {
                            t.status = TransferStatus::Cancelled;
                            self.status_message = format!("Transfer cancelled: {}", t.filename);
                        } else {
                            t.status = TransferStatus::Failed;
                            t.error = Some(error.clone());
                            self.status_message = format!("Transfer failed: {error}");
                        }
                    }
                }

                WorkerEvent::OperationComplete { op } => {
                    self.status_message = op;
                    let path = self.remote_panel.path.clone();
                    self.worker.send(WorkerCmd::ListDir(path));
                }

                WorkerEvent::OperationFailed { op, error } => {
                    self.status_message = format!("{op} failed: {error}");
                }
            }
        }
        changed
    }

    fn navigate_remote(&mut self, entry_idx: usize) {
        let entry = &self.remote_panel.entries[entry_idx];
        let new_path = if entry.name == ".." {
            parent_path(&self.remote_panel.path)
        } else {
            join_remote(&self.remote_panel.path, &entry.name)
        };
        self.worker.send(WorkerCmd::ListDir(new_path.clone()));
        self.status_message = format!("Loading {new_path}…");
    }

    fn navigate_local(&mut self, entry_idx: usize) {
        let entry = self.local_panel.entries[entry_idx].clone();
        let current = PathBuf::from(&self.local_panel.path);
        let new_path = if entry.name == ".." {
            current.parent().unwrap_or(&current).to_path_buf()
        } else {
            current.join(&entry.name)
        };
        self.local_panel.set_path(new_path.to_string_lossy().to_string());
        refresh_local(&mut self.local_panel);
    }

    // ── Transfer planning ─────────────────────────────────────────────────────
    // All transfers funnel through plan_* → enqueue → dispatch so directories
    // are handled recursively and overwrite conflicts are caught up front.

    /// Build a download plan for a single remote entry (file or directory).
    fn plan_download(&self, name: &str, size: u64, is_dir: bool) -> Planned {
        let remote_path = join_remote(&self.remote_panel.path, name);
        let local_path = PathBuf::from(&self.local_panel.path).join(name);
        let conflict = local_path.exists();
        Planned { direction: TransferDirection::Download, local_path, remote_path, size, is_dir, conflict }
    }

    /// Build a download plan where the remote path/name come from a drag payload.
    fn plan_download_from(&self, source_path: &str, name: &str, size: u64, is_dir: bool) -> Planned {
        let remote_path = join_remote(source_path, name);
        let local_path = PathBuf::from(&self.local_panel.path).join(name);
        let conflict = local_path.exists();
        Planned { direction: TransferDirection::Download, local_path, remote_path, size, is_dir, conflict }
    }

    /// Build an upload plan for a local path landing in the current remote dir.
    fn plan_upload(&self, local_path: PathBuf, is_dir: bool) -> Planned {
        let name = local_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let remote_path = join_remote(&self.remote_panel.path, &name);
        let size = if is_dir { 0 } else { std::fs::metadata(&local_path).map(|m| m.len()).unwrap_or(0) };
        // Upload conflict = a remote entry with the same name already exists.
        let conflict = self.remote_panel.entries.iter().any(|e| e.name == name);
        Planned { direction: TransferDirection::Upload, local_path, remote_path, size, is_dir, conflict }
    }

    /// Take a batch of planned transfers: dispatch immediately if nothing would
    /// be overwritten, otherwise raise the overwrite-confirmation dialog.
    fn enqueue(&mut self, planned: Vec<Planned>) {
        if planned.is_empty() {
            return;
        }
        if planned.iter().any(|p| p.conflict) {
            self.overwrite_prompt = Some(OverwritePrompt { planned });
        } else {
            for p in planned {
                self.dispatch(p);
            }
        }
    }

    /// Create the Transfer record and send the command to the worker.
    fn dispatch(&mut self, p: Planned) {
        match p.direction {
            TransferDirection::Download => {
                let mut t = Transfer::new_download(p.remote_path.clone(), p.local_path.clone(), p.size);
                t.status = TransferStatus::InProgress;
                let id = t.id.clone();
                self.transfers.push(t);
                self.worker.send(WorkerCmd::Download {
                    transfer_id: id,
                    remote_path: p.remote_path,
                    local_path: p.local_path,
                    is_dir: p.is_dir,
                });
            }
            TransferDirection::Upload => {
                let mut t = Transfer::new_upload(p.local_path.clone(), p.remote_path.clone());
                t.status = TransferStatus::InProgress;
                let id = t.id.clone();
                self.transfers.push(t);
                self.worker.send(WorkerCmd::Upload {
                    transfer_id: id,
                    local_path: p.local_path,
                    remote_path: p.remote_path,
                    is_dir: p.is_dir,
                });
            }
        }
    }

    fn download_selected(&mut self) {
        let planned: Vec<Planned> = self
            .remote_panel
            .selected_entries()
            .into_iter()
            .filter(|e| e.name != "..")
            .map(|e| (e.name.clone(), e.size, e.is_dir))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(name, size, is_dir)| self.plan_download(&name, size, is_dir))
            .collect();
        self.enqueue(planned);
    }

    fn upload_selected(&mut self) {
        let local_dir = PathBuf::from(&self.local_panel.path);
        let planned: Vec<Planned> = self
            .local_panel
            .selected_entries()
            .into_iter()
            .filter(|e| e.name != "..")
            .map(|e| (e.name.clone(), e.is_dir))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(name, is_dir)| self.plan_upload(local_dir.join(&name), is_dir))
            .collect();
        self.enqueue(planned);
    }

    /// Handle a context-menu action chosen on the *remote* panel.
    fn handle_remote_action(&mut self, idx: usize, action: crate::ui::file_panel::RowAction) {
        use crate::ui::file_panel::RowAction;
        if idx >= self.remote_panel.entries.len() {
            return;
        }
        let entry = self.remote_panel.entries[idx].clone();
        if entry.name == ".." {
            return;
        }
        let remote_path = join_remote(&self.remote_panel.path, &entry.name);
        match action {
            RowAction::Download => {
                let planned = self.plan_download(&entry.name, entry.size, entry.is_dir);
                self.enqueue(vec![planned]);
            }
            RowAction::Rename => {
                self.rename_dialog = Some(RenameDialog {
                    old_path: remote_path,
                    old_name: entry.name.clone(),
                    new_name: entry.name.clone(),
                });
            }
            RowAction::Delete => {
                self.delete_confirm = Some(DeleteConfirm {
                    paths: vec![remote_path],
                    names: vec![entry.name.clone()],
                    is_dirs: vec![entry.is_dir],
                });
            }
            RowAction::NewFolder => {
                let parent = self.remote_panel.path.clone();
                self.mkdir_dialog = Some(MkdirDialog { parent, name: String::new() });
            }
            RowAction::Chmod => {
                let default = if entry.is_dir { "755" } else { "644" };
                self.chmod_dialog = Some(ChmodDialog {
                    path: remote_path,
                    mode_str: entry
                        .permissions
                        .map(|p| format!("{:o}", p & 0o777))
                        .unwrap_or_else(|| default.into()),
                });
            }
            RowAction::Upload => {} // upload is a local-panel action only
        }
    }

    fn has_active_transfers(&self) -> bool {
        self.transfers.iter().any(|t| {
            matches!(t.status, TransferStatus::InProgress | TransferStatus::Queued)
        })
    }
}

// ── Main application ──────────────────────────────────────────────────────────

pub struct LinuxScpApp {
    tabs: Vec<ConnectionTab>,
    active_tab: usize,
    next_tab_id: usize,
    tabs_to_close: Vec<usize>, // indices queued for removal at end of frame

    // Shared dialogs
    connect_dialog: ConnectDialog,
    session_manager: SessionManager,
    save_session_dialog: Option<SaveSessionPrompt>,

    // App-level view state
    show_hidden: bool,
    show_transfer_panel: bool,
    show_about: bool,
    dark_mode: bool,
    palette: theme::Palette,
}

/// Office-inspired colour system with light and dark variants.
pub mod theme {
    use egui::Color32;

    // Status colours that read well on both light and dark backgrounds.
    pub const SUCCESS: Color32 = Color32::from_rgb(0x2E, 0x8B, 0x57); // sea green
    pub const ERROR: Color32 = Color32::from_rgb(0xD0, 0x45, 0x45);

    /// Semantic colours for the hand-painted widgets (file rows, tabs, drag
    /// chips…) that can't just read `egui::Visuals`. Resolved per mode.
    #[derive(Clone, Copy)]
    pub struct Palette {
        pub accent: Color32,
        pub on_accent: Color32,
        pub text: Color32,
        pub text_muted: Color32,
        pub dir: Color32,
        pub symlink: Color32,
        pub row_hover: Color32,
        pub meta_on_selected: Color32,
        pub tab_active: Color32,
        pub tab_inactive: Color32,
    }

    impl Palette {
        pub fn light() -> Self {
            Self {
                accent: Color32::from_rgb(0x2B, 0x57, 0x9A), // Word blue
                on_accent: Color32::WHITE,
                text: Color32::from_rgb(0x24, 0x23, 0x21),
                text_muted: Color32::from_rgb(0x60, 0x5E, 0x5C),
                dir: Color32::from_rgb(0x2B, 0x57, 0x9A),
                symlink: Color32::from_rgb(0x74, 0x3A, 0x8A),
                row_hover: Color32::from_rgb(0xED, 0xEB, 0xE9),
                meta_on_selected: Color32::from_rgb(0xE0, 0xEC, 0xF7),
                tab_active: Color32::WHITE,
                tab_inactive: Color32::from_rgb(0xE1, 0xDF, 0xDD),
            }
        }

        pub fn dark() -> Self {
            Self {
                accent: Color32::from_rgb(0x3D, 0x74, 0xC4), // brighter blue for dark bg
                on_accent: Color32::WHITE,
                text: Color32::from_rgb(0xE7, 0xE7, 0xE7),
                text_muted: Color32::from_rgb(0x9B, 0x9B, 0x9B),
                dir: Color32::from_rgb(0x69, 0xB0, 0xFF),
                symlink: Color32::from_rgb(0xC8, 0x92, 0xF0),
                row_hover: Color32::from_rgb(0x33, 0x37, 0x3D),
                meta_on_selected: Color32::from_rgb(0xDD, 0xE8, 0xF7),
                tab_active: Color32::from_rgb(0x3B, 0x3E, 0x42),
                tab_inactive: Color32::from_rgb(0x2A, 0x2C, 0x2F),
            }
        }

        pub fn for_mode(dark: bool) -> Self {
            if dark { Self::dark() } else { Self::light() }
        }
    }
}

/// Embed Inter (UI) and Adwaita Mono (listings) and make them the primary
/// proportional / monospace fonts, keeping egui's bundled fonts as fallbacks so
/// emoji and icon glyphs still render.
fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".to_owned(),
        FontData::from_static(include_bytes!("../assets/fonts/Inter-Regular.ttf")),
    );
    fonts.font_data.insert(
        "AdwaitaMono".to_owned(),
        FontData::from_static(include_bytes!("../assets/fonts/AdwaitaMono-Regular.ttf")),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Inter".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "AdwaitaMono".to_owned());
    ctx.set_fonts(fonts);
}

/// Apply the light or dark Office-like visual style.
fn apply_theme(ctx: &egui::Context, dark: bool) {
    use egui::{Rounding, Stroke};
    let pal = theme::Palette::for_mode(dark);
    let mut style = (*ctx.style()).clone();
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };

    if dark {
        v.panel_fill = Color32::from_rgb(0x24, 0x26, 0x29);
        v.window_fill = Color32::from_rgb(0x2B, 0x2E, 0x31);
        v.extreme_bg_color = Color32::from_rgb(0x1B, 0x1D, 0x1F);
        v.window_shadow.color = Color32::from_black_alpha(96);
    } else {
        v.panel_fill = Color32::from_rgb(0xF3, 0xF2, 0xF1);
        v.window_fill = Color32::WHITE;
        v.window_shadow.color = Color32::from_black_alpha(40);
    }
    v.window_rounding = Rounding::same(6.0);
    v.hyperlink_color = pal.accent;
    v.override_text_color = Some(pal.text);

    // Selection tint (text fields + selectable widgets).
    let sel = if dark {
        Color32::from_rgb(0x2E, 0x4A, 0x6E)
    } else {
        Color32::from_rgb(0xCC, 0xE0, 0xF5)
    };
    v.selection.bg_fill = sel;
    v.selection.stroke = Stroke::new(1.0, pal.accent);

    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
        &mut v.widgets.noninteractive,
    ] {
        w.rounding = Rounding::same(4.0);
    }
    v.widgets.hovered.bg_fill = pal.row_hover;
    v.widgets.hovered.weak_bg_fill = pal.row_hover;
    v.widgets.active.bg_fill = sel;

    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    ctx.set_style(style);
}

impl LinuxScpApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        // Dark mode is the default.
        let dark_mode = true;
        apply_theme(&cc.egui_ctx, dark_mode);

        let tabs = vec![ConnectionTab::new(0)];

        Self {
            tabs,
            active_tab: 0,
            next_tab_id: 1,
            tabs_to_close: Vec::new(),
            connect_dialog: ConnectDialog::default(),
            session_manager: SessionManager::default(),
            save_session_dialog: None,
            show_hidden: false,
            show_transfer_panel: true,
            show_about: false,
            dark_mode,
            palette: theme::Palette::for_mode(dark_mode),
        }
    }

    fn active(&mut self) -> &mut ConnectionTab {
        &mut self.tabs[self.active_tab]
    }

    /// Start a connection on the active tab, or on a fresh tab if already connected.
    fn initiate_connect(&mut self, params: ConnectionParams) {
        if self.tabs[self.active_tab].connected || self.tabs[self.active_tab].connecting {
            // Open a new tab for the new connection
            let id = self.next_tab_id;
            self.next_tab_id += 1;
            let mut tab = ConnectionTab::new(id);
            tab.start_connect(&params);
            self.tabs.push(tab);
            self.active_tab = self.tabs.len() - 1;
        } else {
            self.tabs[self.active_tab].start_connect(&params);
        }
    }

    fn open_new_tab(&mut self) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(ConnectionTab::new(id));
        self.active_tab = self.tabs.len() - 1;
    }

    fn close_tab(&mut self, index: usize) {
        if self.tabs.len() == 1 {
            // Never close the last tab — just disconnect it
            let tab = &mut self.tabs[0];
            if tab.connected {
                tab.worker.send(WorkerCmd::Disconnect);
            }
            return;
        }
        if self.tabs[index].connected {
            self.tabs[index].worker.send(WorkerCmd::Disconnect);
        }
        // Ask the worker thread to exit cleanly before we drop its handle.
        self.tabs[index].worker.send(WorkerCmd::Quit);
        self.tabs.remove(index);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
    }

    fn show_menu_bar(&mut self, ctx: &Context) {
        TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New Connection…").clicked() {
                        self.connect_dialog.open = true;
                        ui.close_menu();
                    }
                    if ui.button("New Tab").clicked() {
                        self.open_new_tab();
                        ui.close_menu();
                    }
                    if ui.button("Session Manager…").clicked() {
                        self.session_manager.open = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.menu_button("View", |ui| {
                    if ui.checkbox(&mut self.show_hidden, "Show hidden files").changed() {
                        for tab in &mut self.tabs {
                            tab.local_panel.show_hidden = self.show_hidden;
                            tab.remote_panel.show_hidden = self.show_hidden;
                        }
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.checkbox(&mut self.show_transfer_panel, "Transfer queue");
                    ui.separator();
                    if ui.checkbox(&mut self.dark_mode, "Dark mode").changed() {
                        apply_theme(ctx, self.dark_mode);
                        self.palette = theme::Palette::for_mode(self.dark_mode);
                    }
                });

                ui.menu_button("Commands", |ui| {
                    let (connected, has_local_sel, has_remote_sel) = {
                        let tab = &self.tabs[self.active_tab];
                        (
                            tab.connected,
                            !tab.local_panel.selected.is_empty(),
                            !tab.remote_panel.selected.is_empty(),
                        )
                    };

                    if ui.add_enabled(connected, egui::Button::new("Refresh (F5)")).clicked() {
                        let path = self.tabs[self.active_tab].remote_panel.path.clone();
                        self.active().worker.send(WorkerCmd::ListDir(path));
                        ui.close_menu();
                    }
                    if ui.add_enabled(connected && has_local_sel, egui::Button::new("Upload selected")).clicked() {
                        self.active().upload_selected();
                        ui.close_menu();
                    }
                    if ui.add_enabled(connected && has_remote_sel, egui::Button::new("Download selected")).clicked() {
                        self.active().download_selected();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.add_enabled(connected, egui::Button::new("Disconnect")).clicked() {
                        self.active().worker.send(WorkerCmd::Disconnect);
                        ui.close_menu();
                    }
                });

                ui.menu_button("Help", |ui| {
                    if ui.button("About TuxSCP").clicked() {
                        self.show_about = true;
                        ui.close_menu();
                    }
                });
            });
        });
    }

    fn show_tab_bar(&mut self, ctx: &Context) {
        let pal = self.palette;
        TopBottomPanel::top("tab_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 1.0;

                let tab_count = self.tabs.len();
                for i in 0..tab_count {
                    let tab = &self.tabs[i];
                    let is_active = i == self.active_tab;
                    let tab_is_idle = !tab.connected && !tab.connecting;

                    // Prefix icon: spinner while connecting, coloured dot when connected
                    let prefix = if tab.connecting {
                        "⟳ "
                    } else if tab.connected {
                        "● "
                    } else {
                        "○ "
                    };
                    let label_text = format!("{}{}", prefix, tab.label);

                    let tab_color = if tab.connected {
                        theme::SUCCESS
                    } else if tab.connecting {
                        pal.accent
                    } else {
                        pal.text_muted
                    };

                    let rich = if is_active {
                        RichText::new(&label_text).color(tab_color).strong()
                    } else {
                        RichText::new(&label_text).color(tab_color)
                    };

                    let tab_btn = ui.add(
                        egui::Button::new(rich)
                            .fill(if is_active { pal.tab_active } else { pal.tab_inactive })
                            .min_size([10.0, 28.0].into()),
                    );
                    if tab_btn.clicked() {
                        if is_active && tab_is_idle {
                            // Clicking the already-active idle tab opens the connect dialog
                            self.connect_dialog.open = true;
                        }
                        self.active_tab = i;
                    }
                    tab_btn.on_hover_text(if self.tabs[i].connected {
                        format!("Connected: {}", self.tabs[i].connection_info)
                    } else {
                        self.tabs[i].label.clone()
                    });

                    // Close button (✕)
                    let close_btn = ui.add(
                        egui::Button::new(RichText::new("✕").size(11.0).color(pal.text_muted))
                            .fill(Color32::TRANSPARENT)
                            .frame(false)
                            .min_size([18.0, 28.0].into()),
                    );
                    if close_btn.clicked() {
                        self.tabs_to_close.push(i);
                    }

                    ui.add(egui::Separator::default().vertical().spacing(2.0));
                }

                // "+" new tab button
                if ui.add(
                    egui::Button::new(RichText::new("+").size(16.0))
                        .fill(Color32::TRANSPARENT)
                        .min_size([28.0, 26.0].into()),
                ).clicked() {
                    self.open_new_tab();
                }
            });
        });
    }

    fn show_active_tab(&mut self, ctx: &Context) {
        let i = self.active_tab;
        let pal = self.palette;

        // Toolbar
        TopBottomPanel::top("toolbar").show(ctx, |ui| {
            let tab = &self.tabs[i];
            let actions = toolbar::show(
                ui,
                tab.connected,
                self.show_hidden,
                !tab.remote_panel.selected.is_empty(),
                !tab.local_panel.selected.is_empty(),
            );

            if actions.connect_clicked {
                self.connect_dialog.open = true;
            }
            if actions.disconnect_clicked {
                self.tabs[i].worker.send(WorkerCmd::Disconnect);
            }
            if actions.refresh_clicked {
                let path = self.tabs[i].remote_panel.path.clone();
                self.tabs[i].worker.send(WorkerCmd::ListDir(path));
            }
            if actions.upload_clicked {
                self.tabs[i].upload_selected();
            }
            if actions.download_clicked {
                self.tabs[i].download_selected();
            }
            if actions.mkdir_remote_clicked && self.tabs[i].connected {
                let parent = self.tabs[i].remote_panel.path.clone();
                self.tabs[i].mkdir_dialog = Some(MkdirDialog { parent, name: String::new() });
            }
            if actions.delete_remote_clicked {
                let remote_path = self.tabs[i].remote_panel.path.clone();
                let mut paths = Vec::new();
                let mut names = Vec::new();
                let mut is_dirs = Vec::new();
                for e in self.tabs[i].remote_panel.selected_entries().into_iter().filter(|e| e.name != "..") {
                    paths.push(join_remote(&remote_path, &e.name));
                    names.push(e.name.clone());
                    is_dirs.push(e.is_dir);
                }
                if !paths.is_empty() {
                    self.tabs[i].delete_confirm = Some(DeleteConfirm { paths, names, is_dirs });
                }
            }
            if actions.rename_remote_clicked {
                let remote_path = self.tabs[i].remote_panel.path.clone();
                if let Some(entry) = self.tabs[i].remote_panel.selected_entries().into_iter().next() {
                    if entry.name != ".." {
                        let old_path = join_remote(&remote_path, &entry.name);
                        self.tabs[i].rename_dialog = Some(RenameDialog {
                            old_path,
                            old_name: entry.name.clone(),
                            new_name: entry.name.clone(),
                        });
                    }
                }
            }
            if actions.show_hidden_toggled {
                self.show_hidden = !self.show_hidden;
                for tab in &mut self.tabs {
                    tab.local_panel.show_hidden = self.show_hidden;
                    tab.remote_panel.show_hidden = self.show_hidden;
                }
            }
        });

        // Status bar
        TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.tabs[i].status_message);
                if self.tabs[i].connected {
                    ui.separator();
                    ui.colored_label(pal.text_muted, &self.tabs[i].connection_info);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let local_sel = self.tabs[i].local_panel.selected.len();
                    let remote_sel = self.tabs[i].remote_panel.selected.len();
                    if local_sel > 0 {
                        ui.label(format!("Local: {local_sel} selected"));
                        ui.separator();
                    }
                    if remote_sel > 0 {
                        ui.label(format!("Remote: {remote_sel} selected"));
                    }
                });
            });
        });

        // Transfer queue panel
        if self.show_transfer_panel {
            TopBottomPanel::bottom("transfer_panel")
                .resizable(true)
                .min_height(60.0)
                .default_height(160.0)
                .show(ctx, |ui| {
                    let actions = transfer_panel::show(ui, &self.tabs[i].transfers, &pal);
                    if let Some(id) = actions.cancel_id {
                        // Signal the worker out-of-band (the AtomicBool the copy
                        // loop polls) — an in-band command wouldn't be read until
                        // the current transfer finished. Also queue the command so
                        // a not-yet-started transfer is cancelled when it comes up.
                        self.tabs[i].worker.cancel_current_transfer();
                        self.tabs[i].worker.send(WorkerCmd::CancelTransfer(id.clone()));
                        if let Some(t) = self.tabs[i].transfers.iter_mut().find(|t| t.id == id) {
                            t.status = TransferStatus::Cancelled;
                        }
                    }
                    if actions.clear_completed {
                        self.tabs[i].transfers.retain(|t| {
                            matches!(t.status, TransferStatus::InProgress | TransferStatus::Queued)
                        });
                    }
                });
        }

        // Dual-pane content
        let connected = self.tabs[i].connected;
        CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |cols| {
                // ── Local panel ───────────────────────────────────────────────
                {
                    let ui = &mut cols[0];
                    self.tabs[i].local_panel.show_hidden = self.show_hidden;
                    self.tabs[i].local_panel.show(ui, true, false, &pal);

                    if let Some(committed) = self.tabs[i].local_panel.path_committed.take() {
                        let path = PathBuf::from(&committed);
                        if path.is_dir() {
                            self.tabs[i].local_panel.set_path(committed);
                            refresh_local(&mut self.tabs[i].local_panel);
                        }
                    }

                    if let Some(idx) = self.tabs[i].local_panel.double_clicked.take() {
                        if self.tabs[i].local_panel.entries[idx].is_dir {
                            self.tabs[i].navigate_local(idx);
                        } else if connected {
                            // Double-click file → upload
                            let local = PathBuf::from(&self.tabs[i].local_panel.path)
                                .join(&self.tabs[i].local_panel.entries[idx].name);
                            let planned = self.tabs[i].plan_upload(local, false);
                            self.tabs[i].enqueue(vec![planned]);
                        }
                    }

                    // Panel-to-panel drop onto local (remote → local = download)
                    if let Some(payload) = self.tabs[i].local_panel.dropped_payload.take() {
                        let planned: Vec<Planned> = payload
                            .entries
                            .iter()
                            .filter(|e| e.name != "..")
                            .map(|e| {
                                self.tabs[i].plan_download_from(&payload.source_path, &e.name, e.size, e.is_dir)
                            })
                            .collect();
                        self.tabs[i].enqueue(planned);
                    }

                    // Local context-menu action (upload)
                    if let Some((idx, action)) = self.tabs[i].local_panel.requested_action.take() {
                        if connected
                            && action == crate::ui::file_panel::RowAction::Upload
                            && idx < self.tabs[i].local_panel.entries.len()
                        {
                            let entry = self.tabs[i].local_panel.entries[idx].clone();
                            let local = PathBuf::from(&self.tabs[i].local_panel.path).join(&entry.name);
                            let planned = self.tabs[i].plan_upload(local, entry.is_dir);
                            self.tabs[i].enqueue(vec![planned]);
                        }
                    }
                }

                // ── Remote panel ──────────────────────────────────────────────
                {
                    let ui = &mut cols[1];
                    self.tabs[i].remote_panel.show_hidden = self.show_hidden;
                    self.tabs[i].remote_panel.show(ui, connected, true, &pal);

                    if let Some(committed) = self.tabs[i].remote_panel.path_committed.take() {
                        if connected {
                            self.tabs[i].worker.send(WorkerCmd::ListDir(committed));
                        }
                    }

                    if let Some(idx) = self.tabs[i].remote_panel.double_clicked.take() {
                        if self.tabs[i].remote_panel.entries[idx].is_dir {
                            self.tabs[i].navigate_remote(idx);
                        } else {
                            // Double-click file → download
                            let (name, size) = {
                                let e = &self.tabs[i].remote_panel.entries[idx];
                                (e.name.clone(), e.size)
                            };
                            let planned = self.tabs[i].plan_download(&name, size, false);
                            self.tabs[i].enqueue(vec![planned]);
                        }
                    }

                    // Panel-to-panel drop onto remote (local → remote = upload)
                    if connected {
                        if let Some(payload) = self.tabs[i].remote_panel.dropped_payload.take() {
                            let planned: Vec<Planned> = payload
                                .entries
                                .iter()
                                .filter(|e| e.name != "..")
                                .map(|e| {
                                    let local = PathBuf::from(&payload.source_path).join(&e.name);
                                    self.tabs[i].plan_upload(local, e.is_dir)
                                })
                                .collect();
                            self.tabs[i].enqueue(planned);
                        }

                        // OS / desktop drag-in → upload to remote (files and folders)
                        let os_drops = std::mem::take(&mut self.tabs[i].remote_panel.dropped_files);
                        let planned: Vec<Planned> = os_drops
                            .into_iter()
                            .filter_map(|p| {
                                let is_dir = p.is_dir();
                                if !p.is_file() && !is_dir {
                                    return None;
                                }
                                Some(self.tabs[i].plan_upload(p, is_dir))
                            })
                            .collect();
                        self.tabs[i].enqueue(planned);
                    }

                    // Remote context-menu action
                    if let Some((idx, action)) = self.tabs[i].remote_panel.requested_action.take() {
                        if connected && idx < self.tabs[i].remote_panel.entries.len() {
                            self.tabs[i].handle_remote_action(idx, action);
                        }
                    }
                }
            });
        });

        // ── Per-tab modal dialogs ─────────────────────────────────────────────
        let tab_id = self.tabs[i].id;

        // Rename
        if let Some(dialog) = &mut self.tabs[i].rename_dialog {
            let mut close = false;
            let mut do_rename: Option<(String, String)> = None;
            egui::Window::new(format!("Rename##{tab_id}"))
                .collapsible(false).resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("Rename '{}':", dialog.old_name));
                    let resp = ui.text_edit_singleline(&mut dialog.new_name);
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        do_rename = Some((dialog.old_path.clone(), dialog.new_name.clone()));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Rename").clicked() {
                            do_rename = Some((dialog.old_path.clone(), dialog.new_name.clone()));
                        }
                        if ui.button("Cancel").clicked() { close = true; }
                    });
                });
            if let Some((old, new_name)) = do_rename {
                // Strip path separators to prevent directory traversal
                let safe_name: String = new_name.chars().filter(|&c| c != '/' && c != '\0').collect();
                if safe_name.is_empty() || safe_name == ".." { return; }
                let parent = old.rsplit_once('/').map(|(p, _)| p).unwrap_or("/");
                let new_path = format!("{parent}/{safe_name}");
                self.tabs[i].worker.send(WorkerCmd::Rename { from: old, to: new_path });
                close = true;
            }
            if close { self.tabs[i].rename_dialog = None; }
        }

        // Mkdir
        if let Some(dialog) = &mut self.tabs[i].mkdir_dialog {
            let mut close = false;
            let mut do_mkdir: Option<String> = None;
            egui::Window::new(format!("New Directory##{tab_id}"))
                .collapsible(false).resizable(false)
                .show(ctx, |ui| {
                    ui.label("Directory name:");
                    let resp = ui.text_edit_singleline(&mut dialog.name);
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        do_mkdir = Some(join_remote(&dialog.parent, &dialog.name));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked() {
                            do_mkdir = Some(join_remote(&dialog.parent, &dialog.name));
                        }
                        if ui.button("Cancel").clicked() { close = true; }
                    });
                });
            if let Some(path) = do_mkdir {
                if !self.tabs[i].mkdir_dialog.as_ref().map(|d| d.name.is_empty()).unwrap_or(true) {
                    self.tabs[i].worker.send(WorkerCmd::Mkdir(path));
                }
                close = true;
            }
            if close { self.tabs[i].mkdir_dialog = None; }
        }

        // Delete confirm
        if let Some(dialog) = &self.tabs[i].delete_confirm {
            let mut close = false;
            let mut do_delete = false;
            let names = dialog.names.join(", ");
            let count = dialog.paths.len();
            egui::Window::new(format!("Confirm Delete##{tab_id}"))
                .collapsible(false).resizable(false)
                .show(ctx, |ui| {
                    if count == 1 {
                        ui.label(format!("Delete '{names}'?"));
                    } else {
                        ui.label(format!("Delete {count} items?"));
                        ui.label(format!("({names})"));
                    }
                    ui.colored_label(Color32::from_rgb(220, 100, 80), "This cannot be undone.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new("Delete").fill(Color32::from_rgb(180, 40, 40))).clicked() {
                            do_delete = true;
                        }
                        if ui.button("Cancel").clicked() { close = true; }
                    });
                });
            if do_delete {
                let dc = self.tabs[i].delete_confirm.as_ref().unwrap();
                let items: Vec<(String, bool)> =
                    dc.paths.iter().cloned().zip(dc.is_dirs.iter().copied()).collect();
                for (path, is_dir) in items {
                    self.tabs[i].worker.send(WorkerCmd::Delete { path, is_dir });
                }
                close = true;
            }
            if close { self.tabs[i].delete_confirm = None; }
        }

        // chmod
        if let Some(dialog) = &mut self.tabs[i].chmod_dialog {
            let mut close = false;
            let mut do_chmod: Option<(String, String)> = None;
            egui::Window::new(format!("File Permissions##{tab_id}"))
                .collapsible(false).resizable(false)
                .show(ctx, |ui| {
                    ui.label("Octal permissions (e.g. 644, 755):");
                    ui.text_edit_singleline(&mut dialog.mode_str);
                    ui.horizontal(|ui| {
                        if ui.button("Apply").clicked() {
                            do_chmod = Some((dialog.path.clone(), dialog.mode_str.clone()));
                        }
                        if ui.button("Cancel").clicked() { close = true; }
                    });
                });
            if let Some((path, mode_str)) = do_chmod {
                if let Ok(mode) = u32::from_str_radix(mode_str.trim(), 8) {
                    self.tabs[i].worker.send(WorkerCmd::Chmod { path, mode });
                }
                close = true;
            }
            if close { self.tabs[i].chmod_dialog = None; }
        }

        // Overwrite confirmation — shown when a queued batch would clobber files
        if let Some(prompt) = &self.tabs[i].overwrite_prompt {
            let mut decision: Option<OverwriteChoice> = None;
            let conflicts: Vec<String> = prompt
                .planned
                .iter()
                .filter(|p| p.conflict)
                .map(|p| {
                    p.local_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.remote_path.clone())
                })
                .collect();
            let conflict_count = conflicts.len();
            egui::Window::new(format!("Confirm Overwrite##{tab_id}"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!(
                        "{conflict_count} item(s) already exist at the destination:"
                    ));
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        for name in &conflicts {
                            ui.colored_label(Color32::from_rgb(220, 170, 90), format!("• {name}"));
                        }
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new("Overwrite all").fill(Color32::from_rgb(180, 120, 40)))
                            .clicked()
                        {
                            decision = Some(OverwriteChoice::OverwriteAll);
                        }
                        if ui.button("Skip existing").clicked() {
                            decision = Some(OverwriteChoice::SkipExisting);
                        }
                        if ui.button("Cancel").clicked() {
                            decision = Some(OverwriteChoice::Cancel);
                        }
                    });
                });
            if let Some(choice) = decision {
                if let Some(prompt) = self.tabs[i].overwrite_prompt.take() {
                    match choice {
                        OverwriteChoice::OverwriteAll => {
                            for p in prompt.planned {
                                self.tabs[i].dispatch(p);
                            }
                        }
                        OverwriteChoice::SkipExisting => {
                            for p in prompt.planned.into_iter().filter(|p| !p.conflict) {
                                self.tabs[i].dispatch(p);
                            }
                        }
                        OverwriteChoice::Cancel => {}
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum OverwriteChoice {
    OverwriteAll,
    SkipExisting,
    Cancel,
}

impl eframe::App for LinuxScpApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Process events for ALL tabs (transfers continue in background tabs)
        let mut needs_repaint = false;
        for tab in &mut self.tabs {
            if tab.process_events(&mut self.connect_dialog) || tab.has_active_transfers() {
                needs_repaint = true;
            }
        }
        if needs_repaint {
            ctx.request_repaint();
        }

        // Pick up any pending "save session?" prompt from tabs that just connected
        if self.save_session_dialog.is_none() {
            for tab in &mut self.tabs {
                if let Some(params) = tab.pending_save_prompt.take() {
                    self.save_session_dialog = Some(SaveSessionPrompt {
                        name: tab.label.clone(),
                        params,
                    });
                    break;
                }
            }
        }

        // Keyboard shortcuts — ignore them while a text field is focused so
        // typing (e.g. a filename) doesn't trigger Refresh/Delete.
        let editing = ctx.memory(|m| m.focused().is_some());
        let (f5, del) = ctx.input(|i| {
            (i.key_pressed(egui::Key::F5), i.key_pressed(egui::Key::Delete))
        });
        if f5 && !editing {
            let i = self.active_tab;
            if self.tabs.get(i).map(|t| t.connected).unwrap_or(false) {
                let path = self.tabs[i].remote_panel.path.clone();
                self.tabs[i].worker.send(WorkerCmd::ListDir(path));
            }
        }
        if del && !editing {
            let i = self.active_tab;
            let idle = self.tabs.get(i).map(|t| {
                t.connected && t.delete_confirm.is_none() && t.rename_dialog.is_none()
            });
            if idle == Some(true) {
                let remote_path = self.tabs[i].remote_panel.path.clone();
                let mut paths = Vec::new();
                let mut names = Vec::new();
                let mut is_dirs = Vec::new();
                for e in self.tabs[i].remote_panel.selected_entries().into_iter().filter(|e| e.name != "..") {
                    paths.push(join_remote(&remote_path, &e.name));
                    names.push(e.name.clone());
                    is_dirs.push(e.is_dir);
                }
                if !paths.is_empty() {
                    self.tabs[i].delete_confirm = Some(DeleteConfirm { paths, names, is_dirs });
                }
            }
        }

        // Menu → Tab bar → Toolbar → Status → Content (order matters for layout)
        self.show_menu_bar(ctx);
        self.show_tab_bar(ctx);
        self.show_active_tab(ctx);

        // ── Shared dialogs ────────────────────────────────────────────────────

        // Connect dialog
        if let Some(params) = self.connect_dialog.show(ctx) {
            self.initiate_connect(params);
        }

        // Session manager
        if let Some(params) = self.session_manager.show(ctx) {
            // Passwords are never persisted, so a saved password/keyboard session
            // has no secret to connect with — open the connect dialog pre-filled
            // so the user can type it, instead of failing with "auth failed".
            let needs_password = matches!(
                params.auth_method,
                crate::models::AuthMethod::Password | crate::models::AuthMethod::KeyboardInteractive
            ) && params.password.is_empty();
            if needs_password {
                self.connect_dialog.params = params;
                self.connect_dialog.error = Some("Enter your password to connect.".into());
                self.connect_dialog.connecting = false;
                self.connect_dialog.open = true;
            } else {
                self.connect_dialog.open = false;
                self.connect_dialog.connecting = false;
                self.initiate_connect(params);
            }
        }

        // Save session prompt — shown once after a new connection succeeds
        if let Some(prompt) = &mut self.save_session_dialog {
            let mut close = false;
            let mut do_save = false;
            egui::Window::new("Save Session?")
                .collapsible(false)
                .resizable(false)
                .min_width(340.0)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Would you like to save this session for quick access?");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Session name:");
                        ui.text_edit_singleline(&mut prompt.name);
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_sized(
                                [110.0, 28.0],
                                egui::Button::new("Save Session")
                                    .fill(Color32::from_rgb(30, 100, 200)),
                            )
                            .clicked()
                        {
                            do_save = true;
                            close = true;
                        }
                        if ui
                            .add_sized([90.0, 28.0], egui::Button::new("Don't Save"))
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
            if do_save {
                let name = prompt.name.clone();
                let params = prompt.params.clone();
                self.session_manager.remember_connection(name, params);
            }
            if close {
                self.save_session_dialog = None;
            }
        }

        // About dialog
        if self.show_about {
            let mut open = true;
            egui::Window::new("About TuxSCP")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(6.0);
                        ui.heading("TuxSCP");
                        ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                        ui.add_space(6.0);
                        ui.label("A native Linux SSH/SFTP/SCP/FTP file-transfer client.");
                        ui.label("Protocols: SFTP · SCP · FTP · FTPS");
                        ui.add_space(6.0);
                        ui.label("Built with egui and ssh2-rs. MIT licensed.");
                        ui.add_space(10.0);
                        if ui.button("Close").clicked() {
                            self.show_about = false;
                        }
                    });
                });
            if !open {
                self.show_about = false;
            }
        }

        // Process tab closures queued during this frame (in reverse so indices stay valid)
        let closes: Vec<usize> = self.tabs_to_close.drain(..).collect();
        for idx in closes.into_iter().rev() {
            if idx < self.tabs.len() {
                self.close_tab(idx);
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn refresh_local(panel: &mut FilePanel) {
    let path = PathBuf::from(&panel.path);
    match read_local_dir(&path) {
        Ok(entries) => panel.set_entries(entries),
        Err(e) => log::error!("Local dir error: {e}"),
    }
}

fn read_local_dir(path: &PathBuf) -> anyhow::Result<Vec<FileEntry>> {
    use std::os::unix::fs::MetadataExt;

    let has_parent = path.parent().is_some() && path != PathBuf::from("/").as_path();
    let mut entries: Vec<FileEntry> = Vec::new();

    for dir_entry in std::fs::read_dir(path)? {
        let dir_entry = match dir_entry {
            Ok(e) => e,
            Err(_) => continue, // skip entries we can't even read
        };
        let name = dir_entry.file_name().to_string_lossy().to_string();
        let is_symlink = dir_entry
            .file_type()
            .map(|ft| ft.is_symlink())
            .unwrap_or(false);
        // `metadata()` follows symlinks and errors on broken links; fall back to
        // `symlink_metadata` so a single dangling symlink can't blank the whole
        // directory listing. Skip only if both fail.
        let meta = match dir_entry.metadata() {
            Ok(m) => m,
            Err(_) => match std::fs::symlink_metadata(dir_entry.path()) {
                Ok(m) => m,
                Err(_) => continue,
            },
        };
        let link_target = if is_symlink {
            std::fs::read_link(dir_entry.path())
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        } else {
            None
        };
        let modified = meta.modified().ok().map(chrono::DateTime::<chrono::Local>::from);
        entries.push(FileEntry {
            name,
            size: meta.len(),
            modified,
            is_dir: meta.is_dir(),
            is_symlink,
            permissions: Some(meta.mode()),
            owner: None,
            group: None,
            link_target,
        });
    }

    // Sort: dirs first, then alpha (before prepending "..")
    entries.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    // Prepend ".." at index 0 so sorted_visible_indices always finds it at 0
    if has_parent {
        entries.insert(0, FileEntry {
            name: "..".into(),
            size: 0,
            modified: None,
            is_dir: true,
            is_symlink: false,
            permissions: None,
            owner: None,
            group: None,
            link_target: None,
        });
    }

    Ok(entries)
}

fn join_remote(base: &str, name: &str) -> String {
    if base == "/" { format!("/{name}") } else { format!("{base}/{name}") }
}

fn parent_path(path: &str) -> String {
    if let Some(pos) = path.rfind('/') {
        if pos == 0 { "/".to_string() } else { path[..pos].to_string() }
    } else {
        "/".to_string()
    }
}
