//! HTTP calls to the Swift Design server.

use std::collections::HashMap;

use design_model::Design;
use serde::{Deserialize, Serialize};

/// One row of `GET /designs`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DesignSummary {
    /// Design id used in `/designs/{id}` routes.
    pub id: String,
    /// Design title.
    pub title: String,
    /// Theme name.
    pub theme: String,
    /// Number of screens.
    pub screen_count: usize,
    /// Number of titles in the planned outline. More than `screen_count`
    /// for a preview design; 0 when the design has no outline.
    #[serde(default)]
    pub outline_count: usize,
    /// Number of placeholder screens a running or stopped continuation
    /// left in the design. 0 for a finished design.
    #[serde(default)]
    pub pending_count: usize,
}

impl DesignSummary {
    /// True when the design is a preview that waits for its remaining
    /// screens.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.screen_count
    }

    /// True when the design still owes screens: a preview, or a design a
    /// stopped run left with placeholders.
    pub fn is_unfinished(&self) -> bool {
        self.is_preview() || self.pending_count > 0
    }
}

/// The server's error envelope.
#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

/// Inner part of the error envelope.
#[derive(Debug, Deserialize)]
struct ErrorBody {
    message: String,
    #[serde(default)]
    details: Vec<String>,
}

/// Fetches the design listing.
pub async fn fetch_design_list() -> Result<Vec<DesignSummary>, String> {
    let response = gloo_net::http::Request::get("/designs")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!(
            "GET /designs failed with status {}",
            response.status()
        ));
    }
    response.json().await.map_err(|error| error.to_string())
}

/// Fetches one design.
pub async fn fetch_design(id: &str) -> Result<Design, String> {
    let response = gloo_net::http::Request::get(&format!("/designs/{id}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!(
            "GET /designs/{id} failed with status {}",
            response.status()
        ));
    }
    response.json().await.map_err(|error| error.to_string())
}

/// Body of `PUT /briefs`, mirrored from the server.
#[derive(Debug, Serialize)]
pub struct BriefRequest<'value> {
    /// What the design should cover, in the user's words.
    pub prompt: &'value str,
    /// Project name that groups this brief's designs.
    pub project: Option<&'value str>,
    /// How hard to work: `low`, `medium`, or `high`.
    pub effort: &'value str,
    /// True to write preview candidates first.
    pub preview: bool,
    /// The templates the candidates follow. Candidates take one look
    /// each, in order, and wrap. Empty writes a new look for every
    /// candidate.
    pub templates: &'value [String],
}

/// Writes the design brief for the agent to pick up.
pub async fn save_brief(request: &BriefRequest<'_>) -> Result<(), String> {
    let response = gloo_net::http::Request::put("/briefs")
        .json(request)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) => Err(envelope.error.message),
        Err(_) => Err(format!("PUT /briefs failed with status {status}")),
    }
}

/// The CSS class the server puts on a placeholder screen, kept while a
/// continuation writes the real one.
const PENDING_SCREEN_CLASS: &str = "swift-design-pending";

/// How many screens of the design are placeholders a run has not replaced.
pub fn pending_screen_count(design: &Design) -> usize {
    design
        .screens
        .iter()
        .filter(|screen| screen.html.contains(PENDING_SCREEN_CLASS))
        .count()
}

/// One row of `GET /templates`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TemplateSummary {
    /// Template id used in `/templates/{id}` routes.
    pub id: String,
    /// The name the user gave the template.
    pub name: String,
    /// When the template was saved, RFC 3339.
    pub saved_at: String,
    /// Theme name.
    pub theme: String,
    /// How many example screens the template holds.
    pub screen_count: usize,
}

/// Body of `POST /templates`.
#[derive(Debug, Serialize)]
struct SaveTemplateRequest<'value> {
    design_id: &'value str,
    name: &'value str,
}

/// Fetches the saved templates, newest first.
pub async fn fetch_templates() -> Result<Vec<TemplateSummary>, String> {
    let response = gloo_net::http::Request::get("/templates")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!(
            "GET /templates failed with status {}",
            response.status()
        ));
    }
    response.json().await.map_err(|error| error.to_string())
}

