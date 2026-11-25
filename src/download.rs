//! The Download tab: type a path on the server and it's copied to this computer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::complete::{self, Entry};
use crate::list::{self, Status};
use crate::{local, transfer};

/// How long a folder's listing is used before it's read again.
const LISTING_FRESH: Duration = Duration::from_secs(30);
/// Most suggestions drawn at once. Typing more narrows a longer list down.
const MAX_SUGGESTIONS: usize = 100;
const SHOW: &str = if cfg!(target_os = "macos") {
    "Show in Finder"
} else {
    "Show in folder"
};

struct Download {
    id: u64,
    /// What's being downloaded, as the list shows it.
    label: String,
    /// Where the copy is, or will be.
    saved: PathBuf,
    status: Status,
}

/// A remote folder: the host, then the path as typed.
type Folder = (String, String);

/// What's known about one remote folder.
struct Listing {
    /// What the last read found. `None` until the first one comes back.
    found: Option<Result<Rc<[Entry]>, String>>,
    reading: bool,
    asked: Instant,
}

type Listed = (Folder, Result<Vec<Entry>, String>);
type Finished = (u64, Result<(), String>);

/// Keys the path field handles itself instead of leaving them to egui.
#[derive(Default)]
struct Keys {
    /// Arrow presses: down counts up, up counts down.
    step: isize,
    tab: bool,
    enter: bool,
    escape: bool,
}

pub struct DownloadTab {
    path: String,
    /// Where downloads are saved. `None` is the default folder.
    folder: Option<PathBuf>,
    /// Why the last download wasn't started.
    notice: Option<String>,
    /// Whether suggestions are wanted: the field is in use and Escape hasn't
    /// closed them.
    suggesting: bool,
    /// Whether there were any to draw in the last frame.
    list_shown: bool,
    /// The suggestion picked with the arrow keys.
    picked: Option<usize>,
    listings: HashMap<Folder, Listing>,
    listed_tx: Sender<Listed>,
    listed_rx: Receiver<Listed>,
    downloads: Vec<Download>,
    next_id: u64,
    finished_tx: Sender<Finished>,
    finished_rx: Receiver<Finished>,
}

