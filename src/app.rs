use crate::models::{ConnectionParams, FileEntry, Transfer, TransferStatus};
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
}

impl ConnectionTab {
    fn new(id: usize) -> Self {
        let local_path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let mut local_panel = FilePanel::new("Local");
        local_panel.path = local_path.to_string_lossy().to_string();
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
                        t.status = TransferStatus::Failed;
                        t.error = Some(error.clone());
                        self.status_message = format!("Transfer failed: {error}");
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

    fn download_selected(&mut self) {
        let local_dir = PathBuf::from(&self.local_panel.path);
        let entries: Vec<_> = self.remote_panel.selected_entries()
            .into_iter()
            .filter(|e| !e.is_dir && e.name != "..")
            .map(|e| (join_remote(&self.remote_panel.path, &e.name), e.name.clone(), e.size))
            .collect();

        for (remote_path, name, size) in entries {
            let local_path = local_dir.join(&name);
            let mut t = Transfer::new_download(remote_path.clone(), local_path.clone(), size);
            let id = t.id.clone();
            t.status = TransferStatus::InProgress;
            self.transfers.push(t);
            self.worker.send(WorkerCmd::Download { transfer_id: id, remote_path, local_path });
        }
    }

    fn upload_selected(&mut self) {
        let remote_dir = self.remote_panel.path.clone();
        let entries: Vec<_> = self.local_panel.selected_entries()
            .into_iter()
            .filter(|e| !e.is_dir && e.name != "..")
            .map(|e| {
                let local = PathBuf::from(&self.local_panel.path).join(&e.name);
                let remote = join_remote(&remote_dir, &e.name);
                (local, remote)
            })
            .collect();

        for (local_path, remote_path) in entries {
            let mut t = Transfer::new_upload(local_path.clone(), remote_path.clone());
            let id = t.id.clone();
            t.status = TransferStatus::InProgress;
            self.transfers.push(t);
            self.worker.send(WorkerCmd::Upload { transfer_id: id, local_path, remote_path });
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
}

impl LinuxScpApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut tabs = Vec::new();
        tabs.push(ConnectionTab::new(0));

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
                });