/// Saves the style of one design as a template.
pub async fn save_template(design_id: &str, name: &str) -> Result<TemplateSummary, String> {
    let response = gloo_net::http::Request::post("/templates")
        .json(&SaveTemplateRequest { design_id, name })
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return response.json().await.map_err(|error| error.to_string());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) => Err(envelope.error.message),
        Err(_) => Err(format!("POST /templates failed with status {status}")),
    }
}

/// Deletes one template.
pub async fn delete_template(id: &str) -> Result<(), String> {
    let response = gloo_net::http::Request::delete(&format!("/templates/{id}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return Ok(());
    }
    Err(format!(
        "DELETE /templates/{id} failed with status {}",
        response.status()
    ))
}

/// One row of `GET /uploads`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct UploadSummary {
    /// Stored file name, used in `/uploads/{name}`.
    pub name: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Content type from the extension.
    #[serde(default)]
    pub content_type: String,
    /// True for images: screens can use the file as an image source.
    #[serde(default)]
    pub is_image: bool,
}

/// Fetches the upload listing, sorted by name.
pub async fn fetch_uploads() -> Result<Vec<UploadSummary>, String> {
    let response = gloo_net::http::Request::get("/uploads")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!(
            "GET /uploads failed with status {}",
            response.status()
        ));
    }
    response.json().await.map_err(|error| error.to_string())
}

/// Deletes one upload.
pub async fn delete_upload(name: &str) -> Result<(), String> {
    let response = gloo_net::http::Request::delete(&format!("/uploads/{name}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return Ok(());
    }
    Err(format!(
        "DELETE /uploads/{name} failed with status {}",
        response.status()
    ))
}

/// Response of `GET /designs/{id}/authors`.
#[derive(Debug, Deserialize)]
struct AuthorsResponse {
    user_paths: Vec<String>,
}

/// Fetches the field paths the user changed in this design.
pub async fn fetch_user_paths(id: &str) -> Result<Vec<String>, String> {
    let response = gloo_net::http::Request::get(&format!("/designs/{id}/authors"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!(
            "GET /designs/{id}/authors failed with status {}",
            response.status()
        ));
    }
    response
        .json::<AuthorsResponse>()
        .await
        .map(|authors| authors.user_paths)
        .map_err(|error| error.to_string())
}

/// The stored brief, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Brief {
    /// What the design should cover, in the user's words.
    pub prompt: String,
    /// What scenario the design is for. Unset until the user answers the
    /// scenario question.
    #[serde(default)]
    pub scenario: Option<String>,
    /// The target screen count as `min-max`, or `any`. Unset until the
    /// user answers the length question.
    #[serde(default)]
    pub length: Option<String>,
    /// How many candidate designs the agent should write. Unset until the
    /// user answers the count question.
    #[serde(default)]
    pub variations: Option<usize>,
    /// Project name that groups this brief's designs.
    #[serde(default)]
    pub project: Option<String>,
    /// How hard to work: `low`, `medium`, or `high`.
    #[serde(default)]
    pub effort: String,
    /// True when candidates are written as previews first.
    #[serde(default = "default_preview")]
    pub preview: bool,
    /// How different the candidates are: `low`, `medium`, or `high`.
    /// Unset until the user answers the variety question.
    #[serde(default)]
    pub variety: Option<String>,
    /// The user's answers to the agent's questions.
    #[serde(default)]
    pub answers: Vec<BriefAnswer>,
    /// The conversation after the prompt, oldest first.
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
}

/// One conversation turn, mirrored from the server.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    /// `user` or `assistant`.
    pub role: String,
    /// The turn text.
    pub content: String,
    /// The design the turn is about, when the user sent it from the
    /// editor or a card.
    #[serde(default)]
    pub design: Option<String>,
    /// `continue` when the turn asks for the rest of a preview design.
    #[serde(default)]
    pub action: Option<String>,
}

/// The preview flag when the server does not state one.
fn default_preview() -> bool {
    true
}

/// Body of `POST /briefs/messages`.
#[derive(Serialize)]
struct MessageRequest<'a> {
    content: &'a str,
    /// The design open in the editor, when the message is about one.
    #[serde(skip_serializing_if = "Option::is_none")]
    design: Option<&'a str>,
    /// `continue` to ask for the remaining screens of a preview design.
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<&'a str>,
}

