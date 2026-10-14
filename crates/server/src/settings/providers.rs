use super::*;

/// Where OpenRouter logins start.
pub(super) const OPENROUTER_AUTH_URL: &str = "https://openrouter.ai/auth";

/// Where the OpenRouter login code becomes an API key.
pub(super) const OPENROUTER_KEY_EXCHANGE_URL: &str = "https://openrouter.ai/api/v1/auth/keys";

/// Percent-encodes a URL component.
pub(super) fn url_encode_component(value: &str) -> String {
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
pub(super) struct ModelsRequest {
    /// Provider to list models for.
    provider: String,
    /// API key to use. Falls back to the stored key for this provider.
    #[serde(default)]
    api_key: Option<String>,
}

/// Lists the provider's live models with the user's credentials.
pub(super) async fn list_provider_models(
    State(store): State<SettingsStore>,
    Json(request): Json<ModelsRequest>,
) -> Response {
    let mut api_key = request.api_key.filter(|key| !key.trim().is_empty());
    if api_key.is_none()
        && let Ok(Some(mut stored)) = store.read().await
        && stored.provider == request.provider
    {
        if request.provider == "openai" && stored.oauth.is_some() {
            return models_response(load_chatgpt_models(&store, &mut stored).await);
        }
        api_key = stored.api_key;
    }
    models_response(fetch_provider_models(&request.provider, api_key.as_deref()).await)
}

/// Converts a model lookup into the settings API response.
pub(super) fn models_response(result: Result<Vec<String>, String>) -> Response {
    match result {
        Ok(models) => Json(models).into_response(),
        Err(message) => {
            api_error::error_response(StatusCode::UNPROCESSABLE_ENTITY, &message, Vec::new())
        }
    }
}

/// Loads the subscription catalog, renewing expired credentials first.
pub(super) async fn load_chatgpt_models(
    store: &SettingsStore,
    settings: &mut StoredSettings,
) -> Result<Vec<String>, String> {
    let oauth = settings.oauth.as_ref().ok_or("sign in to ChatGPT first")?;
    if is_login_expiring(oauth) {
        let renewed = refresh_chatgpt_login(&oauth.refresh_token)
            .await
            .map_err(|_| "could not renew the ChatGPT login; sign in again".to_owned())?;
        settings.oauth = Some(renewed);
        store
            .write(settings)
            .await
            .map_err(|_| "could not save the renewed ChatGPT login".to_owned())?;
    }
    let oauth = settings.oauth.as_ref().ok_or("sign in to ChatGPT first")?;
    fetch_chatgpt_models(oauth, CHATGPT_MODELS_URL).await
}

/// Requests the live Codex catalog with the user's subscription credentials.
pub(super) async fn fetch_chatgpt_models(
    oauth: &StoredOauth,
    url: &str,
) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|_| "could not create the ChatGPT models client".to_owned())?;
    let mut request = client
        .get(url)
        .query(&[("client_version", CHATGPT_CATALOG_CLIENT_VERSION)])
        .bearer_auth(&oauth.access_token)
        .header("originator", "codex_cli_rs");
    if let Some(account_id) = &oauth.account_id {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    let response = request
        .send()
        .await
        .map_err(|_| "request for ChatGPT models failed; try reloading the list".to_owned())?;
    if !response.status().is_success() {
        return Err(format!(
            "ChatGPT model lookup returned {}; sign in again or retry",
            response.status()
        ));
    }
    let value = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| "ChatGPT returned an invalid model catalog".to_owned())?;
    chatgpt_model_ids(&value)
}

/// Reads picker-visible model slugs without a local model allowlist.
pub(super) fn chatgpt_model_ids(value: &serde_json::Value) -> Result<Vec<String>, String> {
    let entries = value["models"]
        .as_array()
        .ok_or("ChatGPT returned an invalid model catalog")?;
    let mut models: Vec<String> = entries
        .iter()
        .filter(|entry| entry["visibility"].as_str() == Some("list"))
        .filter_map(|entry| entry["slug"].as_str())
        .filter(|slug| !slug.trim().is_empty())
        .map(str::to_owned)
        .collect();
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err("ChatGPT returned no selectable models".to_owned());
    }
    Ok(models)
}

/// Asks the provider's models endpoint for its current model ids.
pub(super) async fn fetch_provider_models(
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
        let chat_url =
            provider_chat_url(provider).ok_or_else(|| format!("unknown provider `{provider}`"))?;
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
pub(super) async fn start_openrouter_login(State(store): State<SettingsStore>) -> Response {
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
pub(super) struct OpenrouterCallbackQuery {
    /// The authorization code OpenRouter appends.
    code: String,
}

/// Finishes an OpenRouter login: exchanges the code for an API key and
/// stores it. The browser lands here from OpenRouter.
pub(super) async fn openrouter_login_callback(
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

/// Initial model choice before the live subscription catalog is loaded.
pub const CHATGPT_DEFAULT_MODEL: &str = "gpt-5.5";

/// The subscription model catalog endpoint used by Codex.
pub(super) const CHATGPT_MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";

// The catalog uses the Codex client version for protocol compatibility.
// This is not a model allowlist: new slugs come from the live response.
pub(super) const CHATGPT_CATALOG_CLIENT_VERSION: &str = "0.161.0";

/// The OAuth client id of the Codex CLI public client, used for
/// ChatGPT-subscription logins (the flow pi and Codex use).
pub(super) const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// Where ChatGPT logins start.
pub(super) const OPENAI_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";

/// Where the ChatGPT login code becomes tokens.
pub(super) const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

/// The registered redirect of the Codex public client. A short-lived
/// local listener on port 1455 catches it.
pub(super) const OPENAI_REDIRECT_URL: &str = "http://localhost:1455/auth/callback";

/// Decodes URL-safe base64 without padding.
pub(super) fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
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
pub(super) fn chatgpt_account_id(access_token: &str) -> Option<String> {
    let payload = access_token.split('.').nth(1)?;
    let decoded = base64_url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims["https://api.openai.com/auth"]["chatgpt_account_id"]
        .as_str()
        .map(str::to_owned)
}

/// Exchanges a ChatGPT login code for tokens.
pub(super) async fn exchange_openai_code(
    code: &str,
    verifier: &str,
) -> Result<StoredOauth, String> {
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
pub(super) async fn start_openai_login(
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
pub(super) async fn run_openai_callback_listener(
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
                        if stored.provider == "openai" && !stored.model.trim().is_empty() =>
                    {
                        stored.model
                    }
                    _ => CHATGPT_DEFAULT_MODEL.to_owned(),
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
