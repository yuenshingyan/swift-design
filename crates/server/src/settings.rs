//! Model settings: the provider, model, and credentials the user picks
//! in the studio.
//!
//! The onboarding flow, like pi's: `GET /settings` lists providers and
//! their models; the user picks one and authenticates with an API key
//! (`PUT /settings`) or with a Claude login
//! (`POST /settings/login/start` then `/settings/login/complete`, the
//! same OAuth PKCE flow pi and opencode use). The choice persists in
//! one JSON file, private to this machine. Credentials are always the
//! user's own.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::api_error;
use crate::events::ChangeNotifier;
use crate::export::base64_encode;

/// The OAuth client id of the Claude Code/pi/opencode public client.
const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// Where the pasted login code is exchanged for tokens.
const ANTHROPIC_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";

/// The redirect target that shows the user the code to paste.
const ANTHROPIC_REDIRECT_URL: &str = "https://console.anthropic.com/oauth/code/callback";

/// One model choice of the catalog.
#[derive(Debug, Serialize)]
pub struct CatalogModel {
    /// Model id sent to the provider.
    pub id: &'static str,
    /// One short line that tells the user when to pick this model.
    pub description: &'static str,
    /// True for the model the setup panel selects first.
    pub is_recommended: bool,
}

/// One provider entry of the model catalog.
#[derive(Debug, Serialize)]
pub struct CatalogProvider {
    /// Provider name. This is the id used by the API and by settings.
    pub name: &'static str,
    /// Provider name as the setup panel shows it.
    pub label: &'static str,
    /// Curated model choices. The studio also accepts a custom name.
    pub models: &'static [CatalogModel],
    /// True when the provider needs an API key.
    pub needs_api_key: bool,
    /// True when the provider supports the login flow.
    pub supports_login: bool,
}

/// The model catalog shown by the studio.
pub const CATALOG: &[CatalogProvider] = &[
    CatalogProvider {
        name: "google",
        label: "Google",
        models: &[
            CatalogModel {
                id: "gemini-2.5-pro",
                description: "Best structure and copy. Slower, higher cost.",
                is_recommended: true,
            },
            CatalogModel {
                id: "gemini-2.5-flash",
                description: "Fast drafts and quick edits.",
                is_recommended: false,
            },
            CatalogModel {
                id: "gemini-2.0-flash",
                description: "Cheapest. Weaker on long designs.",
                is_recommended: false,
            },
        ],
        needs_api_key: true,
        supports_login: false,
    },
    CatalogProvider {
        name: "anthropic",
        label: "Anthropic",
        models: &[
            CatalogModel {
                id: "claude-sonnet-5",
                description: "Best balance of quality and speed.",
                is_recommended: true,
            },
            CatalogModel {
                id: "claude-opus-5",
                description: "Strongest reasoning. Slower, higher cost.",
                is_recommended: false,
            },
            CatalogModel {
                id: "claude-sonnet-4-6",
                description: "The previous Sonnet. Use it for a known result.",
                is_recommended: false,
            },
            CatalogModel {
                id: "claude-haiku-4-5",
                description: "Cheapest. Weaker on long designs.",
                is_recommended: false,
            },
        ],
        needs_api_key: true,
        supports_login: true,
    },
    CatalogProvider {
        name: "openai",
        label: "OpenAI",
        models: &[
            CatalogModel {
                id: "gpt-5.5",
                description: "Best structure and copy. Slower, higher cost.",
                is_recommended: true,
            },
            CatalogModel {
                id: "gpt-5.6-sol",
                description: "Newer model. Test it on one design first.",
                is_recommended: false,
            },
            CatalogModel {
                id: "gpt-5.6-luna",
                description: "Newer model. Test it on one design first.",
                is_recommended: false,
            },
            CatalogModel {
                id: "gpt-5.6-terra",
                description: "Newer model. Test it on one design first.",
                is_recommended: false,
            },
            CatalogModel {
                id: "gpt-5.4",
                description: "The previous model. Use it for a known result.",
                is_recommended: false,
            },
            CatalogModel {
                id: "gpt-5.4-mini",
                description: "Cheapest. Weaker on long designs.",
                is_recommended: false,
            },
        ],
        needs_api_key: true,
        supports_login: true,
    },
    CatalogProvider {
        name: "groq",
        label: "Groq",
        models: &[
            CatalogModel {
                id: "llama-3.3-70b-versatile",
                description: "Fast and good enough for a full design.",
                is_recommended: true,
            },
            CatalogModel {
                id: "openai/gpt-oss-120b",
                description: "Larger open model. Slower than Llama.",
                is_recommended: false,
            },
        ],
        needs_api_key: true,
        supports_login: false,
    },
    CatalogProvider {
        name: "openrouter",
        label: "OpenRouter",
        models: &[
            CatalogModel {
                id: "google/gemini-2.5-flash",
                description: "Fast drafts and quick edits.",
                is_recommended: true,
            },
            CatalogModel {
                id: "deepseek/deepseek-chat",
                description: "Low cost. Weaker on long designs.",
                is_recommended: false,
            },
        ],
        needs_api_key: true,
        supports_login: true,
    },
    CatalogProvider {
        name: "ollama",
        label: "Ollama",
        models: &[
            CatalogModel {
                id: "llama3.1",
                description: "Runs on this machine. No API cost.",
                is_recommended: true,
            },
            CatalogModel {
                id: "qwen2.5",
                description: "Runs on this machine. Similar size to Llama.",
                is_recommended: false,
            },
            CatalogModel {
                id: "gemma3",
                description: "Runs on this machine. Smaller and faster.",
                is_recommended: false,
            },
        ],
        needs_api_key: false,
        supports_login: false,
    },
];

