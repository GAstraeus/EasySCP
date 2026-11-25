mod complete;
mod download;
mod list;
mod local;
mod paste;
mod ssh_config;
mod transfer;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;

use download::DownloadTab;
use list::Status;

/// Storage key for the last host used.
const HOST_KEY: &str = "host";
/// Storage key for the folder chosen for downloads. Empty for the default one.
const DOWNLOAD_FOLDER_KEY: &str = "download_folder";
const PASTE_HINT: &str = if cfg!(target_os = "macos") {
    "or press ⌘V to paste a copied file or screenshot"
} else {
    "or paste a copied file or screenshot with the button below"
};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("EasySCP")
            // An empty icon keeps eframe from replacing the app bundle's
            // icon in the Dock with its own default.
            .with_icon(egui::IconData::default())
            .with_inner_size([440.0, 420.0])
            .with_min_inner_size([340.0, 260.0]),
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
    /// The name it gets on the remote side.
    fn name(&self) -> String {
        match self {
            Source::File(path) => path
                .file_name()
                .map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned()),
            Source::Image(_) => chrono::Local::now()
                .format("paste-%Y%m%d-%H%M%S.png")
                .to_string(),
        }
    }
}

/// Which way files are going.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Upload,
    Download,
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
    tab: Tab,
    download: DownloadTab,
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
        let download_folder = cc
            .storage
            .and_then(|storage| storage.get_string(DOWNLOAD_FOLDER_KEY))
            .filter(|folder| !folder.is_empty())
            .map(PathBuf::from);
        let (finished_tx, finished_rx) = mpsc::channel();
        Self {
            hosts,
            host,
            tab: Tab::Upload,
            download: DownloadTab::new(download_folder),
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
        self.notice = None;

        for source in sources {
            let id = self.next_id;
            self.next_id += 1;
            let remote_name = source.name();
            self.transfers.push(Transfer {
                id,
                host: host.clone(),
                remote_name: remote_name.clone(),
                status: Status::Running,
            });
            let (host, finished, ctx) = (host.clone(), self.finished_tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let result = upload(id, &host, source, &remote_name);
                let _ = finished.send((id, result));
                ctx.request_repaint();
            });
        }
    }

    /// Sends whatever file or image is on the clipboard.
    fn send_clipboard(&mut self, ctx: &egui::Context) {
        let sources = clipboard_sources();
        if sources.is_empty() {
            self.notice = Some("The clipboard doesn't hold a file or an image.".to_owned());
        } else {
            self.send(ctx, sources);
        }
    }

    fn host_picker(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Host");
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
            self.send_clipboard(ui.ctx());
        }
        if let Some(notice) = &self.notice {
            ui.add_space(4.0);
            ui.colored_label(ui.visuals().warn_fg_color, notice);
        }
    }

    fn transfer_list(&mut self, ui: &mut egui::Ui) {
        if self.transfers.is_empty() {
            return;
        }
        let running = |transfer: &Transfer| transfer.status.is_running();
        if list::heading(
            ui,
            "Uploads",
            !self.transfers.iter().all(running),
            "Removes finished uploads from this list. The files stay on the server.",
        ) {
            // Uploads still running stay, so a late failure isn't hidden.
            self.transfers.retain(running);
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Newest first.
                for transfer in self.transfers.iter().rev() {
                    let path = transfer::display_path(&transfer.remote_name);
                    let text = format!("{}:{path}", transfer.host);
                    if list::row(ui, transfer.id, &transfer.status, &text, "Copy path", "Copy the remote path") {
                        ui.ctx().copy_text(path);
                    }
                }
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok((id, result)) = self.finished_rx.try_recv() {
            if let Some(transfer) = self.transfers.iter_mut().find(|t| t.id == id) {
                transfer.status = result.into();
            }
        }
        self.download.poll();

        // On the Download tab, Cmd+V pastes text into the path field.
        if paste::take_paste_key() && self.tab == Tab::Upload {
            self.send_clipboard(ctx);
        }

        // Files can be dropped anywhere in the window, on either tab.
        let dropped = ctx.input(|i| {
            let paths = i.raw.dropped_files.iter().filter_map(|f| f.path.clone());
            paths.map(Source::File).collect::<Vec<_>>()
        });
        if !dropped.is_empty() {
            self.tab = Tab::Upload;
        }
        self.send(ctx, dropped);

        egui::CentralPanel::default().show(ctx, |ui| {
            self.host_picker(ui);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Upload, "Upload");
                ui.selectable_value(&mut self.tab, Tab::Download, "Download");
            });
            ui.add_space(8.0);
            match self.tab {
                Tab::Upload => {
                    self.drop_zone(ui);
                    ui.add_space(8.0);
                    self.transfer_list(ui);
                }
                Tab::Download => self.download.ui(ui, self.host.as_deref()),
            }
        });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some(host) = &self.host {
            storage.set_string(HOST_KEY, host.clone());
        }
        let folder = self.download.folder().and_then(|folder| folder.to_str());
        storage.set_string(DOWNLOAD_FOLDER_KEY, folder.unwrap_or_default().to_owned());
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
