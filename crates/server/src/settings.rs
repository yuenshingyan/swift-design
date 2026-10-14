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

mod providers;
#[cfg(test)]
mod tests;
#[cfg(test)]
use providers::{
    CHATGPT_CATALOG_CLIENT_VERSION, base64_url_decode, chatgpt_account_id, chatgpt_model_ids,
    fetch_chatgpt_models, load_chatgpt_models, models_response, url_encode_component,
};
pub use providers::{CHATGPT_DEFAULT_MODEL, refresh_chatgpt_login};
use providers::{
    list_provider_models, openrouter_login_callback, start_openai_login, start_openrouter_login,
};

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::AppState;
use crate::api_error;
use crate::events::ChangeNotifier;
use crate::export::base64_encode;
use crate::model_client::provider_chat_url;
use crate::screenshots::find_chrome;

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
pub fn routes() -> Router<AppState> {
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
        "has_chrome": find_chrome().is_some(),
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
