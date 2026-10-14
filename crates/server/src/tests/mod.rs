use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use crate::test_support::{
    SAMPLE_ARTWORK, SAMPLE_CAMPAIGN, SAMPLE_DECK, SAMPLE_DESIGN, SAMPLE_DOCUMENT, SAMPLE_MAILING,
    SAMPLE_PRINT, SAMPLE_SOCIAL, application_with_command, invalid_sample_design,
    open_generating_artwork_session, open_generating_campaign_session,
    open_generating_deck_session, open_generating_document_session,
    open_generating_mailing_session, open_generating_print_session, open_generating_session,
    open_generating_social_session, send, send_upload, send_user_put, test_application,
};

mod artwork;
mod campaign;
mod deck;
mod design;
mod document;
mod exports;
mod infrastructure;
mod mailing;
mod print;
mod social;
