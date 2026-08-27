//! Uploads in the browser: the `+` attach button and the chips that
//! list the user's source files.
//!
//! Files go to `POST /uploads` through a small JS snippet, because the
//! WASM bundle has no multipart body type. A chip's `×` deletes the
//! upload at once.

use dioxus::document;
use dioxus::prelude::*;
use serde::Deserialize;

use crate::api;
use crate::icons;

/// Posts the files of the panel's file input as one multipart request,
/// clears the input, and reports the outcome back to Dioxus.
const UPLOAD_SELECTED_FILES: &str = "\
const input = document.querySelector('input[data-upload-input]');
let outcome = { is_ok: false, message: 'no file input on the page' };
if (input && input.files && input.files.length) {
  const form = new FormData();
  for (const file of input.files) { form.append('file', file, file.name); }
  try {
    const response = await fetch('/uploads', { method: 'POST', body: form });
    if (response.ok) {
      outcome = { is_ok: true, message: '' };
    } else {
      let message = 'POST /uploads failed with status ' + response.status;
      try { const body = await response.json(); if (body && body.error && body.error.message) { message = body.error.message; } } catch (error) {}
      outcome = { is_ok: false, message };
    }
  } catch (error) {
    outcome = { is_ok: false, message: String(error) };
  }
  input.value = '';
}
dioxus.send(outcome);
";

/// What the upload snippet reports.
#[derive(Debug, Deserialize)]
struct UploadOutcome {
    is_ok: bool,
    #[serde(default)]
    message: String,
}

/// Posts the files chosen in the page's `data-upload-input` element and
/// returns the server's message when the upload fails.
pub(crate) async fn upload_selected_files() -> Result<(), String> {
    let mut channel = document::eval(UPLOAD_SELECTED_FILES);
    match channel.recv::<UploadOutcome>().await {
        Ok(outcome) if outcome.is_ok => Ok(()),
        Ok(outcome) => Err(outcome.message),
        Err(failure) => Err(failure.to_string()),
    }
}

/// A round `+` button that opens the file picker and uploads the choice.
///
/// `on_uploaded` fires after a successful upload so the caller can
/// refresh its list. `on_error` gets the server's message.
#[component]
pub fn AttachButton(on_uploaded: EventHandler<()>, on_error: EventHandler<String>) -> Element {
    let mut is_uploading = use_signal(|| false);
    let upload = move |_| {
        spawn(async move {
            is_uploading.set(true);
            match upload_selected_files().await {
                Ok(()) => on_uploaded.call(()),
                Err(message) => on_error.call(message),
            }
            is_uploading.set(false);
        });
    };
    rsx! {
        label {
            class: if is_uploading() { "attach-button busy" } else { "attach-button" },
            title: "Attach files as sources: images, PDFs, text. Up to 50 MB each.",
            span { dangerous_inner_html: icons::PAPERCLIP }
            input {
                r#type: "file",
                multiple: true,
                "data-upload-input": "true",
                disabled: is_uploading(),
                onchange: upload,
            }
        }
    }
}

/// The attached source files as chips: name, size, and a `×` that
/// deletes the upload. `on_changed` fires after a delete so the caller
/// refreshes its list. Renders nothing for an empty list.
#[component]
pub fn AttachmentChips(
    uploads: Vec<api::UploadSummary>,
    on_changed: EventHandler<()>,
    on_error: EventHandler<String>,
) -> Element {
    if uploads.is_empty() {
        return rsx! {};
    }
    rsx! {
        ul { class: "brief-attachments",
            for upload in uploads {
                li { key: "{upload.name}", class: "attachment-chip",
                    span { class: "mono", title: "{upload.content_type}", "{upload.name}" }
                    span { class: "attachment-size", "{format_size(upload.size_bytes)}" }
                    button {
                        class: "attachment-remove",
                        title: "Remove this file",
                        onclick: {
                            let name = upload.name.clone();
                            move |_| {
                                let name = name.clone();
                                spawn(async move {
                                    match api::delete_upload(&name).await {
                                        Ok(()) => on_changed.call(()),
                                        Err(message) => on_error.call(message),
                                    }
                                });
                            }
                        },
                        "×"
                    }
                }
            }
        }
    }
}

/// A short human size: `512 B`, `3.4 KB`, `1.2 MB`.
pub(crate) fn format_size(size_bytes: u64) -> String {
    if size_bytes < 1024 {
        format!("{size_bytes} B")
    } else if size_bytes < 1024 * 1024 {
        format!("{:.1} KB", size_bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", size_bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_format_in_bytes_kilobytes_and_megabytes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(3 * 1024 + 410), "3.4 KB");
        assert_eq!(format_size(1_258_291), "1.2 MB");
    }
}