/// OAuth tokens from a login.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredOauth {
    /// Bearer token for requests.
    pub access_token: String,
    /// Token used to renew the access token.
    pub refresh_token: String,
    /// Unix time when the access token expires.
    pub expires_at_unix_seconds: u64,
    /// ChatGPT account id, required by ChatGPT-login requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

/// The persisted model choice.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StoredSettings {
    /// Chosen provider name.
    pub provider: String,
    /// Chosen model identifier.
    pub model: String,
    /// API key, when the user authenticated with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Login tokens, when the user authenticated with a login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<StoredOauth>,
}

impl StoredSettings {
    /// How the stored choice authenticates: `login`, `api_key`, or
    /// `none` (keyless providers).
    pub fn auth_method(&self) -> &'static str {
        if self.oauth.is_some() {
            "login"
        } else if self.api_key.is_some() {
            "api_key"
        } else {
            "none"
        }
    }
}

/// Filesystem-backed settings storage plus the pending login verifier.
#[derive(Clone)]
pub struct SettingsStore {
    path: PathBuf,
    /// The server's own address, for login callback URLs.
    address: String,
    pending_login_verifier: Arc<Mutex<Option<String>>>,
}

impl SettingsStore {
    /// The server's bind address, such as `127.0.0.1:3000`.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Creates a store over `path`. `address` is the server's bind
    /// address, used in login callback URLs.
    pub fn new(path: PathBuf, address: String) -> Self {
        Self {
            path,
            address,
            pending_login_verifier: Arc::new(Mutex::new(None)),
        }
    }

    /// Reads the stored settings. `Ok(None)` means nothing was chosen.
    pub async fn read(&self) -> anyhow::Result<Option<StoredSettings>> {
        match tokio::fs::read_to_string(&self.path).await {
            Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes the settings with owner-only file permissions, because
    /// the file can hold credentials.
    pub async fn write(&self, settings: &StoredSettings) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&self.path, serde_json::to_string_pretty(settings)?).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600)).await?;
        }
        Ok(())
    }

    /// Remembers the PKCE verifier of the login now in progress.
    fn set_pending_login(&self, verifier: String) {
        if let Ok(mut pending) = self.pending_login_verifier.lock() {
            *pending = Some(verifier);
        }
    }

    /// Takes the pending PKCE verifier, ending the login attempt.
    fn take_pending_login(&self) -> Option<String> {
        self.pending_login_verifier
            .lock()
            .ok()
            .and_then(|mut pending| pending.take())
    }
}

