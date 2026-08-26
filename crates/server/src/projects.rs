//! Projects: the design id prefix that groups a chosen design and its
//! candidates. Renaming a project moves every design in it.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;

use crate::api_error;
use crate::briefs::BriefStore;
use crate::designs::{DesignStore, is_valid_design_id};
use crate::events::ChangeNotifier;

/// Body of `POST /projects/{name}/rename`.
#[derive(Debug, Deserialize)]
struct RenameRequest {
    /// The new project name: kebab-case, without `-candidate-`.
    name: String,
}

/// The `/projects` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new().route("/projects/{name}/rename", post(rename_project))
}

/// True when `id` is the project's chosen design or one of its candidates.
pub fn is_in_project(id: &str, project: &str) -> bool {
    id == project || id.starts_with(&format!("{project}-candidate-"))
}

/// The design id after a project rename.
fn renamed_id(id: &str, old: &str, new: &str) -> String {
    format!("{new}{}", &id[old.len()..])
}

/// Moves every design of the project to the new name and updates the
/// brief when it points at the project.
async fn rename_project(
    State(designs): State<DesignStore>,
    State(briefs): State<BriefStore>,
    State(notifier): State<ChangeNotifier>,
    Path(old): Path<String>,
    Json(request): Json<RenameRequest>,
) -> Response {
    let new = request.name.trim().to_owned();
    if !is_valid_design_id(&old) {
        return api_error::invalid_design_id(&old);
    }
    if !is_valid_design_id(&new) || new.contains("-candidate-") {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("project `{new}` is not a valid name: use kebab-case without `-candidate-`"),
            Vec::new(),
        );
    }
    if new == old {
        return Json(serde_json::json!({ "name": new })).into_response();
    }
    let summaries = match designs.list().await {
        Ok(summaries) => summaries,
        Err(error) => return api_error::internal_error(&error),
    };
    if summaries
        .iter()
        .any(|design| is_in_project(&design.id, &new))
    {
        return api_error::error_response(
            StatusCode::CONFLICT,
            &format!("project `{new}` already exists: choose another name"),
            Vec::new(),
        );
    }
    let members: Vec<String> = summaries
        .into_iter()
        .map(|design| design.id)
        .filter(|id| is_in_project(id, &old))
        .collect();
    if members.is_empty() {
        return api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!("project `{old}` has no designs"),
            Vec::new(),
        );
    }
    for id in &members {
        if let Err(error) = designs.rename(id, &renamed_id(id, &old, &new)).await {
            return api_error::internal_error(&error);
        }
    }
    if let Err(error) = briefs.rename_project(&old, &new).await {
        return api_error::internal_error(&error);
    }
    notifier.notify();
    tracing::info!(%old, %new, moved = members.len(), "project renamed");
    Json(serde_json::json!({ "name": new })).into_response()
}

#[cfg(test)]
mod tests {
    use crate::projects::{is_in_project, renamed_id};

    #[test]
    fn membership_matches_the_chosen_design_and_candidates_only() {
        assert!(is_in_project("talk", "talk"));
        assert!(is_in_project("talk-candidate-2", "talk"));
        assert!(!is_in_project("talk-2", "talk"));
        assert!(!is_in_project("talks", "talk"));
    }

    #[test]
    fn renamed_ids_keep_the_candidate_suffix() {
        assert_eq!(renamed_id("talk", "talk", "pitch"), "pitch");
        assert_eq!(
            renamed_id("talk-candidate-3", "talk", "pitch"),
            "pitch-candidate-3"
        );
    }
}
