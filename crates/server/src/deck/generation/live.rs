use super::*;

/// Saves a deck while it streams in, so the canvas shows the slides
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
pub(super) struct DeckLiveSaver {
    decks: DeckStore,
    notifier: ChangeNotifier,
    deck_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final deck. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl DeckLiveSaver {
    pub(super) fn new(decks: &DeckStore, notifier: &ChangeNotifier, deck_id: &str) -> Self {
        Self {
            decks: decks.clone(),
            notifier: notifier.clone(),
            deck_id: deck_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial deck. It is saved when it validates and its
    /// `rank` is above the last saved rank.
    pub(super) fn offer(&self, deck: Deck, rank: usize) {
        if !deck.validate().is_empty() {
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
            if saver.decks.save(&saver.deck_id, &deck).await.is_ok() {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final deck after every partial save landed.
    pub(super) async fn finish(&self, deck: &Deck) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.decks
            .save(&self.deck_id, deck)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// The deck a streaming reply has written so far: everything before the
/// slides plus every complete slide. `None` until the first slide is
/// complete, or when the text before the slides is not a deck.
pub(super) fn partial_deck(text: &str) -> Option<Deck> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "slides")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new slides a streaming continuation reply has completed so far.
pub(super) fn partial_continuation_slides(written: usize, text: &str) -> Vec<Slide> {
    let Some((_, items)) = complete_array_items(text, "slides") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"slides\":[{}]}}", items.join(","));
    continuation_slides(written, &json).unwrap_or_default()
}

/// The deck to show while the chunks run: the preview, then every chunk
/// up to the last one that has slides, with placeholders for the slides
/// an earlier chunk still owes.
pub(super) fn shown_deck(preview: &Deck, chunks: &[ContinueChunk], board: &[Vec<Slide>]) -> Deck {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|slides| !slides.is_empty()) else {
        return shown;
    };
    for (chunk, slides) in chunks.iter().zip(board).take(last) {
        shown.slides.extend(slides.iter().cloned());
        for offset in slides.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.slides.push(placeholder_slide(title));
        }
    }
    shown.slides.extend(board[last].iter().cloned());
    shown
}

/// A slide that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a deck that does not.
pub(super) fn placeholder_slide(title: &str) -> Slide {
    Slide {
        html: format!(
            "<div class=\"{PENDING_SLIDE_CLASS} pending\"><p class=\"pending-label\">Writing</p>\
             <h2 class=\"pending-title\">{}</h2></div>",
            escape_html(title),
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

/// The new slides in a continuation reply, in order. Accepts a patch
/// (the slides of its operations at or past the existing slides) and,
/// as a fallback, a whole deck (its slides past the existing ones).
pub(super) fn continuation_slides(written: usize, content: &str) -> Result<Vec<Slide>, String> {
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
        .get("slides")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no slides array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("slide").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("slide"))
            .filter(|slide| slide.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, slide)| {
            serde_json::from_value::<Slide>(slide.clone()).map_err(|error| {
                format!(
                    "new slide {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new slides to the deck in progress. The outline
/// stays until every title has a slide, so a short reply leaves the deck
/// continuable.
pub(super) fn apply_deck_continuation(original: &Deck, content: &str) -> Result<Deck, String> {
    let new_slides = continuation_slides(original.slides.len(), content)?;
    if new_slides.is_empty() {
        return Err(
            "the reply adds no slides: reply with a patch of inserts, one per new slide".to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.slides.extend(new_slides);
    if continued.slides.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}