/// Asks the engine to write the remaining screens of the preview design
/// `design_id`: a user turn with the `continue` action. The caller starts
/// the run.
pub async fn continue_design(design_id: &str) -> Result<(), String> {
    let content = format!("Continue `{design_id}`: write the remaining screens.");
    post_message(&MessageRequest {
        content: &content,
        design: Some(design_id),
        action: Some("continue"),
    })
    .await
}

/// Body of `POST /projects/{name}/rename`.
#[derive(Serialize)]
struct RenameRequest<'a> {
    name: &'a str,
}

/// Reply of `POST /projects/{name}/rename`.
#[derive(Deserialize)]
struct RenameReply {
    name: String,
}

/// Renames a project: every design in it moves to the new name. Returns
/// the new name.
pub async fn rename_project(old: &str, new: &str) -> Result<String, String> {
    let response = gloo_net::http::Request::post(&format!("/projects/{old}/rename"))
        .json(&RenameRequest { name: new })
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return response
            .json::<RenameReply>()
            .await
            .map(|reply| reply.name)
            .map_err(|error| error.to_string());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) => Err(envelope.error.message),
        Err(_) => Err(format!(
            "POST /projects/{old}/rename failed with status {status}"
        )),
    }
}

/// Appends a user turn to the conversation.
pub async fn send_message(content: &str, design: Option<&str>) -> Result<(), String> {
    post_message(&MessageRequest {
        content,
        design,
        action: None,
    })
    .await
}

/// Posts one turn to `/briefs/messages`.
async fn post_message(request: &MessageRequest<'_>) -> Result<(), String> {
    let response = gloo_net::http::Request::post("/briefs/messages")
        .json(request)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) => Err(envelope.error.message),
        Err(_) => Err(format!("POST /briefs/messages failed with status {status}")),
    }
}

/// One answered question, kept with the brief.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct BriefAnswer {
    /// The question text.
    pub question: String,
    /// The chosen answer.
    pub answer: String,
}

/// Fetches the current brief. `Ok(None)` means none exists.
pub async fn fetch_brief() -> Result<Option<Brief>, String> {
    let response = gloo_net::http::Request::get("/briefs")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status() == 404 {
        return Ok(None);
    }
    if !response.ok() {
        return Err(format!(
            "GET /briefs failed with status {}",
            response.status()
        ));
    }
    response
        .json()
        .await
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Response of `GET /events`.
#[derive(Debug, Deserialize)]
struct EventsResponse {
    revision: u64,
}

/// Waits up to 25 seconds for the server revision to move past `after`
/// and returns the current revision.
pub async fn wait_for_change(after: u64) -> Result<u64, String> {
    let response = gloo_net::http::Request::get(&format!("/events?after={after}&wait=25"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!(
            "GET /events failed with status {}",
            response.status()
        ));
    }
    response
        .json::<EventsResponse>()
        .await
        .map(|events| events.revision)
        .map_err(|error| error.to_string())
}

/// State of the generation run, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AgentRun {
    /// True while the run is active.
    pub is_running: bool,
    /// Exit code of the last run. `None` while running or after a kill.
    pub exit_code: Option<i32>,
    /// Tail of the run's output.
    pub log_tail: String,
    /// Label of the running (or last) model or command.
    #[serde(default)]
    pub active_agent: Option<String>,
    /// Input tokens of the latest model request: the live context size.
    #[serde(default)]
    pub context_tokens: u64,
    /// Input plus output tokens over the whole run.
    #[serde(default)]
    pub total_tokens: u64,
    /// Context window of the running model, in tokens. 0 when unknown.
    #[serde(default)]
    pub context_window: u64,
    /// How far the current turn is, 0 to 100, when the engine reports
    /// it.
    #[serde(default)]
    pub progress: Option<u8>,
    /// How far each design the turn writes is, 0 to 100, by design id.
    #[serde(default)]
    pub designs: HashMap<String, u8>,
}

/// One curated model of the catalog, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CatalogModel {
    /// Model id sent to the provider.
    pub id: String,
    /// One short line that tells the user when to pick this model.
    pub description: String,
    /// True for the model the setup panel selects first.
    pub is_recommended: bool,
}

/// One provider of the model catalog, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CatalogProvider {
    /// Provider name. This is the id used by the API and by settings.
    pub name: String,
    /// Provider name as the setup panel shows it.
    pub label: String,
    /// Curated model choices.
    pub models: Vec<CatalogModel>,
    /// True when the provider needs an API key.
    pub needs_api_key: bool,
    /// True when the provider supports the login flow.
    pub supports_login: bool,
}

