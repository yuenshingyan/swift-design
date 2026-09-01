//! Candidates: several versions of one artifact, one final pick.
//!
//! Agents write up to five candidates with ids `{base}-candidate-1` …
//! `{base}-candidate-5`. A demo session writes designs, a deck session
//! writes decks, a document session writes documents; the session's
//! artifact kind says which store the chooser reads. The chooser page
//! shows the candidates side by side; choosing one saves a copy as
//! `{base}`. The candidates stay until the user deletes them, so the
//! pick can change.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use design_model::{ArtifactKind, DECK_VIEWPORT};
use serde::Deserialize;

use crate::api_error;
use crate::campaigns::CampaignStore;
use crate::decks::DeckStore;
use crate::designs::{DesignStore, is_valid_design_id};
use crate::documents::DocumentStore;
use crate::events::ChangeNotifier;
use crate::mailings::MailingStore;
use crate::prints::PrintStore;
use crate::session_routes::ArtifactStores;
use crate::sessions::SessionStore;
use crate::socials::SocialStore;

/// Most variations one base id may have. Keeps generation cost bounded.
pub const CANDIDATE_LIMIT: usize = 5;

/// Most canvases a demo run may build for: desktop, phone, and tablet.
pub const PLATFORM_LIMIT: usize = 3;

/// Most candidate cards the chooser shows: every variation on every
/// canvas, since a run writes one design per platform per variation.
pub const CARD_LIMIT: usize = CANDIDATE_LIMIT * PLATFORM_LIMIT;

/// The number after the highest `{base}-candidate-{n}` in `ids`, or 1.
/// Ids of another base and ids without a numeric tail are skipped. A
/// fork, a merge, and a later run all number after the candidates
/// that exist, so none overwrites an earlier one.
pub(crate) fn next_candidate_number<'id>(base: &str, ids: impl Iterator<Item = &'id str>) -> usize {
    let prefix = format!("{base}{}", crate::sessions::CANDIDATE_MARKER);
    ids.filter_map(|id| id.strip_prefix(&prefix))
        .filter_map(|tail| tail.parse::<usize>().ok())
        .max()
        .map_or(1, |highest| highest + 1)
}

/// The id of candidate `number` of `base`.
pub(crate) fn candidate_id(base: &str, number: usize) -> String {
    format!("{base}{}{number}", crate::sessions::CANDIDATE_MARKER)
}

/// The candidate number in `id`, when its tail is one.
pub(crate) fn candidate_number_of(id: &str) -> Option<usize> {
    let (_, tail) = id.rsplit_once(crate::sessions::CANDIDATE_MARKER)?;
    tail.parse().ok()
}

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

/// The document candidates of `base` as cards, sorted by id.
async fn document_cards(store: &DocumentStore, base: &str) -> anyhow::Result<Vec<CandidateCard>> {
    let prefix = format!("{base}-candidate-");
    Ok(store
        .list()
        .await?
        .into_iter()
        .filter(|summary| summary.id.starts_with(&prefix))
        .map(|summary| CandidateCard {
            preview_url: format!("/documents/{}/render", summary.id),
            ratio: summary.paper.viewport().aspect_ratio_css(),
            id: summary.id,
            theme: summary.theme,
        })
        .collect())
}

/// The social candidates of `base` as cards, sorted by id.
async fn social_cards(store: &SocialStore, base: &str) -> anyhow::Result<Vec<CandidateCard>> {
    let prefix = format!("{base}-candidate-");
    Ok(store
        .list()
        .await?
        .into_iter()
        .filter(|summary| summary.id.starts_with(&prefix))
        .map(|summary| CandidateCard {
            preview_url: format!("/socials/{}/render", summary.id),
            ratio: summary.format.viewport().aspect_ratio_css(),
            id: summary.id,
            theme: summary.theme,
        })
        .collect())
}

