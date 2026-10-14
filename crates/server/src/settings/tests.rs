#![allow(clippy::unwrap_used)]

use axum::extract::Query;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};

use crate::settings::{
    CATALOG, CHATGPT_CATALOG_CLIENT_VERSION, SettingsStore, StoredOauth, StoredSettings,
    anthropic_authorize_url, base64_url_decode, base64_url_encode, chatgpt_account_id,
    chatgpt_model_ids, fetch_chatgpt_models, is_login_expiring, load_chatgpt_models,
    models_response, pkce_pair, provider_chat_url, unix_now_seconds, url_encode_component,
};

#[test]
fn subscription_catalog_accepts_new_slugs_and_omits_hidden_models() {
    let value = serde_json::json!({ "models": [
        { "slug": "gpt-6.1-sol", "visibility": "list" },
        { "slug": "gpt-6.0-sol", "visibility": "list" },
        { "slug": "gpt-6.1-sol", "visibility": "list" },
        { "slug": "hidden", "visibility": "hide" },
        { "slug": "internal", "visibility": "none" },
        { "slug": "", "visibility": "list" },
        { "visibility": "list" }
    ] });
    assert_eq!(
        chatgpt_model_ids(&value).unwrap(),
        vec!["gpt-6.0-sol", "gpt-6.1-sol"]
    );
}

#[test]
fn subscription_catalog_rejects_invalid_or_empty_lists() {
    for value in [
        serde_json::json!({ "data": [{ "id": "wrong-format" }] }),
        serde_json::json!({ "models": [] }),
        serde_json::json!({ "models": [{ "slug": "hidden", "visibility": "hide" }] }),
    ] {
        assert!(chatgpt_model_ids(&value).is_err());
    }
}

#[test]
fn model_lookup_errors_use_the_api_error_status() {
    assert_eq!(
        models_response(Err("lookup failed".to_owned())).status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        models_response(Ok(vec!["new-model".to_owned()])).status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn subscription_catalog_requires_a_login() {
    let directory = tempfile::tempdir().unwrap();
    let store = SettingsStore::new(
        directory.path().join("settings.json"),
        "127.0.0.1:3001".to_owned(),
    );
    let mut settings = StoredSettings::default();
    assert_eq!(
        load_chatgpt_models(&store, &mut settings)
            .await
            .unwrap_err(),
        "sign in to ChatGPT first"
    );
}

#[tokio::test]
async fn subscription_catalog_uses_authenticated_live_lookup() {
    let router = Router::new()
        .route("/models", get(subscription_catalog_reply))
        .route(
            "/denied",
            get(|| async { (StatusCode::UNAUTHORIZED, "private provider response") }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let oauth = StoredOauth {
        access_token: "test-token".to_owned(),
        refresh_token: "test-refresh".to_owned(),
        expires_at_unix_seconds: unix_now_seconds() + 3600,
        account_id: Some("test-account".to_owned()),
    };
    let models = fetch_chatgpt_models(&oauth, &format!("http://{address}/models"))
        .await
        .unwrap();
    assert_eq!(models, vec!["gpt-6.1-sol"]);
    let error = fetch_chatgpt_models(&oauth, &format!("http://{address}/denied"))
        .await
        .unwrap_err();
    assert!(error.contains("401"));
    assert!(!error.contains("private provider response"));
    assert!(!error.contains("test-token"));
    server.abort();
}

async fn subscription_catalog_reply(
    headers: HeaderMap,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    assert_eq!(headers["authorization"], "Bearer test-token");
    assert_eq!(headers["chatgpt-account-id"], "test-account");
    assert_eq!(headers["originator"], "codex_cli_rs");
    assert_eq!(query["client_version"], CHATGPT_CATALOG_CLIENT_VERSION);
    Json(serde_json::json!({ "models": [
        { "slug": "gpt-6.1-sol", "visibility": "list" }
    ] }))
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
        base64_url_encode(claims.to_string().as_bytes()),
    );
    assert_eq!(chatgpt_account_id(&token).as_deref(), Some("account-123"));
    assert_eq!(chatgpt_account_id("not-a-jwt"), None);
}

#[test]
fn base64_url_decoding_reverses_encoding() {
    let bytes = b"swift-design?~\xfb\xff";
    let encoded = base64_url_encode(bytes);
    assert_eq!(base64_url_decode(&encoded).unwrap(), bytes.to_vec());
    assert_eq!(base64_url_decode("!!"), None);
}

#[test]
fn url_components_are_percent_encoded() {
    assert_eq!(
        url_encode_component("http://127.0.0.1:3000/a b"),
        "http%3A%2F%2F127.0.0.1%3A3000%2Fa%20b"
    );
    assert_eq!(url_encode_component("safe-._~09Az"), "safe-._~09Az");
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
            provider_chat_url(provider.name).is_some(),
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
