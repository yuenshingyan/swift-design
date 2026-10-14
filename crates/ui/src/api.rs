//! HTTP calls to the Swift Design server.
//!
//! Every function returns `Result<T, String>`, except `save_design`,
//! which returns the server's full validation error list. The shared
//! domain types come from `design_model`, so the studio and the server
//! agree on one definition.
//!
//! Some endpoints are part of the client surface but not yet wired to a
//! screen, so the whole module allows dead code.
#![allow(dead_code)]

use std::collections::HashMap;

use design_model::{
    AdSize, ArtifactKind, Artwork, BriefQuestionSet, Campaign, CoverSize, DECK_VIEWPORT, Deck,
    Design, Document, EmailFormat, Format, Mailing, Orientation, Paper, Print, PrintSize,
    QuestionAnswer, Social, WorkflowState,
};
use gloo_net::http::{Request, RequestBuilder, Response};
use serde::{Deserialize, Serialize};

mod agents_settings_login;
mod artworks;
mod campaigns;
mod decks;
mod designs;
mod documents;
mod mailings;
mod prints;
mod sessions;
mod socials;
mod templates_uploads_events;

pub use agents_settings_login::*;
pub use artworks::*;
pub use campaigns::*;
pub use decks::*;
pub use designs::*;
pub use documents::*;
pub use mailings::*;
pub use prints::*;
pub use sessions::*;
pub use socials::*;
pub use templates_uploads_events::*;

pub(crate) use designs::{AuthorsResponse, PENDING_SCREEN_CLASS};

fn built(builder: RequestBuilder) -> Result<Request, String> {
    builder.build().map_err(|error| error.to_string())
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

/// Sends `request` and returns the response when it is 2xx, else the
/// server's envelope message or a status line.
async fn send_checked(request: Request, label: &str) -> Result<Response, String> {
    let response = request.send().await.map_err(|error| error.to_string())?;
    if response.ok() {
        return Ok(response);
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) => Err(envelope.error.message),
        Err(_) => Err(format!("{label} failed with status {status}")),
    }
}

/// Reads a 2xx JSON body, or the envelope message.
async fn get_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T, String> {
    let response = send_checked(built(Request::get(url))?, url).await?;
    response.json().await.map_err(|error| error.to_string())
}

/// Sends a POST or DELETE with no body, checking the response.
async fn send_empty(builder: RequestBuilder, label: &str) -> Result<(), String> {
    send_checked(built(builder)?, label).await.map(|_| ())
}

#[cfg(test)]
mod tests;
