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

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

/// The fixed image model included with a ChatGPT subscription.
pub const CHATGPT_IMAGE_MODEL: &str = "gpt-image-2";
/// The default OpenAI platform image model.
pub const OPENAI_IMAGE_MODEL: &str = "gpt-image-1";
/// The default Google image model.
pub const GOOGLE_IMAGE_MODEL: &str = "gemini-2.5-flash-image";
/// The default OpenRouter image model.
pub const OPENROUTER_IMAGE_MODEL: &str = "google/gemini-2.5-flash-image";

/// The independent image provider catalog shown by the studio.
pub const IMAGE_CATALOG: &[CatalogProvider] = &[
    CatalogProvider {
        name: "chatgpt",
        label: "ChatGPT subscription",
        models: &[CatalogModel {
            id: CHATGPT_IMAGE_MODEL,
            description: "Use your ChatGPT subscription.",
            is_recommended: true,
        }],
        needs_api_key: false,
        supports_login: true,
    },
    CatalogProvider {
        name: "openai",
        label: "OpenAI",
        models: &[CatalogModel {
            id: OPENAI_IMAGE_MODEL,
            description: "Use an OpenAI platform image API key.",
            is_recommended: true,
        }],
        needs_api_key: true,
        supports_login: false,
    },
    CatalogProvider {
        name: "google",
        label: "Google",
        models: &[CatalogModel {
            id: GOOGLE_IMAGE_MODEL,
            description: "Use a Google image API key.",
            is_recommended: true,
        }],
        needs_api_key: true,
        supports_login: false,
    },
    CatalogProvider {
        name: "openrouter",
        label: "OpenRouter",
        models: &[CatalogModel {
            id: OPENROUTER_IMAGE_MODEL,
            description: "Use an OpenRouter image API key.",
            is_recommended: true,
        }],
        needs_api_key: true,
        supports_login: false,
    },
];

/// OAuth tokens from a login.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Chosen image provider, independent of the text provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_provider: Option<String>,
    /// ChatGPT credentials retained independently of text authentication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chatgpt_oauth: Option<StoredOauth>,
    /// API key used only for image generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_api_key: Option<String>,
    /// Chosen image model. None uses the configured default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_model: Option<String>,
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
    mutation_lock: Arc<tokio::sync::Mutex<()>>,
}

/// A rejected settings update or a storage failure.
#[derive(Debug, thiserror::Error)]
pub enum SettingsMutationError {
    /// The requested settings are invalid.
    #[error("{0}")]
    Invalid(String),
    /// Settings could not be read or saved.
    #[error("cannot update settings")]
    Storage(#[from] anyhow::Error),
}

impl IntoResponse for SettingsMutationError {
    fn into_response(self) -> Response {
        match self {
            Self::Invalid(message) => {
                api_error::error_response(StatusCode::UNPROCESSABLE_ENTITY, &message, Vec::new())
            }
            Self::Storage(_) => api_error::error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot update settings: check the settings file and its permissions",
                Vec::new(),
            ),
        }
    }
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
            mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
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

    /// Mutates the latest settings and atomically saves them under one shared lock.
    /// A rejected mutation does not write. Do external work before calling this method.
    pub async fn mutate(
        &self,
        update: impl FnOnce(&mut StoredSettings) -> Result<(), String>,
    ) -> Result<StoredSettings, SettingsMutationError> {
        let _guard = self.mutation_lock.lock().await;
        let mut settings = self.read().await?.unwrap_or_default();
        update(&mut settings).map_err(SettingsMutationError::Invalid)?;
        self.write(&settings).await?;
        Ok(settings)
    }

    /// Writes credentials with owner-only permissions. Use `mutate` for read-modify-write updates.
    pub async fn write(&self, settings: &StoredSettings) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        crate::files::write_atomically_private(
            &self.path,
            &serde_json::to_string_pretty(settings)?,
        )
        .await?;
        #[cfg(unix)]
        tokio::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600)).await?;
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
        .map_err(|_| "login exchange failed: try the login again".to_owned())?;
    let tokens = login_tokens(response).await?;
    Ok(StoredOauth {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at_unix_seconds: unix_now_seconds() + tokens.expires_in,
        account_id: None,
    })
}

/// Decodes login tokens without returning provider bodies or credentials in errors.
async fn login_tokens(response: reqwest::Response) -> Result<TokenResponse, String> {
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "login exchange returned {status}: try the login again"
        ));
    }
    response
        .json()
        .await
        .map_err(|_| "login exchange response is invalid".to_owned())
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
        .route("/settings/image-models", post(list_image_models))
        .route("/settings/images", put(put_image_settings))
        .route(
            "/settings/images/login/openai/start",
            post(start_image_openai_login),
        )
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
    /// Image provider update. Omitted preserves the image choice.
    #[serde(default)]
    image_provider: Option<String>,
    /// Image API key update. Omitted preserves it and an empty value clears it.
    #[serde(default)]
    image_api_key: Option<String>,
    /// Image model update. Omitted preserves it and empty resets the default.
    #[serde(default)]
    image_model: Option<String>,
}

/// Body of `PUT /settings/images`.
#[derive(Debug, Deserialize)]
struct ImageSettingsRequest {
    /// Image provider from the image catalog.
    provider: String,
    /// Omitted preserves the key. Null or empty clears it.
    #[serde(
        default,
        alias = "image_api_key",
        deserialize_with = "image_setting_update"
    )]
    api_key: Option<Option<String>>,
    /// Omitted preserves the model. Null or empty resets its default.
    #[serde(
        default,
        alias = "image_model",
        deserialize_with = "image_setting_update"
    )]
    model: Option<Option<String>>,
}

/// Distinguishes an omitted image field from an explicit null reset.
fn image_setting_update<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

/// One independent image choice update.
struct ImageSettingsUpdate {
    provider: Option<String>,
    api_key: Option<Option<String>>,
    model: Option<Option<String>>,
}

/// Validates the image provider before any credentials are used.
fn validate_image_provider(provider: &str) -> Result<(), String> {
    if IMAGE_CATALOG.iter().any(|entry| entry.name == provider) {
        return Ok(());
    }
    Err(format!(
        "unknown image provider `{provider}`: pick chatgpt, openai, google, or openrouter"
    ))
}

