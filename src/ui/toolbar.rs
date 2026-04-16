use egui::{Button, Color32, RichText, Ui};

/// Returns true if the button was clicked.
fn toolbar_btn(ui: &mut Ui, icon: &str, tooltip: &str, enabled: bool) -> bool {
    let btn = ui
        .add_enabled(
            enabled,
            Button::new(RichText::new(icon).size(18.0)).min_size([36.0, 32.0].into()),
        )
        .on_hover_text(tooltip);
    btn.clicked()
}

pub struct ToolbarActions {
    pub connect_clicked: bool,
    pub disconnect_clicked: bool,
    pub refresh_clicked: bool,
    pub upload_clicked: bool,
    pub download_clicked: bool,
    pub delete_remote_clicked: bool,
    pub mkdir_remote_clicked: bool,
    pub rename_remote_clicked: bool,
    pub show_hidden_toggled: bool,
}

pub fn show(
    ui: &mut Ui,
    connected: bool,
    show_hidden: bool,
    has_remote_selection: bool,
    has_local_selection: bool,
) -> ToolbarActions {
    let mut actions = ToolbarActions {
        connect_clicked: false,
        disconnect_clicked: false,
        refresh_clicked: false,
        upload_clicked: false,
        download_clicked: false,
        delete_remote_clicked: false,
        mkdir_remote_clicked: false,
        rename_remote_clicked: false,
        show_hidden_toggled: false,
    };

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;

        // Always enabled — opens the connect dialog (creates a new tab if already connected)
        actions.connect_clicked = toolbar_btn(ui, "🔌", "New Connection…", true);
        actions.disconnect_clicked = toolbar_btn(ui, "⏏", "Disconnect", connected);

        ui.separator();

        actions.refresh_clicked = toolbar_btn(ui, "🔄", "Refresh (F5)", connected);

        ui.separator();

        actions.upload_clicked =
            toolbar_btn(ui, "⬆", "Upload selected file(s) to remote", connected && has_local_selection);
        actions.download_clicked =
            toolbar_btn(ui, "⬇", "Download selected file(s) to local", connected && has_remote_selection);

        ui.separator();

        actions.mkdir_remote_clicked =
            toolbar_btn(ui, "📁+", "New remote directory", connected);
        actions.delete_remote_clicked =
            toolbar_btn(ui, "🗑", "Delete selected remote file(s)", connected && has_remote_selection);
        actions.rename_remote_clicked =
            toolbar_btn(ui, "✏", "Rename selected remote item", connected && has_remote_selection);

        ui.separator();

        let hidden_icon = if show_hidden { "👁" } else { "🙈" };
        if toolbar_btn(ui, hidden_icon, "Toggle hidden files", true) {
            actions.show_hidden_toggled = true;
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if connected {
                ui.colored_label(Color32::from_rgb(80, 200, 80), "● Connected");
            } else {
                ui.colored_label(Color32::from_rgb(200, 80, 80), "● Disconnected");
            }
        });
    });

    actions
}
