use super::*;

// -- Agent runs ----------------------------------------------------------

/// State of the run, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AgentRun {
    /// True while the run is active.
    pub is_running: bool,
    /// Exit code of the last run.
    pub exit_code: Option<i32>,
    /// Tail of the run's output.
    pub log_tail: String,
    /// Label of the running (or last) model or command.
    #[serde(default)]
    pub active_agent: Option<String>,
    /// The session the run is for.
    #[serde(default)]
    pub session_id: Option<String>,
    /// `briefing` or `generation`.
    #[serde(default)]
    pub mode: Option<String>,
    /// Input tokens of the latest request.
    #[serde(default)]
    pub context_tokens: u64,
    /// Input plus output tokens over the run.
    #[serde(default)]
    pub total_tokens: u64,
    /// Context window of the running model.
    #[serde(default)]
    pub context_window: u64,
    /// How far the current turn is, 0 to 100.
    #[serde(default)]
    pub progress: Option<u8>,
    /// How far each design the turn writes is, by id.
    #[serde(default)]
    pub designs: HashMap<String, u8>,
}

/// Body of `POST /agent-runs` and `DELETE /agent-runs`.
#[derive(Serialize)]
struct RunRequest<'value> {
    session_id: &'value str,
}

/// Fetches the state of the run.
pub async fn fetch_agent_run() -> Result<AgentRun, String> {
    get_json("/agent-runs").await
}

/// Starts a run for the named session.
pub async fn start_agent_run(session_id: &str) -> Result<(), String> {
    let builder = Request::post("/agent-runs")
        .json(&RunRequest { session_id })
        .map_err(|error| error.to_string())?;
    send_checked(builder, "POST /agent-runs").await.map(|_| ())
}

/// Stops the active run.
pub async fn stop_agent_run() -> Result<(), String> {
    send_empty(Request::delete("/agent-runs"), "DELETE /agent-runs").await
}

// -- Settings ------------------------------------------------------------

/// One curated model of the catalog.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CatalogModel {
    /// Model id.
    pub id: String,
    /// One short line telling the user when to pick it.
    pub description: String,
    /// True for the model the setup panel selects first.
    pub is_recommended: bool,
}

/// One provider of the catalog.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CatalogProvider {
    /// Provider name.
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

/// The current model choice.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CurrentSettings {
    /// Chosen provider name.
    pub provider: String,
    /// Chosen model identifier.
    pub model: String,
    /// How it authenticates.
    pub auth: String,
}

/// Response of `GET /settings`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SettingsView {
    /// The model catalog.
    pub providers: Vec<CatalogProvider>,
    /// The current choice, when one was made.
    pub current: Option<CurrentSettings>,
    /// True when the server found Chrome.
    #[serde(default)]
    pub has_chrome: bool,
}

/// Fetches the model catalog and the current choice.
pub async fn fetch_settings() -> Result<SettingsView, String> {
    get_json("/settings").await
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
    let builder = Request::put("/settings")
        .json(&SettingsRequest {
            provider,
            model,
            api_key,
        })
        .map_err(|error| error.to_string())?;
    send_checked(builder, "PUT /settings").await.map(|_| ())
}

/// Body of `POST /settings/models`.
#[derive(Debug, Serialize)]
struct ModelsRequest<'value> {
    provider: &'value str,
    api_key: Option<&'value str>,
}

/// Lists the provider's live models.
pub async fn fetch_provider_models(
    provider: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>, String> {
    let builder = Request::post("/settings/models")
        .json(&ModelsRequest { provider, api_key })
        .map_err(|error| error.to_string())?;
    let response = send_checked(builder, "POST /settings/models").await?;
    response.json().await.map_err(|error| error.to_string())
}

/// Response of the login-start routes.
#[derive(Debug, Deserialize)]
struct LoginStartResponse {
    authorize_url: String,
}

/// Starts a login and returns the URL to open.
async fn start_login_at(route: &str) -> Result<String, String> {
    let response = send_checked(built(Request::post(route))?, route).await?;
    response
        .json::<LoginStartResponse>()
        .await
        .map(|start| start.authorize_url)
        .map_err(|error| error.to_string())
}

/// Starts a Claude login.
pub async fn start_login() -> Result<String, String> {
    start_login_at("/settings/login/start").await
}

/// Starts an OpenRouter login.
pub async fn start_openrouter_login() -> Result<String, String> {
    start_login_at("/settings/login/openrouter/start").await
}

/// Starts a ChatGPT login.
pub async fn start_openai_login() -> Result<String, String> {
    start_login_at("/settings/login/openai/start").await
}

/// Body of `POST /settings/login/complete`.
#[derive(Debug, Serialize)]
struct LoginCompleteRequest<'value> {
    code: &'value str,
    model: Option<&'value str>,
}

/// Completes a Claude login with the pasted code.
pub async fn complete_login(code: &str, model: Option<&str>) -> Result<(), String> {
    let builder = Request::post("/settings/login/complete")
        .json(&LoginCompleteRequest { code, model })
        .map_err(|error| error.to_string())?;
    send_checked(builder, "POST /settings/login/complete")
        .await
        .map(|_| ())
}