/// Validates a provider-specific image model before it reaches a URL or request body.
fn validate_image_model(provider: &str, model: &str) -> Result<(), String> {
    let model = model.trim();
    if model.is_empty() || model.len() > 200 || model.chars().any(char::is_control) {
        return Err(
            "image model is invalid: use a printable model id up to 200 characters".to_owned(),
        );
    }
    if provider == "chatgpt" && model != CHATGPT_IMAGE_MODEL {
        return Err(format!(
            "ChatGPT subscription images use the fixed `{CHATGPT_IMAGE_MODEL}` model"
        ));
    }
    let google_model = model.strip_prefix("models/").unwrap_or(model);
    if provider == "google"
        && (!google_model.starts_with("gemini-")
            || !google_model.contains("image")
            || !google_model
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    {
        return Err("Google image models must be supported Gemini image model ids".to_owned());
    }
    Ok(())
}

/// Applies image updates without moving a saved key between providers.
fn apply_image_settings(
    settings: &mut StoredSettings,
    update: ImageSettingsUpdate,
) -> Result<(), String> {
    if let Some(provider) = &update.provider {
        validate_image_provider(provider)?;
    }
    migrate_image_settings(settings);
    if let Some(provider) = update.provider {
        if provider != selected_image_provider(Some(settings)) {
            settings.image_api_key = None;
            settings.image_model = None;
        }
        settings.image_provider = Some(provider);
    }
    if let Some(api_key) = update.api_key {
        settings.image_api_key = updated_image_setting(api_key, None);
    }
    if let Some(model) = update.model {
        let model = updated_image_setting(model, None);
        if let Some(model) = model.as_deref() {
            validate_image_model(&selected_image_provider(Some(settings)), model)?;
        }
        settings.image_model = model;
    }
    if settings
        .image_api_key
        .as_ref()
        .is_some_and(|key| key.len() > 4096 || key.chars().any(char::is_control))
    {
        return Err(
            "image API key is invalid: use a printable key up to 4096 characters".to_owned(),
        );
    }
    Ok(())
}

/// Saves only image settings, including before a text provider is chosen.
async fn put_image_settings(
    State(store): State<SettingsStore>,
    State(notifier): State<ChangeNotifier>,
    Json(request): Json<ImageSettingsRequest>,
) -> Response {
    let update = ImageSettingsUpdate {
        provider: Some(request.provider),
        api_key: request.api_key,
        model: request.model,
    };
    match store
        .mutate(|settings| apply_image_settings(settings, update))
        .await
    {
        Ok(_) => {
            notifier.notify();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => error.into_response(),
    }
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

/// Applies the optional image setting update semantics of `PUT /settings`.
fn updated_image_setting(requested: Option<String>, existing: Option<String>) -> Option<String> {
    match requested {
        None => existing,
        Some(value) if value.trim().is_empty() => None,
        Some(value) => Some(value.trim().to_owned()),
    }
}

/// Retains a legacy ChatGPT subscription before text credentials are replaced.
fn migrate_image_settings(settings: &mut StoredSettings) {
    if settings.provider != "openai" {
        return;
    }
    if let Some(oauth) = &settings.oauth {
        if settings.chatgpt_oauth.is_none() {
            settings.chatgpt_oauth = Some(oauth.clone());
        }
        if settings.image_provider.is_none()
            && oauth
                .account_id
                .as_deref()
                .is_some_and(|id| !id.trim().is_empty())
        {
            settings.image_provider = Some("chatgpt".to_owned());
        }
    }
}

/// Resolves the image provider with the same legacy defaults as generation.
fn selected_image_provider(current: Option<&StoredSettings>) -> String {
    if let Some(settings) = current {
        let mut settings = settings.clone();
        migrate_image_settings(&mut settings);
        if let Some(provider) = settings.image_provider {
            return provider;
        }
        if settings.image_api_key.is_some() {
            return "openai".to_owned();
        }
    }
    std::env::var("SWIFT_DESIGN_IMAGE_PROVIDER").unwrap_or_else(|_| "openai".to_owned())
}

/// Preserves image settings when a login replaces the text provider credentials.
fn settings_after_login(
    existing: Option<StoredSettings>,
    mut choice: StoredSettings,
) -> StoredSettings {
    if let Some(mut existing) = existing {
        migrate_image_settings(&mut existing);
        choice.image_provider = existing.image_provider;
        choice.image_api_key = existing.image_api_key;
        choice.image_model = existing.image_model;
        choice.chatgpt_oauth = existing.chatgpt_oauth;
    }
    if choice.provider == "openai" && choice.oauth.is_some() {
        if choice.chatgpt_oauth.is_none() {
            choice.chatgpt_oauth = choice.oauth.clone();
        }
        if choice.image_provider.is_none() {
            choice.image_provider = Some("chatgpt".to_owned());
        }
    }
    choice
}

/// Returns the public image generation status without exposing its key.
fn image_generation_summary(current: Option<&StoredSettings>) -> serde_json::Value {
    let configuration = match current {
        Some(settings) => crate::model_client::image_configuration_from_settings(settings),
        None => crate::model_client::image_configuration_from_environment(),
    };
    let model = configuration
        .as_ref()
        .map(|configuration| configuration.model.clone())
        .or_else(|| current.map(crate::model_client::image_model_from_settings))
        .unwrap_or_else(crate::model_client::configured_image_model);
    let source = configuration
        .as_ref()
        .map(|configuration| match &configuration.auth {
            crate::model_client::ImageAuth::ApiKey(_) => "api_key",
            crate::model_client::ImageAuth::ChatGptSubscription { .. } => "chatgpt_subscription",
        });
    serde_json::json!({
        "provider": selected_image_provider(current),
        "configured": configuration.is_some(),
        "model": model,
        "source": source,
    })
}

/// Returns the catalog, the current choice, and `has_chrome`: true when
/// Chrome or Chromium was found, so screen images and PDF export work.
async fn get_settings(State(store): State<SettingsStore>) -> Response {
    let current = match store.read().await {
        Ok(current) => current,
        Err(error) => return api_error::internal_error(&error),
    };
    let image_generation = image_generation_summary(current.as_ref());
    Json(serde_json::json!({
        "providers": CATALOG,
        "image_providers": IMAGE_CATALOG,
        "current": current.filter(|settings| {
            !settings.provider.trim().is_empty() && !settings.model.trim().is_empty()
        }).map(|settings| serde_json::json!({
            "provider": settings.provider,
            "model": settings.model,
            "auth": settings.auth_method(),
        })),
        "image_generation": image_generation,
        "has_chrome": crate::screenshots::find_chrome().is_some(),
    }))
    .into_response()
}

/// Saves a provider and model with optional chat and image API keys.
async fn put_settings(
    State(store): State<SettingsStore>,
    State(notifier): State<ChangeNotifier>,
    Json(request): Json<SettingsRequest>,
) -> Response {
    match store
        .mutate(|settings| apply_settings(settings, request))
        .await
    {
        Ok(settings) => {
            notifier.notify();
            tracing::info!(provider = %settings.provider, model = %settings.model, "settings saved");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// Validates the text provider and model without reading credentials.
fn settings_provider(request: &SettingsRequest) -> Result<&'static CatalogProvider, String> {
    let provider = CATALOG
        .iter()
        .find(|provider| provider.name == request.provider)
        .ok_or_else(|| {
            let names: Vec<&str> = CATALOG.iter().map(|provider| provider.name).collect();
            format!(
                "unknown provider `{}`: pick one of {names:?}",
                request.provider
            )
        })?;
    if request.model.trim().is_empty() {
        return Err("model is empty: pick or type a model name".to_owned());
    }
    Ok(provider)
}

/// Applies a whole settings request while preserving omitted choices.
fn apply_settings(settings: &mut StoredSettings, request: SettingsRequest) -> Result<(), String> {
    let provider = settings_provider(&request)?;
    apply_image_settings(
        settings,
        ImageSettingsUpdate {
            provider: request.image_provider,
            api_key: request.image_api_key.map(Some),
            model: request.image_model.map(Some),
        },
    )?;
    if settings.provider != provider.name {
        settings.api_key = None;
        settings.oauth = None;
    }
    settings.provider = provider.name.to_owned();
    settings.model = request.model.trim().to_owned();
    if let Some(api_key) = request.api_key.filter(|key| !key.trim().is_empty()) {
        settings.api_key = Some(api_key);
    }
    if provider.needs_api_key && settings.api_key.is_none() && settings.oauth.is_none() {
        return Err(format!(
            "`{}` needs an API key or a login: enter a key, or use the login flow",
            provider.name
        ));
    }
    Ok(())
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
    let saved = store
        .mutate(|settings| {
            *settings = settings_after_claude_login(std::mem::take(settings), oauth, request.model);
            Ok(())
        })
        .await;
    match saved {
        Ok(_) => {
            notifier.notify();
            tracing::info!("login completed");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// Preserves the latest image choices when Claude supplies login tokens.
fn settings_after_claude_login(
    existing: StoredSettings,
    oauth: StoredOauth,
    model: Option<String>,
) -> StoredSettings {
    settings_after_login(
        Some(existing),
        StoredSettings {
            provider: "anthropic".to_owned(),
            model: model
                .filter(|model| !model.trim().is_empty())
                .unwrap_or_else(|| "claude-sonnet-5".to_owned()),
            oauth: Some(oauth),
            ..StoredSettings::default()
        },
    )
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

/// Body of `POST /settings/image-models`.
#[derive(Debug, Deserialize)]
struct ImageModelsRequest {
    /// Image provider. Omitted uses the saved image choice.
    #[serde(default)]
    provider: Option<String>,
    /// Dedicated image key. Omitted uses the saved key or the environment.
    #[serde(default)]
    image_api_key: Option<String>,
}

/// The official OpenAI model-list endpoint, independent of generation URL overrides.
const OPENAI_IMAGE_MODELS_URL: &str = "https://api.openai.com/v1/models";

/// The official Google model-list endpoint.
const GOOGLE_IMAGE_MODELS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// The official OpenRouter image model-list endpoint.
const OPENROUTER_IMAGE_MODELS_URL: &str = "https://openrouter.ai/api/v1/images/models";

/// Selects dedicated credentials belonging only to the requested provider.
fn image_models_authentication(
    provider: &str,
    requested: Option<&str>,
    stored: Option<&StoredSettings>,
    environment: Option<String>,
) -> Result<ModelAuthentication, String> {
    validate_image_provider(provider)?;
    let saved = stored
        .filter(|settings| selected_image_provider(Some(settings)) == provider)
        .and_then(|settings| settings.image_api_key.as_deref());
    let environment = environment.filter(|_| selected_image_provider(None) == provider);
    let saved = crate::model_client::resolved_image_api_key(saved, environment);
    crate::model_client::resolved_image_api_key(requested, saved)
        .map(ModelAuthentication::ApiKey)
        .ok_or_else(|| format!("enter a {provider} image API key to list image models"))
}

/// Lists the subscription image model or compatible platform image models.
async fn list_image_models(
    State(store): State<SettingsStore>,
    Json(request): Json<ImageModelsRequest>,
) -> Response {
    let stored = match store.read().await {
        Ok(stored) => stored,
        Err(error) => return api_error::internal_error(&error),
    };
    let provider = request
        .provider
        .unwrap_or_else(|| selected_image_provider(stored.as_ref()));
    let result =
        discover_image_models(&provider, request.image_api_key.as_deref(), stored.as_ref()).await;
    match result {
        Ok(models) => Json(models).into_response(),
        Err(message) => {
            api_error::error_response(StatusCode::UNPROCESSABLE_ENTITY, &message, Vec::new())
        }
    }
}

/// Discovers models without using text credentials or another image provider's key.
async fn discover_image_models(
    provider: &str,
    requested: Option<&str>,
    stored: Option<&StoredSettings>,
) -> Result<Vec<String>, String> {
    validate_image_provider(provider)?;
    if provider == "chatgpt" {
        let mut settings = stored.cloned().unwrap_or_default();
        migrate_image_settings(&mut settings);
        let oauth = settings
            .chatgpt_oauth
            .as_ref()
            .ok_or("log in to ChatGPT to list image models")?;
        if oauth.access_token.trim().is_empty()
            || !oauth
                .account_id
                .as_deref()
                .is_some_and(|id| !id.trim().is_empty())
        {
            return Err("ChatGPT image login is incomplete: log in again".to_owned());
        }
        return Ok(vec![CHATGPT_IMAGE_MODEL.to_owned()]);
    }
    let authentication = image_models_authentication(
        provider,
        requested,
        stored,
        crate::model_client::image_api_key_from_environment(),
    )?;
    let url = match provider {
        "google" => GOOGLE_IMAGE_MODELS_URL,
        "openrouter" => OPENROUTER_IMAGE_MODELS_URL,
        _ => OPENAI_IMAGE_MODELS_URL,
    };
    let models =
        fetch_provider_models_at(&models_client()?, provider, &authentication, url).await?;
    filter_image_models(provider, models)
}

/// Keeps the image model family supported by each provider.
fn filter_image_models(provider: &str, models: Vec<String>) -> Result<Vec<String>, String> {
    if provider == "openai" {
        return compatible_image_models(models);
    }
    let models: Vec<_> = models
        .into_iter()
        .filter(|id| provider != "google" || (id.starts_with("gemini-") && id.contains("image")))
        .collect();
    if models.is_empty() {
        return Err(format!("{provider} returned no image models"));
    }
    Ok(models)
}

/// Keeps only the GPT Image family supported by the current generation request.
fn compatible_image_models(models: Vec<String>) -> Result<Vec<String>, String> {
    let mut models: Vec<_> = models
        .into_iter()
        .filter(|id| id.starts_with("gpt-image-") && id.len() > "gpt-image-".len())
        .collect();
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err(
            "OpenAI returned no compatible GPT Image models for this image API key".to_owned(),
        );
    }
    Ok(models)
}

/// Credentials for one provider-owned model-list request.
enum ModelAuthentication {
    None,
    ApiKey(String),
    OAuth(StoredOauth),
}

/// The provider-owned Anthropic model-list endpoint.
const ANTHROPIC_MODELS_URL: &str = "https://api.anthropic.com/v1/models";

/// Builds the provider-owned Codex model-list endpoint.
fn chatgpt_models_url() -> String {
    format!(
        "https://chatgpt.com/backend-api/codex/models?client_version={}",
        env!("CARGO_PKG_VERSION")
    )
}

/// Lists the provider's live models with the user's credentials.
async fn list_provider_models(
    State(store): State<SettingsStore>,
    Json(request): Json<ModelsRequest>,
) -> Response {
    let authentication = match request.api_key.filter(|key| !key.trim().is_empty()) {
        Some(api_key) => ModelAuthentication::ApiKey(api_key),
        None => {
            let stored = match store.read().await {
                Ok(stored) => stored,
                Err(error) => return api_error::internal_error(&error),
            };
            match stored.filter(|stored| stored.provider == request.provider) {
                Some(stored) => match (stored.oauth, stored.api_key) {
                    (Some(oauth), _) => ModelAuthentication::OAuth(oauth),
                    (None, Some(api_key)) => ModelAuthentication::ApiKey(api_key),
                    (None, None) => ModelAuthentication::None,
                },
                None => ModelAuthentication::None,
            }
        }
    };
    match fetch_provider_models(&request.provider, &authentication).await {
        Ok(models) => Json(models).into_response(),
        Err(message) => {
            api_error::error_response(StatusCode::UNPROCESSABLE_ENTITY, &message, Vec::new())
        }
    }
}

/// Asks the provider's models endpoint for its current model ids.
async fn fetch_provider_models(
    provider: &str,
    authentication: &ModelAuthentication,
) -> Result<Vec<String>, String> {
    let client = models_client()?;
    let url = match (provider, authentication) {
        ("openai", ModelAuthentication::OAuth(_)) => chatgpt_models_url(),
        ("anthropic", _) => ANTHROPIC_MODELS_URL.to_owned(),
        _ => crate::model_client::provider_chat_url(provider)
            .ok_or_else(|| format!("unknown provider `{provider}`"))?
            .replace("/chat/completions", "/models"),
    };
    fetch_provider_models_at(&client, provider, authentication, &url).await
}

/// Bounds model-list requests and refuses redirects away from the provider.
fn models_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "cannot prepare the model request".to_owned())
}

/// Builds one authenticated provider-owned model-list request.
fn provider_models_request(
    client: &reqwest::Client,
    provider: &str,
    authentication: &ModelAuthentication,
    url: &str,
) -> Result<reqwest::RequestBuilder, String> {
    let request = client.get(url).header("accept", "application/json");
    match (provider, authentication) {
        ("anthropic", ModelAuthentication::ApiKey(api_key)) => Ok(request
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")),
        ("anthropic", ModelAuthentication::OAuth(oauth)) => Ok(request
            .bearer_auth(&oauth.access_token)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "oauth-2025-04-20")),
        ("anthropic", ModelAuthentication::None) => {
            Err("enter the API key or log in first, then load the models".to_owned())
        }
        ("openai", ModelAuthentication::OAuth(oauth)) => {
            let account_id = oauth
                .account_id
                .as_deref()
                .ok_or("ChatGPT login has no account id: log in again")?;
            Ok(request
                .bearer_auth(&oauth.access_token)
                .header("chatgpt-account-id", account_id)
                .header("OpenAI-Beta", "responses=experimental")
                .header("originator", "codex_cli_rs"))
        }
        ("google", ModelAuthentication::ApiKey(api_key)) => {
            Ok(request.header("x-goog-api-key", api_key))
        }
        (_, ModelAuthentication::ApiKey(api_key)) => Ok(request.bearer_auth(api_key)),
        (_, ModelAuthentication::None) => Ok(request),
        (_, ModelAuthentication::OAuth(_)) => Err(format!(
            "{provider} does not support model listing with a login"
        )),
    }
}

/// Sends one provider-owned model-list request to `url`.
async fn fetch_provider_models_at(
    client: &reqwest::Client,
    provider: &str,
    authentication: &ModelAuthentication,
    url: &str,
) -> Result<Vec<String>, String> {
    let response = provider_models_request(client, provider, authentication, url)?
        .send()
        .await
        .map_err(|_| format!("request to {provider} failed"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{provider} returned {status}"));
    }
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|_| format!("{provider} returned invalid model data"))?;
    let models = parse_model_ids(&value);
    if models.is_empty() {
        return Err(format!("{provider} returned no models"));
    }
    Ok(models)
}

/// Reads model ids from OpenAI-compatible and provider-owned response shapes.
fn parse_model_ids(value: &serde_json::Value) -> Vec<String> {
    let mut models = Vec::new();
    for (array_name, fields) in [
        ("data", ["id", "name", "slug"]),
        ("models", ["slug", "id", "name"]),
    ] {
        let Some(entries) = value[array_name].as_array() else {
            continue;
        };
        for entry in entries {
            let Some(id) = fields.iter().find_map(|field| entry[*field].as_str()) else {
                continue;
            };
            let id = id.trim().strip_prefix("models/").unwrap_or(id.trim());
            if !id.is_empty() {
                models.push(id.to_owned());
            }
        }
    }
    models.sort();
    models.dedup();
    models
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
    let exchange = exchange_openrouter_code(&query.code, &verifier, OPENROUTER_KEY_EXCHANGE_URL);
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
    let saved = store
        .mutate(|settings| {
            *settings = settings_after_openrouter_login(std::mem::take(settings), key);
            Ok(())
        })
        .await;
    match saved {
        Ok(_) => {
            notifier.notify();
            tracing::info!("openrouter login completed");
            axum::response::Html(OPENROUTER_LOGIN_COMPLETE_PAGE).into_response()
        }
        Err(error) => error.into_response(),
    }
}

const OPENROUTER_LOGIN_COMPLETE_PAGE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
     <title>Swift Design</title></head><body style=\"font-family:Inter,system-ui,\
     sans-serif;background:#F7F6F3;color:#15181C;display:flex;align-items:center;\
     justify-content:center;min-height:100vh\"><main><h1>Login complete.</h1>\
     <p>Return to the Swift Design tab; it updates by itself.</p>\
     <p><a href=\"/\">Or open the studio here.</a></p></main></body></html>";

/// Exchanges an OpenRouter code without holding the settings lock.
async fn exchange_openrouter_code(
    code: &str,
    verifier: &str,
    endpoint: &str,
) -> Result<String, String> {
    let response = reqwest::Client::new()
        .post(endpoint)
        .json(&serde_json::json!({
            "code": code,
            "code_verifier": verifier,
            "code_challenge_method": "S256",
        }))
        .send()
        .await
        .map_err(|_| "key exchange failed: try the login again".to_owned())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "key exchange returned {status}: try the login again"
        ));
    }
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "key exchange response is invalid".to_owned())?;
    value["key"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "key exchange response has no key".to_owned())
}

/// Preserves the latest model and image choices when OpenRouter supplies a key.
fn settings_after_openrouter_login(existing: StoredSettings, key: String) -> StoredSettings {
    let model = if existing.provider == "openrouter" {
        existing.model.clone()
    } else {
        "google/gemini-2.5-flash".to_owned()
    };
    settings_after_login(
        Some(existing),
        StoredSettings {
            provider: "openrouter".to_owned(),
            model,
            api_key: Some(key),
            ..StoredSettings::default()
        },
    )
}

/// Curated ChatGPT defaults used before live model discovery succeeds.
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
        .map_err(|_| "login exchange failed: try the login again".to_owned())?;
    let tokens = login_tokens(response).await?;
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

/// The settings changed by a ChatGPT callback.
#[derive(Clone, Copy)]
enum OpenaiLoginScope {
    Text,
    Image,
}

/// One pending ChatGPT login and its persistence scope.
struct OpenaiCallback {
    store: SettingsStore,
    notifier: ChangeNotifier,
    verifier: String,
    scope: OpenaiLoginScope,
}

/// Starts a ChatGPT login for text generation.
async fn start_openai_login(
    State(store): State<SettingsStore>,
    State(notifier): State<ChangeNotifier>,
) -> Response {
    start_scoped_openai_login(store, notifier, OpenaiLoginScope::Text).await
}

/// Starts a ChatGPT login without changing text settings.
async fn start_image_openai_login(
    State(store): State<SettingsStore>,
    State(notifier): State<ChangeNotifier>,
) -> Response {
    start_scoped_openai_login(store, notifier, OpenaiLoginScope::Image).await
}

/// Opens the existing local callback listener for the selected login scope.
async fn start_scoped_openai_login(
    store: SettingsStore,
    notifier: ChangeNotifier,
    scope: OpenaiLoginScope,
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
        listener,
        OpenaiCallback {
            store,
            notifier,
            verifier,
            scope,
        },
    ));
    Json(serde_json::json!({ "authorize_url": authorize_url })).into_response()
}

