use super::*;

/// Saves a print while it streams in, so the canvas shows the sheets
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
pub(super) struct PrintLiveSaver {
    prints: PrintStore,
    notifier: ChangeNotifier,
    print_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final print. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl PrintLiveSaver {
    pub(super) fn new(prints: &PrintStore, notifier: &ChangeNotifier, print_id: &str) -> Self {
        Self {
            prints: prints.clone(),
            notifier: notifier.clone(),
            print_id: print_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial print. It is saved when it validates and its
    /// `rank` is above the last saved rank.
    pub(super) fn offer(&self, print: Print, rank: usize) {
        if !print.validate().is_empty() {
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
            if saver.prints.save(&saver.print_id, &print).await.is_ok() {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final print after every partial save landed.
    pub(super) async fn finish(&self, print: &Print) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.prints
            .save(&self.print_id, print)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// The print a streaming reply has written so far: everything before
/// the sheets plus every complete sheet. `None` until the first sheet is
/// complete, or when the text before the sheets is not a print.
pub(super) fn partial_print(text: &str) -> Option<Print> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "sheets")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new sheets a streaming continuation reply has completed so far.
pub(super) fn partial_continuation_sheets(written: usize, text: &str) -> Vec<Sheet> {
    let Some((_, items)) = complete_array_items(text, "sheets") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"sheets\":[{}]}}", items.join(","));
    continuation_sheets(written, &json).unwrap_or_default()
}

/// The print to show while the chunks run: the preview, then every
/// chunk up to the last one that has sheets, with placeholders for the
/// sheets an earlier chunk still owes.
pub(super) fn shown_print(
    preview: &Print,
    chunks: &[ContinueChunk],
    board: &[Vec<Sheet>],
) -> Print {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|sheets| !sheets.is_empty()) else {
        return shown;
    };
    for (chunk, sheets) in chunks.iter().zip(board).take(last) {
        shown.sheets.extend(sheets.iter().cloned());
        for offset in sheets.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.sheets.push(placeholder_sheet(title));
        }
    }
    shown.sheets.extend(board[last].iter().cloned());
    shown
}

/// A sheet that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a print that does
/// not.
pub(super) fn placeholder_sheet(title: &str) -> Sheet {
    Sheet {
        html: format!(
            "<div class=\"{PENDING_SHEET_CLASS} pending\"><p class=\"pending-label\">Writing</p>\
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

/// The new sheets in a continuation reply, in order. Accepts a patch
/// (the sheets of its operations at or past the existing sheets) and, as
/// a fallback, a whole print (its sheets past the existing ones).
pub(super) fn continuation_sheets(written: usize, content: &str) -> Result<Vec<Sheet>, String> {
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
        .get("sheets")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no sheets array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("sheet").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("sheet"))
            .filter(|sheet| sheet.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, sheet)| {
            serde_json::from_value::<Sheet>(sheet.clone()).map_err(|error| {
                format!(
                    "new sheet {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new sheets to the print in progress. The
/// outline stays until every title has a sheet, so a short reply leaves
/// the print continuable.
pub(super) fn apply_print_continuation(original: &Print, content: &str) -> Result<Print, String> {
    let new_sheets = continuation_sheets(original.sheets.len(), content)?;
    if new_sheets.is_empty() {
        return Err(
            "the reply adds no sheets: reply with a patch of inserts, one per new sheet".to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.sheets.extend(new_sheets);
    if continued.sheets.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}
