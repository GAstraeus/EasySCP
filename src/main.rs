mod paste;
mod ssh_config;
mod transfer;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;

/// Storage key for the last host used.
const HOST_KEY: &str = "host";
const SENT_GREEN: egui::Color32 = egui::Color32::from_rgb(46, 160, 67);
const PASTE_HINT: &str = if cfg!(target_os = "macos") {
    "or press ⌘V to paste a copied file or screenshot"
} else {
    "or paste a copied file or screenshot with the button below"
};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("EasySCP")
            .with_inner_size([440.0, 460.0])
            .with_min_inner_size([340.0, 300.0]),
        ..Default::default()
    };
    eframe::run_native(
        "EasySCP",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

/// Something to upload.
enum Source {
    File(PathBuf),
    /// An image pasted from the clipboard; not on disk yet.
    Image(arboard::ImageData<'static>),
}

impl Source {
    /// The name it gets on the remote side unless the user renames it.
    fn name(&self) -> String {
        match self {
            Source::File(path) => path
                .file_name()
                .map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned()),
            Source::Image(_) => chrono::Local::now()
                .format("screenshot-%Y%m%d-%H%M%S.png")
                .to_string(),
        }
    }
}

enum Status {
    Sending,
    Sent,
    Failed(String),
}

struct Transfer {
    id: u64,
    host: String,
    remote_name: String,
    status: Status,
}

type Finished = (u64, Result<(), String>);

struct App {
    hosts: Vec<String>,
    host: Option<String>,
    rename: String,
    /// Why the last drop or paste wasn't sent.
    notice: Option<String>,
    transfers: Vec<Transfer>,
    next_id: u64,
    finished_tx: Sender<Finished>,
    finished_rx: Receiver<Finished>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        paste::watch_paste_key(&cc.egui_ctx);
        let hosts = ssh_config::load_hosts();
        let host = cc
            .storage
            .and_then(|storage| storage.get_string(HOST_KEY))
            .filter(|host| hosts.contains(host));
        let (finished_tx, finished_rx) = mpsc::channel();
        Self {
            hosts,
            host,
            rename: String::new(),
            notice: None,
            transfers: Vec::new(),
            next_id: 0,
            finished_tx,
            finished_rx,
        }
    }

    /// Starts uploading `sources` to the selected host, one thread each.
    fn send(&mut self, ctx: &egui::Context, sources: Vec<Source>) {
        if sources.is_empty() {
            return;
        }
        let Some(host) = self.host.clone() else {
            self.notice = Some("Choose a host first.".to_owned());
            return;
        };
        // Settle every name before starting, so a bad name sends nothing.
        let mut jobs = Vec::new();
        for (index, source) in sources.into_iter().enumerate() {
            match transfer::remote_name(&source.name(), &self.rename, index) {
                Ok(remote_name) => jobs.push((source, remote_name)),
                Err(problem) => {
                    self.notice = Some(problem);
                    return;
                }
            }
        }
        self.notice = None;
        self.rename.clear();

        for (source, remote_name) in jobs {
            let id = self.next_id;
            self.next_id += 1;
            self.transfers.push(Transfer {
                id,
                host: host.clone(),
                remote_name: remote_name.clone(),
                status: Status::Sending,
            });
            let (host, finished, ctx) = (host.clone(), self.finished_tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let result = upload(id, &host, source, &remote_name);
                let _ = finished.send((id, result));
                ctx.request_repaint();
            });
        }
    }

    /// Cmd+V: send a copied file or image. Plain text is left alone so it can
    /// still be pasted into the "Save as" field.
    fn handle_paste_key(&mut self, ctx: &egui::Context) {
        let pressed = paste::take_paste_key();
        let is_text_paste = |e: &egui::Event| matches!(e, egui::Event::Paste(_));
        let text_paste = ctx.input(|i| i.events.iter().any(is_text_paste));
        if !pressed && !text_paste {
            return;
        }
        let sources = clipboard_sources();
        if sources.is_empty() {
            if pressed && ctx.memory(|m| m.focused().is_none()) {
                self.notice = Some("The clipboard doesn't hold a file or an image.".to_owned());
            }
            return;
        }
        // A copied file also puts its name on the clipboard as text; keep
        // that from landing in the "Save as" field (also on key repeat).
        ctx.input_mut(|i| i.events.retain(|e| !is_text_paste(e)));
        if pressed {
            self.send(ctx, sources);
        }
    }

    fn form(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("form")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("Host");
                ui.horizontal(|ui| {
                    let reload_width = 32.0;
                    let selected = match &self.host {
                        Some(host) => host.as_str(),
                        None if self.hosts.is_empty() => "No hosts in ~/.ssh/config",
                        None => "Choose a host…",
                    };
                    egui::ComboBox::from_id_salt("host")
                        .width(ui.available_width() - reload_width)
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            for host in &self.hosts {
                                ui.selectable_value(&mut self.host, Some(host.clone()), host);
                            }
                        });
                    if ui
                        .button("🔄")
                        .on_hover_text("Reload hosts from ~/.ssh/config")
                        .clicked()
                    {
                        self.hosts = ssh_config::load_hosts();
                        if self.host.as_ref().is_some_and(|h| !self.hosts.contains(h)) {
                            self.host = None;
                        }
                    }
                });
                ui.end_row();

                ui.label("Save as");
                ui.add(
                    egui::TextEdit::singleline(&mut self.rename)
                        .hint_text("optional — blank keeps the original name")
                        .desired_width(f32::INFINITY),
                );
                ui.end_row();
            });
    }

    fn drop_zone(&mut self, ui: &mut egui::Ui) {
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let visuals = ui.visuals();
        let accent = visuals.selection.bg_fill;
        let (fill, stroke) = if hovering {
            (accent.gamma_multiply(0.25), egui::Stroke::new(2.0, accent))
        } else {
            (visuals.faint_bg_color, visuals.widgets.noninteractive.bg_stroke)
        };

        let mut paste_clicked = false;
        egui::Frame::new()
            .fill(fill)
            .stroke(stroke)
            .corner_radius(8.0)
            .inner_margin(egui::Margin::symmetric(12, 18))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("Drop files here").size(17.0).strong());
                    ui.label(egui::RichText::new(PASTE_HINT).weak());
                    ui.add_space(8.0);
                    paste_clicked = ui.button("Paste from clipboard").clicked();
                    ui.add_space(8.0);
                    let destination = match &self.host {
                        Some(host) => format!("Sends to {host}:{}", transfer::display_path("")),
                        None => "Choose a host to send to".to_owned(),
                    };
                    ui.label(egui::RichText::new(destination).small().weak());
                });
            });

        if paste_clicked {
            let sources = clipboard_sources();
            if sources.is_empty() {
                self.notice = Some("The clipboard doesn't hold a file or an image.".to_owned());
            } else {
                self.send(ui.ctx(), sources);
            }
        }
        if let Some(notice) = &self.notice {
            ui.add_space(4.0);
            ui.colored_label(ui.visuals().warn_fg_color, notice);
        }
    }

    fn transfer_list(&self, ui: &mut egui::Ui) {
        let error_color = ui.visuals().error_fg_color;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Newest first.
                for transfer in self.transfers.iter().rev() {
                    let path = transfer::display_path(&transfer.remote_name);
                    ui.horizontal(|ui| {
                        match transfer.status {
                            Status::Sending => drop(ui.spinner()),
                            Status::Sent => drop(ui.colored_label(SENT_GREEN, "✔")),
                            Status::Failed(_) => drop(ui.colored_label(error_color, "✖")),
                        }
                        // Button pinned right; the path takes what's left.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if matches!(transfer.status, Status::Sent)
                                && ui
                                    .small_button("Copy path")
                                    .on_hover_text("Copy the remote path")
                                    .clicked()
                            {
                                ui.ctx().copy_text(path.clone());
                            }
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                let text = format!("{}:{path}", transfer.host);
                                ui.add(egui::Label::new(text).truncate());
                            });
                        });
                    });
                    if let Status::Failed(error) = &transfer.status {
                        ui.indent(transfer.id, |ui| {
                            ui.label(egui::RichText::new(error).small().color(error_color));
                        });
                    }
                }
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok((id, result)) = self.finished_rx.try_recv() {
            if let Some(transfer) = self.transfers.iter_mut().find(|t| t.id == id) {
                transfer.status = match result {
                    Ok(()) => Status::Sent,
                    Err(error) => Status::Failed(error),
                };
            }
        }

        // Before any widget runs, so a file paste never reaches a text field.
        self.handle_paste_key(ctx);

        // Files can be dropped anywhere in the window.
        let dropped = ctx.input(|i| {
            let paths = i.raw.dropped_files.iter().filter_map(|f| f.path.clone());
            paths.map(Source::File).collect::<Vec<_>>()
        });
        self.send(ctx, dropped);

        egui::CentralPanel::default().show(ctx, |ui| {
            self.form(ui);
            ui.add_space(8.0);
            self.drop_zone(ui);
            ui.add_space(8.0);
            self.transfer_list(ui);
        });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some(host) = &self.host {
            storage.set_string(HOST_KEY, host.clone());
        }
    }
}

fn clipboard_sources() -> Vec<Source> {
    match paste::read_clipboard() {
        paste::Clip::Files(files) => files.into_iter().map(Source::File).collect(),
        paste::Clip::Image(image) => vec![Source::Image(image)],
        paste::Clip::Nothing => Vec::new(),
    }
}

/// Runs one upload to completion. Blocks; call from a worker thread.
fn upload(id: u64, host: &str, source: Source, remote_name: &str) -> Result<(), String> {
    match source {
        Source::File(path) => transfer::upload(host, &path, remote_name),
        Source::Image(image) => {
            let file_name = format!("easyscp-{}-{id}.png", std::process::id());
            let temp = std::env::temp_dir().join(file_name);
            let result = paste::save_png(&image, &temp)
                .and_then(|()| transfer::upload(host, &temp, remote_name));
            let _ = std::fs::remove_file(&temp);
            result
        }
    }
}
