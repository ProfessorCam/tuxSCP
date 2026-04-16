use crate::models::{AuthMethod, ConnectionParams, Protocol};
use egui::{ComboBox, Grid, TextEdit, Ui, Window};
use std::path::PathBuf;

pub struct ConnectDialog {
    pub open: bool,
    pub params: ConnectionParams,
    pub key_path_str: String,
    pub error: Option<String>,
    pub connecting: bool,
}

impl Default for ConnectDialog {
    fn default() -> Self {
        Self {
            open: false,
            params: ConnectionParams::default(),
            key_path_str: String::new(),
            error: None,
            connecting: false,
        }
    }
}

impl ConnectDialog {
    /// Returns Some(params) when the user clicks Connect.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<ConnectionParams> {
        let mut result = None;

        let mut open = self.open;
        Window::new("Connect to Server")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .min_width(420.0)
            .show(ctx, |ui| {
                result = self.ui(ui);
            });
        self.open = open;

        result
    }

    fn ui(&mut self, ui: &mut Ui) -> Option<ConnectionParams> {
        let mut connect_clicked = false;

        Grid::new("connect_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                // Protocol
                ui.label("Protocol:");
                ComboBox::from_id_salt("protocol_combo")
                    .selected_text(self.params.protocol.label())
                    .show_ui(ui, |ui| {
                        for p in Protocol::all() {
                            let old_port = self.params.protocol.default_port();
                            if ui
                                .selectable_value(&mut self.params.protocol, *p, p.label())
                                .changed()
                            {
                                // Auto-update port only if it was the default for the old protocol
                                if self.params.port == old_port {
                                    self.params.port = p.default_port();
                                }
                            }
                        }
                    });
                ui.end_row();

                // Host
                ui.label("Host name:");
                ui.text_edit_singleline(&mut self.params.host);
                ui.end_row();

                // Port
                ui.label("Port number:");
                let mut port_str = self.params.port.to_string();
                if ui.text_edit_singleline(&mut port_str).changed() {
                    if let Ok(p) = port_str.parse::<u16>() {
                        self.params.port = p;
                    }
                }
                ui.end_row();

                // Username
                ui.label("User name:");
                ui.text_edit_singleline(&mut self.params.username);
                ui.end_row();

                // Auth method
                ui.label("Auth method:");
                let auth_label = self.params.auth_method.label();
                ComboBox::from_id_salt("auth_combo")
                    .selected_text(auth_label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                matches!(self.params.auth_method, AuthMethod::Password),
                                "Password",
                            )
                            .clicked()
                        {
                            self.params.auth_method = AuthMethod::Password;
                        }
                        if ui
                            .selectable_label(
                                matches!(self.params.auth_method, AuthMethod::PublicKey { .. }),
                                "Public Key",
                            )
                            .clicked()
                        {
                            self.params.auth_method = AuthMethod::PublicKey {
                                key_path: PathBuf::new(),
                            };
                        }
                        if ui
                            .selectable_label(
                                matches!(self.params.auth_method, AuthMethod::Agent),
                                "SSH Agent",
                            )
                            .clicked()
                        {
                            self.params.auth_method = AuthMethod::Agent;
                        }
                        if ui
                            .selectable_label(
                                matches!(
                                    self.params.auth_method,
                                    AuthMethod::KeyboardInteractive
                                ),
                                "Keyboard Interactive",
                            )
                            .clicked()
                        {
                            self.params.auth_method = AuthMethod::KeyboardInteractive;
                        }
                    });
                ui.end_row();

                // Auth-specific fields
                match &self.params.auth_method {
                    AuthMethod::Password | AuthMethod::KeyboardInteractive => {
                        ui.label("Password:");
                        ui.add(TextEdit::singleline(&mut self.params.password).password(true));
                        ui.end_row();
                    }
                    AuthMethod::PublicKey { .. } => {
                        ui.label("Private key:");
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.key_path_str);
                            if ui.button("Browse…").clicked() {
                                if let Some(path) = rfd::FileDialog::new()
                                    .set_title("Select private key")
                                    .pick_file()
                                {
                                    self.key_path_str = path.to_string_lossy().to_string();
                                    self.params.auth_method = AuthMethod::PublicKey {
                                        key_path: path,
                                    };
                                }
                            }
                        });
                        ui.end_row();
                    }
                    AuthMethod::Agent => {
                        ui.label("");
                        ui.label("(using SSH_AUTH_SOCK)");
                        ui.end_row();
                    }
                }

                // Initial remote dir
                ui.label("Remote dir:");
                ui.text_edit_singleline(&mut self.params.initial_remote_dir);
                ui.end_row();
            });

        // Status / error display
        if self.connecting {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("Connecting…").color(egui::Color32::from_rgb(100, 180, 255)));
            });
        } else if let Some(err) = &self.error {
            ui.colored_label(egui::Color32::RED, err);
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            let btn_label = if self.connecting { "Connecting…" } else { "Connect" };
            if ui
                .add_enabled(
                    !self.connecting,
                    egui::Button::new(btn_label).min_size([100.0, 28.0].into()),
                )
                .clicked()
            {
                connect_clicked = true;
            }
            if ui
                .add_sized([80.0, 28.0], egui::Button::new("Cancel"))
                .clicked()
            {
                self.connecting = false;
                self.open = false;
            }
        });

        if connect_clicked {
            if self.params.host.trim().is_empty() {
                self.error = Some("Host name is required".into());
                return None;
            }
            if self.params.username.trim().is_empty() {
                self.error = Some("Username is required".into());
                return None;
            }
            self.error = None;
            self.connecting = true;
            Some(self.params.clone())
        } else {
            None
        }
    }
}
