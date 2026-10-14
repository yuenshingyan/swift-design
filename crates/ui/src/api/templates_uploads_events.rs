use super::*;

// -- Templates -----------------------------------------------------------

/// One row of `GET /templates`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TemplateSummary {
    /// Template id.
    pub id: String,
    /// The name the user gave the template.
    pub name: String,
    /// When it was saved, RFC 3339.
    pub saved_at: String,
    /// Theme name.
    pub theme: String,
    /// How many example screens it holds.
    pub screen_count: usize,
    /// True when a new session starts with this template picked.
    #[serde(default)]
    pub is_default: bool,
}

/// Body of `POST /templates/extract`: one of `url` and `uploads`.
#[derive(Serialize)]
struct ExtractTemplateRequest<'value> {
    name: &'value str,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<&'value str>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    uploads: &'value [String],
    scope: &'value str,
}

/// Makes a template from a website or from uploaded brand files in
/// `scope`: the model reads them and answers with a theme.
pub async fn extract_template(
    name: &str,
    url: Option<&str>,
    uploads: &[String],
    scope: &str,
) -> Result<TemplateSummary, String> {
    let builder = Request::post("/templates/extract")
        .json(&ExtractTemplateRequest {
            name,
            url,
            uploads,
            scope,
        })
        .map_err(|error| error.to_string())?;
    let response = send_checked(builder, "POST /templates/extract").await?;
    response.json().await.map_err(|error| error.to_string())
}

/// Body of `PUT /templates/{id}/default`.
#[derive(Serialize)]
struct DefaultTemplateRequest {
    is_default: bool,
}

/// Marks a template as one every new session starts with, or clears
/// the mark.
pub async fn set_default_template(id: &str, is_default: bool) -> Result<(), String> {
    let builder = Request::put(&format!("/templates/{id}/default"))
        .json(&DefaultTemplateRequest { is_default })
        .map_err(|error| error.to_string())?;
    send_checked(builder, "PUT /templates/default")
        .await
        .map(|_| ())
}

/// Body of `POST /templates`. Exactly one source id is set.
#[derive(Debug, Serialize)]
struct SaveTemplateRequest<'value> {
    #[serde(skip_serializing_if = "Option::is_none")]
    design_id: Option<&'value str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deck_id: Option<&'value str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    document_id: Option<&'value str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    social_id: Option<&'value str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    print_id: Option<&'value str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mailing_id: Option<&'value str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    campaign_id: Option<&'value str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artwork_id: Option<&'value str>,
    name: &'value str,
}

/// Fetches the saved templates, newest first.
pub async fn fetch_templates() -> Result<Vec<TemplateSummary>, String> {
    get_json("/templates").await
}

/// Saves the style of one design as a template.
pub async fn save_template(design_id: &str, name: &str) -> Result<TemplateSummary, String> {
    save_template_from(SaveTemplateRequest {
        design_id: Some(design_id),
        deck_id: None,
        document_id: None,
        social_id: None,
        print_id: None,
        mailing_id: None,
        campaign_id: None,
        artwork_id: None,
        name,
    })
    .await
}

/// Saves the style of one deck as a template.
pub async fn save_deck_template(deck_id: &str, name: &str) -> Result<TemplateSummary, String> {
    save_template_from(SaveTemplateRequest {
        design_id: None,
        deck_id: Some(deck_id),
        document_id: None,
        social_id: None,
        print_id: None,
        mailing_id: None,
        campaign_id: None,
        artwork_id: None,
        name,
    })
    .await
}

/// Saves the style of one document as a template.
pub async fn save_document_template(
    document_id: &str,
    name: &str,
) -> Result<TemplateSummary, String> {
    save_template_from(SaveTemplateRequest {
        design_id: None,
        deck_id: None,
        document_id: Some(document_id),
        social_id: None,
        print_id: None,
        mailing_id: None,
        campaign_id: None,
        artwork_id: None,
        name,
    })
    .await
}