/// Applies a ChatGPT login to the selected settings scope.
fn settings_after_openai_login(
    stored: Option<StoredSettings>,
    oauth: StoredOauth,
    scope: OpenaiLoginScope,
) -> StoredSettings {
    if matches!(scope, OpenaiLoginScope::Image) {
        let mut settings = stored.unwrap_or_default();
        if selected_image_provider(Some(&settings)) != "chatgpt" {
            settings.image_api_key = None;
            settings.image_model = None;
        }
        settings.chatgpt_oauth = Some(oauth);
        settings.image_provider = Some("chatgpt".to_owned());
        return settings;
    }
    let model = stored
        .as_ref()
        .filter(|settings| settings.provider == "openai" && !settings.model.trim().is_empty())
        .map(|settings| settings.model.clone())
        .unwrap_or_else(|| CHATGPT_LOGIN_MODELS[0].to_owned());
    settings_after_login(
        stored,
        StoredSettings {
            provider: "openai".to_owned(),
            model,
            oauth: Some(oauth),
            ..StoredSettings::default()
        },
    )
}

/// Exchanges and saves a callback using the latest text and image choices.
async fn complete_openai_callback(callback: &OpenaiCallback, code: &str) -> Result<(), String> {
    let oauth = exchange_openai_code(code, &callback.verifier).await?;
    callback
        .store
        .mutate(|settings| {
            *settings =
                settings_after_openai_login(Some(std::mem::take(settings)), oauth, callback.scope);
            Ok(())
        })
        .await
        .map_err(|_| "cannot save the login settings".to_owned())?;
    callback.notifier.notify();
    tracing::info!(
        is_image_login = matches!(callback.scope, OpenaiLoginScope::Image),
        "chatgpt login completed"
    );
    Ok(())
}