/// The current model choice, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CurrentSettings {
    /// Chosen provider name.
    pub provider: String,
    /// Chosen model identifier.
    pub model: String,
    /// How it authenticates: `api_key`, `login`, or `none`.
    pub auth: String,
}

/// Response of `GET /settings`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SettingsView {
    /// The model catalog.
    pub providers: Vec<CatalogProvider>,
    /// The current choice, when one was made.
    pub current: Option<CurrentSettings>,
    /// True when the server found Chrome or Chromium, so screen images
    /// and PDF export work.
    #[serde(default)]
    pub has_chrome: bool,
}

/// Fetches the model catalog and the current choice.
pub async fn fetch_settings() -> Result<SettingsView, String> {
    let response = gloo_net::http::Request::get("/settings")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!(
            "GET /settings failed with status {}",
            response.status()
        ));
    }
    response.json().await.map_err(|error| error.to_string())
}

/// Body of `PUT /settings`.
#[derive(Debug, Serialize)]
struct SettingsRequest<'value> {
    provider: &'value str,
    model: &'value str,
    api_key: Option<&'value str>,
}

/// Saves the model choice with an optional API key.
pub async fn save_settings(
    provider: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<(), String> {
    let body = SettingsRequest {
        provider,
        model,
        api_key,
    };
    let response = gloo_net::http::Request::put("/settings")
        .json(&body)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) => Err(envelope.error.message),
        Err(_) => Err(format!("PUT /settings failed with status {status}")),
    }
}

/// Response of the login-start routes.
#[derive(Debug, Deserialize)]
struct LoginStartResponse {
    authorize_url: String,
}

/// Starts a login and returns the URL to open. `route` is the
/// provider's start route.
async fn start_login_at(route: &str) -> Result<String, String> {
    let response = gloo_net::http::Request::post(route)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!(
            "POST {route} failed with status {}",
            response.status()
        ));
    }
    response
        .json::<LoginStartResponse>()
        .await
        .map(|start| start.authorize_url)
        .map_err(|error| error.to_string())
}

/// Starts a Claude login and returns the URL to open.
pub async fn start_login() -> Result<String, String> {
    start_login_at("/settings/login/start").await
}

/// Starts an OpenRouter login and returns the URL to open. The login
/// finishes on the server's callback route; nothing to paste.
pub async fn start_openrouter_login() -> Result<String, String> {
    start_login_at("/settings/login/openrouter/start").await
}

/// Starts a ChatGPT login and returns the URL to open. The login
/// finishes on a local callback; nothing to paste.
pub async fn start_openai_login() -> Result<String, String> {
    start_login_at("/settings/login/openai/start").await
}

/// One row of `GET /designs/{id}/history`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct HistorySnapshot {
    /// File stem of the snapshot, used in the restore route.
    pub stamp: String,
    /// When the snapshot was taken, RFC 3339.
    pub saved_at: String,
    /// Size of the snapshot file.
    pub size_bytes: u64,
}

/// Fetches the saved snapshots of one design, newest first.
pub async fn fetch_design_history(id: &str) -> Result<Vec<HistorySnapshot>, String> {
    let response = gloo_net::http::Request::get(&format!("/designs/{id}/history"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!(
            "GET /designs/{id}/history failed with status {}",
            response.status()
        ));
    }
    response.json().await.map_err(|error| error.to_string())
}

/// Writes one snapshot back as the current design.
pub async fn restore_design_history(id: &str, stamp: &str) -> Result<(), String> {
    let response = gloo_net::http::Request::post(&format!("/designs/{id}/history/{stamp}/restore"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) => Err(envelope.error.message),
        Err(_) => Err(format!(
            "POST /designs/{id}/history/{stamp}/restore failed with status {status}"
        )),
    }
}

/// Deletes one design.
pub async fn delete_design(id: &str) -> Result<(), String> {
    let response = gloo_net::http::Request::delete(&format!("/designs/{id}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return Ok(());
    }
    Err(format!(
        "DELETE /designs/{id} failed with status {}",
        response.status()
    ))
}

/// Body of `POST /settings/models`.
#[derive(Debug, Serialize)]
struct ModelsRequest<'value> {
    provider: &'value str,
    api_key: Option<&'value str>,
}

