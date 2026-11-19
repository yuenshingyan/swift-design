//! Run-owned previews and durable review drafts, separate from accepted designs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use design_model::Design;
use serde::Serialize;

use super::{DesignStore, DesignSummary, is_pending_screen, is_valid_design_id};
use crate::events::ChangeNotifier;
use crate::files::write_atomically;

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Default)]
pub(super) struct PreviewStore(Arc<Mutex<HashMap<String, PreviewEntry>>>);

struct PreviewEntry {
    owner: u64,
    design: Option<Design>,
}

/// The independent visual draft returned by the preview endpoint.
#[derive(Serialize)]
pub(super) struct PreviewView {
    pub(super) design: Design,
    pub(super) is_recovery: bool,
}

/// Keeps a candidate's preview alive until its request and workers finish.
#[derive(Clone)]
pub(crate) struct LivePreview(Arc<PreviewOwner>);

struct PreviewOwner {
    store: PreviewStore,
    id: String,
    owner: u64,
    notifier: ChangeNotifier,
    last_attempt: Mutex<Instant>,
    protected: Option<(Design, Vec<usize>)>,
}

impl DesignStore {
    /// Starts an isolated preview, optionally protecting unselected baseline screens.
    pub(crate) fn start_preview(
        &self,
        id: &str,
        notifier: &ChangeNotifier,
        protected: Option<(Design, Vec<usize>)>,
    ) -> LivePreview {
        let owner = NEXT_OWNER.fetch_add(1, Ordering::Relaxed);
        let cleared = if let Ok(mut entries) = self.previews.0.lock() {
            entries
                .insert(
                    id.to_owned(),
                    PreviewEntry {
                        owner,
                        design: None,
                    },
                )
                .is_some_and(|entry| entry.design.is_some())
        } else {
            false
        };
        if cleared {
            notifier.notify();
        }
        LivePreview(Arc::new(PreviewOwner {
            store: self.previews.clone(),
            id: id.to_owned(),
            owner,
            notifier: notifier.clone(),
            last_attempt: Mutex::new(Instant::now() - Duration::from_secs(1)),
            protected,
        }))
    }

    pub(super) fn preview(&self, id: &str) -> Option<PreviewView> {
        let entries = self.previews.0.lock().ok()?;
        Some(PreviewView {
            design: entries.get(id)?.design.clone()?,
            is_recovery: false,
        })
    }

    pub(super) async fn preview_view(&self, id: &str) -> anyhow::Result<Option<PreviewView>> {
        if let Some(live) = self.preview(id) {
            return Ok(Some(live));
        }
        Ok(self.load_review_draft(id).await?.map(|design| PreviewView {
            design,
            is_recovery: true,
        }))
    }

    fn review_draft_path(&self, id: &str) -> anyhow::Result<PathBuf> {
        anyhow::ensure!(
            is_valid_design_id(id),
            "Cannot access review draft: invalid design ID."
        );
        Ok(self
            .directory
            .join(".review-drafts")
            .join(format!("{id}.json")))
    }

    /// Checkpoints an unverified edit under its existing source candidate only.
    pub(crate) async fn keep_review_draft(
        &self,
        source_id: &str,
        design: &Design,
        before: Option<&Design>,
    ) -> anyhow::Result<()> {
        let path = self.review_draft_path(source_id)?;
        anyhow::ensure!(
            design.validate().is_empty(),
            "Cannot preserve review draft: invalid design."
        );
        let _writer = self.write_lock.lock().await;
        if let Some(before) = before {
            anyhow::ensure!(
                self.load(source_id).await?.as_ref() == Some(before),
                "The saved version changed during the edit. The review draft was not replaced."
            );
        } else {
            tokio::fs::metadata(self.path_of(source_id)).await?;
        }
        tokio::fs::create_dir_all(self.directory.join(".review-drafts")).await?;
        write_atomically(&path, &serde_json::to_string(design)?).await
    }

    /// Loads the durable unverified draft, without reading or changing accepted data.
    pub(crate) async fn load_review_draft(&self, id: &str) -> anyhow::Result<Option<Design>> {
        match tokio::fs::read(self.review_draft_path(id)?).await {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Loads a draft for repair only if the accepted file did not change after it.
    pub(crate) async fn review_draft_for_edit(&self, id: &str) -> anyhow::Result<Design> {
        let _writer = self.write_lock.lock().await;
        let draft = self
            .load_review_draft(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("This candidate has no retained review draft."))?;
        let written = tokio::fs::metadata(self.review_draft_path(id)?)
            .await?
            .modified()?;
        let saved = tokio::fs::metadata(self.path_of(id)).await?.modified()?;
        anyhow::ensure!(
            saved <= written,
            "The saved version changed after this review draft. Repairing it could overwrite newer work."
        );
        Ok(draft)
    }

    /// Removes the draft after the candidate is accepted or deleted.
    pub(crate) async fn clear_review_draft(&self, id: &str) -> anyhow::Result<()> {
        match tokio::fs::remove_file(self.review_draft_path(id)?).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn overlay_previews(&self, summaries: &mut Vec<DesignSummary>) {
        let Ok(entries) = self.previews.0.lock() else {
            return;
        };
        for (id, entry) in entries.iter() {
            let Some(design) = &entry.design else {
                continue;
            };
            summaries.retain(|summary| summary.id != *id);
            summaries.push(DesignSummary {
                id: id.clone(),
                title: design.title.clone(),
                theme: design.theme.name.clone(),
                viewport: design.viewport,
                screen_count: design.screens.len(),
                pending_count: design
                    .screens
                    .iter()
                    .filter(|screen| is_pending_screen(screen))
                    .count(),
                outline_count: design.outline.len(),
            });
        }
        summaries.sort_by(|left, right| left.id.cmp(&right.id));
    }
}

impl LivePreview {
    /// Limits parsing and frame refreshes to five updates a second.
    pub(crate) fn should_update(&self) -> bool {
        let Ok(mut last) = self.0.last_attempt.lock() else {
            return false;
        };
        if last.elapsed() < Duration::from_millis(200) {
            return false;
        }
        *last = Instant::now();
        true
    }

    /// Publishes only safe, well-formed designs without changing saved data.
    pub(crate) fn show(&self, design: &Design) {
        if !design.validate().is_empty() {
            return;
        }
        if let Some((baseline, scope)) = &self.0.protected
            && !scope.is_empty()
            && baseline.screens.iter().enumerate().any(|(index, screen)| {
                !scope.contains(&index) && design.screens.get(index) != Some(screen)
            })
        {
            return;
        }
        let changed = if let Ok(mut entries) = self.0.store.0.lock() {
            match entries.get_mut(&self.0.id) {
                Some(entry)
                    if entry.owner == self.0.owner && entry.design.as_ref() != Some(design) =>
                {
                    entry.design = Some(design.clone());
                    true
                }
                _ => false,
            }
        } else {
            false
        };
        if changed {
            self.0.notifier.notify();
        }
    }
}

#[cfg(test)]
mod tests;

impl Drop for PreviewOwner {
    fn drop(&mut self) {
        let removed = if let Ok(mut entries) = self.store.0.lock() {
            if entries
                .get(&self.id)
                .is_some_and(|entry| entry.owner == self.owner)
            {
                entries
                    .remove(&self.id)
                    .is_some_and(|entry| entry.design.is_some())
            } else {
                false
            }
        } else {
            false
        };
        if removed {
            self.notifier.notify();
        }
    }
}