                ui.menu_button("Commands", |ui| {
                    let tab = &self.tabs[self.active_tab];
                    let connected = tab.connected;
                    let has_local_sel = !tab.local_panel.selected.is_empty();
                    let has_remote_sel = !tab.remote_panel.selected.is_empty();
                    drop(tab);

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
                        ui.close_menu();
                    }
                });
            });
        });
    }

    fn show_tab_bar(&mut self, ctx: &Context) {
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
                        Color32::from_rgb(80, 200, 80)
                    } else if tab.connecting {
                        Color32::from_rgb(100, 180, 255)
                    } else {
                        Color32::from_gray(180)
                    };

                    let rich = if is_active {
                        RichText::new(&label_text).color(tab_color).strong()
                    } else {
                        RichText::new(&label_text).color(tab_color)
                    };

                    let tab_btn = ui.add(
                        egui::Button::new(rich)
                            .fill(if is_active {
                                Color32::from_gray(55)
                            } else {
                                Color32::from_gray(35)
                            })
                            .min_size([10.0, 26.0].into()),
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
                        egui::Button::new(RichText::new("✕").size(11.0).color(Color32::from_gray(160)))
                            .fill(Color32::TRANSPARENT)
                            .frame(false)
                            .min_size([18.0, 26.0].into()),
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
                let paths: Vec<String> = self.tabs[i].remote_panel.selected_entries()
                    .into_iter().filter(|e| e.name != "..")
                    .map(|e| join_remote(&remote_path, &e.name))
                    .collect();
                let names: Vec<String> = self.tabs[i].remote_panel.selected_entries()
                    .into_iter().filter(|e| e.name != "..")
                    .map(|e| e.name.clone())
                    .collect();
                if !paths.is_empty() {
                    self.tabs[i].delete_confirm = Some(DeleteConfirm { paths, names });
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
                    ui.colored_label(Color32::from_gray(160), &self.tabs[i].connection_info);
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
                    let actions = transfer_panel::show(ui, &self.tabs[i].transfers);
                    if let Some(id) = actions.cancel_id {
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
                    self.tabs[i].local_panel.show(ui, true, false);

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
                            let entry = self.tabs[i].local_panel.entries[idx].clone();
                            let local = PathBuf::from(&self.tabs[i].local_panel.path).join(&entry.name);
                            let remote = join_remote(&self.tabs[i].remote_panel.path, &entry.name);
                            let mut t = Transfer::new_upload(local.clone(), remote.clone());
                            let id = t.id.clone();
                            t.status = TransferStatus::InProgress;
                            self.tabs[i].transfers.push(t);
                            self.tabs[i].worker.send(WorkerCmd::Upload {
                                transfer_id: id, local_path: local, remote_path: remote,
                            });
                        }
                    }

                    // Panel-to-panel drop onto local (remote → local = download)
                    if let Some(payload) = self.tabs[i].local_panel.dropped_payload.take() {
                        let local_dir = PathBuf::from(&self.tabs[i].local_panel.path);
                        for entry in payload.entries.iter().filter(|e| !e.is_dir && e.name != "..") {
                            let remote = join_remote(&payload.source_path, &entry.name);
                            let local = local_dir.join(&entry.name);
                            let mut t = Transfer::new_download(remote.clone(), local.clone(), entry.size);
                            let id = t.id.clone();
                            t.status = TransferStatus::InProgress;
                            self.tabs[i].transfers.push(t);
                            self.tabs[i].worker.send(WorkerCmd::Download {
                                transfer_id: id, remote_path: remote, local_path: local,
                            });
                        }
                    }

                    // Local context menu
                    if let Some(idx) = self.tabs[i].local_panel.context_menu_entry.take() {
                        let entry = self.tabs[i].local_panel.entries[idx].clone();
                        if !entry.is_dir && connected {
                            egui::Window::new(format!("local_ctx_{}", self.tabs[i].id))
                                .fixed_size([180.0, 10.0])
                                .title_bar(false)
                                .show(ui.ctx(), |ui| {
                                    if ui.button("⬆ Upload").clicked() {
                                        let local = PathBuf::from(&self.tabs[i].local_panel.path)
                                            .join(&entry.name);
                                        let remote = join_remote(&self.tabs[i].remote_panel.path, &entry.name);
                                        let mut t = Transfer::new_upload(local.clone(), remote.clone());
                                        let id = t.id.clone();
                                        t.status = TransferStatus::InProgress;
                                        self.tabs[i].transfers.push(t);
                                        self.tabs[i].worker.send(WorkerCmd::Upload {
                                            transfer_id: id, local_path: local, remote_path: remote,
                                        });
                                    }
                                });
                        }
                    }
                }

                // ── Remote panel ──────────────────────────────────────────────
                {
                    let ui = &mut cols[1];
                    self.tabs[i].remote_panel.show_hidden = self.show_hidden;
                    self.tabs[i].remote_panel.show(ui, connected, true);

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
                            let entry = self.tabs[i].remote_panel.entries[idx].clone();
                            let remote = join_remote(&self.tabs[i].remote_panel.path, &entry.name);
                            let local = PathBuf::from(&self.tabs[i].local_panel.path).join(&entry.name);
                            let mut t = Transfer::new_download(remote.clone(), local.clone(), entry.size);
                            let id = t.id.clone();
                            t.status = TransferStatus::InProgress;
                            self.tabs[i].transfers.push(t);
                            self.tabs[i].worker.send(WorkerCmd::Download {
                                transfer_id: id, remote_path: remote, local_path: local,
                            });
                        }
                    }

                    // Panel-to-panel drop onto remote (local → remote = upload)
                    if connected {
                        if let Some(payload) = self.tabs[i].remote_panel.dropped_payload.take() {
                            let remote_dir = self.tabs[i].remote_panel.path.clone();
                            for entry in payload.entries.iter().filter(|e| !e.is_dir && e.name != "..") {
                                let local = PathBuf::from(&payload.source_path).join(&entry.name);
                                let remote = join_remote(&remote_dir, &entry.name);
                                let mut t = Transfer::new_upload(local.clone(), remote.clone());
                                let id = t.id.clone();
                                t.status = TransferStatus::InProgress;
                                self.tabs[i].transfers.push(t);
                                self.tabs[i].worker.send(WorkerCmd::Upload {
                                    transfer_id: id, local_path: local, remote_path: remote,
                                });
                            }
                        }

                        // OS / desktop drag-in → upload to remote
                        let os_drops = std::mem::take(&mut self.tabs[i].remote_panel.dropped_files);
                        for local_path in os_drops {
                            if local_path.is_file() {
                                let name = local_path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                if name.is_empty() { continue; }
                                let remote = join_remote(&self.tabs[i].remote_panel.path, &name);
                                let mut t = Transfer::new_upload(local_path.clone(), remote.clone());
                                let id = t.id.clone();
                                t.status = TransferStatus::InProgress;
                                self.tabs[i].transfers.push(t);
                                self.tabs[i].worker.send(WorkerCmd::Upload {
                                    transfer_id: id, local_path, remote_path: remote,
                                });
                            }
                        }
                    }

                    // Remote context menu
                    if let Some(idx) = self.tabs[i].remote_panel.context_menu_entry.take() {
                        let entry = self.tabs[i].remote_panel.entries[idx].clone();
                        if entry.name != ".." {
                            let remote_path = join_remote(&self.tabs[i].remote_panel.path, &entry.name);
                            egui::Window::new(format!("remote_ctx_{}", self.tabs[i].id))
                                .fixed_size([200.0, 10.0])
                                .title_bar(false)
                                .show(ui.ctx(), |ui| {
                                    if !entry.is_dir {
                                        if ui.button("⬇ Download").clicked() {
                                            let local = PathBuf::from(&self.tabs[i].local_panel.path)
                                                .join(&entry.name);
                                            let mut t = Transfer::new_download(
                                                remote_path.clone(), local.clone(), entry.size,
                                            );
                                            let id = t.id.clone();
                                            t.status = TransferStatus::InProgress;
                                            self.tabs[i].transfers.push(t);
                                            self.tabs[i].worker.send(WorkerCmd::Download {
                                                transfer_id: id,
                                                remote_path: remote_path.clone(),
                                                local_path: local,
                                            });
                                        }
                                    }
                                    if ui.button("✏ Rename").clicked() {
                                        self.tabs[i].rename_dialog = Some(RenameDialog {
                                            old_path: remote_path.clone(),
                                            old_name: entry.name.clone(),
                                            new_name: entry.name.clone(),
                                        });
                                    }
                                    if ui.button("🗑 Delete").clicked() {
                                        self.tabs[i].delete_confirm = Some(DeleteConfirm {
                                            paths: vec![remote_path.clone()],
                                            names: vec![entry.name.clone()],
                                        });
                                    }
                                    ui.separator();
                                    if ui.button("📁 New folder here").clicked() {
                                        let parent = self.tabs[i].remote_panel.path.clone();
                                        self.tabs[i].mkdir_dialog = Some(MkdirDialog {
                                            parent, name: String::new(),
                                        });
                                    }
                                    if ui.button("🔑 Properties / chmod").clicked() {
                                        self.tabs[i].chmod_dialog = Some(ChmodDialog {
                                            path: remote_path.clone(),
                                            mode_str: entry.permissions
                                                .map(|p| format!("{:o}", p & 0o777))
                                                .unwrap_or_else(|| "644".into()),
                                        });
                                    }
                                });
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
                let paths: Vec<_> = self.tabs[i].delete_confirm.as_ref().unwrap().paths.clone();
                for path in paths {
                    self.tabs[i].worker.send(WorkerCmd::Delete(path));
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
    }
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

        // Keyboard shortcuts
        ctx.input(|input| {
            if input.key_pressed(egui::Key::F5) {
                let i = self.active_tab;
                if self.tabs.get(i).map(|t| t.connected).unwrap_or(false) {
                    let path = self.tabs[i].remote_panel.path.clone();
                    self.tabs[i].worker.send(WorkerCmd::ListDir(path));
                }
            }
        });

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
            self.connect_dialog.open = false;
            self.connect_dialog.connecting = false;
            self.initiate_connect(params);
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
        let dir_entry = dir_entry?;
        let meta = dir_entry.metadata()?;
        let name = dir_entry.file_name().to_string_lossy().to_string();
        let is_symlink = dir_entry.file_type()?.is_symlink();
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
            link_target: None,
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