/// URL-safe base64 without padding, as PKCE requires.
fn base64_url_encode(bytes: &[u8]) -> String {
    base64_encode(bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_owned()
}

/// A fresh PKCE verifier and its S256 challenge.
fn pkce_pair() -> anyhow::Result<(String, String)> {
    let mut random_bytes = [0u8; 48];
    getrandom::getrandom(&mut random_bytes)
        .map_err(|error| anyhow::anyhow!("no randomness source: {error}"))?;
    let verifier = base64_url_encode(&random_bytes);
    let challenge = base64_url_encode(&sha2::Sha256::digest(verifier.as_bytes()));
    Ok((verifier, challenge))
}

/// The Claude authorize URL for a challenge. The verifier doubles as
/// the `state` parameter, as in pi's and opencode's flow.
fn anthropic_authorize_url(challenge: &str, state: &str) -> String {
    format!(
        "https://claude.ai/oauth/authorize?code=true&client_id={ANTHROPIC_CLIENT_ID}\
         &response_type=code&redirect_uri={redirect}\
         &scope=org%3Acreate_api_key%20user%3Aprofile%20user%3Ainference\
         &code_challenge={challenge}&code_challenge_method=S256&state={state}",
        redirect = "https%3A%2F%2Fconsole.anthropic.com%2Foauth%2Fcode%2Fcallback",
    )
}

/// One hour, when a token response does not state its own expiry.
fn default_token_expiry_seconds() -> u64 {
    3600
}

/// Tokens from an OAuth token endpoint.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default = "default_token_expiry_seconds")]
    expires_in: u64,
}

/// Current unix time in seconds.
fn unix_now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// Exchanges a pasted `code#state` login code for tokens.
async fn exchange_login_code(code_and_state: &str, verifier: &str) -> Result<StoredOauth, String> {
    let (code, state) = code_and_state
        .trim()
        .split_once('#')
        .unwrap_or((code_and_state.trim(), ""));
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "state": state,
        "client_id": ANTHROPIC_CLIENT_ID,
        "redirect_uri": ANTHROPIC_REDIRECT_URL,
        "code_verifier": verifier,
    });
    let response = reqwest::Client::new()
        .post(ANTHROPIC_TOKEN_URL)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("login exchange failed: {error}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        let mut detail = text;
        detail.truncate(200);
        return Err(format!("login exchange returned {status}: {detail}"));
    }
    let tokens: TokenResponse = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    Ok(StoredOauth {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at_unix_seconds: unix_now_seconds() + tokens.expires_in,
        account_id: None,
    })
}

/// Renews an access token with its refresh token.
pub async fn refresh_login(refresh_token: &str) -> Result<StoredOauth, String> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": ANTHROPIC_CLIENT_ID,
    });
    let response = reqwest::Client::new()
        .post(ANTHROPIC_TOKEN_URL)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("login refresh failed: {error}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!("login refresh returned {status}: log in again"));
    }
    let tokens: TokenResponse = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    Ok(StoredOauth {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at_unix_seconds: unix_now_seconds() + tokens.expires_in,
        account_id: None,
    })
}

/// True when the access token needs a refresh soon.
pub fn is_login_expiring(oauth: &StoredOauth) -> bool {
    oauth.expires_at_unix_seconds <= unix_now_seconds() + 120
}

/// The `/settings` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/settings", get(get_settings).put(put_settings))
        .route("/settings/models", post(list_provider_models))
        .route("/settings/login/start", post(start_login))
        .route("/settings/login/complete", post(complete_login))
        .route(
            "/settings/login/openrouter/start",
            post(start_openrouter_login),
        )
        .route("/settings/login/openai/start", post(start_openai_login))
        .route(
            "/settings/login/openrouter/callback",
            get(openrouter_login_callback),
        )
}

/// Body of `PUT /settings`.
#[derive(Debug, Deserialize)]
struct SettingsRequest {
    /// Provider name from the catalog.
    provider: String,
    /// Model identifier.
    model: String,
    /// API key, when authenticating with one.
    #[serde(default)]
    api_key: Option<String>,
}

