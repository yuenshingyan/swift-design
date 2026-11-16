//! Reverting an assistant turn: every artifact the turn wrote goes
//! back to its snapshot from just before the turn, and an artifact
//! that did not exist before is deleted.

use design_model::ArtifactKind;

use crate::api;

/// The stamp of the earliest snapshot taken at or after `since`: the
/// content the artifact had when the turn began. `since` is RFC 3339;
/// a snapshot's `saved_at` is the same form without the zone.
pub(crate) fn snapshot_since(history: &[api::HistorySnapshot], since: &str) -> Option<String> {
    let since = since.trim_end_matches('Z');
    history
        .iter()
        .filter(|snapshot| snapshot.saved_at.trim_end_matches('Z') >= since)
        .min_by(|first, second| first.saved_at.cmp(&second.saved_at))
        .map(|snapshot| snapshot.stamp.clone())
}

/// The time the turn at `index` started: the `at` of the nearest user
/// message before it.
pub(crate) fn turn_start(messages: &[api::ChatMessage], index: usize) -> Option<String> {
    messages[..index]
        .iter()
        .rev()
        .find(|message| message.role == "user" && !message.is_queued)
        .and_then(|message| message.at.clone())
}

fn restore_stamp(
    history: &[api::HistorySnapshot],
    since: &str,
    exact: Option<&str>,
) -> Option<String> {
    exact
        .map(str::to_owned)
        .or_else(|| snapshot_since(history, since))
}

/// Reverts one artifact to its exact saved baseline, or the legacy time-based baseline. Returns what
/// happened, for the log.
pub(crate) async fn revert_artifact(
    kind: ArtifactKind,
    id: &str,
    since: &str,
    exact_stamp: Option<&str>,
) -> Result<&'static str, String> {
    let history = match kind {
        ArtifactKind::Demo => crate::design::api::fetch_design_history(id).await?,
        ArtifactKind::Deck => crate::deck::api::fetch_deck_history(id).await?,
        ArtifactKind::Document => crate::document::api::fetch_document_history(id).await?,
        ArtifactKind::Social => crate::social::api::fetch_social_history(id).await?,
        ArtifactKind::Print => crate::print::api::fetch_print_history(id).await?,
        ArtifactKind::Mailing => crate::mailing::api::fetch_mailing_history(id).await?,
        ArtifactKind::Campaign => crate::campaign::api::fetch_campaign_history(id).await?,
        ArtifactKind::Artwork => crate::artwork::api::fetch_artwork_history(id).await?,
    };
    match restore_stamp(&history, since, exact_stamp) {
        Some(stamp) => {
            match kind {
                ArtifactKind::Demo => {
                    crate::design::api::restore_design_history(id, &stamp).await?
                }
                ArtifactKind::Deck => crate::deck::api::restore_deck_history(id, &stamp).await?,
                ArtifactKind::Document => {
                    crate::document::api::restore_document_history(id, &stamp).await?
                }
                ArtifactKind::Social => {
                    crate::social::api::restore_social_history(id, &stamp).await?
                }
                ArtifactKind::Print => crate::print::api::restore_print_history(id, &stamp).await?,
                ArtifactKind::Mailing => {
                    crate::mailing::api::restore_mailing_history(id, &stamp).await?
                }
                ArtifactKind::Campaign => {
                    crate::campaign::api::restore_campaign_history(id, &stamp).await?
                }
                ArtifactKind::Artwork => {
                    crate::artwork::api::restore_artwork_history(id, &stamp).await?
                }
            }
            Ok("restored")
        }
        // No snapshot since the turn began: the turn created it.
        None => {
            match kind {
                ArtifactKind::Demo => crate::design::api::delete_design(id).await?,
                ArtifactKind::Deck => crate::deck::api::delete_deck(id).await?,
                ArtifactKind::Document => crate::document::api::delete_document(id).await?,
                ArtifactKind::Social => crate::social::api::delete_social(id).await?,
                ArtifactKind::Print => crate::print::api::delete_print(id).await?,
                ArtifactKind::Mailing => crate::mailing::api::delete_mailing(id).await?,
                ArtifactKind::Campaign => crate::campaign::api::delete_campaign(id).await?,
                ArtifactKind::Artwork => crate::artwork::api::delete_artwork(id).await?,
            }
            Ok("deleted")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(stamp: &str) -> api::HistorySnapshot {
        api::HistorySnapshot {
            stamp: stamp.to_owned(),
            saved_at: stamp[..19].replace('-', ":").replacen(':', "-", 2),
            size_bytes: 1,
        }
    }

    #[test]
    fn the_snapshot_since_a_turn_is_the_earliest_one_taken_after_it() {
        let history = vec![
            snapshot("2026-08-29T07-27-47Z"),
            snapshot("2026-08-29T07-26-53Z"),
            snapshot("2026-08-29T06-59-55Z"),
        ];
        assert_eq!(
            snapshot_since(&history, "2026-08-29T07:26:12Z"),
            Some("2026-08-29T07-26-53Z".to_owned())
        );
        assert_eq!(snapshot_since(&history, "2026-08-29T07:30:00Z"), None);
    }

    #[test]
    fn a_queued_edit_restores_its_exact_snapshot_not_the_turn_running_when_it_was_typed() {
        let history = vec![
            snapshot("2026-08-29T07-26-12Z"),
            snapshot("2026-08-29T07-28-00Z"),
        ];
        assert_eq!(
            restore_stamp(
                &history,
                "2026-08-29T07:25:00Z",
                Some("2026-08-29T07-28-00Z")
            )
            .as_deref(),
            Some("2026-08-29T07-28-00Z")
        );
        assert_eq!(
            restore_stamp(&history, "2026-08-29T07:25:00Z", None).as_deref(),
            Some("2026-08-29T07-26-12Z")
        );
        assert_eq!(
            restore_stamp(&[], "2026-08-29T07:25:00Z", Some("pruned-stamp")).as_deref(),
            Some("pruned-stamp")
        );
    }

    #[test]
    fn a_turn_starts_at_the_user_message_before_it() {
        let user = api::ChatMessage {
            role: "user".to_owned(),
            content: "Fix it.".to_owned(),
            design: None,
            pinned: Vec::new(),
            question_set: None,
            is_continue: false,
            is_queued: false,
            at: Some("2026-08-29T07:26:12Z".to_owned()),
            artifacts: Vec::new(),
            snapshot_stamps: None,
        };
        let assistant = api::ChatMessage {
            role: "assistant".to_owned(),
            content: "Done.".to_owned(),
            at: Some("2026-08-29T07:28:00Z".to_owned()),
            ..user.clone()
        };
        let mut pending = user.clone();
        pending.is_queued = true;
        pending.at = Some("2026-08-29T07:27:00Z".to_owned());
        let messages = vec![user, pending, assistant];
        assert_eq!(
            turn_start(&messages, 2).as_deref(),
            Some("2026-08-29T07:26:12Z")
        );
        assert_eq!(turn_start(&messages, 0), None);
    }
}
