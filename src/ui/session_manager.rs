use crate::models::{ConnectionParams, SavedSession, SessionStore};
use egui::{Button, Color32, Grid, ScrollArea, Ui, Window};

pub struct SessionManager {
    pub open: bool,
    store: SessionStore,
    selected_id: Option<String>,
    edit_name: String,
    edit_params: ConnectionParams,
    is_editing: bool,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            open: false,
            store: SessionStore::load(),
            selected_id: None,
            edit_name: String::new(),
            edit_params: ConnectionParams::default(),
            is_editing: false,
        }
    }
}

impl SessionManager {
    /// Save a new session from a connection that just succeeded.
    pub fn remember_connection(&mut self, name: impl Into<String>, params: ConnectionParams) {
        let session = SavedSession::new(name, params);
        self.store.add_or_update(session);
        let _ = self.store.save();
    }

    /// Returns Some(params) when user double-clicks / presses Connect on a session.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<ConnectionParams> {
        let mut result = None;
        let mut open = self.open;

        Window::new("Session Manager")
            .open(&mut open)
            .resizable(true)
            .min_size([640.0, 400.0])
            .show(ctx, |ui| {
                result = self.ui(ui);
            });

        self.open = open;
        result
    }

    fn ui(&mut self, ui: &mut Ui) -> Option<ConnectionParams> {
        let mut connect_params: Option<ConnectionParams> = None;

        ui.columns(2, |cols| {
            // Left: session list
            cols[0].label("Saved Sessions");
            cols[0].separator();

            ScrollArea::vertical().id_salt("session_list").show(&mut cols[0], |ui| {
                let sessions: Vec<(String, String)> = self
                    .store
                    .sessions
                    .iter()
                    .map(|s| (s.id.clone(), s.name.clone()))
                    .collect();

                for (id, name) in sessions {
                    let selected = self.selected_id.as_deref() == Some(&id);
                    let label = ui.selectable_label(selected, &name);
                    if label.clicked() {
                        if !selected {
                            self.selected_id = Some(id.clone());
                            if let Some(s) =
                                self.store.sessions.iter().find(|s| s.id == id)
                            {
                                self.edit_name = s.name.clone();
                                self.edit_params = s.params.clone();
                            }
                        }
                    }
                    if label.double_clicked() {
                        if let Some(s) = self.store.sessions.iter().find(|s| s.id == id) {
                            connect_params = Some(s.params.clone());
                        }
                    }
                }
            });

            cols[0].separator();
            cols[0].horizontal(|ui| {
                if ui.button("New").clicked() {
                    self.selected_id = None;
                    self.edit_name = String::from("New Session");
                    self.edit_params = ConnectionParams::default();
                    self.is_editing = true;
                }
                if ui
                    .add_enabled(self.selected_id.is_some(), Button::new("Delete"))
                    .clicked()
                {
                    if let Some(id) = &self.selected_id.clone() {
                        self.store.remove(id);
                        let _ = self.store.save();
                        self.selected_id = None;
                    }
                }
            });

            // Right: session editor
            cols[1].label("Session Properties");
            cols[1].separator();

            if self.selected_id.is_some() || self.is_editing {
                Grid::new("session_props")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(&mut cols[1], |ui| {
                        ui.label("Session name:");
                        ui.text_edit_singleline(&mut self.edit_name);
                        ui.end_row();

                        ui.label("Host:");
                        ui.text_edit_singleline(&mut self.edit_params.host);
                        ui.end_row();

                        ui.label("Port:");
                        let mut port_str = self.edit_params.port.to_string();
                        if ui.text_edit_singleline(&mut port_str).changed() {
                            if let Ok(p) = port_str.parse::<u16>() {
                                self.edit_params.port = p;
                            }
                        }
                        ui.end_row();

                        ui.label("Username:");
                        ui.text_edit_singleline(&mut self.edit_params.username);
                        ui.end_row();

                        ui.label("Remote dir:");
                        ui.text_edit_singleline(
                            &mut self.edit_params.initial_remote_dir,
                        );
                        ui.end_row();
                    });

                cols[1].separator();
                cols[1].horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        let session = if let Some(id) = &self.selected_id {
                            let mut s = SavedSession::new(
                                self.edit_name.clone(),
                                self.edit_params.clone(),
                            );
                            s.id = id.clone();
                            s
                        } else {
                            SavedSession::new(
                                self.edit_name.clone(),
                                self.edit_params.clone(),
                            )
                        };
                        self.selected_id = Some(session.id.clone());
                        self.store.add_or_update(session);
                        let _ = self.store.save();
                        self.is_editing = false;
                    }

                    if ui
                        .add_sized(
                            [100.0, 24.0],
                            Button::new("Connect").fill(Color32::from_rgb(30, 100, 200)),
                        )
                        .clicked()
                    {
                        connect_params = Some(self.edit_params.clone());
                    }
                });
            } else {
                cols[1].label("Select a session to edit or click New.");
            }
        });

        connect_params
    }
}