impl DownloadTab {
    pub fn new(folder: Option<PathBuf>) -> Self {
        let (listed_tx, listed_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        Self {
            path: String::new(),
            folder,
            notice: None,
            suggesting: false,
            list_shown: false,
            picked: None,
            listings: HashMap::new(),
            listed_tx,
            listed_rx,
            downloads: Vec::new(),
            next_id: 0,
            finished_tx,
            finished_rx,
        }
    }

    /// The folder chosen for downloads, if it isn't the default one.
    pub fn folder(&self) -> Option<&Path> {
        self.folder.as_deref()
    }

    /// Takes in what the worker threads have finished.
    pub fn poll(&mut self) {
        while let Ok((folder, found)) = self.listed_rx.try_recv() {
            if let Some(listing) = self.listings.get_mut(&folder) {
                listing.found = Some(found.map(Rc::from));
                listing.reading = false;
            }
        }
        while let Ok((id, result)) = self.finished_rx.try_recv() {
            if let Some(download) = self.downloads.iter_mut().find(|d| d.id == id) {
                download.status = result.into();
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, host: Option<&str>) {
        let visuals = ui.visuals();
        egui::Frame::new()
            .fill(visuals.faint_bg_color)
            .stroke(visuals.widgets.noninteractive.bg_stroke)
            .corner_radius(8.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                self.path_row(ui, host);
                ui.add_space(2.0);
                self.hint(ui, host);
                ui.add_space(8.0);
                self.folder_row(ui);
            });
        ui.add_space(8.0);
        self.download_list(ui);
    }

    /// The path field with its suggestions, and the Download button.
    fn path_row(&mut self, ui: &mut egui::Ui, host: Option<&str>) {
        let field = egui::Id::new("remote_path");

        // These keys mean something else here than in a plain text field, so
        // they're taken before it sees them.
        let mut keys = Keys::default();
        if ui.memory(|mem| mem.has_focus(field)) {
            use egui::{Key, Modifiers};
            ui.input_mut(|input| {
                let presses = |input: &mut egui::InputState, key| {
                    input.count_and_consume_key(Modifiers::NONE, key) as isize
                };
                keys.step = presses(input, Key::ArrowDown) - presses(input, Key::ArrowUp);
                keys.tab = input.consume_key(Modifiers::NONE, Key::Tab);
                keys.enter = input.consume_key(Modifiers::NONE, Key::Enter);
                keys.escape = self.list_shown && input.consume_key(Modifiers::NONE, Key::Escape);
            });
        }

        let mut start = keys.enter;
        let mut edited = false;
        let response = ui
            .horizontal(|ui| {
                // Button pinned right; the field takes what's left.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ready = host.is_some() && !self.path.trim().is_empty();
                    start |= ui.add_enabled(ready, egui::Button::new("Download")).clicked();
                    // egui draws no outline round a text field that isn't
                    // being typed in, and in light mode this one is white on
                    // white without it.
                    ui.visuals_mut().widgets.inactive.bg_stroke = ui.visuals().widgets.noninteractive.bg_stroke;
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.path)
                            .id(field)
                            .hint_text("~/path/to/file")
                            .desired_width(f32::INFINITY)
                            // Enter is handled above; left to the field,
                            // it would give up the keyboard.
                            .return_key(None),
                    );
                    edited = response.changed();
                    response
                })
                .inner
            })
            .inner;
        if edited || response.gained_focus() {
            self.suggesting = true;
            self.picked = None;
            self.notice = None;
        }

        // What the folder being typed in holds, narrowed to what's typed.
        let mut entries = self.entries(host);
        let mut matches = matching(&entries, &self.path);

        if keys.escape {
            self.suggesting = false;
            self.picked = None;
        }
        if keys.step != 0 && !matches.is_empty() {
            let shown = matches.len().min(MAX_SUGGESTIONS) as isize;
            let from = match self.picked {
                Some(picked) => picked as isize,
                // The first press lands on the first row, or the last going up.
                None if keys.step > 0 => -1,
                None => shown,
            };
            self.picked = Some((from + keys.step).rem_euclid(shown) as usize);
            self.suggesting = true;
        }
        let picked = self.picked.and_then(|picked| matches.get(picked).copied());
        // Enter takes the picked suggestion; with none picked, it downloads.
        let filled = if keys.tab || (keys.enter && picked.is_some()) {
            start = false;
            completed(&self.path, &matches, picked)
        } else {
            None
        };
        if let Some(path) = filled {
            self.set_path(ui.ctx(), field, path);
            entries = self.entries(host);
            matches = matching(&entries, &self.path);
        }

        let (dir, partial) = complete::split(&self.path);
        // Nothing to suggest once the one match is typed out in full.
        let typed_out = matches!(matches.as_slice(), [only] if !only.is_dir && only.name == partial);
        let mut clicked = None;
        let mut pointer_on_list = false;
        self.list_shown = self.suggesting && !matches.is_empty() && !typed_out;
        if self.list_shown {
            let frame = egui::Frame::popup(ui.style());
            let width = response.rect.width() - frame.total_margin().sum().x;
            // As tall as fits under the field, so the list doesn't cover it.
            let below = ui.ctx().screen_rect().bottom() - response.rect.bottom();
            let room = (below - 24.0).clamp(60.0, 180.0);
            let area = egui::Area::new(field.with("suggestions"))
                .kind(egui::UiKind::Popup)
                .order(egui::Order::Foreground)
                .fixed_pos(response.rect.left_bottom() + egui::vec2(0.0, 2.0))
                .default_width(width)
                .show(ui.ctx(), |ui| {
                    frame.show(ui, |ui| {
                        ui.set_width(width);
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        egui::ScrollArea::vertical().max_height(room).show(ui, |ui| {
                            let rows = egui::Layout::top_down_justified(egui::Align::LEFT);
                            ui.with_layout(rows, |ui| {
                                for (row, entry) in matches.iter().take(MAX_SUGGESTIONS).enumerate() {
                                    let is_picked = self.picked == Some(row);
                                    let label = ui.selectable_label(is_picked, entry.typed());
                                    if is_picked && keys.step != 0 {
                                        label.scroll_to_me(None);
                                    }
                                    if label.clicked() {
                                        clicked = Some(format!("{dir}{}", entry.typed()));
                                    }
                                }
                                if matches.len() > MAX_SUGGESTIONS {
                                    let more = matches.len() - MAX_SUGGESTIONS;
                                    ui.weak(format!("…and {more} more. Keep typing to narrow it down."));
                                }
                            });
                        });
                    });
                });
            let pointer = ui.input(|input| input.pointer.interact_pos());
            pointer_on_list = pointer.is_some_and(|pos| area.response.rect.contains(pos));
        }
        if let Some(path) = clicked {
            self.set_path(ui.ctx(), field, path);
            // Clicking the list took the keyboard away from the field.
            response.request_focus();
        }

        if response.has_focus() {
            // Keys that would otherwise move the keyboard to another widget.
            // Tab completes instead. Escape closes the suggestions first, and
            // leaves the field only when there are none showing.
            let keep = egui::EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: self.list_shown,
            };
            ui.memory_mut(|mem| mem.set_focus_lock_filter(field, keep));
            if let Some(host) = host {
                let dir = complete::split(&self.path).0.to_owned();
                self.read_folder(ui.ctx(), (host.to_owned(), dir));
            }
        } else if !pointer_on_list {
            // A press on a suggestion takes the focus before the click
            // lands, so the list has to outlive that.
            self.suggesting = false;
            self.picked = None;
        }

        if start {
            self.start(ui.ctx(), host);
        }
    }

    /// What's in the folder being typed in, if it has been read.
    fn entries(&self, host: Option<&str>) -> Option<Rc<[Entry]>> {
        let dir = complete::split(&self.path).0;
        let listing = self.listings.get(&(host?.to_owned(), dir.to_owned()))?;
        listing.found.as_ref()?.as_ref().ok().cloned()
    }

    /// Replaces the typed path, leaving the cursor at its end.
    fn set_path(&mut self, ctx: &egui::Context, field: egui::Id, path: String) {
        use egui::text::{CCursor, CCursorRange};

        let end = CCursor::new(path.chars().count());
        let mut state = egui::TextEdit::load_state(ctx, field).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::one(end)));
        state.store(ctx, field);

        self.path = path;
        self.suggesting = true;
        self.picked = None;
        self.notice = None;
        // The field drew the old text this frame.
        ctx.request_repaint();
    }

    /// Reads `folder` on a worker thread, unless a recent read will do.
    fn read_folder(&mut self, ctx: &egui::Context, folder: Folder) {
        let listing = self.listings.entry(folder.clone()).or_insert(Listing {
            found: None,
            reading: false,
            asked: Instant::now(),
        });
        if listing.reading || (listing.found.is_some() && listing.asked.elapsed() < LISTING_FRESH) {
            return;
        }
        listing.reading = true;
        listing.asked = Instant::now();

        let (listed, ctx) = (self.listed_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let found = transfer::list(&folder.0, &folder.1).map(|names| complete::parse_listing(&names));
            let _ = listed.send((folder, found));
            ctx.request_repaint();
        });
    }

    /// Starts downloading the typed path on a worker thread.
    fn start(&mut self, ctx: &egui::Context, host: Option<&str>) {
        let Some(host) = host else {
            self.notice = Some("Choose a host first.".to_owned());
            return;
        };
        let remote_path = self.path.trim().to_owned();
        let Some(name) = complete::file_name(&remote_path) else {
            self.notice = Some("Type the path of a file or folder on the server.".to_owned());
            return;
        };
        let Some(folder) = self.folder.clone().or_else(local::default_dir) else {
            self.notice = Some("Choose a folder to save to first.".to_owned());
            return;
        };
        self.notice = None;
        self.suggesting = false;
        self.picked = None;

        let id = self.next_id;
        self.next_id += 1;
        self.downloads.push(Download {
            id,
            label: label(host, &remote_path, name),
            saved: folder.join(name),
            status: Status::Running,
        });
        let (host, finished, ctx) = (host.to_owned(), self.finished_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = transfer::download(&host, &remote_path, &folder);
            let _ = finished.send((id, result));
            ctx.request_repaint();
        });
    }

    /// The line under the path field: what to do, or what went wrong.
    fn hint(&self, ui: &mut egui::Ui, host: Option<&str>) {
        let dir = complete::split(&self.path).0;
        let listing = host.and_then(|host| self.listings.get(&(host.to_owned(), dir.to_owned())));

        let warn = ui.visuals().warn_fg_color;
        let small = |text: &str| egui::RichText::new(text).small();
        match (&self.notice, host, listing) {
            (Some(notice), _, _) => drop(ui.label(small(notice).color(warn))),
            (None, None, _) => drop(ui.label(small("Choose a host to download from").weak())),
            // Why the folder being typed in can't be listed. The last line
            // is the reason; anything before it is the server's greeting.
            (None, Some(_), Some(Listing { found: Some(Err(error)), .. })) => {
                let reason = error.lines().last().unwrap_or(error);
                ui.label(small(reason).color(warn)).on_hover_text(error);
            }
            (None, Some(_), Some(Listing { found: None, reading: true, .. })) => {
                let dir = if dir.is_empty() { "~/" } else { dir };
                // Kept to one line, so a long path doesn't push the rest down.
                ui.add(egui::Label::new(small(&format!("Listing {dir}…")).weak()).truncate());
            }
            (None, Some(host), _) => {
                let hint = format!("Type a path on {host}. Tab completes it.");
                ui.add(egui::Label::new(small(&hint).weak()).truncate());
            }
        }
    }

    /// Where downloads are saved, and the buttons that change it.
    fn folder_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Buttons pinned right; the folder takes what's left.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if local::CAN_PICK_FOLDER
                    && ui
                        .small_button("Choose folder…")
                        .on_hover_text("Choose where downloads are saved")
                        .clicked()
                {
                    let start = self.folder.clone().or_else(crate::ssh_config::home_dir);
                    if let Some(folder) = local::pick_folder(start.as_deref()) {
                        self.folder = Some(folder);
                    }
                }
                if self.folder.is_some()
                    && ui
                        .small_button("Reset")
                        .on_hover_text(format!("Save to ~/{} again", local::LOCAL_DIR))
                        .clicked()
                {
                    self.folder = None;
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let folder = match self.folder.clone().or_else(local::default_dir) {
                        Some(folder) => format!("Saves to {}", local::display(&folder)),
                        None => "Choose a folder to save to".to_owned(),
                    };
                    ui.add(egui::Label::new(egui::RichText::new(folder).small().weak()).truncate());
                });
            });
        });
    }

    fn download_list(&mut self, ui: &mut egui::Ui) {
        if self.downloads.is_empty() {
            return;
        }
        let running = |download: &Download| download.status.is_running();
        if list::heading(
            ui,
            "Downloads",
            !self.downloads.iter().all(running),
            "Removes finished downloads from this list. The files stay where they were saved.",
        ) {
            // Downloads still running stay, so a late failure isn't hidden.
            self.downloads.retain(running);
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Newest first.
                for download in self.downloads.iter().rev() {
                    let hint = format!("Saved as {}", local::display(&download.saved));
                    if list::row(ui, download.id, &download.status, &download.label, SHOW, &hint) {
                        local::reveal(&download.saved);
                    }
                }
            });
    }
}

