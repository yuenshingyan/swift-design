//! Candidates: several versions of one artifact, one final pick.
//!
//! Agents write up to five candidates with ids `{base}-candidate-1` …
//! `{base}-candidate-5`. A demo session writes designs, a deck session
//! writes decks; the session's artifact kind says which store the
//! chooser reads. The chooser page shows the candidates side by side;
//! choosing one saves a copy as `{base}`. The candidates stay until the
//! user deletes them, so the pick can change.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use design_model::{ArtifactKind, DECK_VIEWPORT};
use serde::Deserialize;

use crate::api_error;
use crate::decks::DeckStore;
use crate::designs::{DesignStore, is_valid_design_id};
use crate::events::ChangeNotifier;
use crate::sessions::SessionStore;

/// Most candidates one base id may have. Keeps generation cost bounded.
pub const CANDIDATE_LIMIT: usize = 5;

/// The `/candidates` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/candidates/{base}", get(chooser))
        .route("/candidates/{base}/choose", post(choose))
}

/// Body of `POST /candidates/{base}/choose`.
#[derive(Debug, Deserialize)]
struct ChooseRequest {
    /// Id of the winning candidate.
    id: String,
}

/// One card on the chooser page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CandidateCard {
    /// The candidate id.
    pub id: String,
    /// The theme name shown under the preview.
    pub theme: String,
    /// The CSS `aspect-ratio` of the preview frame.
    pub ratio: String,
    /// The render URL the preview iframe loads.
    pub preview_url: String,
}

/// The artifact kind of the session `base`, or demo when no session
/// owns the base.
async fn session_kind(sessions: &SessionStore, base: &str) -> ArtifactKind {
    match sessions.read(base).await {
        Ok(Some(session)) => session.artifact_kind,
        _ => ArtifactKind::Demo,
    }
}

/// The design candidates of `base` as cards, sorted by id.
async fn design_cards(store: &DesignStore, base: &str) -> anyhow::Result<Vec<CandidateCard>> {
    let prefix = format!("{base}-candidate-");
    Ok(store
        .list()
        .await?
        .into_iter()
        .filter(|summary| summary.id.starts_with(&prefix))
        .map(|summary| CandidateCard {
            preview_url: format!("/designs/{}/render", summary.id),
            ratio: summary.viewport.aspect_ratio_css(),
            id: summary.id,
            theme: summary.theme,
        })
        .collect())
}

/// The deck candidates of `base` as cards, sorted by id.
async fn deck_cards(store: &DeckStore, base: &str) -> anyhow::Result<Vec<CandidateCard>> {
    let prefix = format!("{base}-candidate-");
    Ok(store
        .list()
        .await?
        .into_iter()
        .filter(|summary| summary.id.starts_with(&prefix))
        .map(|summary| CandidateCard {
            preview_url: format!("/decks/{}/render", summary.id),
            ratio: DECK_VIEWPORT.aspect_ratio_css(),
            id: summary.id,
            theme: summary.theme,
        })
        .collect())
}

/// Shows every candidate for `base` side by side with a choose button.
async fn chooser(
    State(designs): State<DesignStore>,
    State(decks): State<DeckStore>,
    State(sessions): State<SessionStore>,
    Path(base): Path<String>,
) -> Response {
    if !is_valid_design_id(&base) {
        return api_error::invalid_design_id(&base);
    }
    let kind = session_kind(&sessions, &base).await;
    let cards = match kind {
        ArtifactKind::Demo => design_cards(&designs, &base).await,
        ArtifactKind::Deck => deck_cards(&decks, &base).await,
    };
    let mut cards = match cards {
        Ok(cards) => cards,
        Err(error) => return api_error::internal_error(&error),
    };
    if cards.is_empty() {
        return api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "no candidates for `{base}`: save {}s named `{base}-candidate-1` and so on",
                unit_name(kind)
            ),
            Vec::new(),
        );
    }
    cards.truncate(CANDIDATE_LIMIT);
    Html(chooser_page(&base, kind, &cards)).into_response()
}

