//! Background monitoring for file-changes.
//!
//! Whenever a file changes, we want to regenerate the HTML and send it to the UI for rendering to
//! the user. This is done with the `init_update_loop` function.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::marker::Send;

use dirs::home_dir;
use log::{debug, error, warn};
use notify::{Watcher, RecursiveMode, DebouncedEvent, watcher};

use crate::ui;
use crate::markdown;

/// A common trait for `glib::Sender` and `std::mpsc::Sender`.
///
/// Both of them have the exact same `send` method, down to the error type they use. Still, we need
/// a shared trait to use in the `init_update_loop` function.
///
/// In practice, we only use `glib::Sender` in "real code", but `std::mpsc::Sender` allows easier
/// testing, so that's why this trait exists.
///
pub trait Sender {
    /// Send a `ui::Event` to the receiver at the other end
    fn send(&mut self, event: ui::Event) -> Result<(), mpsc::SendError<ui::Event>>;
}

impl Sender for glib::Sender<ui::Event> {
    fn send(&mut self, event: ui::Event) -> Result<(), mpsc::SendError<ui::Event>> {
        glib::Sender::<ui::Event>::send(self, event)
    }
}

impl Sender for mpsc::Sender<ui::Event> {
    fn send(&mut self, event: ui::Event) -> Result<(), mpsc::SendError<ui::Event>> {
        mpsc::Sender::<ui::Event>::send(self, event)
    }
}

/// The main background worker. Spawns a thread and uses the `notify` crate to listen for file changes.
///
/// Input:
///
/// - `renderer`:  The struct that takes care of rendering the markdown file into HTML. Used to get
///                the filename to monitor and to generate the HTML on update.
/// - `ui_sender`: The channel to send `ui::Event` records to when a change is detected.
///
/// A change to the main markdown file triggers a rerender and webview refresh. A change to the
/// user-level configuration files is only going to trigger a refresh.
///
pub fn init_update_loop<S>(renderer: markdown::Renderer, mut ui_sender: S)
    where S: Sender + Send + 'static
{
    thread::spawn(move || {
        let (watcher_sender, watcher_receiver) = mpsc::channel();

        // Initial render
        if let Err(e) = watcher_sender.send(DebouncedEvent::Write(renderer.canonical_md_path.clone())) {
            error!("Couldn't render markdown: {}", e);
        }

        let mut watcher = match watcher(watcher_sender, Duration::from_millis(200)) {
            Ok(w) => w,
            Err(e) => {
                warn!("Couldn't initialize watcher: {}", e);
                return;
            }
        };
        if let Err(e) = watcher.watch(&renderer.canonical_md_path, RecursiveMode::NonRecursive) {
            warn!("Couldn't initialize watcher: {}", e);
            return;
        }

        if let Some(home) = home_dir() {
            if let Ok(_) = watcher.watch(home.join(".quickmd.css"), RecursiveMode::NonRecursive) {
                debug!("Watching ~/.quickmd.css");
            }
            if let Ok(_) = watcher.watch(home.join(".config/quickmd.css"), RecursiveMode::NonRecursive) {
                debug!("Watching ~/.config/quickmd.css");
            }
        }

        loop {
            match watcher_receiver.recv() {
                Ok(DebouncedEvent::Write(file)) => {
                    debug!("File updated: {}", file.display());

                    if file == renderer.canonical_md_path {
                        match renderer.run() {
                            Ok(html) => {
                                let _ = ui_sender.send(ui::Event::LoadHtml(html));
                            },
                            Err(e) => {
                                error! {
                                    "Error rendering markdown ({}): {:?}",
                                    renderer.canonical_md_path.display(), e
                                };
                            }
                        }
                    } else {
                        let _ = ui_sender.send(ui::Event::Reload);
                    }
                },
                Ok(event) => debug!("Ignored watcher event: {:?}", event),
                Err(e) => error!("Error watching file for changes: {:?}", e),
            }
        }
    });
}
