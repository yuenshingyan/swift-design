use super::*;
use crate::design::core::{DesignStore, PENDING_SCREEN_CLASS};
use crate::events::ChangeNotifier;
use crate::render::escape_html;

/// The design a streaming reply has written so far: everything before
/// the screens plus every complete screen. `None` until the first screen
/// is complete, or when the text before the screens is not a design.
pub(crate) fn partial_design(text: &str) -> Option<Design> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "screens")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new screens a streaming continuation reply has completed so far.
pub(crate) fn partial_continuation_screens(
    written: usize,
    text: &str,
) -> Vec<design_model::Screen> {
    let Some((_, items)) = complete_array_items(text, "screens") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"screens\":[{}]}}", items.join(","));
    continuation_screens(written, &json).unwrap_or_default()
}

/// The design to show while the chunks run: the preview, then every chunk
/// up to the last one that has screens. A chunk with screens still to come
/// is padded with one placeholder per outline title it owes, so the
/// screens of a later chunk appear in their real place instead of waiting
/// for the chunks before them.
///
/// Nothing is padded past the last chunk with screens. The shown design
/// therefore stays shorter than its outline until the run finishes, so a
/// run that dies leaves a design the user can still continue.
pub(crate) fn shown_design(
    preview: &Design,
    chunks: &[ContinueChunk],
    board: &[Vec<design_model::Screen>],
) -> Design {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|screens| !screens.is_empty()) else {
        return shown;
    };
    for (chunk, screens) in chunks.iter().zip(board).take(last) {
        shown.screens.extend(screens.iter().cloned());
        for offset in screens.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            let mut placeholder = placeholder_screen(title);
            if preview.app_window.is_some() {
                placeholder.html =
                    format!("<div data-swift-design-window>{}</div>", placeholder.html);
            }
            shown.screens.push(placeholder);
        }
    }
    // The last chunk with screens ends the design. Padding after it would
    // fill every outline slot and make the design look finished.
    shown.screens.extend(board[last].iter().cloned());
    shown
}

/// A screen that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a design that does not.
pub(super) fn placeholder_screen(title: &str) -> design_model::Screen {
    design_model::Screen {
        name: title.to_owned(),
        html: format!(
            "<div class=\"{pending} pending\"><p class=\"pending-label\">Writing</p>\
             <h2 class=\"pending-title\">{title}</h2></div>",
            pending = PENDING_SCREEN_CLASS,
            title = escape_html(title),
        ),
        css: Some(
            ".pending { display: flex; flex-direction: column; align-items: center; \
             justify-content: center; height: 100%; gap: 24px; opacity: 0.55; }\n\
             .pending-label { margin: 0; font-size: 26px; letter-spacing: 0.3em; \
             text-transform: uppercase; color: var(--muted); }\n\
             .pending-title { margin: 0; max-width: 1400px; text-align: center; \
             font-size: 64px; color: var(--text); }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// Saves a design while it streams in, so the canvas shows the screens
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
pub(crate) struct LiveSaver {
    designs: DesignStore,
    notifier: ChangeNotifier,
    design_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final design. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl LiveSaver {
    pub(crate) fn new(engine: &GenerationEngine, design_id: &str) -> Self {
        Self {
            designs: engine.designs.clone(),
            notifier: engine.notifier.clone(),
            design_id: design_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial design. It is saved when it validates and its
    /// `rank` is above the last saved rank. `rank` counts the real
    /// progress behind the design, which is not always its screen count: a
    /// continuation pads the gaps with placeholder screens, so only the
    /// screens the model wrote may raise it.
    pub(crate) fn offer(&self, design: Design, rank: usize) {
        if !design.validate().is_empty() {
            return;
        }
        {
            let Ok(mut saved) = self.saved_rank.lock() else {
                return;
            };
            if saved.is_some_and(|saved| rank <= saved) {
                return;
            }
            *saved = Some(rank);
        }
        let saver = self.clone();
        tokio::spawn(async move {
            let _guard = saver.write_lock.lock().await;
            if saver.is_finished.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            if saver.designs.save(&saver.design_id, &design).await.is_ok() {
                saver.notifier.notify();
            }
        });
    }

    /// Stops pending draft writes and restores the baseline after verification fails.
    pub(crate) async fn abort(&self, previous: Option<&Design>) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        let restored = if let Some(previous) = previous {
            self.designs.save(&self.design_id, previous).await
        } else {
            self.designs.delete(&self.design_id).await.map(|_| ())
        };
        restored.map_err(|error| {
            tracing::error!(design_id = %self.design_id, %error, "failed to restore the candidate after layout verification failed");
            "Could not restore the candidate after verification failed. Retry the run.".to_owned()
        })?;
        self.notifier.notify();
        Ok(())
    }

    /// Saves the final design after every partial save landed.
    pub(crate) async fn finish(&self, design: &Design) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.designs
            .save(&self.design_id, design)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}