/// Saves a copy of the chosen candidate as `base`. The candidates
/// stay, so the user can come back and pick another.
async fn choose(
    State(designs): State<DesignStore>,
    State(decks): State<DeckStore>,
    State(sessions): State<SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path(base): Path<String>,
    Json(request): Json<ChooseRequest>,
) -> Response {
    if !is_valid_design_id(&base) {
        return api_error::invalid_design_id(&base);
    }
    if !is_valid_design_id(&request.id) || !request.id.starts_with(&format!("{base}-candidate-")) {
        return api_error::error_response(
            StatusCode::BAD_REQUEST,
            &format!("`{}` is not a candidate of `{base}`", request.id),
            Vec::new(),
        );
    }
    let kind = session_kind(&sessions, &base).await;
    let copied = match kind {
        ArtifactKind::Demo => copy_design(&designs, &request.id, &base).await,
        ArtifactKind::Deck => copy_deck(&decks, &request.id, &base).await,
    };
    if let Err(response) = copied {
        return response;
    }
    // Record the choice on the owning session, when one exists.
    let _ = sessions
        .update(&base, |session| {
            session.chosen_design = Some(request.id.clone());
        })
        .await;
    notifier.notify();
    tracing::info!(%base, chosen = %request.id, kind = kind.as_str(), "candidate chosen");
    Json(serde_json::json!({ "id": base })).into_response()
}

