use super::*;

/// Saves a campaign while it streams in, so the canvas shows the ads
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
pub(super) struct CampaignLiveSaver {
    campaigns: CampaignStore,
    notifier: ChangeNotifier,
    campaign_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final campaign. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl CampaignLiveSaver {
    pub(super) fn new(
        campaigns: &CampaignStore,
        notifier: &ChangeNotifier,
        campaign_id: &str,
    ) -> Self {
        Self {
            campaigns: campaigns.clone(),
            notifier: notifier.clone(),
            campaign_id: campaign_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial campaign. It is saved when it validates and its
    /// `rank` is above the last saved rank.
    pub(super) fn offer(&self, campaign: Campaign, rank: usize) {
        if !campaign.validate().is_empty() {
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
                .campaigns
                .save(&saver.campaign_id, &campaign)
                .await
                .is_ok()
            {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final campaign after every partial save landed.
    pub(super) async fn finish(&self, campaign: &Campaign) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.campaigns
            .save(&self.campaign_id, campaign)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// The campaign a streaming reply has written so far: everything before
/// the ads plus every complete ad. `None` until the first ad is
/// complete, or when the text before the ads is not a campaign.
pub(super) fn partial_campaign(text: &str) -> Option<Campaign> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "ads")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new ads a streaming continuation reply has completed so far.
pub(super) fn partial_continuation_ads(written: usize, text: &str) -> Vec<Ad> {
    let Some((_, items)) = complete_array_items(text, "ads") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"ads\":[{}]}}", items.join(","));
    continuation_ads(written, &json).unwrap_or_default()
}

/// The campaign to show while the chunks run: the preview, then every
/// chunk up to the last one that has ads, with placeholders for the
/// ads an earlier chunk still owes.
pub(super) fn shown_campaign(
    preview: &Campaign,
    chunks: &[ContinueChunk],
    board: &[Vec<Ad>],
) -> Campaign {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|ads| !ads.is_empty()) else {
        return shown;
    };
    for (chunk, ads) in chunks.iter().zip(board).take(last) {
        shown.ads.extend(ads.iter().cloned());
        for offset in ads.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.ads.push(placeholder_ad(title));
        }
    }
    shown.ads.extend(board[last].iter().cloned());
    shown
}

/// An ad that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a campaign that does
/// not.
pub(super) fn placeholder_ad(title: &str) -> Ad {
    Ad {
        html: format!(
            "<div class=\"{PENDING_AD_CLASS} pending\"><p class=\"pending-label\">Writing</p>\
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

/// The new ads in a continuation reply, in order. Accepts a patch
/// (the ads of its operations at or past the existing ads) and, as
/// a fallback, a whole campaign (its ads past the existing ones).
pub(super) fn continuation_ads(written: usize, content: &str) -> Result<Vec<Ad>, String> {
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
        .get("ads")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no ads array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("ad").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("ad"))
            .filter(|ad| ad.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, ad)| {
            serde_json::from_value::<Ad>(ad.clone()).map_err(|error| {
                format!(
                    "new ad {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new ads to the campaign in progress. The
/// outline stays until every title has an ad, so a short reply
/// leaves the campaign continuable.
pub(super) fn apply_campaign_continuation(
    original: &Campaign,
    content: &str,
) -> Result<Campaign, String> {
    let new_ads = continuation_ads(original.ads.len(), content)?;
    if new_ads.is_empty() {
        return Err(
            "the reply adds no ads: reply with a patch of inserts, one per new ad".to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.ads.extend(new_ads);
    if continued.ads.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}