/// How a download is listed: its name first, so a long path can't crowd it
/// out of the row, then the folder it came from.
fn label(host: &str, remote_path: &str, name: &str) -> String {
    let from = match complete::split(remote_path.trim_end_matches('/')).0 {
        "" => "~/",
        from => from,
    };
    format!("{name} from {host}:{from}")
}

/// The entries the name being typed at the end of `path` could become.
fn matching<'a>(entries: &'a Option<Rc<[Entry]>>, path: &str) -> Vec<&'a Entry> {
    let partial = complete::split(path).1;
    entries
        .as_deref()
        .map(|entries| complete::matches(entries, partial))
        .unwrap_or_default()
}

/// `path` with as much filled in as is certain: the picked suggestion, the
/// only match, or the start that all the matches share. `None` if that adds
/// nothing.
fn completed(path: &str, matches: &[&Entry], picked: Option<&Entry>) -> Option<String> {
    let (dir, partial) = complete::split(path);
    let name = match (picked, matches) {
        (Some(picked), _) => picked.typed(),
        (None, [only]) => only.typed(),
        (None, []) if path == "~" => "~/".to_owned(),
        (None, _) => complete::common_prefix(matches),
    };
    // A shared start can come out shorter than what's typed when the matches
    // differ from it only in capitals.
    (name != partial && name.len() >= partial.len()).then(|| format!("{dir}{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(names: &str) -> Option<Rc<[Entry]>> {
        Some(complete::parse_listing(names).into())
    }

    fn tab(entries: &Option<Rc<[Entry]>>, path: &str) -> Option<String> {
        completed(path, &matching(entries, path), None)
    }

    #[test]
    fn tab_fills_in_what_is_certain() {
        let entries = listing("app.log\napp.log.1\narchive/\nnotes.txt\nDocs/\ndownloads/\n");

        // One match: all of it, and a folder is ready for the next name.
        assert_eq!(tab(&entries, "~/logs/n").as_deref(), Some("~/logs/notes.txt"));
        assert_eq!(tab(&entries, "~/logs/ar").as_deref(), Some("~/logs/archive/"));
        // Several: as far as they agree.
        assert_eq!(tab(&entries, "/var/ap").as_deref(), Some("/var/app.log"));
        // Nothing more is certain.
        assert_eq!(tab(&entries, "/var/app.log"), None);
        assert_eq!(tab(&entries, "/var/a"), None);
        assert_eq!(tab(&entries, "/var/zzz"), None);
        // Capitals are put right.
        assert_eq!(tab(&entries, "NOT").as_deref(), Some("notes.txt"));
        // "do" matches only "downloads" exactly, so "Docs" isn't in the running.
        assert_eq!(tab(&entries, "do").as_deref(), Some("downloads/"));
    }

    #[test]
    fn tab_takes_the_picked_suggestion() {
        let entries = listing("app.log\napp.log.1\narchive/\n");
        let matches = matching(&entries, "logs/a");
        assert_eq!(
            completed("logs/a", &matches, Some(matches[1])).as_deref(),
            Some("logs/app.log.1")
        );
    }

    #[test]
    fn tab_opens_the_home_directory() {
        assert_eq!(tab(&None, "~").as_deref(), Some("~/"));
        assert_eq!(tab(&None, "~/x"), None);
    }

    #[test]
    fn lists_a_download_by_name_then_source() {
        assert_eq!(label("box", "~/logs/app.log", "app.log"), "app.log from box:~/logs/");
        assert_eq!(label("box", "/var/log/", "log"), "log from box:/var/");
        assert_eq!(label("box", "notes.txt", "notes.txt"), "notes.txt from box:~/");
    }

    /// A name that can't be looked up, in case a test strays into a folder
    /// it didn't fill in and `ssh` gets run.
    const HOST: &str = "host.invalid";

    /// The tab in a window with no screen: events go in, drawn text comes out.
    struct Window {
        ctx: egui::Context,
        tab: DownloadTab,
        /// Every piece of text drawn in the last frame, and where.
        drawn: Vec<(String, egui::Pos2)>,
    }

    impl Window {
        /// A window whose server has a `logs` folder with a few things in it.
        fn new() -> Self {
            let mut tab = DownloadTab::new(Some(PathBuf::from("/nowhere")));
            for (dir, names) in [
                ("", "logs/\nnotes.txt\n"),
                ("~/", "logs/\nnotes.txt\n"),
                ("logs/", "app.log\napp.log.1\narchive/\nnotes.txt\n"),
                ("logs/archive/", "old.log\n"),
            ] {
                let found = Some(Ok(complete::parse_listing(names).into()));
                let listing = Listing { found, reading: false, asked: Instant::now() };
                tab.listings.insert((HOST.to_owned(), dir.to_owned()), listing);
            }
            let mut window = Self { ctx: egui::Context::default(), tab, drawn: Vec::new() };
            window.frame(Vec::new());
            window
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(440.0, 420.0))),
                focused: true,
                events,
                ..Default::default()
            };
            let tab = &mut self.tab;
            let output = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| tab.ui(ui, Some(HOST)));
            });
            self.drawn.clear();
            for clipped in &output.shapes {
                collect_text(&clipped.shape, &mut self.drawn);
            }
        }

        fn click(&mut self, pos: egui::Pos2) {
            self.frame(vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                self.frame(vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }]);
            }
            // One more for what the click set off to be drawn.
            self.frame(Vec::new());
        }

        fn click_field(&mut self) {
            let field = self.ctx.read_response(egui::Id::new("remote_path")).expect("the field is drawn");
            self.click(field.rect.center());
        }

        fn type_text(&mut self, text: &str) {
            self.frame(vec![egui::Event::Text(text.to_owned())]);
            self.frame(Vec::new());
        }

        fn press(&mut self, key: egui::Key) {
            let event = |pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(vec![event(true), event(false)]);
            self.frame(Vec::new());
        }

        fn shows(&self, text: &str) -> bool {
            self.drawn.iter().any(|(drawn, _)| drawn == text)
        }

        fn field_has_focus(&self) -> bool {
            self.ctx.memory(|mem| mem.has_focus(egui::Id::new("remote_path")))
        }
    }

    fn collect_text(shape: &egui::Shape, drawn: &mut Vec<(String, egui::Pos2)>) {
        match shape {
            egui::Shape::Text(text) => drawn.push((text.galley.text().to_owned(), text.pos)),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect_text(shape, drawn)),
            _ => {}
        }
    }

    #[test]
    fn typing_narrows_the_suggestions() {
        let mut window = Window::new();
        assert!(!window.shows("logs/"), "nothing is suggested until the field is clicked");

        window.click_field();
        assert!(window.shows("logs/") && window.shows("notes.txt"));

        window.type_text("logs/a");
        assert!(window.shows("app.log") && window.shows("app.log.1") && window.shows("archive/"));
        assert!(!window.shows("notes.txt"));
    }

    #[test]
    fn tab_completes_and_keeps_the_keyboard() {
        let mut window = Window::new();
        window.click_field();
        window.type_text("lo");
        window.press(egui::Key::Tab);
        assert_eq!(window.tab.path, "logs/");
        assert!(window.field_has_focus());

        // Typing carries on at the end of what Tab filled in.
        window.type_text("ap");
        window.press(egui::Key::Tab);
        assert_eq!(window.tab.path, "logs/app.log");
        assert!(window.shows("app.log.1"), "both matches are still offered");
    }

    #[test]
    fn arrows_pick_and_enter_takes_the_pick() {
        let mut window = Window::new();
        window.click_field();
        window.type_text("logs/");
        window.press(egui::Key::ArrowDown);
        window.press(egui::Key::ArrowDown);
        window.press(egui::Key::ArrowDown);
        window.press(egui::Key::ArrowUp);
        window.press(egui::Key::Enter);
        assert_eq!(window.tab.path, "logs/app.log.1");
        assert!(window.tab.downloads.is_empty(), "Enter took the pick; it didn't download yet");
        assert!(window.field_has_focus());

        // Up from nothing picked wraps to the last one.
        window.type_text("x");
        window.press(egui::Key::Backspace);
        for _ in 0..".1".len() + "app.log".len() {
            window.press(egui::Key::Backspace);
        }
        assert_eq!(window.tab.path, "logs/");
        window.press(egui::Key::ArrowUp);
        window.press(egui::Key::Enter);
        assert_eq!(window.tab.path, "logs/notes.txt");
    }

    #[test]
    fn escape_closes_the_suggestions_before_leaving_the_field() {
        let mut window = Window::new();
        window.click_field();
        window.type_text("logs/");
        assert!(window.shows("archive/"));

        window.press(egui::Key::Escape);
        assert!(!window.shows("archive/"));
        assert!(window.field_has_focus());

        window.press(egui::Key::Escape);
        assert!(!window.field_has_focus());

        // With nothing to suggest, one press is enough.
        for _ in 0..60 {
            // Long enough since the last click that this isn't a double click.
            window.frame(Vec::new());
        }
        window.click_field();
        window.type_text("zzz");
        assert_eq!(window.tab.path, "logs/zzz");
        window.press(egui::Key::Escape);
        assert!(!window.field_has_focus());
    }

    #[test]
    fn clicking_a_suggestion_fills_it_in() {
        let mut window = Window::new();
        window.click_field();
        window.type_text("logs/");
        let (_, pos) = window.drawn.iter().find(|(text, _)| text == "archive/").expect("it's suggested").clone();
        window.click(pos + egui::vec2(4.0, 4.0));
        assert_eq!(window.tab.path, "logs/archive/");
        assert!(window.shows("old.log"), "the folder's own listing is offered next");

        // The click didn't take the keyboard away for good.
        window.type_text("o");
        assert_eq!(window.tab.path, "logs/archive/o");
    }

    #[test]
    fn enter_needs_a_path_that_names_something() {
        let mut window = Window::new();
        window.click_field();
        window.type_text("~/");
        window.press(egui::Key::Escape);
        window.press(egui::Key::Enter);
        assert!(window.tab.downloads.is_empty());
        assert!(window.shows("Type the path of a file or folder on the server."));
    }
}