/// Lists the provider's live models with the given (or stored) key.
pub async fn fetch_provider_models(
    provider: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>, String> {
    let body = ModelsRequest { provider, api_key };
    let response = gloo_net::http::Request::post("/settings/models")
        .json(&body)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return response.json().await.map_err(|error| error.to_string());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) => Err(envelope.error.message),
        Err(_) => Err(format!("POST /settings/models failed with status {status}")),
    }
}

/// Body of `POST /settings/login/complete`.
#[derive(Debug, Serialize)]
struct LoginCompleteRequest<'value> {
    code: &'value str,
    model: Option<&'value str>,
}

/// Completes a Claude login with the pasted code.
pub async fn complete_login(code: &str, model: Option<&str>) -> Result<(), String> {
    let body = LoginCompleteRequest { code, model };
    let response = gloo_net::http::Request::post("/settings/login/complete")
        .json(&body)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) => Err(envelope.error.message),
        Err(_) => Err(format!(
            "POST /settings/login/complete failed with status {status}"
        )),
    }
}

/// Fetches the state of the agent run.
pub async fn fetch_agent_run() -> Result<AgentRun, String> {
    let response = gloo_net::http::Request::get("/agent-runs")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!(
            "GET /agent-runs failed with status {}",
            response.status()
        ));
    }
    response.json().await.map_err(|error| error.to_string())
}

/// Starts a generation run with the current settings.
pub async fn start_agent_run() -> Result<(), String> {
    let response = gloo_net::http::Request::post("/agent-runs")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) => Err(envelope.error.message),
        Err(_) => Err(format!("POST /agent-runs failed with status {status}")),
    }
}

/// Stops the agent subprocess.
pub async fn stop_agent_run() -> Result<(), String> {
    let response = gloo_net::http::Request::delete("/agent-runs")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return Ok(());
    }
    Err(format!(
        "DELETE /agent-runs failed with status {}",
        response.status()
    ))
}

/// One agent question, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Question {
    /// The question text.
    pub question: String,
    /// Preset answers the user can pick.
    #[serde(default)]
    pub options: Vec<String>,
}

/// Fetches the agent's open questions. `Ok(None)` means none are open.
pub async fn fetch_questions() -> Result<Option<Vec<Question>>, String> {
    let response = gloo_net::http::Request::get("/questions")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status() == 404 {
        return Ok(None);
    }
    if !response.ok() {
        return Err(format!(
            "GET /questions failed with status {}",
            response.status()
        ));
    }
    response
        .json()
        .await
        .map(Some)
        .map_err(|error| error.to_string())
}

/// One answered question in `POST /questions/answers`.
#[derive(Debug, Serialize)]
struct AnswerRequest {
    question: String,
    answer: String,
}

/// Body of `POST /questions/answers`.
#[derive(Debug, Serialize)]
struct AnswersRequest {
    answers: Vec<AnswerRequest>,
}

/// Sends the user's answers; the server appends them to the brief.
pub async fn send_answers(answers: Vec<(String, String)>) -> Result<(), String> {
    let body = AnswersRequest {
        answers: answers
            .into_iter()
            .map(|(question, answer)| AnswerRequest { question, answer })
            .collect(),
    };
    let response = gloo_net::http::Request::post("/questions/answers")
        .json(&body)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) => Err(envelope.error.message),
        Err(_) => Err(format!(
            "POST /questions/answers failed with status {status}"
        )),
    }
}

/// Saves one design as a user edit, so changed fields get a `you` badge.
/// `Err` carries one message per problem, so the editor can show every
/// validation error at once.
pub async fn save_design(id: &str, design: &Design) -> Result<(), Vec<String>> {
    let response = gloo_net::http::Request::put(&format!("/designs/{id}"))
        .header("x-swift-design-author", "user")
        .json(design)
        .map_err(|error| vec![error.to_string()])?
        .send()
        .await
        .map_err(|error| vec![error.to_string()])?;
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) if !envelope.error.details.is_empty() => Err(envelope.error.details),
        Ok(envelope) => Err(vec![envelope.error.message]),
        Err(_) => Err(vec![format!(
            "PUT /designs/{id} failed with status {status}"
        )]),
    }
}
