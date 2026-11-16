use super::*;

/// Saves a social while it streams in, so the canvas shows the frames
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
pub(super) struct SocialLiveSaver {
    socials: SocialStore,
    notifier: ChangeNotifier,
    social_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final social. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl SocialLiveSaver {
    pub(super) fn new(socials: &SocialStore, notifier: &ChangeNotifier, social_id: &str) -> Self {
        Self {
            socials: socials.clone(),
            notifier: notifier.clone(),
            social_id: social_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial social. It is saved when it validates and its
    /// `rank` is above the last saved rank.
    pub(super) fn offer(&self, social: Social, rank: usize) {
        if !social.validate().is_empty() {
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
            if saver.socials.save(&saver.social_id, &social).await.is_ok() {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final social after every partial save landed.
    pub(super) async fn finish(&self, social: &Social) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.socials
            .save(&self.social_id, social)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// The social a streaming reply has written so far: everything before
/// the frames plus every complete frame. `None` until the first frame is
/// complete, or when the text before the frames is not a social.
pub(super) fn partial_social(text: &str) -> Option<Social> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "frames")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new frames a streaming continuation reply has completed so far.
pub(super) fn partial_continuation_frames(written: usize, text: &str) -> Vec<Frame> {
    let Some((_, items)) = complete_array_items(text, "frames") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"frames\":[{}]}}", items.join(","));
    continuation_frames(written, &json).unwrap_or_default()
}

/// The social to show while the chunks run: the preview, then every
/// chunk up to the last one that has frames, with placeholders for the
/// frames an earlier chunk still owes.
pub(super) fn shown_social(
    preview: &Social,
    chunks: &[ContinueChunk],
    board: &[Vec<Frame>],
) -> Social {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|frames| !frames.is_empty()) else {
        return shown;
    };
    for (chunk, frames) in chunks.iter().zip(board).take(last) {
        shown.frames.extend(frames.iter().cloned());
        for offset in frames.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.frames.push(placeholder_frame(title));
        }
    }
    shown.frames.extend(board[last].iter().cloned());
    shown
}

/// A frame that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a social that does
/// not.
pub(super) fn placeholder_frame(title: &str) -> Frame {
    Frame {
        html: format!(
            "<div class=\"{PENDING_FRAME_CLASS} pending\"><p class=\"pending-label\">Writing</p>\
             <h2 class=\"pending-title\">{}</h2></div>",
            escape_html(title),
        ),
        css: Some(
            ".pending { display: flex; flex-direction: column; align-items: center; \
             justify-content: center; height: 100%; gap: 16px; opacity: 0.55; }\n\
             .pending-label { margin: 0; font-size: 14px; letter-spacing: 0.3em; \
             text-transform: uppercase; color: var(--muted); }\n\
             .pending-title { margin: 0; max-width: 600px; text-align: center; \
             font-size: 32px; color: var(--text); }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// The new frames in a continuation reply, in order. Accepts a patch
/// (the frames of its operations at or past the existing frames) and, as
/// a fallback, a whole social (its frames past the existing ones).
pub(super) fn continuation_frames(written: usize, content: &str) -> Result<Vec<Frame>, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    let value: serde_json::Value = serde_json::from_str(&content[start..=end])
        .map_err(|error| format!("invalid JSON: {error}"))?;
    let items = value
        .get("frames")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no frames array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("frame").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("frame"))
            .filter(|frame| frame.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, frame)| {
            serde_json::from_value::<Frame>(frame.clone()).map_err(|error| {
                format!(
                    "new frame {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new frames to the social in progress. The
/// outline stays until every title has a frame, so a short reply leaves
/// the social continuable.
pub(super) fn apply_social_continuation(
    original: &Social,
    content: &str,
) -> Result<Social, String> {
    let new_frames = continuation_frames(original.frames.len(), content)?;
    if new_frames.is_empty() {
        return Err(
            "the reply adds no frames: reply with a patch of inserts, one per new frame".to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.frames.extend(new_frames);
    if continued.frames.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}
