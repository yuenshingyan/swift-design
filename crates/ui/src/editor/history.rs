use super::*;

use crate::uploads::format_size;

/// The save history of the open design: one row per snapshot, newest
/// first, each with a Restore button. `on_restored` fires after a
/// restore, so the caller reloads the design.
#[component]
pub(crate) fn HistorySection(
    design_id: String,
    kind: ArtifactKind,
    on_restored: EventHandler<()>,
) -> Element {
    let id_for_fetch = design_id.clone();
    let mut snapshots = use_resource(move || {
        let id = id_for_fetch.clone();
        async move {
            match kind {
                ArtifactKind::Demo => api::fetch_design_history(&id).await,
                ArtifactKind::Deck => api::fetch_deck_history(&id).await,
                ArtifactKind::Document => api::fetch_document_history(&id).await,
                ArtifactKind::Social => api::fetch_social_history(&id).await,
                ArtifactKind::Print => api::fetch_print_history(&id).await,
                ArtifactKind::Mailing => api::fetch_mailing_history(&id).await,
                ArtifactKind::Campaign => api::fetch_campaign_history(&id).await,
                ArtifactKind::Artwork => api::fetch_artwork_history(&id).await,
            }
        }
    });
    let mut error = use_signal(|| Option::<String>::None);
    let rows = snapshots.read().clone();
    rsx! {
        div { class: "sheet-section",
            div { class: "head", "History" }
            if let Some(message) = error() {
                p { class: "error", "{message}" }
            }
            match rows {
                Some(Ok(rows)) if rows.is_empty() => rsx! {
                    p { class: "inspector-hint", "No earlier versions yet. Each save keeps the version before it here." }
                },
                Some(Ok(rows)) => rsx! {
                    ul { class: "history-list",
                        for row in rows {
                            li { key: "{row.stamp}", class: "history-row",
                                span { class: "mono", title: "{row.saved_at}", "{history_label(&row.saved_at)}" }
                                span { class: "attachment-size", "{format_size(row.size_bytes)}" }
                                button {
                                    title: "Make this version the current design",
                                    onclick: {
                                        let design_id = design_id.clone();
                                        let stamp = row.stamp.clone();
                                        move |_| {
                                            let design_id = design_id.clone();
                                            let stamp = stamp.clone();
                                            spawn(async move {
                                                let restored = match kind {
                                                    ArtifactKind::Demo => {
                                                        api::restore_design_history(&design_id, &stamp).await
                                                    }
                                                    ArtifactKind::Deck => {
                                                        api::restore_deck_history(&design_id, &stamp).await
                                                    }
                                                    ArtifactKind::Document => {
                                                        api::restore_document_history(&design_id, &stamp).await
                                                    }
                                                    ArtifactKind::Social => {
                                                        api::restore_social_history(&design_id, &stamp).await
                                                    }
                                                    ArtifactKind::Print => {
                                                        api::restore_print_history(&design_id, &stamp).await
                                                    }
                                                    ArtifactKind::Mailing => {
                                                        api::restore_mailing_history(&design_id, &stamp).await
                                                    }
                                                    ArtifactKind::Campaign => {
                                                        api::restore_campaign_history(&design_id, &stamp).await
                                                    }
                                                    ArtifactKind::Artwork => {
                                                        api::restore_artwork_history(&design_id, &stamp).await
                                                    }
                                                };
                                                match restored {
                                                    Ok(()) => {
                                                        error.set(None);
                                                        on_restored.call(());
                                                        snapshots.restart();
                                                    }
                                                    Err(message) => error.set(Some(message)),
                                                }
                                            });
                                        }
                                    },
                                    "Restore"
                                }
                            }
                        }
                    }
                },
                Some(Err(message)) => rsx! {
                    p { class: "error", "{message}" }
                },
                None => rsx! {
                    p { class: "inspector-hint", "Loading history…" }
                },
            }
        }
    }
}

/// A saved-at time for the history list: `2026-08-25 10:14:02 UTC`.
pub(crate) fn history_label(saved_at: &str) -> String {
    saved_at.replacen('T', " ", 1).replacen('Z', " UTC", 1)
}
