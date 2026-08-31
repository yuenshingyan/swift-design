//! Projects: the id prefix that groups a chosen design, deck, document,
//! or social and its candidates. Renaming a project moves every
//! artifact in it.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use design_model::WorkflowState;
use serde::Deserialize;

use crate::api_error;
use crate::designs::is_valid_design_id;
use crate::events::ChangeNotifier;
use crate::session_routes::ArtifactStores;
use crate::sessions::SessionStore;

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

/// Moves every design, deck, document, and social of the project to
/// the new name and renames the session that owns it.
async fn rename_project(
    State(stores): State<ArtifactStores>,
    State(sessions): State<SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path(old): Path<String>,
    Json(request): Json<RenameRequest>,
) -> Response {
    let ArtifactStores {
        designs,
        decks,
        documents,
        socials,
    } = &stores;
    let new = request.name.trim().to_owned();
    if !is_valid_design_id(&old) {
        return api_error::invalid_design_id(&old);
    }
    // A session that is generating must not have its designs moved out
    // from under the run.
    if let Ok(Some(session)) = sessions.read(&old).await
        && session.state == WorkflowState::Generating
    {
        return api_error::error_response(
            StatusCode::CONFLICT,
            "cannot rename a project while its session is generating",
            Vec::new(),
        );
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
    let design_ids: Vec<String> = match designs.list().await {
        Ok(summaries) => summaries.into_iter().map(|design| design.id).collect(),
        Err(error) => return api_error::internal_error(&error),
    };
    let deck_ids: Vec<String> = match decks.list().await {
        Ok(summaries) => summaries.into_iter().map(|deck| deck.id).collect(),
        Err(error) => return api_error::internal_error(&error),
    };
    let document_ids: Vec<String> = match documents.list().await {
        Ok(summaries) => summaries.into_iter().map(|document| document.id).collect(),
        Err(error) => return api_error::internal_error(&error),
    };
    let social_ids: Vec<String> = match socials.list().await {
        Ok(summaries) => summaries.into_iter().map(|social| social.id).collect(),
        Err(error) => return api_error::internal_error(&error),
    };
    let ids = ProjectIds {
        designs: &design_ids,
        decks: &deck_ids,
        documents: &document_ids,
        socials: &social_ids,
    };
    let members = match project_members(&ids, &old, &new) {
        Ok(members) => members,
        Err((status, message)) => return api_error::error_response(status, &message, Vec::new()),
    };
    for id in &members.designs {
        if let Err(error) = designs.rename(id, &renamed_id(id, &old, &new)).await {
            return api_error::internal_error(&error);
        }
    }
    for id in &members.decks {
        if let Err(error) = decks.rename(id, &renamed_id(id, &old, &new)).await {
            return api_error::internal_error(&error);
        }
    }
    for id in &members.documents {
        if let Err(error) = documents.rename(id, &renamed_id(id, &old, &new)).await {
            return api_error::internal_error(&error);
        }
    }
    for id in &members.socials {
        if let Err(error) = socials.rename(id, &renamed_id(id, &old, &new)).await {
            return api_error::internal_error(&error);
        }
    }
    if let Err(error) = sessions.rename(&old, &new).await {
        return api_error::internal_error(&anyhow::anyhow!(error.to_string()));
    }
    notifier.notify();
    tracing::info!(
        %old,
        %new,
        moved = members.designs.len()
            + members.decks.len()
            + members.documents.len()
            + members.socials.len(),
        "project renamed"
    );
    Json(serde_json::json!({ "name": new })).into_response()
}

/// Every stored id, by store.
struct ProjectIds<'ids> {
    designs: &'ids [String],
    decks: &'ids [String],
    documents: &'ids [String],
    socials: &'ids [String],
}

/// The ids that move with a project, by store.
#[derive(Debug, Default, PartialEq, Eq)]
struct ProjectMembers {
    designs: Vec<String>,
    decks: Vec<String>,
    documents: Vec<String>,
    socials: Vec<String>,
}

/// The design, deck, document, and social ids that move with the
/// project `old`. Fails with a status and a message when `new` is
/// taken by any store, or when `old` has no members.
fn project_members(
    ids: &ProjectIds<'_>,
    old: &str,
    new: &str,
) -> Result<ProjectMembers, (StatusCode, String)> {
    let is_taken = ids
        .designs
        .iter()
        .chain(ids.decks)
        .chain(ids.documents)
        .chain(ids.socials)
        .any(|id| is_in_project(id, new));
    if is_taken {
        return Err((
            StatusCode::CONFLICT,
            format!("project `{new}` already exists: choose another name"),
        ));
    }
    let members = |ids: &[String]| -> Vec<String> {
        ids.iter()
            .filter(|id| is_in_project(id, old))
            .cloned()
            .collect()
    };
    let found = ProjectMembers {
        designs: members(ids.designs),
        decks: members(ids.decks),
        documents: members(ids.documents),
        socials: members(ids.socials),
    };
    if found.designs.is_empty()
        && found.decks.is_empty()
        && found.documents.is_empty()
        && found.socials.is_empty()
    {
        return Err((
            StatusCode::NOT_FOUND,
            format!("project `{old}` has no designs, decks, documents, or socials"),
        ));
    }
    Ok(found)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::projects::{ProjectIds, is_in_project, project_members, renamed_id};

    #[test]
    fn members_come_from_every_store_and_a_taken_name_is_refused() {
        let designs = vec!["talk-candidate-1".to_owned(), "other".to_owned()];
        let decks = vec!["talk".to_owned(), "talk-candidate-2".to_owned()];
        let documents = vec!["talk-candidate-3".to_owned(), "memo".to_owned()];
        let socials = vec!["talk-candidate-4".to_owned(), "launch".to_owned()];
        let ids = ProjectIds {
            designs: &designs,
            decks: &decks,
            documents: &documents,
            socials: &socials,
        };
        let members = project_members(&ids, "talk", "pitch").unwrap();
        assert_eq!(members.designs, ["talk-candidate-1"]);
        assert_eq!(members.decks, ["talk", "talk-candidate-2"]);
        assert_eq!(members.documents, ["talk-candidate-3"]);
        assert_eq!(members.socials, ["talk-candidate-4"]);
        assert!(project_members(&ids, "talk", "other").is_err());
        assert!(project_members(&ids, "talk", "memo").is_err());
        assert!(project_members(&ids, "talk", "launch").is_err());
        assert!(project_members(&ids, "missing", "pitch").is_err());
    }

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
