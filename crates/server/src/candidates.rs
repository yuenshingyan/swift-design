//! Candidate designs: several versions of one design, one final pick.
//!
//! Agents write up to five candidates as designs with ids
//! `{base}-candidate-1` … `{base}-candidate-5`. The chooser page shows
//! them side by side; choosing one saves a copy as `{base}`. The
//! candidates stay until the user deletes them, so the pick can change.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::api_error;
use crate::designs::{DesignStore, DesignSummary, is_valid_design_id};
use crate::events::ChangeNotifier;

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
    /// Id of the winning candidate design.
    id: String,
}

/// Ids of every stored candidate for `base`, sorted.
async fn candidate_ids(store: &DesignStore, base: &str) -> anyhow::Result<Vec<DesignSummary>> {
    let prefix = format!("{base}-candidate-");
    let summaries = store.list().await?;
    Ok(summaries
        .into_iter()
        .filter(|summary| summary.id.starts_with(&prefix))
        .collect())
}

/// Shows every candidate for `base` side by side with a choose button.
async fn chooser(State(store): State<DesignStore>, Path(base): Path<String>) -> Response {
    if !is_valid_design_id(&base) {
        return api_error::invalid_design_id(&base);
    }
    let mut candidates = match candidate_ids(&store, &base).await {
        Ok(candidates) => candidates,
        Err(error) => return api_error::internal_error(&error),
    };
    if candidates.is_empty() {
        return api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "no candidates for `{base}`: save designs named `{base}-candidate-1` and so on"
            ),
            Vec::new(),
        );
    }
    candidates.truncate(CANDIDATE_LIMIT);
    Html(chooser_page(&base, &candidates)).into_response()
}

/// Saves a copy of the chosen candidate as `base`. The candidates
/// stay, so the user can come back and pick another.
async fn choose(
    State(store): State<DesignStore>,
    State(sessions): State<crate::sessions::SessionStore>,
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
    let design = match store.load(&request.id).await {
        Ok(Some(design)) => design,
        Ok(None) => return api_error::design_not_found(&request.id),
        Err(error) => return api_error::internal_error(&error),
    };
    if let Err(error) = store.save(&base, &design).await {
        return api_error::internal_error(&error);
    }
    // The candidate replaces the design wholesale, so it is agent-authored.
    if let Err(error) = store.clear_user_paths(&base).await {
        return api_error::internal_error(&error);
    }
    // Record the choice on the owning session, when one exists.
    let _ = sessions
        .update(&base, |session| {
            session.chosen_design = Some(request.id.clone());
        })
        .await;
    notifier.notify();
    tracing::info!(%base, chosen = %request.id, "candidate chosen");
    Json(serde_json::json!({ "id": base })).into_response()
}

/// Builds the chooser HTML: one card per candidate with an iframe
/// preview and a choose button.
/// Candidate ids pass `is_valid_design_id`, so they are safe to embed.
/// Theme names come from design JSON and are escaped.
fn chooser_page(base: &str, candidates: &[DesignSummary]) -> String {
    let mut cards = String::new();
    for candidate in candidates {
        cards.push_str(&format!(
            "<article>\n\
             <iframe src=\"/designs/{id}/render\" title=\"{id}\" style=\"aspect-ratio: {ratio}\"></iframe>\n\
             <div class=\"card-footer\">\n\
             <span class=\"card-label\">{id} · {theme}</span>\n\
             <button data-id=\"{id}\">Choose this design</button>\n\
             </div>\n</article>\n",
            id = candidate.id,
            theme = crate::render::escape_html(&candidate.theme),
            ratio = candidate.viewport.aspect_ratio_css(),
        ));
    }
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Choose a design: {base}</title>\n<style>\n\
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
         <h1>Choose a design for <code>{base}</code></h1>\n\
         <p>Candidates written by your agent in one run. \
         Picking one saves it as <code>designs/{base}.json</code>.</p>\n\
         </div>\n<span class=\"design-count\">{count} candidates</span>\n</header>\n\
         <main>\n{cards}</main>\n<script>\n\
         document.querySelectorAll('button[data-id]').forEach((button) => {{\n\
           button.addEventListener('click', async () => {{\n\
             const response = await fetch('/candidates/{base}/choose', {{\n\
               method: 'POST',\n\
               headers: {{ 'content-type': 'application/json' }},\n\
               body: JSON.stringify({{ id: button.dataset.id }}),\n\
             }});\n\
             if (response.ok) {{ window.location = '/designs/{base}/render'; }}\n\
           }});\n\
         }});\n</script>\n</body>\n</html>\n",
        count = candidates.len(),
    )
}
