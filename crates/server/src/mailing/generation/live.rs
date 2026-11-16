use super::*;

/// Saves a mailing while it streams in, so the canvas shows the emails
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
pub(super) struct MailingLiveSaver {
    mailings: MailingStore,
    notifier: ChangeNotifier,
    mailing_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final mailing. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl MailingLiveSaver {
    pub(super) fn new(
        mailings: &MailingStore,
        notifier: &ChangeNotifier,
        mailing_id: &str,
    ) -> Self {
        Self {
            mailings: mailings.clone(),
            notifier: notifier.clone(),
            mailing_id: mailing_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial mailing. It is saved when it validates and its
    /// `rank` is above the last saved rank.
    pub(super) fn offer(&self, mailing: Mailing, rank: usize) {
        if !mailing.validate().is_empty() {
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
                .mailings
                .save(&saver.mailing_id, &mailing)
                .await
                .is_ok()
            {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final mailing after every partial save landed.
    pub(super) async fn finish(&self, mailing: &Mailing) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.mailings
            .save(&self.mailing_id, mailing)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// The mailing a streaming reply has written so far: everything before
/// the emails plus every complete email. `None` until the first email is
/// complete, or when the text before the emails is not a mailing.
pub(super) fn partial_mailing(text: &str) -> Option<Mailing> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "emails")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new emails a streaming continuation reply has completed so far.
pub(super) fn partial_continuation_emails(written: usize, text: &str) -> Vec<Email> {
    let Some((_, items)) = complete_array_items(text, "emails") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"emails\":[{}]}}", items.join(","));
    continuation_emails(written, &json).unwrap_or_default()
}

/// The mailing to show while the chunks run: the preview, then every
/// chunk up to the last one that has emails, with placeholders for the
/// emails an earlier chunk still owes.
pub(super) fn shown_mailing(
    preview: &Mailing,
    chunks: &[ContinueChunk],
    board: &[Vec<Email>],
) -> Mailing {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|emails| !emails.is_empty()) else {
        return shown;
    };
    for (chunk, emails) in chunks.iter().zip(board).take(last) {
        shown.emails.extend(emails.iter().cloned());
        for offset in emails.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.emails.push(placeholder_email(title));
        }
    }
    shown.emails.extend(board[last].iter().cloned());
    shown
}

/// An email that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a mailing that does
/// not.
pub(super) fn placeholder_email(title: &str) -> Email {
    Email {
        html: format!(
            "<div class=\"{PENDING_EMAIL_CLASS} pending\"><p class=\"pending-label\">Writing</p>\
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

/// The new emails in a continuation reply, in order. Accepts a patch
/// (the emails of its operations at or past the existing emails) and, as
/// a fallback, a whole mailing (its emails past the existing ones).
pub(super) fn continuation_emails(written: usize, content: &str) -> Result<Vec<Email>, String> {
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
        .get("emails")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no emails array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("email").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("email"))
            .filter(|email| email.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, email)| {
            serde_json::from_value::<Email>(email.clone()).map_err(|error| {
                format!(
                    "new email {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new emails to the mailing in progress. The
/// outline stays until every title has an email, so a short reply
/// leaves the mailing continuable.
pub(super) fn apply_mailing_continuation(
    original: &Mailing,
    content: &str,
) -> Result<Mailing, String> {
    let new_emails = continuation_emails(original.emails.len(), content)?;
    if new_emails.is_empty() {
        return Err(
            "the reply adds no emails: reply with a patch of inserts, one per new email".to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.emails.extend(new_emails);
    if continued.emails.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}