/// Body of `POST /settings/login/complete`.
#[derive(Debug, Deserialize)]
struct LoginCompleteRequest {
    /// The `code#state` value the login page shows.
    code: String,
    /// Model to use after login.
    #[serde(default)]
    model: Option<String>,
}

/// Returns the catalog, the current choice, and `has_chrome`: true when
/// Chrome or Chromium was found, so screen images and PDF export work.
async fn get_settings(State(store): State<SettingsStore>) -> Response {
    let current = match store.read().await {
        Ok(current) => current,
        Err(error) => return api_error::internal_error(&error),
    };
    Json(serde_json::json!({
        "providers": CATALOG,
        "current": current.map(|settings| serde_json::json!({
            "provider": settings.provider,
            "model": settings.model,
            "auth": settings.auth_method(),
        })),
        "has_chrome": crate::screenshots::find_chrome().is_some(),
    }))
    .into_response()
}

/// Saves a provider and model with an optional API key.
async fn put_settings(
    State(store): State<SettingsStore>,
    State(notifier): State<ChangeNotifier>,
    Json(request): Json<SettingsRequest>,
) -> Response {
    let Some(provider) = CATALOG
        .iter()
        .find(|provider| provider.name == request.provider)
    else {
        let names: Vec<&str> = CATALOG.iter().map(|provider| provider.name).collect();
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!(
                "unknown provider `{}`: pick one of {names:?}",
                request.provider
            ),
            Vec::new(),
        );
    };
    if request.model.trim().is_empty() {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "model is empty: pick or type a model name",
            Vec::new(),
        );
    }
    let existing = store.read().await.ok().flatten();
    let api_key = request.api_key.filter(|key| !key.trim().is_empty());
    let keeps_credentials = existing
        .as_ref()
        .is_some_and(|settings| settings.provider == provider.name);
    let settings = StoredSettings {
        provider: provider.name.to_owned(),
        model: request.model.trim().to_owned(),
        api_key: api_key.clone().or_else(|| {
            keeps_credentials
                .then(|| existing.as_ref()?.api_key.clone())
                .flatten()
        }),
        oauth: if keeps_credentials {
            existing.and_then(|settings| settings.oauth)
        } else {
            None
        },
    };
    if provider.needs_api_key && settings.api_key.is_none() && settings.oauth.is_none() {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!(
                "`{}` needs an API key or a login: enter a key, or use the login flow",
                provider.name
            ),
            Vec::new(),
        );
    }
    match store.write(&settings).await {
        Ok(()) => {
            notifier.notify();
            tracing::info!(provider = %settings.provider, model = %settings.model, "settings saved");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Starts a Claude login: returns the URL to open in a browser.
async fn start_login(State(store): State<SettingsStore>) -> Response {
    match pkce_pair() {
        Ok((verifier, challenge)) => {
            let authorize_url = anthropic_authorize_url(&challenge, &verifier);
            store.set_pending_login(verifier);
            Json(serde_json::json!({ "authorize_url": authorize_url })).into_response()
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Completes a Claude login with the pasted code.
async fn complete_login(
    State(store): State<SettingsStore>,
    State(notifier): State<ChangeNotifier>,
    Json(request): Json<LoginCompleteRequest>,
) -> Response {
    let Some(verifier) = store.take_pending_login() else {
        return api_error::error_response(
            StatusCode::CONFLICT,
            "no login in progress: start one first",
            Vec::new(),
        );
    };
    let oauth = match exchange_login_code(&request.code, &verifier).await {
        Ok(oauth) => oauth,
        Err(message) => {
            return api_error::error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                &message,
                Vec::new(),
            );
        }
    };
    let model = request
        .model
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| "claude-sonnet-5".to_owned());
    let settings = StoredSettings {
        provider: "anthropic".to_owned(),
        model,
        api_key: None,
        oauth: Some(oauth),
    };
    match store.write(&settings).await {
        Ok(()) => {
            notifier.notify();
            tracing::info!("login completed");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Where OpenRouter logins start.
const OPENROUTER_AUTH_URL: &str = "https://openrouter.ai/auth";

/// Where the OpenRouter login code becomes an API key.
const OPENROUTER_KEY_EXCHANGE_URL: &str = "https://openrouter.ai/api/v1/auth/keys";

/// Percent-encodes a URL component.
fn url_encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Body of `POST /settings/models`.
#[derive(Debug, Deserialize)]
struct ModelsRequest {
    /// Provider to list models for.
    provider: String,
    /// API key to use. Falls back to the stored key for this provider.
    #[serde(default)]
    api_key: Option<String>,
}

/// Lists the provider's live models with the user's credentials.
async fn list_provider_models(
    State(store): State<SettingsStore>,
    Json(request): Json<ModelsRequest>,
) -> Response {
    let mut api_key = request.api_key.filter(|key| !key.trim().is_empty());
    if api_key.is_none()
        && let Ok(Some(stored)) = store.read().await
        && stored.provider == request.provider
    {
        // A ChatGPT login has no key and no models endpoint; offer the
        // models that backend serves.
        if request.provider == "openai" && stored.oauth.is_some() {
            return Json(CHATGPT_LOGIN_MODELS.to_vec()).into_response();
        }
        api_key = stored.api_key;
    }
    match fetch_provider_models(&request.provider, api_key.as_deref()).await {
        Ok(models) => Json(models).into_response(),
        Err(message) => {
            api_error::error_response(StatusCode::UNPROCESSABLE_ENTITY, &message, Vec::new())
        }
    }
}

/// Asks the provider's models endpoint for its current model ids.
async fn fetch_provider_models(
    provider: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let request = if provider == "anthropic" {
        let api_key = api_key.ok_or("enter the API key first, then load the models")?;
        client
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
    } else {
        let chat_url = crate::generation::provider_chat_url(provider)
            .ok_or_else(|| format!("unknown provider `{provider}`"))?;
        let mut request = client.get(chat_url.replace("/chat/completions", "/models"));
        if let Some(api_key) = api_key {
            request = request.bearer_auth(api_key);
        }
        request
    };
    let response = request
        .send()
        .await
        .map_err(|error| format!("request to {provider} failed: {error}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        let mut detail = text;
        detail.truncate(200);
        return Err(format!("{provider} returned {status}: {detail}"));
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let mut models: Vec<String> = value["data"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .map(|id| id.strip_prefix("models/").unwrap_or(id).to_owned())
        .collect();
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err(format!("{provider} returned no models"));
    }
    Ok(models)
}

/// Starts an OpenRouter login: returns the URL to open. The login
/// finishes on the callback route; nothing to paste.
async fn start_openrouter_login(State(store): State<SettingsStore>) -> Response {
    match pkce_pair() {
        Ok((verifier, challenge)) => {
            let callback = format!(
                "http://{}/settings/login/openrouter/callback",
                store.address
            );
            let authorize_url = format!(
                "{OPENROUTER_AUTH_URL}?callback_url={callback}\
                 &code_challenge={challenge}&code_challenge_method=S256",
                callback = url_encode_component(&callback),
            );
            store.set_pending_login(verifier);
            Json(serde_json::json!({ "authorize_url": authorize_url })).into_response()
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Query of the OpenRouter callback.
#[derive(Debug, Deserialize)]
struct OpenrouterCallbackQuery {
    /// The authorization code OpenRouter appends.
    code: String,
}

/// Finishes an OpenRouter login: exchanges the code for an API key and
/// stores it. The browser lands here from OpenRouter.
async fn openrouter_login_callback(
    State(store): State<SettingsStore>,
    State(notifier): State<ChangeNotifier>,
    axum::extract::Query(query): axum::extract::Query<OpenrouterCallbackQuery>,
) -> Response {
    let Some(verifier) = store.take_pending_login() else {
        return api_error::error_response(
            StatusCode::CONFLICT,
            "no login in progress: start one from the studio",
            Vec::new(),
        );
    };
    let body = serde_json::json!({
        "code": query.code,
        "code_verifier": verifier,
        "code_challenge_method": "S256",
    });
    let exchange = async {
        let response = reqwest::Client::new()
            .post(OPENROUTER_KEY_EXCHANGE_URL)
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("key exchange failed: {error}"))?;
        let status = response.status();
        let text = response.text().await.map_err(|error| error.to_string())?;
        if !status.is_success() {
            let mut detail = text;
            detail.truncate(200);
            return Err(format!("key exchange returned {status}: {detail}"));
        }
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| error.to_string())?;
        value["key"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "key exchange response has no key".to_owned())
    };
    let key = match exchange.await {
        Ok(key) => key,
        Err(message) => {
            return api_error::error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                &message,
                Vec::new(),
            );
        }
    };
    let model = match store.read().await {
        Ok(Some(stored)) if stored.provider == "openrouter" => stored.model,
        _ => "google/gemini-2.5-flash".to_owned(),
    };
    let settings = StoredSettings {
        provider: "openrouter".to_owned(),
        model,
        api_key: Some(key),
        oauth: None,
    };
    match store.write(&settings).await {
        Ok(()) => {
            notifier.notify();
            tracing::info!("openrouter login completed");
            axum::response::Html(
                "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
                 <title>Swift Design</title></head><body style=\"font-family:Inter,system-ui,\
                 sans-serif;background:#F7F6F3;color:#15181C;display:flex;align-items:center;\
                 justify-content:center;min-height:100vh\"><main><h1>Login complete.</h1>\
                 <p>Return to the Swift Design tab; it updates by itself.</p>\
                 <p><a href=\"/\">Or open the studio here.</a></p></main></body></html>",
            )
            .into_response()
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Models the ChatGPT backend serves to subscription logins. Other
/// models are API-key only ("not supported when using Codex with a
/// ChatGPT account").
pub const CHATGPT_LOGIN_MODELS: &[&str] = &[
    "gpt-5.5",
    "gpt-5.6-sol",
    "gpt-5.6-luna",
    "gpt-5.6-terra",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.3-codex-spark",
];

/// The OAuth client id of the Codex CLI public client, used for
/// ChatGPT-subscription logins (the flow pi and Codex use).
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// Where ChatGPT logins start.
const OPENAI_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";

/// Where the ChatGPT login code becomes tokens.
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

/// The registered redirect of the Codex public client. A short-lived
/// local listener on port 1455 catches it.
const OPENAI_REDIRECT_URL: &str = "http://localhost:1455/auth/callback";

/// Decodes URL-safe base64 without padding.
fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    let mut bits: u32 = 0;
    let mut bit_count = 0u32;
    let mut bytes = Vec::with_capacity(input.len() * 3 / 4);
    for character in input.bytes() {
        let value = match character {
            b'A'..=b'Z' => character - b'A',
            b'a'..=b'z' => character - b'a' + 26,
            b'0'..=b'9' => character - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            bytes.push((bits >> bit_count) as u8);
        }
    }
    Some(bytes)
}

/// The ChatGPT account id from an access token's JWT claims.
fn chatgpt_account_id(access_token: &str) -> Option<String> {
    let payload = access_token.split('.').nth(1)?;
    let decoded = base64_url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims["https://api.openai.com/auth"]["chatgpt_account_id"]
        .as_str()
        .map(str::to_owned)
}

/// Exchanges a ChatGPT login code for tokens.
async fn exchange_openai_code(code: &str, verifier: &str) -> Result<StoredOauth, String> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", OPENAI_REDIRECT_URL),
        ("client_id", OPENAI_CLIENT_ID),
        ("code_verifier", verifier),
    ];
    let response = reqwest::Client::new()
        .post(OPENAI_TOKEN_URL)
        .form(&form)
        .send()
        .await
        .map_err(|error| format!("login exchange failed: {error}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        let mut detail = text;
        detail.truncate(200);
        return Err(format!("login exchange returned {status}: {detail}"));
    }
    let tokens: TokenResponse = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let account_id = chatgpt_account_id(&tokens.access_token);
    Ok(StoredOauth {
        account_id,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at_unix_seconds: unix_now_seconds() + tokens.expires_in,
    })
}

/// Renews a ChatGPT access token with its refresh token.
pub async fn refresh_chatgpt_login(refresh_token: &str) -> Result<StoredOauth, String> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", OPENAI_CLIENT_ID),
        ("scope", "openid profile email"),
    ];
    let response = reqwest::Client::new()
        .post(OPENAI_TOKEN_URL)
        .form(&form)
        .send()
        .await
        .map_err(|error| format!("login refresh failed: {error}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!("login refresh returned {status}: log in again"));
    }
    let tokens: TokenResponse = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let account_id = chatgpt_account_id(&tokens.access_token);
    Ok(StoredOauth {
        account_id,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at_unix_seconds: unix_now_seconds() + tokens.expires_in,
    })
}

/// Starts a ChatGPT login: opens a short-lived listener on port 1455
/// for the registered redirect and returns the URL to open.
async fn start_openai_login(
    State(store): State<SettingsStore>,
    State(notifier): State<ChangeNotifier>,
) -> Response {
    let (verifier, challenge) = match pkce_pair() {
        Ok(pair) => pair,
        Err(error) => return api_error::internal_error(&error),
    };
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:1455").await {
        Ok(listener) => listener,
        Err(error) => {
            return api_error::error_response(
                StatusCode::CONFLICT,
                &format!("cannot listen on port 1455 for the login callback: {error}"),
                Vec::new(),
            );
        }
    };
    let state_value = challenge.clone();
    let authorize_url = format!(
        "{OPENAI_AUTHORIZE_URL}?response_type=code&client_id={OPENAI_CLIENT_ID}\
         &redirect_uri={redirect}&scope=openid%20profile%20email%20offline_access\
         &code_challenge={challenge}&code_challenge_method=S256\
         &id_token_add_organizations=true&codex_cli_simplified_flow=true&state={state_value}",
        redirect = url_encode_component(OPENAI_REDIRECT_URL),
    );
    tokio::spawn(run_openai_callback_listener(
        listener, store, notifier, verifier,
    ));
    Json(serde_json::json!({ "authorize_url": authorize_url })).into_response()
}

/// Waits for the ChatGPT redirect, exchanges the code, and stores the
/// login. Gives up after five minutes.
async fn run_openai_callback_listener(
    listener: tokio::net::TcpListener,
    store: SettingsStore,
    notifier: ChangeNotifier,
    verifier: String,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let accepted = tokio::time::timeout(std::time::Duration::from_secs(300), listener.accept());
    let Ok(Ok((mut socket, _))) = accepted.await else {
        tracing::warn!("chatgpt login: no callback arrived");
        return;
    };
    let mut request_head = vec![0u8; 4096];
    let read = socket.read(&mut request_head).await.unwrap_or(0);
    let request = String::from_utf8_lossy(&request_head[..read]).to_string();
    let code = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|path| path.split_once("code=").map(|(_, rest)| rest))
        .map(|rest| {
            rest.split('&')
                .next()
                .unwrap_or(rest)
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_owned()
        });
    let outcome = match code {
        Some(code) if !code.is_empty() => match exchange_openai_code(&code, &verifier).await {
            Ok(oauth) => {
                let model = match store.read().await {
                    Ok(Some(stored))
                        if stored.provider == "openai"
                            && CHATGPT_LOGIN_MODELS.contains(&stored.model.as_str()) =>
                    {
                        stored.model
                    }
                    _ => CHATGPT_LOGIN_MODELS[0].to_owned(),
                };
                let settings = StoredSettings {
                    provider: "openai".to_owned(),
                    model,
                    api_key: None,
                    oauth: Some(oauth),
                };
                match store.write(&settings).await {
                    Ok(()) => {
                        notifier.notify();
                        tracing::info!("chatgpt login completed");
                        Ok(())
                    }
                    Err(error) => Err(error.to_string()),
                }
            }
            Err(message) => Err(message),
        },
        _ => Err("the callback carried no code".to_owned()),
    };
    let body = match &outcome {
        Ok(()) => {
            "<h1>Login complete.</h1><p>Return to the Swift Design tab; it updates by itself.</p>"
        }
        Err(_) => "<h1>Login failed.</h1><p>Return to Swift Design and try again.</p>",
    };
    if let Err(message) = outcome {
        tracing::warn!(%message, "chatgpt login failed");
    }
    let page = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <title>Swift Design</title></head><body style=\"font-family:Inter,system-ui,sans-serif;\
         background:#F7F6F3;color:#15181C;display:flex;align-items:center;justify-content:center;\
         min-height:100vh\"><main>{body}</main></body></html>",
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n{page}",
        page.len(),
    );
    let _ = socket.write_all(response.as_bytes()).await;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::settings::{
        CATALOG, StoredOauth, StoredSettings, anthropic_authorize_url, base64_url_encode,
        is_login_expiring, pkce_pair, unix_now_seconds,
    };

    #[test]
    fn pkce_values_are_url_safe() {
        let (verifier, challenge) = pkce_pair().unwrap();
        for value in [&verifier, &challenge] {
            assert!(!value.contains(['+', '/', '=']));
            assert!(value.len() >= 43);
        }
        let url = anthropic_authorize_url(&challenge, &verifier);
        assert!(url.starts_with("https://claude.ai/oauth/authorize?"));
        assert!(url.contains(&challenge));
        assert!(url.contains(&format!("&state={verifier}")));
    }

    #[test]
    fn base64_url_has_no_padding() {
        assert_eq!(base64_url_encode(b"f"), "Zg");
        assert_eq!(base64_url_encode(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn jwt_account_ids_decode_from_access_tokens() {
        let claims = serde_json::json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "account-123" }
        });
        let token = format!(
            "header.{}.signature",
            crate::settings::base64_url_encode(claims.to_string().as_bytes()),
        );
        assert_eq!(
            crate::settings::chatgpt_account_id(&token).as_deref(),
            Some("account-123")
        );
        assert_eq!(crate::settings::chatgpt_account_id("not-a-jwt"), None);
    }

    #[test]
    fn base64_url_decoding_reverses_encoding() {
        let bytes = b"swift-design?~\xfb\xff";
        let encoded = crate::settings::base64_url_encode(bytes);
        assert_eq!(
            crate::settings::base64_url_decode(&encoded).unwrap(),
            bytes.to_vec()
        );
        assert_eq!(crate::settings::base64_url_decode("!!"), None);
    }

    #[test]
    fn url_components_are_percent_encoded() {
        assert_eq!(
            crate::settings::url_encode_component("http://127.0.0.1:3000/a b"),
            "http%3A%2F%2F127.0.0.1%3A3000%2Fa%20b"
        );
        assert_eq!(
            crate::settings::url_encode_component("safe-._~09Az"),
            "safe-._~09Az"
        );
    }

    #[test]
    fn auth_method_reflects_stored_credentials() {
        let mut settings = StoredSettings {
            provider: "ollama".to_owned(),
            model: "llama3.1".to_owned(),
            api_key: None,
            oauth: None,
        };
        assert_eq!(settings.auth_method(), "none");
        settings.api_key = Some("key".to_owned());
        assert_eq!(settings.auth_method(), "api_key");
        settings.oauth = Some(StoredOauth {
            access_token: "a".to_owned(),
            refresh_token: "r".to_owned(),
            expires_at_unix_seconds: unix_now_seconds() + 3600,
            account_id: None,
        });
        assert_eq!(settings.auth_method(), "login");
        assert!(!is_login_expiring(settings.oauth.as_ref().unwrap()));
    }

    #[test]
    fn every_catalog_provider_has_one_recommended_model() {
        for provider in CATALOG {
            let recommended = provider
                .models
                .iter()
                .filter(|model| model.is_recommended)
                .count();
            assert_eq!(
                recommended, 1,
                "provider `{}` must mark exactly one model recommended",
                provider.name
            );
        }
    }

    #[test]
    fn every_catalog_provider_resolves_to_a_chat_url() {
        for provider in CATALOG {
            assert!(
                crate::generation::provider_chat_url(provider.name).is_some(),
                "provider `{}` has no chat URL in the generation registry",
                provider.name
            );
        }
    }

    #[test]
    fn catalog_model_ids_are_unique_within_a_provider() {
        for provider in CATALOG {
            let mut seen = std::collections::HashSet::new();
            for model in provider.models {
                assert!(
                    seen.insert(model.id),
                    "provider `{}` lists model `{}` twice",
                    provider.name,
                    model.id
                );
                assert!(!model.description.is_empty());
            }
        }
    }
}