/// Waits for the ChatGPT redirect, exchanges the code, and stores the
/// login. Gives up after five minutes.
async fn run_openai_callback_listener(listener: tokio::net::TcpListener, callback: OpenaiCallback) {
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
        Some(code) if !code.is_empty() => complete_openai_callback(&callback, &code).await,
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
    use axum::Json;
    use axum::extract::State;
    use axum::http::StatusCode;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::events::ChangeNotifier;

    #[tokio::test]
    async fn concurrent_text_and_image_saves_preserve_both_updates() {
        for has_existing_settings in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let store = SettingsStore::new(directory.path().join("settings.json"), String::new());
            if has_existing_settings {
                store
                    .write(&StoredSettings {
                        provider: "openai".to_owned(),
                        model: "old-text".to_owned(),
                        oauth: Some(image_oauth()),
                        ..StoredSettings::default()
                    })
                    .await
                    .unwrap();
            }
            save_text_and_image_together(&store).await;
            let saved = store.read().await.unwrap().unwrap();
            assert_eq!(saved.provider, "ollama");
            assert_eq!(saved.model, "new-text");
            assert_eq!(saved.image_provider.as_deref(), Some("google"));
            assert_eq!(saved.image_api_key.as_deref(), Some("new-image-secret"));
            assert_eq!(saved.image_model.as_deref(), Some(GOOGLE_IMAGE_MODEL));
            assert!(saved.oauth.is_none());
            assert_eq!(saved.chatgpt_oauth, has_existing_settings.then(image_oauth));
        }
    }

    async fn save_text_and_image_together(store: &SettingsStore) {
        let guard = store.mutation_lock.lock().await;
        let text = put_settings(
            State(store.clone()),
            State(ChangeNotifier::new()),
            Json(
                serde_json::from_value(serde_json::json!({
                    "provider": "ollama", "model": "new-text"
                }))
                .unwrap(),
            ),
        );
        let image = put_image_settings(
            State(store.clone()),
            State(ChangeNotifier::new()),
            Json(
                serde_json::from_value(serde_json::json!({
                    "provider": "google", "api_key": "new-image-secret", "model": GOOGLE_IMAGE_MODEL
                }))
                .unwrap(),
            ),
        );
        tokio::pin!(text, image);
        let wait = std::time::Duration::from_millis(10);
        let (text_wait, image_wait) = tokio::join!(
            tokio::time::timeout(wait, &mut text),
            tokio::time::timeout(wait, &mut image),
        );
        assert!(text_wait.is_err());
        assert!(image_wait.is_err());
        drop(guard);
        let (text, image) = tokio::join!(text, image);
        assert_eq!(text.status(), StatusCode::NO_CONTENT);
        assert_eq!(image.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn rejected_settings_mutations_leave_the_original_file_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(directory.path().join("settings.json"), String::new());
        store.write(&StoredSettings::default()).await.unwrap();
        let original = tokio::fs::read(&store.path).await.unwrap();
        let response = put_settings(
            State(store.clone()),
            State(ChangeNotifier::new()),
            Json(
                serde_json::from_value(serde_json::json!({
                    "provider": "google", "model": "text", "image_api_key": "partial-secret"
                }))
                .unwrap(),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(tokio::fs::read(&store.path).await.unwrap(), original);
        store
            .mutate(|settings| {
                settings.model = "later-update".to_owned();
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(store.read().await.unwrap().unwrap().model, "later-update");
    }

    #[tokio::test]
    async fn settings_mutation_storage_errors_hide_paths_and_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(
            directory.path().join("private-settings.json"),
            String::new(),
        );
        tokio::fs::write(
            &store.path,
            r#"{"provider": "secret", "oauth": "credential-secret"}"#,
        )
        .await
        .unwrap();
        let error = store
            .mutate(|_| panic!("invalid settings must not be mutated"))
            .await
            .unwrap_err();
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(!body.contains("secret"));
        assert!(!body.contains("private-settings"));
        assert!(!body.contains(&directory.path().to_string_lossy().to_string()));
    }

    #[test]
    fn claude_and_openrouter_logins_preserve_image_choices_and_model_defaults() {
        let existing = StoredSettings {
            provider: "openrouter".to_owned(),
            model: "chosen-text".to_owned(),
            image_provider: Some("google".to_owned()),
            image_api_key: Some("image-secret".to_owned()),
            image_model: Some(GOOGLE_IMAGE_MODEL.to_owned()),
            chatgpt_oauth: Some(image_oauth()),
            ..StoredSettings::default()
        };
        let openrouter =
            settings_after_openrouter_login(existing.clone(), "text-secret".to_owned());
        assert_eq!(openrouter.model, "chosen-text");
        assert_eq!(openrouter.api_key.as_deref(), Some("text-secret"));
        let claude = settings_after_claude_login(existing.clone(), image_oauth(), None);
        assert_eq!(claude.model, "claude-sonnet-5");
        assert_eq!(claude.oauth, Some(image_oauth()));
        for saved in [openrouter, claude] {
            assert_eq!(saved.image_provider, existing.image_provider);
            assert_eq!(saved.image_api_key, existing.image_api_key);
            assert_eq!(saved.image_model, existing.image_model);
            assert_eq!(saved.chatgpt_oauth, existing.chatgpt_oauth);
        }
        assert_eq!(
            settings_after_openrouter_login(StoredSettings::default(), String::new()).model,
            "google/gemini-2.5-flash"
        );
        assert_eq!(
            settings_after_claude_login(existing, image_oauth(), Some("chosen".to_owned())).model,
            "chosen"
        );
    }

    #[tokio::test]
    async fn login_token_errors_do_not_expose_provider_bodies() {
        for status in ["400 Bad Request", "200 OK"] {
            let (url, server) =
                fake_models_server("/tokens", status, r#"{"access_token":"credential-secret"}"#)
                    .await;
            let response = reqwest::get(url).await.unwrap();
            let error = login_tokens(response).await.err().unwrap();
            assert!(!error.contains("credential-secret"));
            server.await.unwrap();
        }
        let (url, server) = fake_models_server(
            "/tokens",
            "200 OK",
            r#"{"access_token":"access","refresh_token":"refresh","expires_in":3600}"#,
        )
        .await;
        let tokens = login_tokens(reqwest::get(url).await.unwrap())
            .await
            .unwrap();
        assert_eq!(tokens.refresh_token, "refresh");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn openrouter_exchange_reads_keys_and_hides_error_bodies() {
        let (url, server) = fake_models_server("/keys", "200 OK", r#"{"key":"new-secret"}"#).await;
        assert_eq!(
            exchange_openrouter_code("code", "verifier", &url)
                .await
                .unwrap(),
            "new-secret"
        );
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /keys "));
        for (status, body) in [
            ("400 Bad Request", "credential-secret"),
            ("200 OK", "credential-secret"),
            ("200 OK", r#"{"unexpected":"credential-secret"}"#),
        ] {
            let (url, server) = fake_models_server("/keys", status, body).await;
            let error = exchange_openrouter_code("code", "verifier", &url)
                .await
                .unwrap_err();
            assert!(!error.contains("credential-secret"));
            server.await.unwrap();
        }
    }

    async fn fake_models_server(
        path: &str,
        status: &'static str,
        body: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let path = path.to_owned();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len(),
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request).unwrap()
        });
        (format!("http://{address}{path}"), task)
    }

    fn image_oauth() -> StoredOauth {
        StoredOauth {
            access_token: "image-access-secret".to_owned(),
            refresh_token: "image-refresh-secret".to_owned(),
            expires_at_unix_seconds: u64::MAX,
            account_id: Some("image-account-secret".to_owned()),
        }
    }

    #[test]
    fn image_catalog_has_exactly_the_supported_providers_and_defaults() {
        assert_eq!(
            IMAGE_CATALOG
                .iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>(),
            ["chatgpt", "openai", "google", "openrouter"]
        );
        for entry in IMAGE_CATALOG {
            assert!(!entry.label.is_empty());
            assert_eq!(entry.models.len(), 1);
            assert!(entry.models[0].is_recommended);
            assert_eq!(entry.supports_login, entry.name == "chatgpt");
            assert_eq!(entry.needs_api_key, entry.name != "chatgpt");
            let settings = StoredSettings {
                image_provider: Some(entry.name.to_owned()),
                ..StoredSettings::default()
            };
            assert_eq!(
                image_generation_summary(Some(&settings))["provider"],
                entry.name
            );
        }
        for provider in ["anthropic", "ollama", "groq", "", "OpenAI"] {
            assert!(validate_image_provider(provider).is_err());
        }
        assert_eq!(OPENAI_IMAGE_MODELS_URL, "https://api.openai.com/v1/models");
        assert_eq!(
            GOOGLE_IMAGE_MODELS_URL,
            "https://generativelanguage.googleapis.com/v1beta/models"
        );
        assert_eq!(
            OPENROUTER_IMAGE_MODELS_URL,
            "https://openrouter.ai/api/v1/images/models"
        );
    }

    #[test]
    fn independent_image_updates_distinguish_omission_null_and_empty() {
        for (json, expected) in [
            (r#"{"provider":"google"}"#, None),
            (
                r#"{"provider":"google","api_key":null,"model":null}"#,
                Some(None),
            ),
            (
                r#"{"provider":"google","api_key":"","model":""}"#,
                Some(Some("")),
            ),
            (
                r#"{"provider":"google","api_key":"gemini-new-image","model":"gemini-new-image"}"#,
                Some(Some("gemini-new-image")),
            ),
        ] {
            let request: ImageSettingsRequest = serde_json::from_str(json).unwrap();
            assert_eq!(
                request.api_key.as_ref().map(|value| value.as_deref()),
                expected
            );
            assert_eq!(
                request.model.as_ref().map(|value| value.as_deref()),
                expected
            );
            let mut settings = StoredSettings {
                image_provider: Some("google".to_owned()),
                image_api_key: Some("old".to_owned()),
                image_model: Some("old".to_owned()),
                ..StoredSettings::default()
            };
            apply_image_settings(
                &mut settings,
                ImageSettingsUpdate {
                    provider: Some(request.provider),
                    api_key: request.api_key,
                    model: request.model,
                },
            )
            .unwrap();
            let saved = match expected {
                None => Some("old"),
                Some(Some("gemini-new-image")) => Some("gemini-new-image"),
                _ => None,
            };
            assert_eq!(settings.image_api_key.as_deref(), saved);
            assert_eq!(settings.image_model.as_deref(), saved);
        }
    }

    #[test]
    fn scoped_chatgpt_login_preserves_the_other_settings() {
        let original = StoredSettings {
            provider: "anthropic".to_owned(),
            model: "text-model".to_owned(),
            api_key: Some("text-key".to_owned()),
            oauth: Some(StoredOauth {
                account_id: None,
                access_token: "text-token".to_owned(),
                ..image_oauth()
            }),
            image_provider: Some("google".to_owned()),
            image_api_key: Some("google-key".to_owned()),
            image_model: Some("google-image".to_owned()),
            ..StoredSettings::default()
        };
        let saved = settings_after_openai_login(
            Some(original.clone()),
            image_oauth(),
            OpenaiLoginScope::Image,
        );
        assert_eq!(saved.provider, original.provider);
        assert_eq!(saved.model, original.model);
        assert_eq!(saved.api_key, original.api_key);
        assert_eq!(saved.oauth.unwrap().access_token, "text-token");
        assert_eq!(saved.image_provider.as_deref(), Some("chatgpt"));
        assert!(saved.image_api_key.is_none());
        assert!(saved.image_model.is_none());
        assert_eq!(
            saved.chatgpt_oauth.unwrap().access_token,
            "image-access-secret"
        );
        let saved =
            settings_after_openai_login(Some(original), image_oauth(), OpenaiLoginScope::Text);
        assert_eq!(saved.provider, "openai");
        assert_eq!(saved.image_provider.as_deref(), Some("google"));
        assert_eq!(saved.image_api_key.as_deref(), Some("google-key"));
        assert_eq!(saved.image_model.as_deref(), Some("google-image"));
        assert_eq!(
            saved.chatgpt_oauth.unwrap().access_token,
            saved.oauth.unwrap().access_token
        );
        let fresh = settings_after_openai_login(None, image_oauth(), OpenaiLoginScope::Text);
        assert_eq!(fresh.image_provider.as_deref(), Some("chatgpt"));
        assert!(fresh.chatgpt_oauth.is_some());
    }

    #[test]
    fn other_logins_migrate_and_retain_the_chatgpt_subscription() {
        for provider in ["anthropic", "openrouter"] {
            for is_legacy in [true, false] {
                let original = StoredSettings {
                    provider: "openai".to_owned(),
                    oauth: Some(image_oauth()),
                    image_provider: (!is_legacy).then(|| "chatgpt".to_owned()),
                    chatgpt_oauth: (!is_legacy).then(image_oauth),
                    image_model: Some("saved-model".to_owned()),
                    image_api_key: Some("saved-key".to_owned()),
                    ..StoredSettings::default()
                };
                let saved = settings_after_login(
                    Some(original),
                    StoredSettings {
                        provider: provider.to_owned(),
                        ..StoredSettings::default()
                    },
                );
                let restored: StoredSettings =
                    serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
                assert_eq!(restored.image_provider.as_deref(), Some("chatgpt"));
                assert_eq!(
                    restored.chatgpt_oauth.unwrap().refresh_token,
                    "image-refresh-secret"
                );
                assert_eq!(restored.image_api_key.as_deref(), Some("saved-key"));
                assert_eq!(restored.image_model.as_deref(), Some("saved-model"));
                assert_eq!(image_generation_summary(Some(&saved))["configured"], true);
            }
        }
    }

    #[test]
    fn discovery_never_reuses_another_providers_key() {
        for saved_provider in ["chatgpt", "openai", "google", "openrouter"] {
            let settings = StoredSettings {
                image_provider: Some(saved_provider.to_owned()),
                image_api_key: Some("saved-key".to_owned()),
                api_key: Some("text-key".to_owned()),
                oauth: Some(image_oauth()),
                ..StoredSettings::default()
            };
            for provider in ["openai", "google", "openrouter"] {
                assert_eq!(
                    image_models_authentication(provider, None, Some(&settings), None).is_ok(),
                    saved_provider == provider
                );
                let authentication =
                    image_models_authentication(provider, Some("typed-key"), Some(&settings), None)
                        .unwrap();
                assert!(
                    matches!(authentication, ModelAuthentication::ApiKey(key) if key == "typed-key")
                );
                let environment = image_models_authentication(
                    provider,
                    None,
                    None,
                    Some("environment-key".to_owned()),
                );
                assert_eq!(
                    environment.is_ok(),
                    selected_image_provider(None) == provider
                );
            }
        }
    }

    #[tokio::test]
    async fn chatgpt_discovery_uses_only_the_independent_or_legacy_chatgpt_login() {
        let mut settings = StoredSettings {
            provider: "anthropic".to_owned(),
            oauth: Some(image_oauth()),
            image_provider: Some("google".to_owned()),
            ..StoredSettings::default()
        };
        assert!(
            discover_image_models("chatgpt", None, Some(&settings))
                .await
                .is_err()
        );
        settings.chatgpt_oauth = Some(image_oauth());
        assert_eq!(
            discover_image_models("chatgpt", None, Some(&settings))
                .await
                .unwrap(),
            [CHATGPT_IMAGE_MODEL]
        );
        settings.chatgpt_oauth.as_mut().unwrap().account_id = None;
        assert!(
            discover_image_models("chatgpt", None, Some(&settings))
                .await
                .is_err()
        );
        settings.chatgpt_oauth = None;
        settings.provider = "openai".to_owned();
        assert_eq!(
            discover_image_models("chatgpt", None, Some(&settings))
                .await
                .unwrap(),
            [CHATGPT_IMAGE_MODEL]
        );
        assert!(
            discover_image_models("unknown", Some("secret"), None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn google_and_openrouter_discovery_use_their_own_headers_and_filters() {
        for (provider, path, body, expected) in [
            (
                "google",
                "/v1beta/models",
                r#"{"models":[{"name":"models/gemini-image"},{"name":"models/gemini-text"},{"name":"models/imagen-4.0-generate-001"}]}"#,
                vec!["gemini-image"],
            ),
            (
                "openrouter",
                "/api/v1/images/models",
                r#"{"data":[{"id":"vendor/art"},{"id":"google/gemini-image"}]}"#,
                vec!["google/gemini-image", "vendor/art"],
            ),
        ] {
            let (url, task) = fake_models_server(path, "200 OK", body).await;
            let authentication = ModelAuthentication::ApiKey("provider-image-key".to_owned());
            let models = fetch_provider_models_at(
                &models_client().unwrap(),
                provider,
                &authentication,
                &url,
            )
            .await
            .unwrap();
            assert_eq!(filter_image_models(provider, models).unwrap(), expected);
            let request = task.await.unwrap().to_ascii_lowercase();
            assert!(request.starts_with(&format!("get {path} http/1.1")));
            assert_eq!(
                request.contains("x-goog-api-key: provider-image-key\r\n"),
                provider == "google"
            );
            assert_eq!(
                request.contains("authorization: bearer provider-image-key\r\n"),
                provider == "openrouter"
            );
            assert!(!request.contains("chatgpt-account-id"));
        }
        assert!(filter_image_models("google", vec!["gemini-text".to_owned()]).is_err());
    }

    #[tokio::test]
    async fn text_provider_switch_route_preserves_a_legacy_subscription() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(
            directory.path().join("data/settings.json"),
            "127.0.0.1:3000".to_owned(),
        );
        store
            .write(&StoredSettings {
                provider: "openai".to_owned(),
                model: "text-model".to_owned(),
                oauth: Some(image_oauth()),
                ..StoredSettings::default()
            })
            .await
            .unwrap();
        let application = crate::test_support::test_application(&directory);
        let (status, _) = crate::test_support::send(
            application.clone(),
            "PUT",
            "/settings",
            Some(r#"{"provider":"ollama","model":"new-text"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let saved = store.read().await.unwrap().unwrap();
        assert_eq!(saved.provider, "ollama");
        assert!(saved.oauth.is_none());
        assert_eq!(
            saved.chatgpt_oauth.unwrap().refresh_token,
            "image-refresh-secret"
        );
        assert_eq!(saved.image_provider.as_deref(), Some("chatgpt"));
        let (status, body) =
            crate::test_support::send(application.clone(), "GET", "/settings", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!body.contains("secret"));
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["image_providers"].as_array().unwrap().len(), 4);
        assert_eq!(
            value["image_generation"],
            serde_json::json!({
                "provider":"chatgpt", "configured":true, "model":CHATGPT_IMAGE_MODEL, "source":"chatgpt_subscription"
            })
        );
        let (status, body) = crate::test_support::send(
            application,
            "POST",
            "/settings/image-models",
            Some(r#"{"provider":"chatgpt"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&body).unwrap(),
            [CHATGPT_IMAGE_MODEL]
        );
    }

    #[tokio::test]
    async fn image_settings_route_is_independent_and_clears_keys_on_provider_changes() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(
            directory.path().join("data/settings.json"),
            "127.0.0.1:3000".to_owned(),
        );
        let application = crate::test_support::test_application(&directory);
        for (body, expected_key, expected_model) in [
            (
                r#"{"provider":"google","api_key":" google-secret ","model":" gemini-custom-image "}"#,
                Some("google-secret"),
                Some("gemini-custom-image"),
            ),
            (
                r#"{"provider":"google"}"#,
                Some("google-secret"),
                Some("gemini-custom-image"),
            ),
            (r#"{"provider":"openai"}"#, None, None),
            (
                r#"{"provider":"openrouter","api_key":"router-secret","model":"vendor/model"}"#,
                Some("router-secret"),
                Some("vendor/model"),
            ),
            (
                r#"{"provider":"openrouter","api_key":null,"model":null}"#,
                None,
                None,
            ),
        ] {
            let (status, _) = crate::test_support::send(
                application.clone(),
                "PUT",
                "/settings/images",
                Some(body),
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
            let saved = store.read().await.unwrap().unwrap();
            assert_eq!(saved.provider, "");
            assert_eq!(saved.model, "");
            assert!(saved.api_key.is_none());
            assert!(saved.oauth.is_none());
            assert_eq!(saved.image_api_key.as_deref(), expected_key);
            assert_eq!(saved.image_model.as_deref(), expected_model);
        }
        let before = store.read().await.unwrap().unwrap();
        for route in ["/settings/images", "/settings/image-models"] {
            let method = if route.ends_with("images") {
                "PUT"
            } else {
                "POST"
            };
            let (status, _) = crate::test_support::send(
                application.clone(),
                method,
                route,
                Some(r#"{"provider":"anthropic"}"#),
            )
            .await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        }
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(store.read().await.unwrap().unwrap()).unwrap()
        );
    }

    #[tokio::test]
    async fn combined_settings_updates_clear_only_the_changed_image_provider() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(
            directory.path().join("data/settings.json"),
            "127.0.0.1:3000".to_owned(),
        );
        store
            .write(&StoredSettings {
                provider: "ollama".to_owned(),
                model: "old-text".to_owned(),
                image_provider: Some("google".to_owned()),
                image_api_key: Some("google-secret".to_owned()),
                image_model: Some("google-image".to_owned()),
                chatgpt_oauth: Some(image_oauth()),
                ..StoredSettings::default()
            })
            .await
            .unwrap();
        let application = crate::test_support::test_application(&directory);
        for (body, provider, key, model) in [
            (
                r#"{"provider":"ollama","model":"new-text"}"#,
                "google",
                Some("google-secret"),
                Some("google-image"),
            ),
            (
                r#"{"provider":"ollama","model":"new-text","image_provider":"openai","image_model":"gpt-image-custom"}"#,
                "openai",
                None,
                Some("gpt-image-custom"),
            ),
            (
                r#"{"provider":"ollama","model":"new-text","image_provider":"google","image_api_key":"new-google-secret"}"#,
                "google",
                Some("new-google-secret"),
                None,
            ),
        ] {
            let (status, _) =
                crate::test_support::send(application.clone(), "PUT", "/settings", Some(body))
                    .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
            let saved = store.read().await.unwrap().unwrap();
            assert_eq!(saved.image_provider.as_deref(), Some(provider));
            assert_eq!(saved.image_api_key.as_deref(), key);
            assert_eq!(saved.image_model.as_deref(), model);
            assert_eq!(
                saved.chatgpt_oauth.unwrap().access_token,
                "image-access-secret"
            );
        }
    }

    #[tokio::test]
    async fn image_login_start_route_preserves_text_settings() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(
            directory.path().join("data/settings.json"),
            "127.0.0.1:3000".to_owned(),
        );
        let original = StoredSettings {
            provider: "ollama".to_owned(),
            model: "text-model".to_owned(),
            ..StoredSettings::default()
        };
        store.write(&original).await.unwrap();
        let application = crate::test_support::test_application(&directory);
        let (status, body) = crate::test_support::send(
            application,
            "POST",
            "/settings/images/login/openai/start",
            None,
        )
        .await;
        // An existing user login can already own the registered callback port.
        if status == StatusCode::CONFLICT {
            assert!(body.contains("port 1455"));
        } else {
            assert_eq!(status, StatusCode::OK);
            let value: serde_json::Value = serde_json::from_str(&body).unwrap();
            let url = value["authorize_url"].as_str().unwrap();
            assert!(url.starts_with(OPENAI_AUTHORIZE_URL));
            assert!(url.contains(&url_encode_component(OPENAI_REDIRECT_URL)));
            assert!(url.contains("code_challenge_method=S256"));
            // End this test's listener without exchanging any credentials.
            let response = models_client()
                .unwrap()
                .get(OPENAI_REDIRECT_URL)
                .send()
                .await
                .unwrap();
            assert!(response.text().await.unwrap().contains("Login failed"));
        }
        assert_eq!(
            serde_json::to_value(original).unwrap(),
            serde_json::to_value(store.read().await.unwrap().unwrap()).unwrap()
        );
    }

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
            image_api_key: None,
            image_model: None,
            oauth: None,
            ..StoredSettings::default()
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
    fn image_key_updates_preserve_replace_and_clear() {
        assert_eq!(
            updated_image_setting(None, Some("old".to_owned())).as_deref(),
            Some("old")
        );
        assert_eq!(
            updated_image_setting(Some(" new ".to_owned()), Some("old".to_owned())).as_deref(),
            Some("new")
        );
        assert_eq!(
            updated_image_setting(Some("  ".to_owned()), Some("old".to_owned())),
            None
        );
    }

    #[tokio::test]
    async fn put_settings_preserves_replaces_and_clears_the_image_key() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(
            directory.path().join("settings.json"),
            "127.0.0.1:3000".to_owned(),
        );
        store
            .write(&StoredSettings {
                provider: "ollama".to_owned(),
                model: "llama3.1".to_owned(),
                api_key: None,
                image_api_key: Some("old".to_owned()),
                image_model: Some("gpt-image-custom".to_owned()),
                oauth: None,
                ..StoredSettings::default()
            })
            .await
            .unwrap();
        for (requested, expected) in [
            (None, Some("old")),
            (Some("new".to_owned()), Some("new")),
            (Some(String::new()), None),
        ] {
            let response = put_settings(
                State(store.clone()),
                State(ChangeNotifier::new()),
                Json(SettingsRequest {
                    provider: "ollama".to_owned(),
                    model: "llama3.1".to_owned(),
                    api_key: None,
                    image_api_key: requested,
                    image_provider: None,
                    image_model: None,
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            assert_eq!(
                store.read().await.unwrap().unwrap().image_model.as_deref(),
                Some("gpt-image-custom")
            );
            assert_eq!(
                store
                    .read()
                    .await
                    .unwrap()
                    .unwrap()
                    .image_api_key
                    .as_deref(),
                expected
            );
        }
    }

    #[test]
    fn stored_settings_round_trip_the_image_key_but_public_status_does_not() {
        let settings = StoredSettings {
            provider: "anthropic".to_owned(),
            model: "claude-sonnet-5".to_owned(),
            api_key: Some("chat-secret".to_owned()),
            image_api_key: Some("image-secret".to_owned()),
            image_model: Some("gpt-image-custom".to_owned()),
            oauth: None,
            ..StoredSettings::default()
        };
        let serialized = serde_json::to_string(&settings).unwrap();
        let restored: StoredSettings = serde_json::from_str(&serialized).unwrap();
        assert_eq!(restored.image_api_key.as_deref(), Some("image-secret"));
        let summary = image_generation_summary(Some(&restored));
        assert_eq!(summary["configured"], true);
        assert_eq!(summary["model"], "gpt-image-custom");
        let without_key = StoredSettings {
            image_api_key: None,
            ..restored
        };
        assert_eq!(
            image_generation_summary(Some(&without_key))["model"],
            "gpt-image-custom"
        );
        assert!(!summary.to_string().contains("image-secret"));
    }

    #[test]
    fn chatgpt_login_reports_subscription_image_generation() {
        let settings = StoredSettings {
            provider: "openai".to_owned(),
            model: "gpt-text".to_owned(),
            oauth: Some(StoredOauth {
                access_token: "oauth-secret".to_owned(),
                refresh_token: "refresh-secret".to_owned(),
                expires_at_unix_seconds: u64::MAX,
                account_id: Some("account-123".to_owned()),
            }),
            ..StoredSettings::default()
        };

        assert_eq!(
            image_generation_summary(Some(&settings)),
            serde_json::json!({
                "configured": true,
                "provider": "chatgpt",
                "model": "gpt-image-2",
                "source": "chatgpt_subscription"
            })
        );
    }

    #[test]
    fn old_settings_without_an_image_key_still_deserialize() {
        let settings: StoredSettings = serde_json::from_value(serde_json::json!({
            "provider": "ollama",
            "model": "llama3.1"
        }))
        .unwrap();
        assert!(settings.image_api_key.is_none());
        assert!(settings.image_model.is_none());
        assert!(settings.image_provider.is_none());
        assert!(settings.chatgpt_oauth.is_none());
    }

    #[tokio::test]
    async fn image_model_updates_survive_text_provider_changes_and_reset_to_default() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(
            directory.path().join("settings.json"),
            "127.0.0.1:3000".to_owned(),
        );
        store
            .write(&StoredSettings {
                provider: "openai".to_owned(),
                model: "text-model".to_owned(),
                image_provider: Some("openai".to_owned()),
                image_api_key: Some("image-secret".to_owned()),
                image_model: Some("gpt-image-saved".to_owned()),
                oauth: Some(StoredOauth {
                    access_token: "oauth-secret".to_owned(),
                    refresh_token: "refresh-secret".to_owned(),
                    expires_at_unix_seconds: u64::MAX,
                    account_id: Some("account".to_owned()),
                }),
                ..StoredSettings::default()
            })
            .await
            .unwrap();
        for (provider, update, expected) in [
            ("openai", None, Some("gpt-image-saved")),
            ("ollama", None, Some("gpt-image-saved")),
            (
                "ollama",
                Some(" gpt-image-custom "),
                Some("gpt-image-custom"),
            ),
            ("ollama", Some("  "), None),
        ] {
            let response = put_settings(
                State(store.clone()),
                State(ChangeNotifier::new()),
                Json(SettingsRequest {
                    provider: provider.to_owned(),
                    model: "new-text-model".to_owned(),
                    api_key: None,
                    image_api_key: None,
                    image_provider: None,
                    image_model: update.map(str::to_owned),
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            let saved = store.read().await.unwrap().unwrap();
            assert_eq!(saved.image_model.as_deref(), expected);
            assert_eq!(saved.image_api_key.as_deref(), Some("image-secret"));
            assert_eq!(saved.oauth.is_some(), provider == "openai");
            assert_eq!(saved.image_provider.as_deref(), Some("openai"));
            assert_eq!(
                saved.chatgpt_oauth.as_ref().unwrap().access_token,
                "oauth-secret"
            );
            assert_eq!(
                image_generation_summary(Some(&saved))["model"],
                expected.unwrap_or("gpt-image-1")
            );
        }
    }

    #[test]
    fn every_login_keeps_image_settings_while_replacing_text_credentials() {
        for provider in ["anthropic", "openrouter", "openai"] {
            let existing = StoredSettings {
                provider: "old-provider".to_owned(),
                model: "old-text-model".to_owned(),
                api_key: Some("old-text-key".to_owned()),
                image_api_key: Some("image-key".to_owned()),
                image_model: Some("gpt-image-exact".to_owned()),
                chatgpt_oauth: Some(StoredOauth {
                    access_token: "image-access-token".to_owned(),
                    refresh_token: "image-refresh-token".to_owned(),
                    expires_at_unix_seconds: u64::MAX,
                    account_id: Some("image-account".to_owned()),
                }),
                ..StoredSettings::default()
            };
            let choice = StoredSettings {
                provider: provider.to_owned(),
                model: "new-text-model".to_owned(),
                api_key: (provider == "openrouter").then(|| "new-key".to_owned()),
                oauth: (provider != "openrouter").then(|| StoredOauth {
                    access_token: "new-access-token".to_owned(),
                    refresh_token: "new-refresh-token".to_owned(),
                    expires_at_unix_seconds: u64::MAX,
                    account_id: Some("account".to_owned()),
                }),
                ..StoredSettings::default()
            };
            let fresh = settings_after_login(None, choice.clone());
            assert!(fresh.image_model.is_none());
            assert!(fresh.image_api_key.is_none());
            let saved = settings_after_login(Some(existing), choice);
            assert_eq!(saved.image_model.as_deref(), Some("gpt-image-exact"));
            assert_eq!(saved.image_api_key.as_deref(), Some("image-key"));
            assert_eq!(saved.provider, provider);
            assert_eq!(saved.model, "new-text-model");
            assert_eq!(saved.oauth.is_some(), provider != "openrouter");
            assert_eq!(saved.api_key, fresh.api_key);
            assert_eq!(
                saved.chatgpt_oauth.as_ref().unwrap().account_id.as_deref(),
                Some("image-account")
            );
        }
    }

    #[tokio::test]
    async fn settings_routes_accept_image_model_updates_and_hide_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let application = crate::test_support::test_application(&directory);
        let (status, _) = crate::test_support::send(application.clone(), "PUT", "/settings", Some(
            r#"{"provider":"ollama","model":"text-model","image_api_key":"image-secret","image_model":" gpt-image-custom "}"#
        )).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) =
            crate::test_support::send(application.clone(), "GET", "/settings", None).await;
        assert_eq!(status, StatusCode::OK);
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            value["image_generation"],
            serde_json::json!({
                "configured": true,
                "provider": "openai",
                "model": "gpt-image-custom",
                "source": "api_key"
            })
        );
        assert!(!body.contains("image-secret"));
        let (status, body) = crate::test_support::send(
            application,
            "POST",
            "/settings/image-models",
            Some(r#"{"image_api_key":"invalid\nsecret"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!body.contains("secret"));
        assert!(body.contains("request to openai failed"));
    }

    #[test]
    fn settings_json_distinguishes_omitted_reset_and_selected_image_model() {
        let mut value = serde_json::json!({"provider":"ollama", "model":"text-model"});
        let request: SettingsRequest = serde_json::from_value(value.clone()).unwrap();
        assert!(request.image_model.is_none());
        assert!(request.image_api_key.is_none());
        for model in ["", "gpt-image-exact"] {
            value["image_model"] = serde_json::json!(model);
            let request: SettingsRequest = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(request.image_model.as_deref(), Some(model));
            assert!(request.image_api_key.is_none());
        }
    }

    #[test]
    fn image_models_filter_mixed_ids_and_report_no_compatible_models() {
        let value = serde_json::json!({"data": [
            {"id":"gpt-5"}, {"id":"dall-e-3"}, {"id":"gpt-image-1-mini"},
            {"id":"gpt-image-1"}, {"id":"gpt-image-1"}, {"id":"chatgpt-image-latest"},
            {"id":"gpt-image-"}, {"id":"vendor/gpt-image-1"}, {"id":"imagen-4"}
        ]});
        assert_eq!(
            compatible_image_models(parse_model_ids(&value)).unwrap(),
            ["gpt-image-1", "gpt-image-1-mini"]
        );
        let error = compatible_image_models(vec!["private-text-model".to_owned()]).unwrap_err();
        assert!(error.contains("no compatible GPT Image models"));
        assert!(!error.contains("private-text-model"));
    }

    #[test]
    fn image_discovery_uses_only_dedicated_keys_in_priority_order() {
        let mut stored = StoredSettings {
            api_key: Some("text-secret".to_owned()),
            oauth: Some(StoredOauth {
                access_token: "oauth-secret".to_owned(),
                refresh_token: "refresh".to_owned(),
                expires_at_unix_seconds: u64::MAX,
                account_id: Some("account".to_owned()),
            }),
            ..StoredSettings::default()
        };
        assert!(image_models_authentication("openai", None, Some(&stored), None).is_err());
        for (requested, saved, environment, expected) in [
            (Some("typed"), Some("saved"), Some("environment"), "typed"),
            (None, Some("saved"), Some("environment"), "saved"),
            (Some("  "), None, Some("environment"), "environment"),
        ] {
            stored.image_api_key = saved.map(str::to_owned);
            let authentication = image_models_authentication(
                "openai",
                requested,
                Some(&stored),
                environment.map(str::to_owned),
            )
            .unwrap();
            let request = provider_models_request(
                &models_client().unwrap(),
                "openai",
                &authentication,
                OPENAI_IMAGE_MODELS_URL,
            )
            .unwrap()
            .build()
            .unwrap();
            assert_eq!(request.url().as_str(), "https://api.openai.com/v1/models");
            assert_eq!(
                request.headers()["authorization"],
                format!("Bearer {expected}")
            );
            assert!(!request.headers().contains_key("chatgpt-account-id"));
            assert!(!request.headers().contains_key("openai-beta"));
        }
    }

    #[tokio::test]
    async fn image_model_route_uses_the_subscription_or_an_explicit_platform_key() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(
            directory.path().join("settings.json"),
            "127.0.0.1:3000".to_owned(),
        );
        store
            .write(&StoredSettings {
                provider: "openai".to_owned(),
                image_api_key: Some("invalid\nimage-key".to_owned()),
                oauth: Some(StoredOauth {
                    access_token: "oauth-secret".to_owned(),
                    refresh_token: "refresh".to_owned(),
                    expires_at_unix_seconds: u64::MAX,
                    account_id: Some("account".to_owned()),
                }),
                ..StoredSettings::default()
            })
            .await
            .unwrap();

        let response = list_image_models(
            State(store.clone()),
            Json(ImageModelsRequest {
                provider: None,
                image_api_key: None,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!(["gpt-image-2"])
        );

        let response = list_image_models(
            State(store),
            Json(ImageModelsRequest {
                provider: Some("openai".to_owned()),
                image_api_key: Some("typed\nsecret".to_owned()),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["message"], "request to openai failed");
    }

    #[tokio::test]
    async fn live_image_discovery_filters_the_official_response_shape() {
        let (url, request_task) = fake_models_server(
            "/v1/models",
            "200 OK",
            r#"{"data":[{"id":"gpt-5"},{"id":"gpt-image-1"},{"id":"dall-e-3"}]}"#,
        )
        .await;
        let authentication =
            image_models_authentication("openai", Some("image-key"), None, None).unwrap();
        let models =
            fetch_provider_models_at(&models_client().unwrap(), "openai", &authentication, &url)
                .await
                .unwrap();
        assert_eq!(compatible_image_models(models).unwrap(), ["gpt-image-1"]);
        let request = request_task.await.unwrap().to_ascii_lowercase();
        assert!(request.contains("authorization: bearer image-key\r\n"));
        assert!(!request.contains("chatgpt-account-id"));
    }

    #[test]
    fn model_ids_parse_from_data_entries() {
        let value = serde_json::json!({
            "data": [
                { "id": "models/gemini-2.5-pro" },
                { "name": "named-model" },
                { "slug": "slugged-model" },
                { "id": "named-model" }
            ]
        });
        assert_eq!(
            parse_model_ids(&value),
            ["gemini-2.5-pro", "named-model", "slugged-model"]
        );
    }

    #[test]
    fn model_ids_parse_from_codex_model_entries() {
        let value = serde_json::json!({
            "models": [
                { "slug": "models/gpt-codex" },
                { "id": "fallback-id" },
                { "name": "fallback-name" }
            ]
        });
        assert_eq!(
            parse_model_ids(&value),
            ["fallback-id", "fallback-name", "gpt-codex"]
        );
    }

    #[test]
    fn chatgpt_model_url_has_the_server_version() {
        assert_eq!(
            chatgpt_models_url(),
            format!(
                "https://chatgpt.com/backend-api/codex/models?client_version={}",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    #[tokio::test]
    async fn chatgpt_model_request_uses_codex_url_and_oauth_headers() {
        let path = format!(
            "/backend-api/codex/models?client_version={}",
            env!("CARGO_PKG_VERSION")
        );
        let (url, request_task) =
            fake_models_server(&path, "200 OK", r#"{"models":[{"slug":"gpt-live"}]}"#).await;
        let client = reqwest::Client::new();
        let authentication = ModelAuthentication::OAuth(StoredOauth {
            access_token: "test-oauth-token".to_owned(),
            refresh_token: "test-refresh-token".to_owned(),
            expires_at_unix_seconds: unix_now_seconds() + 3600,
            account_id: Some("test-account".to_owned()),
        });

        let models = fetch_provider_models_at(&client, "openai", &authentication, &url)
            .await
            .unwrap();
        let request = request_task.await.unwrap().to_ascii_lowercase();

        assert_eq!(models, ["gpt-live"]);
        assert!(request.starts_with(&format!("get {path} http/1.1\r\n")));
        assert!(request.contains("authorization: bearer test-oauth-token\r\n"));
        assert!(request.contains("chatgpt-account-id: test-account\r\n"));
        assert!(request.contains("openai-beta: responses=experimental\r\n"));
        assert!(request.contains("originator: codex_cli_rs\r\n"));
        assert!(request.contains("accept: application/json\r\n"));
    }

    #[tokio::test]
    async fn anthropic_model_request_uses_oauth_headers() {
        let (url, request_task) =
            fake_models_server("/v1/models", "200 OK", r#"{"data":[{"id":"claude-live"}]}"#).await;
        let client = reqwest::Client::new();
        let authentication = ModelAuthentication::OAuth(StoredOauth {
            access_token: "test-anthropic-token".to_owned(),
            refresh_token: "test-refresh-token".to_owned(),
            expires_at_unix_seconds: unix_now_seconds() + 3600,
            account_id: None,
        });

        let models = fetch_provider_models_at(&client, "anthropic", &authentication, &url)
            .await
            .unwrap();
        let request = request_task.await.unwrap().to_ascii_lowercase();

        assert_eq!(models, ["claude-live"]);
        assert!(request.starts_with("get /v1/models http/1.1\r\n"));
        assert!(request.contains("authorization: bearer test-anthropic-token\r\n"));
        assert!(request.contains("anthropic-version: 2023-06-01\r\n"));
        assert!(request.contains("anthropic-beta: oauth-2025-04-20\r\n"));
        assert!(!request.contains("x-api-key"));
    }

    #[tokio::test]
    async fn anthropic_api_key_model_request_uses_x_api_key() {
        let (url, request_task) =
            fake_models_server("/v1/models", "200 OK", r#"{"data":[{"id":"claude-live"}]}"#).await;
        let client = reqwest::Client::new();
        let authentication = ModelAuthentication::ApiKey("test-anthropic-key".to_owned());

        let models = fetch_provider_models_at(&client, "anthropic", &authentication, &url)
            .await
            .unwrap();
        let request = request_task.await.unwrap().to_ascii_lowercase();

        assert_eq!(models, ["claude-live"]);
        assert!(request.contains("x-api-key: test-anthropic-key\r\n"));
        assert!(request.contains("anthropic-version: 2023-06-01\r\n"));
        assert!(!request.contains("authorization"));
        assert!(!request.contains("anthropic-beta"));
    }

    #[tokio::test]
    async fn provider_status_errors_omit_the_response_body() {
        let (url, request_task) = fake_models_server(
            "/v1/models",
            "401 Unauthorized",
            r#"{"token":"provider-response-secret"}"#,
        )
        .await;
        let client = reqwest::Client::new();
        let authentication = ModelAuthentication::ApiKey("test-api-key".to_owned());

        let error = fetch_provider_models_at(&client, "openai", &authentication, &url)
            .await
            .unwrap_err();
        request_task.await.unwrap();

        assert_eq!(error, "openai returned 401 Unauthorized");
        assert!(!error.contains("provider-response-secret"));
        assert!(!error.contains("test-api-key"));
    }

    #[tokio::test]
    async fn model_listing_reports_settings_read_errors() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(directory.path().to_owned(), "127.0.0.1:3000".to_owned());

        let response = list_provider_models(
            State(store),
            Json(ModelsRequest {
                provider: "openai".to_owned(),
                api_key: None,
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
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
                crate::model_client::provider_chat_url(provider.name).is_some(),
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
