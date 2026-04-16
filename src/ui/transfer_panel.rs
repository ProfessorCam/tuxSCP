use crate::models::{Transfer, TransferDirection, TransferStatus};
use egui::{Color32, ProgressBar, RichText, ScrollArea, Ui};

pub struct TransferActions {
    pub cancel_id: Option<String>,
    pub clear_completed: bool,
}

pub fn show(ui: &mut Ui, transfers: &[Transfer]) -> TransferActions {
    let mut actions = TransferActions { cancel_id: None, clear_completed: false };

    ui.horizontal(|ui| {
        ui.label(RichText::new("Transfer Queue").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Clear completed").clicked() {
                actions.clear_completed = true;
            }
        });
    });

    ui.separator();

    if transfers.is_empty() {
        ui.colored_label(Color32::from_gray(140), "No transfers.");
        return actions;
    }

    // Header row
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let w = ui.available_width();
        ui.add_sized([w * 0.25, 18.0], egui::Label::new(RichText::new("File").strong()));
        ui.add_sized([w * 0.10, 18.0], egui::Label::new(RichText::new("Dir").strong()));
        ui.add_sized([w * 0.30, 18.0], egui::Label::new(RichText::new("Progress").strong()));
        ui.add_sized([w * 0.12, 18.0], egui::Label::new(RichText::new("Speed").strong()));
        ui.add_sized([w * 0.10, 18.0], egui::Label::new(RichText::new("ETA").strong()));
        ui.add_sized([w * 0.08, 18.0], egui::Label::new(RichText::new("Status").strong()));
    });

    ui.separator();

    ScrollArea::vertical().id_salt("transfer_scroll").max_height(140.0).show(ui, |ui| {
        for t in transfers {
            let status_color = match t.status {
                TransferStatus::Completed => Color32::from_rgb(80, 200, 80),
                TransferStatus::Failed => Color32::from_rgb(220, 80, 80),
                TransferStatus::Cancelled => Color32::from_gray(160),
                TransferStatus::InProgress => Color32::from_rgb(100, 180, 255),
                TransferStatus::Queued => Color32::from_gray(200),
            };

            let dir_icon = match t.direction {
                TransferDirection::Download => "⬇",
                TransferDirection::Upload => "⬆",
            };

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let w = ui.available_width();

                // Filename
                ui.add_sized(
                    [w * 0.25, 20.0],
                    egui::Label::new(RichText::new(&t.filename).monospace()).truncate(),
                );

                // Direction
                ui.add_sized([w * 0.10, 20.0], egui::Label::new(dir_icon));

                // Progress bar
                let progress_text = if t.total_bytes > 0 {
                    format!(
                        "{} / {}",
                        human_bytes::human_bytes(t.transferred_bytes as f64),
                        human_bytes::human_bytes(t.total_bytes as f64),
                    )
                } else {
                    human_bytes::human_bytes(t.transferred_bytes as f64)
                };
                ui.add_sized(
                    [w * 0.30, 20.0],
                    ProgressBar::new(t.progress()).text(progress_text),
                );

                // Speed
                ui.add_sized(
                    [w * 0.12, 20.0],
                    egui::Label::new(RichText::new(t.speed_display()).monospace()),
                );

                // ETA
                ui.add_sized(
                    [w * 0.10, 20.0],
                    egui::Label::new(RichText::new(t.eta_display()).monospace()),
                );

                // Status + cancel button
                ui.horizontal(|ui| {
                    ui.colored_label(status_color, t.status.label());
                    if matches!(t.status, TransferStatus::InProgress | TransferStatus::Queued) {
                        if ui.small_button("✕").clicked() {
                            actions.cancel_id = Some(t.id.clone());
                        }
                    }
                });
            });

            // Show error message if failed
            if let Some(err) = &t.error {
                ui.colored_label(Color32::from_rgb(220, 80, 80), format!("  ↳ {err}"));
            }
        }
    });

    actions
}