/// Copies the design `id` to `base`. The copy is agent-authored: the
/// candidate replaces the design wholesale.
async fn copy_design(store: &DesignStore, id: &str, base: &str) -> Result<(), Response> {
    let design = match store.load(id).await {
        Ok(Some(design)) => design,
        Ok(None) => return Err(api_error::design_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    store
        .save(base, &design)
        .await
        .map_err(|error| api_error::internal_error(&error))?;
    store
        .clear_user_paths(base)
        .await
        .map_err(|error| api_error::internal_error(&error))
}

/// Copies the deck `id` to `base`. The copy is agent-authored.
async fn copy_deck(store: &DeckStore, id: &str, base: &str) -> Result<(), Response> {
    let deck = match store.load(id).await {
        Ok(Some(deck)) => deck,
        Ok(None) => return Err(api_error::deck_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    store
        .save(base, &deck)
        .await
        .map_err(|error| api_error::internal_error(&error))?;
    store
        .clear_user_paths(base)
        .await
        .map_err(|error| api_error::internal_error(&error))
}

/// The word for one artifact of `kind`, for the page text.
fn unit_name(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Demo => "design",
        ArtifactKind::Deck => "deck",
    }
}

/// The page the browser opens after a choice: the chosen artifact's
/// render route.
fn chosen_url(kind: ArtifactKind, base: &str) -> String {
    match kind {
        ArtifactKind::Demo => format!("/designs/{base}/render"),
        ArtifactKind::Deck => format!("/decks/{base}/render"),
    }
}

/// Builds the chooser HTML: one card per candidate with an iframe
/// preview and a choose button.
/// Candidate ids pass `is_valid_design_id`, so they are safe to embed.
/// Theme names come from artifact JSON and are escaped.
fn chooser_page(base: &str, kind: ArtifactKind, candidates: &[CandidateCard]) -> String {
    let unit = unit_name(kind);
    let mut cards = String::new();
    for candidate in candidates {
        cards.push_str(&format!(
            "<article>\n\
             <iframe src=\"{preview}\" title=\"{id}\" style=\"aspect-ratio: {ratio}\"></iframe>\n\
             <div class=\"card-footer\">\n\
             <span class=\"card-label\">{id} · {theme}</span>\n\
             <button data-id=\"{id}\">Choose this {unit}</button>\n\
             </div>\n</article>\n",
            preview = candidate.preview_url,
            id = candidate.id,
            theme = crate::render::escape_html(&candidate.theme),
            ratio = candidate.ratio,
        ));
    }
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Choose a {unit}: {base}</title>\n<style>\n\
         body {{ margin: 0; padding: 3.2rem 3.8rem; background: #F7F6F3; color: #15181C;\n\
           font-family: Inter, system-ui, sans-serif; }}\n\
         .page-header {{ display: flex; align-items: flex-end; justify-content: space-between;\n\
           gap: 1.5rem; padding-bottom: 2.1rem; }}\n\
         h1 {{ margin: 0 0 0.6rem; font-size: 2.1rem; letter-spacing: -0.03em; font-weight: 600; }}\n\
         h1 code {{ font-family: 'JetBrains Mono', ui-monospace, monospace; font-size: 1.85rem;\n\
           background: #EAE7E0; padding: 0.1rem 0.5rem; border-radius: 4px; }}\n\
         .page-header p {{ margin: 0; font-size: 0.95rem; color: #4E545B; }}\n\
         .page-header p code {{ font-family: 'JetBrains Mono', ui-monospace, monospace;\n\
           font-size: 0.85rem; }}\n\
         .design-count {{ font-family: 'JetBrains Mono', ui-monospace, monospace;\n\
           font-size: 0.75rem; color: #6C7178; white-space: nowrap; }}\n\
         main {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(24rem, 1fr));\n\
           gap: 1.9rem; align-content: start; }}\n\
         article {{ display: flex; flex-direction: column; border: 1px solid #E3E1DB;\n\
           background: #FFFFFF; }}\n\
         iframe {{ width: 100%; aspect-ratio: 16 / 9; border: 0;\n\
           border-bottom: 1px solid #E3E1DB; }}\n\
         .card-footer {{ display: flex; align-items: center; justify-content: space-between;\n\
           gap: 0.9rem; padding: 0.95rem 1.1rem; }}\n\
         .card-label {{ font-family: 'JetBrains Mono', ui-monospace, monospace;\n\
           font-size: 0.75rem; color: #6C7178; }}\n\
         button {{ font: inherit; font-size: 0.8rem; font-weight: 500; cursor: pointer;\n\
           border: 1px solid #15181C; border-radius: 5px; background: #FFFFFF;\n\
           padding: 0.45rem 0.9rem; white-space: nowrap; }}\n\
         button:hover {{ background: #15181C; color: #F7F6F3; }}\n\
         </style>\n</head>\n<body>\n\
         <header class=\"page-header\">\n<div>\n\
         <h1>Choose a {unit} for <code>{base}</code></h1>\n\
         <p>Candidates written by your agent in one run. \
         Picking one saves it as <code>{unit}s/{base}.json</code>.</p>\n\
         </div>\n<span class=\"design-count\">{count} candidates</span>\n</header>\n\
         <main>\n{cards}</main>\n<script>\n\
         document.querySelectorAll('button[data-id]').forEach((button) => {{\n\
           button.addEventListener('click', async () => {{\n\
             const response = await fetch('/candidates/{base}/choose', {{\n\
               method: 'POST',\n\
               headers: {{ 'content-type': 'application/json' }},\n\
               body: JSON.stringify({{ id: button.dataset.id }}),\n\
             }});\n\
             if (response.ok) {{ window.location = '{chosen}'; }}\n\
           }});\n\
         }});\n</script>\n</body>\n</html>\n",
        count = candidates.len(),
        chosen = chosen_url(kind, base),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::ArtifactKind;

    use super::{CandidateCard, chooser_page, chosen_url, deck_cards, design_cards};
    use crate::decks::DeckStore;
    use crate::designs::DesignStore;
    use crate::test_support::sample_deck;

    #[tokio::test]
    async fn cards_carry_the_right_preview_url_per_kind() {
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let decks = DeckStore::new(directory.path().join("decks"));
        let design: design_model::Design =
            serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap();
        designs.save("talk-candidate-1", &design).await.unwrap();
        designs.save("other-candidate-1", &design).await.unwrap();
        decks
            .save("talk-candidate-1", &sample_deck())
            .await
            .unwrap();
        decks
            .save("talk-candidate-2", &sample_deck())
            .await
            .unwrap();
        let design_cards = design_cards(&designs, "talk").await.unwrap();
        assert_eq!(design_cards.len(), 1);
        assert_eq!(
            design_cards[0].preview_url,
            "/designs/talk-candidate-1/render"
        );
        assert_eq!(design_cards[0].ratio, "1440 / 900");
        let deck_cards = deck_cards(&decks, "talk").await.unwrap();
        assert_eq!(deck_cards.len(), 2);
        assert_eq!(deck_cards[1].preview_url, "/decks/talk-candidate-2/render");
        assert_eq!(deck_cards[1].ratio, "1920 / 1080");
    }

    #[test]
    fn the_chooser_page_names_the_kind_and_the_chosen_url() {
        let card = CandidateCard {
            id: "talk-candidate-1".to_owned(),
            theme: "midnight <b>".to_owned(),
            ratio: "1920 / 1080".to_owned(),
            preview_url: "/decks/talk-candidate-1/render".to_owned(),
        };
        let page = chooser_page("talk", ArtifactKind::Deck, &[card]);
        assert!(page.contains("<title>Choose a deck: talk</title>"));
        assert!(page.contains("Choose this deck"));
        assert!(page.contains("midnight &lt;b&gt;"));
        assert!(page.contains("src=\"/decks/talk-candidate-1/render\""));
        assert!(page.contains("window.location = '/decks/talk/render'"));
        assert_eq!(
            chosen_url(ArtifactKind::Demo, "talk"),
            "/designs/talk/render"
        );
    }
}