/// Saves the style of one social as a template.
pub async fn save_social_template(social_id: &str, name: &str) -> Result<TemplateSummary, String> {
    save_template_from(SaveTemplateRequest {
        design_id: None,
        deck_id: None,
        document_id: None,
        social_id: Some(social_id),
        print_id: None,
        mailing_id: None,
        campaign_id: None,
        artwork_id: None,
        name,
    })
    .await
}

/// Saves the style of one print as a template.
pub async fn save_print_template(print_id: &str, name: &str) -> Result<TemplateSummary, String> {
    save_template_from(SaveTemplateRequest {
        design_id: None,
        deck_id: None,
        document_id: None,
        social_id: None,
        print_id: Some(print_id),
        mailing_id: None,
        campaign_id: None,
        artwork_id: None,
        name,
    })
    .await
}

/// Saves the style of one mailing as a template.
pub async fn save_mailing_template(
    mailing_id: &str,
    name: &str,
) -> Result<TemplateSummary, String> {
    save_template_from(SaveTemplateRequest {
        design_id: None,
        deck_id: None,
        document_id: None,
        social_id: None,
        print_id: None,
        mailing_id: Some(mailing_id),
        campaign_id: None,
        artwork_id: None,
        name,
    })
    .await
}

/// Saves the style of one campaign as a template.
pub async fn save_campaign_template(
    campaign_id: &str,
    name: &str,
) -> Result<TemplateSummary, String> {
    save_template_from(SaveTemplateRequest {
        design_id: None,
        deck_id: None,
        document_id: None,
        social_id: None,
        print_id: None,
        mailing_id: None,
        campaign_id: Some(campaign_id),
        artwork_id: None,
        name,
    })
    .await
}

/// Saves the style of one artwork as a template.
pub async fn save_artwork_template(
    artwork_id: &str,
    name: &str,
) -> Result<TemplateSummary, String> {
    save_template_from(SaveTemplateRequest {
        design_id: None,
        deck_id: None,
        document_id: None,
        social_id: None,
        print_id: None,
        mailing_id: None,
        campaign_id: None,
        artwork_id: Some(artwork_id),
        name,
    })
    .await
}

/// Posts one template save request.
async fn save_template_from(request: SaveTemplateRequest<'_>) -> Result<TemplateSummary, String> {
    let builder = Request::post("/templates")
        .json(&request)
        .map_err(|error| error.to_string())?;
    let response = send_checked(builder, "POST /templates").await?;
    response.json().await.map_err(|error| error.to_string())
}

/// Deletes one template.
pub async fn delete_template(id: &str) -> Result<(), String> {
    send_empty(
        Request::delete(&format!("/templates/{id}")),
        "DELETE /templates",
    )
    .await
}

// -- Uploads -------------------------------------------------------------

/// One row of `GET /uploads`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct UploadSummary {
    /// Stored file name.
    pub name: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Content type from the extension.
    #[serde(default)]
    pub content_type: String,
    /// True for images.
    #[serde(default)]
    pub is_image: bool,
}

/// Fetches the uploads of one scope: a session id, or `DRAFT_SCOPE`
/// for the landing page.
pub async fn fetch_uploads(scope: &str) -> Result<Vec<UploadSummary>, String> {
    get_json(&format!("/uploads?session={scope}")).await
}

/// The scope of files attached before a session exists. Creating a
/// session takes them.
pub const DRAFT_SCOPE: &str = "_draft";

/// Deletes one upload.
pub async fn delete_upload(name: &str) -> Result<(), String> {
    send_empty(
        Request::delete(&format!("/uploads/{name}")),
        "DELETE /uploads",
    )
    .await
}

// -- Events --------------------------------------------------------------

/// Response of `GET /events`.
#[derive(Debug, Deserialize)]
struct EventsResponse {
    revision: u64,
}

/// Waits up to 25 seconds for the server revision to move past `after`.
pub async fn wait_for_change(after: u64) -> Result<u64, String> {
    get_json::<EventsResponse>(&format!("/events?after={after}&wait=25"))
        .await
        .map(|events| events.revision)
}
