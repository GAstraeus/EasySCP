//! The list of transfers under each tab.

use eframe::egui;

const DONE_GREEN: egui::Color32 = egui::Color32::from_rgb(46, 160, 67);

pub enum Status {
    Running,
    Done,
    Failed(String),
}

impl Status {
    pub fn is_running(&self) -> bool {
        matches!(self, Status::Running)
    }
}

impl From<Result<(), String>> for Status {
    fn from(result: Result<(), String>) -> Self {
        match result {
            Ok(()) => Status::Done,
            Err(error) => Status::Failed(error),
        }
    }
}

/// The list's title, with "Clear list" on the right. True when that's clicked.
pub fn heading(ui: &mut egui::Ui, title: &str, can_clear: bool, clear_hint: &str) -> bool {
    let mut clear = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            clear = ui
                .add_enabled(can_clear, egui::Button::new("Clear list").small())
                .on_hover_text(clear_hint)
                .clicked();
        });
    });
    // Keep "Clear list" clear of the first row's button below it, so one
    // isn't clicked for the other.
    ui.add_space(8.0);
    clear
}

/// One transfer: how it's going, what it is, and a button once it's done.
/// True when the button is clicked.
pub fn row(
    ui: &mut egui::Ui,
    id: u64,
    status: &Status,
    text: &str,
    button: &str,
    button_hint: &str,
) -> bool {
    let error_color = ui.visuals().error_fg_color;
    let mut clicked = false;
    ui.horizontal(|ui| {
        match status {
            Status::Running => drop(ui.spinner()),
            Status::Done => drop(ui.colored_label(DONE_GREEN, "✔")),
            Status::Failed(_) => drop(ui.colored_label(error_color, "✖")),
        }
        // Button pinned right; the text takes what's left.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if matches!(status, Status::Done) {
                clicked = ui.small_button(button).on_hover_text(button_hint).clicked();
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                // macOS screenshot names contain a narrow no-break space,
                // which the UI font draws as an empty box.
                let text = text.replace('\u{202f}', " ");
                ui.add(egui::Label::new(text).truncate());
            });
        });
    });
    if let Status::Failed(error) = status {
        ui.indent(id, |ui| {
            ui.label(egui::RichText::new(error).small().color(error_color));
        });
    }
    clicked
}
