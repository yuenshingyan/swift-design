use super::*;

/// Saves a document while it streams in, so the canvas shows the pages
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
pub(super) struct DocumentLiveSaver {
    documents: DocumentStore,
    notifier: ChangeNotifier,
    document_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final document. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl DocumentLiveSaver {
    pub(super) fn new(
        documents: &DocumentStore,
        notifier: &ChangeNotifier,
        document_id: &str,
    ) -> Self {
        Self {
            documents: documents.clone(),
            notifier: notifier.clone(),
            document_id: document_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial document. It is saved when it validates and its
    /// `rank` is above the last saved rank.
    pub(super) fn offer(&self, document: Document, rank: usize) {
        if !document.validate().is_empty() {
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
            if saver
                .documents
                .save(&saver.document_id, &document)
                .await
                .is_ok()
            {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final document after every partial save landed.
    pub(super) async fn finish(&self, document: &Document) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.documents
            .save(&self.document_id, document)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// The document a streaming reply has written so far: everything before
/// the pages plus every complete page. `None` until the first page is
/// complete, or when the text before the pages is not a document.
pub(super) fn partial_document(text: &str) -> Option<Document> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "pages")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new pages a streaming continuation reply has completed so far.
pub(super) fn partial_continuation_pages(written: usize, text: &str) -> Vec<Page> {
    let Some((_, items)) = complete_array_items(text, "pages") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"pages\":[{}]}}", items.join(","));
    continuation_pages(written, &json).unwrap_or_default()
}

/// The document to show while the chunks run: the preview, then every
/// chunk up to the last one that has pages, with placeholders for the
/// pages an earlier chunk still owes.
pub(super) fn shown_document(
    preview: &Document,
    chunks: &[ContinueChunk],
    board: &[Vec<Page>],
) -> Document {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|pages| !pages.is_empty()) else {
        return shown;
    };
    for (chunk, pages) in chunks.iter().zip(board).take(last) {
        shown.pages.extend(pages.iter().cloned());
        for offset in pages.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.pages.push(placeholder_page(title));
        }
    }
    shown.pages.extend(board[last].iter().cloned());
    shown
}

/// A page that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a document that does
/// not.
pub(super) fn placeholder_page(title: &str) -> Page {
    Page {
        html: format!(
            "<div class=\"{PENDING_PAGE_CLASS} pending\"><p class=\"pending-label\">Writing</p>\
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

/// The new pages in a continuation reply, in order. Accepts a patch
/// (the pages of its operations at or past the existing pages) and, as
/// a fallback, a whole document (its pages past the existing ones).
pub(super) fn continuation_pages(written: usize, content: &str) -> Result<Vec<Page>, String> {
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
        .get("pages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no pages array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("page").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("page"))
            .filter(|page| page.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, page)| {
            serde_json::from_value::<Page>(page.clone()).map_err(|error| {
                format!(
                    "new page {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new pages to the document in progress. The
/// outline stays until every title has a page, so a short reply leaves
/// the document continuable.
pub(super) fn apply_document_continuation(
    original: &Document,
    content: &str,
) -> Result<Document, String> {
    let new_pages = continuation_pages(original.pages.len(), content)?;
    if new_pages.is_empty() {
        return Err(
            "the reply adds no pages: reply with a patch of inserts, one per new page".to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.pages.extend(new_pages);
    if continued.pages.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}
