//! Reading files/images off the clipboard, and noticing Cmd+V.

use std::path::{Path, PathBuf};

pub enum Clip {
    Files(Vec<PathBuf>),
    Image(arboard::ImageData<'static>),
    /// Text, or empty: nothing to upload.
    Nothing,
}

pub fn read_clipboard() -> Clip {
    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return Clip::Nothing;
    };
    // Files first: a file copied in Finder also carries its icon as an image.
    if let Ok(files) = clipboard.get().file_list() {
        if !files.is_empty() {
            return Clip::Files(files);
        }
    }
    match clipboard.get_image() {
        Ok(image) => Clip::Image(image),
        Err(_) => Clip::Nothing,
    }
}

pub fn save_png(image: &arboard::ImageData<'_>, path: &Path) -> Result<(), String> {
    image::save_buffer(
        path,
        &image.bytes,
        image.width as u32,
        image.height as u32,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| format!("couldn't save the pasted image: {e}"))
}

/// Starts watching for Cmd+V. Call once, on the main thread, after the app
/// has started.
pub fn watch_paste_key(ctx: &eframe::egui::Context) {
    #[cfg(target_os = "macos")]
    mac::install(ctx.clone());
    #[cfg(not(target_os = "macos"))]
    let _ = ctx;
}

/// True once for each Cmd+V press since the last call (key repeats don't count).
pub fn take_paste_key() -> bool {
    #[cfg(target_os = "macos")]
    return mac::PASTE_KEY.swap(false, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(target_os = "macos"))]
    false
}

/// egui only reports Cmd+V when the clipboard holds text, so a pasted
/// screenshot would go unnoticed. This listens for the key press itself.
#[cfg(target_os = "macos")]
mod mac {
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};

    use block2::RcBlock;
    use objc2_app_kit::{NSEvent, NSEventMask};

    pub static PASTE_KEY: AtomicBool = AtomicBool::new(false);

    const SHIFT: usize = 1 << 17;
    const CONTROL: usize = 1 << 18;
    const OPTION: usize = 1 << 19;
    const COMMAND: usize = 1 << 20;
    /// Physical "V" key, for layouts where Cmd+V doesn't produce a Latin "v".
    const KEYCODE_V: u16 = 9;

    pub fn install(ctx: eframe::egui::Context) {
        let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit passes a valid event that outlives this call.
            if is_cmd_v(unsafe { event.as_ref() }) {
                PASTE_KEY.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            }
            // Hand the event on untouched so text fields still get it.
            event.as_ptr()
        });
        // SAFETY: called on the main thread; the block is copied by AppKit.
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
        };
        // Watch for as long as the app runs.
        std::mem::forget(monitor);
    }

    fn is_cmd_v(event: &NSEvent) -> bool {
        // SAFETY: these are plain getters, valid on any key-down event.
        unsafe {
            let flags = event.modifierFlags().0;
            if flags & COMMAND == 0 || flags & (SHIFT | CONTROL | OPTION) != 0 || event.isARepeat() {
                return false;
            }
            match event.characters().map(|s| s.to_string()).as_deref() {
                Some("v" | "V") => true,
                Some(chars) if chars.is_ascii() => false,
                _ => event.keyCode() == KEYCODE_V,
            }
        }
    }
}