/// The print candidates of `base` as cards, sorted by id.
async fn print_cards(store: &PrintStore, base: &str) -> anyhow::Result<Vec<CandidateCard>> {
    let prefix = format!("{base}-candidate-");
    Ok(store
        .list()
        .await?
        .into_iter()
        .filter(|summary| summary.id.starts_with(&prefix))
        .map(|summary| CandidateCard {
            preview_url: format!("/prints/{}/render", summary.id),
            ratio: summary
                .orientation
                .apply(summary.size.viewport())
                .aspect_ratio_css(),
            id: summary.id,
            theme: summary.theme,
        })
        .collect())
}

/// The mailing candidates of `base` as cards, sorted by id.
async fn mailing_cards(store: &MailingStore, base: &str) -> anyhow::Result<Vec<CandidateCard>> {
    let prefix = format!("{base}-candidate-");
    Ok(store
        .list()
        .await?
        .into_iter()
        .filter(|summary| summary.id.starts_with(&prefix))
        .map(|summary| CandidateCard {
            preview_url: format!("/mailings/{}/render", summary.id),
            ratio: summary.format.viewport().aspect_ratio_css(),
            id: summary.id,
            theme: summary.theme,
        })
        .collect())
}

/// The campaign candidates of `base` as cards, sorted by id.
async fn campaign_cards(store: &CampaignStore, base: &str) -> anyhow::Result<Vec<CandidateCard>> {
    let prefix = format!("{base}-candidate-");
    Ok(store
        .list()
        .await?
        .into_iter()
        .filter(|summary| summary.id.starts_with(&prefix))
        .map(|summary| CandidateCard {
            preview_url: format!("/campaigns/{}/render", summary.id),
            ratio: summary.size.viewport().aspect_ratio_css(),
            id: summary.id,
            theme: summary.theme,
        })
        .collect())
}

