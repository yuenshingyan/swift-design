use super::*;

impl GenerationEngine {
    /// Continues every requested preview at the same time, and every
    /// preview whose Finish is pressed while they run. Returns one
    /// outcome per artifact, in start order: the screens added, or the
    /// error.
    ///
    /// A late Finish would otherwise wait for the first continue to end.
    /// The loop wakes on every store change and on a timer, reads the
    /// trailing continue requests again, and starts the new ones.
    pub(crate) async fn continue_artifacts(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        ids: Vec<String>,
        log: &LogSink,
    ) -> Vec<(String, Result<usize, String>)> {
        let shared = ContinueShared {
            client,
            context,
            attachments: Arc::new(self.load_attachments(&context.session_id, log).await),
            group: self.progress_group(5, 95),
            log,
        };
        let mut batch = ContinueBatch::default();
        let mut outcomes: Vec<(String, Result<usize, String>)> = Vec::new();
        let mut changes = self.notifier.subscribe();
        self.start_continues(&mut batch, ids, &shared);
        loop {
            if batch.tasks.is_empty() {
                let late = self.late_continue_ids(context, &batch.started).await;
                if late.is_empty() {
                    break;
                }
                self.start_continues(&mut batch, late, &shared);
                continue;
            }
            tokio::select! {
                joined = batch.tasks.join_next() => match joined {
                    Some(Ok((id, outcome))) => outcomes.push((id, outcome)),
                    Some(Err(error)) => log(&format!("continue task failed: {error}")),
                    None => {}
                },
                changed = changes.changed() => {
                    if changed.is_err() {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    }
                }
                () = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
            }
            let late = self.late_continue_ids(context, &batch.started).await;
            self.start_continues(&mut batch, late, &shared);
        }
        outcomes.sort_by_key(|(id, _)| batch.started.iter().position(|known| known == id));
        outcomes
    }

    /// Starts a continue for each id and records it as started.
    pub(super) fn start_continues(
        &self,
        batch: &mut ContinueBatch,
        ids: Vec<String>,
        shared: &ContinueShared<'_>,
    ) {
        if ids.is_empty() {
            return;
        }
        if !batch.started.is_empty() {
            (shared.log)(&format!(
                "continuing {} more: {}",
                ids.len(),
                ids.join(", ")
            ));
        }
        for id in ids {
            let share = shared.group.share(&id);
            self.spawn_continue(&mut batch.tasks, &id, share, shared);
            batch.started.push(id);
        }
    }

    /// The trailing continue requests that are not running yet: a
    /// Finish pressed after this turn read its task.
    pub(crate) async fn late_continue_ids(
        &self,
        context: &GenerationContext,
        started: &[String],
    ) -> Vec<String> {
        match self.pick_task(context).await {
            Ok(Some(GenerationTask::Continue(ids))) => {
                ids.into_iter().filter(|id| !started.contains(id)).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Starts one continue on the session's kind of artifact.
    pub(super) fn spawn_continue(
        &self,
        tasks: &mut tokio::task::JoinSet<(String, Result<usize, String>)>,
        id: &str,
        share: ShareSink,
        shared: &ContinueShared<'_>,
    ) {
        let engine = self.clone();
        let client = shared.client.clone();
        let context = shared.context.clone();
        let id = id.to_owned();
        let attachments = Arc::clone(&shared.attachments);
        let log = Arc::clone(shared.log);
        tasks.spawn(async move {
            let outcome = match context.request.kind {
                design_model::ArtifactKind::Demo => {
                    engine
                        .continue_design(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Deck => {
                    engine
                        .continue_deck(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Document => {
                    engine
                        .continue_document(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Social => {
                    engine
                        .continue_social(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Print => {
                    engine
                        .continue_print(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Mailing => {
                    engine
                        .continue_mailing(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Campaign => {
                    engine
                        .continue_campaign(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Artwork => {
                    engine
                        .continue_artwork(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
            };
            (id, outcome)
        });
    }
}