/// Shows every candidate for `base` side by side with a choose button.
async fn chooser(
    State(stores): State<ArtifactStores>,
    State(sessions): State<SessionStore>,
    Path(base): Path<String>,
) -> Response {
    if !is_valid_design_id(&base) {
        return api_error::invalid_design_id(&base);
    }
    let kind = session_kind(&sessions, &base).await;
    let cards = match kind {
        ArtifactKind::Demo => design_cards(&stores.designs, &base).await,
        ArtifactKind::Deck => deck_cards(&stores.decks, &base).await,
        ArtifactKind::Document => document_cards(&stores.documents, &base).await,
        ArtifactKind::Social => social_cards(&stores.socials, &base).await,
        ArtifactKind::Print => print_cards(&stores.prints, &base).await,
        ArtifactKind::Mailing => mailing_cards(&stores.mailings, &base).await,
        ArtifactKind::Campaign => campaign_cards(&stores.campaigns, &base).await,
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
    cards.truncate(CARD_LIMIT);
    Html(chooser_page(&base, kind, &cards)).into_response()
}

/// Saves a copy of the chosen candidate as `base`. The candidates
/// stay, so the user can come back and pick another.
async fn choose(
    State(stores): State<ArtifactStores>,
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
        ArtifactKind::Demo => copy_design(&stores.designs, &request.id, &base).await,
        ArtifactKind::Deck => copy_deck(&stores.decks, &request.id, &base).await,
        ArtifactKind::Document => copy_document(&stores.documents, &request.id, &base).await,
        ArtifactKind::Social => copy_social(&stores.socials, &request.id, &base).await,
        ArtifactKind::Print => copy_print(&stores.prints, &request.id, &base).await,
        ArtifactKind::Mailing => copy_mailing(&stores.mailings, &request.id, &base).await,
        ArtifactKind::Campaign => copy_campaign(&stores.campaigns, &request.id, &base).await,
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
/// candidate replaces the design wholesale. A fork copies the same way.
pub(crate) async fn copy_design(store: &DesignStore, id: &str, base: &str) -> Result<(), Response> {
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
pub(crate) async fn copy_deck(store: &DeckStore, id: &str, base: &str) -> Result<(), Response> {
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

/// Copies the document `id` to `base`. The copy is agent-authored.
pub(crate) async fn copy_document(
    store: &DocumentStore,
    id: &str,
    base: &str,
) -> Result<(), Response> {
    let document = match store.load(id).await {
        Ok(Some(document)) => document,
        Ok(None) => return Err(api_error::document_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    store
        .save(base, &document)
        .await
        .map_err(|error| api_error::internal_error(&error))?;
    store
        .clear_user_paths(base)
        .await
        .map_err(|error| api_error::internal_error(&error))
}

/// Copies the social `id` to `base`. The copy is agent-authored.
pub(crate) async fn copy_social(store: &SocialStore, id: &str, base: &str) -> Result<(), Response> {
    let social = match store.load(id).await {
        Ok(Some(social)) => social,
        Ok(None) => return Err(api_error::social_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    store
        .save(base, &social)
        .await
        .map_err(|error| api_error::internal_error(&error))?;
    store
        .clear_user_paths(base)
        .await
        .map_err(|error| api_error::internal_error(&error))
}

/// Copies the print `id` to `base`. The copy is agent-authored.
pub(crate) async fn copy_print(store: &PrintStore, id: &str, base: &str) -> Result<(), Response> {
    let print = match store.load(id).await {
        Ok(Some(print)) => print,
        Ok(None) => return Err(api_error::print_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    store
        .save(base, &print)
        .await
        .map_err(|error| api_error::internal_error(&error))?;
    store
        .clear_user_paths(base)
        .await
        .map_err(|error| api_error::internal_error(&error))
}

/// Copies the mailing `id` to `base`. The copy is agent-authored.
pub(crate) async fn copy_mailing(
    store: &MailingStore,
    id: &str,
    base: &str,
) -> Result<(), Response> {
    let mailing = match store.load(id).await {
        Ok(Some(mailing)) => mailing,
        Ok(None) => return Err(api_error::mailing_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    store
        .save(base, &mailing)
        .await
        .map_err(|error| api_error::internal_error(&error))?;
    store
        .clear_user_paths(base)
        .await
        .map_err(|error| api_error::internal_error(&error))
}

/// Copies the campaign `id` to `base`. The copy is agent-authored.
pub(crate) async fn copy_campaign(
    store: &CampaignStore,
    id: &str,
    base: &str,
) -> Result<(), Response> {
    let campaign = match store.load(id).await {
        Ok(Some(campaign)) => campaign,
        Ok(None) => return Err(api_error::campaign_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    store
        .save(base, &campaign)
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
        ArtifactKind::Document => "document",
        ArtifactKind::Social => "social post",
        ArtifactKind::Print => "print piece",
        ArtifactKind::Mailing => "mailing",
        ArtifactKind::Campaign => "campaign",
    }
}

/// The page the browser opens after a choice: the chosen artifact's
/// render route.
fn chosen_url(kind: ArtifactKind, base: &str) -> String {
    match kind {
        ArtifactKind::Demo => format!("/designs/{base}/render"),
        ArtifactKind::Deck => format!("/decks/{base}/render"),
        ArtifactKind::Document => format!("/documents/{base}/render"),
        ArtifactKind::Social => format!("/socials/{base}/render"),
        ArtifactKind::Print => format!("/prints/{base}/render"),
        ArtifactKind::Mailing => format!("/mailings/{base}/render"),
        ArtifactKind::Campaign => format!("/campaigns/{base}/render"),
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

    use super::{
        CandidateCard, candidate_id, candidate_number_of, chooser_page, chosen_url, deck_cards,
        design_cards, document_cards, next_candidate_number, social_cards,
    };
    use axum::http::StatusCode;

    use crate::decks::DeckStore;
    use crate::designs::DesignStore;
    use crate::documents::DocumentStore;
    use crate::sessions::SessionStore;
    use crate::socials::SocialStore;
    use crate::test_support::{
        sample_deck, sample_document, sample_social, send, test_application,
    };

    #[tokio::test]
    async fn social_cards_carry_the_format_ratio_and_the_social_url() {
        let directory = tempfile::tempdir().unwrap();
        let socials = SocialStore::new(directory.path().join("socials"));
        socials
            .save("launch-candidate-1", &sample_social())
            .await
            .unwrap();
        let mut story = sample_social();
        story.format = design_model::Format::Story;
        socials.save("launch-candidate-2", &story).await.unwrap();
        let cards = social_cards(&socials, "launch").await.unwrap();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].preview_url, "/socials/launch-candidate-1/render");
        assert_eq!(cards[0].ratio, "1080 / 1350");
        assert_eq!(cards[1].ratio, "1080 / 1920");
        assert_eq!(
            chosen_url(design_model::ArtifactKind::Social, "launch"),
            "/socials/launch/render"
        );
    }

    #[tokio::test]
    async fn document_cards_carry_the_paper_ratio_and_the_document_url() {
        let directory = tempfile::tempdir().unwrap();
        let documents = DocumentStore::new(directory.path().join("documents"));
        documents
            .save("report-candidate-1", &sample_document())
            .await
            .unwrap();
        let mut letter = sample_document();
        letter.paper = design_model::Paper::Letter;
        documents.save("report-candidate-2", &letter).await.unwrap();
        let cards = document_cards(&documents, "report").await.unwrap();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].preview_url, "/documents/report-candidate-1/render");
        assert_eq!(cards[0].ratio, "794 / 1123");
        assert_eq!(cards[1].ratio, "816 / 1056");
        assert_eq!(
            chosen_url(design_model::ArtifactKind::Document, "report"),
            "/documents/report/render"
        );
    }

    #[tokio::test]
    async fn a_fork_takes_the_next_candidate_number_and_reads_as_agent_authored() {
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let design: design_model::Design =
            serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap();
        designs.save("talk-candidate-1", &design).await.unwrap();
        let mut edited = design.clone();
        edited.title = "Edited by hand".to_owned();
        designs.save("talk-candidate-3", &edited).await.unwrap();
        designs
            .record_authors("talk-candidate-3", Some(&design), &edited, true)
            .await
            .unwrap();
        assert!(
            !designs
                .user_paths("talk-candidate-3")
                .await
                .unwrap()
                .is_empty()
        );
        let application = test_application(&directory);
        let (status, body) = send(
            application.clone(),
            "POST",
            "/designs/talk-candidate-3/fork",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, r#"{"id":"talk-candidate-4"}"#);
        let copy = designs.load("talk-candidate-4").await.unwrap().unwrap();
        assert_eq!(copy.title, "Edited by hand");
        assert!(
            designs
                .user_paths("talk-candidate-4")
                .await
                .unwrap()
                .is_empty()
        );
        let (status, _) = send(
            application.clone(),
            "POST",
            "/designs/talk-candidate-9/fork",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_deck_fork_is_refused_while_the_session_generates() {
        let directory = tempfile::tempdir().unwrap();
        let decks = DeckStore::new(directory.path().join("decks"));
        decks
            .save("talk-candidate-1", &sample_deck())
            .await
            .unwrap();
        let application = test_application(&directory);
        let (status, body) = send(
            application.clone(),
            "POST",
            "/sessions",
            Some(r#"{"id":"talk","request":"A deck.","artifact_kind":"deck"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let (status, body) = send(
            application.clone(),
            "POST",
            "/decks/talk-candidate-1/fork",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, r#"{"id":"talk-candidate-2"}"#);
        // The state moves through the store, so no run starts.
        SessionStore::new(directory.path().join("data/sessions"))
            .apply("talk", design_model::WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        let (status, body) = send(
            application.clone(),
            "POST",
            "/decks/talk-candidate-1/fork",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(decks.load("talk-candidate-3").await.unwrap().is_none());
    }

    #[test]
    fn the_next_number_follows_the_highest_candidate() {
        let ids = [
            "talk-candidate-1",
            "talk-candidate-3",
            "other-candidate-9",
            "talk-candidate-x",
            "talk",
        ];
        assert_eq!(next_candidate_number("talk", ids.into_iter()), 4);
        assert_eq!(next_candidate_number("talk", std::iter::empty()), 1);
        assert_eq!(candidate_id("talk", 4), "talk-candidate-4");
        assert_eq!(candidate_number_of("talk-candidate-4"), Some(4));
        assert_eq!(candidate_number_of("talk-candidate-x"), None);
        assert_eq!(candidate_number_of("talk"), None);
    }

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
