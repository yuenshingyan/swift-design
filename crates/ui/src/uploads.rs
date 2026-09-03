//! Uploads in the browser: the `+` attach button, the paste listener,
//! and the chips that list the user's source files.
//!
//! Files go to `POST /uploads` through a small JS snippet, because the
//! WASM bundle has no multipart body type. The same snippet serves the
//! file picker and the clipboard, so both paths obey one set of limits.
//! A chip's `×` deletes the upload at once.

use dioxus::document;
use dioxus::prelude::*;
use serde::Deserialize;

use crate::api;
use crate::icons;

/// The JS every upload path shares: the folder walk, the skip rules,
/// and the batched POST.
///
/// The batch limits keep each request under the server's 50 MB body
/// limit. A pasted folder can hold far more than one request carries.
const UPLOAD_HELPERS: &str = r#"
const FILE_LIMIT = 200;
const BATCH_FILE_LIMIT = 20;
const BATCH_BYTES_LIMIT = 20 * 1024 * 1024;
const FILE_BYTES_LIMIT = 45 * 1024 * 1024;
const SKIPPED_FOLDERS = ['node_modules', 'target', 'dist'];

const isSkippedName = (name) => name.startsWith('.') || SKIPPED_FOLDERS.includes(name);
const readEntries = (reader) => new Promise((resolve, reject) => reader.readEntries(resolve, reject));
const readFile = (entry) => new Promise((resolve, reject) => entry.file(resolve, reject));

// Walks one clipboard entry. A file joins the list under its path from
// the pasted folder, so the server can keep the folders in the name.
async function collectEntry(entry, prefix, report) {
  if (isSkippedName(entry.name) || report.files.length >= FILE_LIMIT) {
    report.skipped += 1;
    return;
  }
  if (entry.isFile) {
    const file = await readFile(entry);
    if (file.size > FILE_BYTES_LIMIT) { report.skipped += 1; return; }
    report.files.push({ file, name: prefix + entry.name });
    return;
  }
  const reader = entry.createReader();
  for (;;) {
    const children = await readEntries(reader);
    if (!children.length) { break; }
    for (const child of children) {
      await collectEntry(child, prefix + entry.name + '/', report);
    }
  }
}

async function postBatch(batch) {
  const form = new FormData();
  for (const item of batch) { form.append('file', item.file, item.name); }
  const response = await fetch('/uploads?session=' + encodeURIComponent(uploadScope), { method: 'POST', body: form });
  if (response.ok) { return; }
  let message = 'POST /uploads failed with status ' + response.status;
  try {
    const body = await response.json();
    if (body && body.error && body.error.message) { message = body.error.message; }
  } catch (error) {}
  throw new Error(message);
}

// Posts the collected files in batches and reports after each one, so
// the chips appear while a large folder is still going up.
async function postFiles(report, send) {
  let batch = [];
  let bytes = 0;
  for (const item of report.files) {
    const isFull = batch.length >= BATCH_FILE_LIMIT || bytes + item.file.size > BATCH_BYTES_LIMIT;
    if (batch.length && isFull) {
      await postBatch(batch);
      report.stored += batch.length;
      send(false, '');
      batch = [];
      bytes = 0;
    }
    batch.push(item);
    bytes += item.file.size;
  }
  if (batch.length) {
    await postBatch(batch);
    report.stored += batch.length;
  }
}

// `roots` holds `{ entry }` for a clipboard entry and `{ file }` for a
// plain file. Reports one message per batch, then a final message.
async function attach(roots) {
  const report = { files: [], stored: 0, skipped: 0, opaque: 0 };
  const send = (isDone, message) => dioxus.send({
    is_ok: !message,
    is_done: isDone,
    stored: report.stored,
    skipped: report.skipped,
    message: message || '',
  });
  try {
    for (const root of roots) {
      if (root.entry) {
        await collectEntry(root.entry, '', report);
      } else if (isSkippedName(root.file.name) || root.file.size > FILE_BYTES_LIMIT) {
        report.skipped += 1;
      } else if (!root.file.size && !root.file.type) {
        // A folder this browser will not open: it hands over an item
        // with no content instead of a directory to walk.
        report.opaque += 1;
      } else {
        report.files.push({ file: root.file, name: root.file.webkitRelativePath || root.file.name });
      }
    }
    if (!report.files.length) {
      const message = report.opaque
        ? 'This browser pasted the folder as an empty item. Open the folder and paste the files inside it.'
        : 'Nothing to attach: the pasted items are hidden files, build folders, or above 45 MB.';
      send(true, message);
      return;
    }
    await postFiles(report, send);
    // A skip is never silent: the user must know the folder went up
    // short.
    send(true, report.skipped
      ? 'Attached ' + report.stored + ' files. Skipped ' + report.skipped + ': hidden files, build folders, files above 45 MB, or files past the limit of ' + FILE_LIMIT + '.'
      : '');
  } catch (error) {
    send(true, String(error && error.message ? error.message : error));
  }
}
"#;

/// Posts the files of the panel's file input, then clears the input.
const UPLOAD_SELECTED_FILES: &str = r#"
const input = document.querySelector('input[data-upload-input]');
if (!input || !input.files || !input.files.length) {
  dioxus.send({ is_ok: false, is_done: true, stored: 0, skipped: 0, message: 'no file input on the page' });
} else {
  const roots = Array.from(input.files).map((file) => ({ file }));
  input.value = '';
  await attach(roots);
}
"#;

/// Listens for files and folders the user pastes or drops on the page.
///
/// A transfer item lives only for the length of the handler, so the
/// handler reads every item before it awaits anything. Paste is the
/// first path. Drop is the second, because a browser hands over the
/// contents of a folder on a drop but not always on a paste.
///
/// One set of listeners serves the page: a new mount replaces the
/// previous set.
const LISTEN_FOR_FILES: &str = r#"
const previous = window.swiftDesignFileListeners;
if (previous) {
  for (const [name, listener] of previous) { document.removeEventListener(name, listener); }
}

// Reads the transfer items in order: a folder as an entry to walk, a
// file as itself.
const rootsOf = (data) => {
  const roots = [];
  for (const item of Array.from((data && data.items) || [])) {
    if (item.kind !== 'file') { continue; }
    const entry = item.webkitGetAsEntry ? item.webkitGetAsEntry() : null;
    if (entry) { roots.push({ entry }); continue; }
    const file = item.getAsFile();
    if (file) { roots.push({ file }); }
  }
  return roots;
};

const onPaste = (event) => {
  const roots = rootsOf(event.clipboardData);
  if (!roots.length) { return; }
  // The paste carries files, so it must not also type into the box.
  event.preventDefault();
  attach(roots);
};

// `dragenter` and `dragleave` fire for every element under the pointer,
// so a counter decides when the drag has really left the page.
let dragDepth = 0;
const showDropTarget = (isOver) => {
  document.body.style.outline = isOver ? '2px dashed var(--ink)' : '';
  document.body.style.outlineOffset = isOver ? '-8px' : '';
};
const hasFiles = (event) => Array.from((event.dataTransfer && event.dataTransfer.types) || []).includes('Files');
const onDragEnter = (event) => {
  if (!hasFiles(event)) { return; }
  dragDepth += 1;
  showDropTarget(true);
};
const onDragLeave = (event) => {
  if (!hasFiles(event)) { return; }
  dragDepth = Math.max(0, dragDepth - 1);
  if (!dragDepth) { showDropTarget(false); }
};
// Without this the browser opens the file instead of dropping it.
const onDragOver = (event) => { if (hasFiles(event)) { event.preventDefault(); } };
const onDrop = (event) => {
  if (!hasFiles(event)) { return; }
  event.preventDefault();
  dragDepth = 0;
  showDropTarget(false);
  const roots = rootsOf(event.dataTransfer);
  if (roots.length) { attach(roots); }
};

const listeners = [
  ['paste', onPaste],
  ['dragenter', onDragEnter],
  ['dragleave', onDragLeave],
  ['dragover', onDragOver],
  ['drop', onDrop],
];
for (const [name, listener] of listeners) { document.addEventListener(name, listener); }
window.swiftDesignFileListeners = listeners;
"#;

/// What an upload script reports. A script sends one message per posted
/// batch and one final message with `is_done`.
#[derive(Debug, Deserialize)]
struct UploadOutcome {
    is_ok: bool,
    #[serde(default)]
    is_done: bool,
    #[serde(default)]
    message: String,
}

/// The JS line that names the scope every upload of a page belongs to.
///
/// A file belongs to one session, so the page has to say which. The
/// landing page uses the draft scope; a session page uses its own id.
fn scope_line(scope: &str) -> String {
    // Session ids are slugs, but the value still goes in as a quoted
    // string with nothing that could close it.
    let safe: String = scope
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .collect();
    format!("const uploadScope = \"{safe}\";\n")
}

/// Posts the files chosen in the page's `data-upload-input` element into
/// `scope` and returns the server's message when the upload fails.
pub(crate) async fn upload_selected_files(scope: &str) -> Result<(), String> {
    let mut channel = document::eval(&format!(
        "{}{UPLOAD_HELPERS}{UPLOAD_SELECTED_FILES}",
        scope_line(scope)
    ));
    loop {
        match channel.recv::<UploadOutcome>().await {
            Ok(outcome) if !outcome.is_ok => return Err(outcome.message),
            Ok(outcome) if outcome.is_done => return Ok(()),
            Ok(_) => continue,
            Err(failure) => return Err(failure.to_string()),
        }
    }
}

/// Uploads files and folders the user pastes or drops on the page.
/// Renders nothing.
///
/// `on_uploaded` fires after every posted batch, so a large folder
/// fills the chip list as it goes. `on_error` gets the failure message.
#[component]
pub fn PasteUploads(
    /// The session the files belong to, or `api::DRAFT_SCOPE`.
    scope: String,
    on_uploaded: EventHandler<()>,
    on_error: EventHandler<String>,
) -> Element {
    use_future(move || {
        let script = format!("{}{UPLOAD_HELPERS}{LISTEN_FOR_FILES}", scope_line(&scope));
        async move {
            let mut channel = document::eval(&script);
            while let Ok(outcome) = channel.recv::<UploadOutcome>().await {
                // Files land batch by batch, so the list is worth a refresh
                // even when the run ends with a message.
                on_uploaded.call(());
                if !outcome.message.is_empty() {
                    on_error.call(outcome.message);
                }
            }
        }
    });
    rsx! {}
}

/// A round `+` button that opens the file picker and uploads the choice.
///
/// `on_uploaded` fires after a successful upload so the caller can
/// refresh its list. `on_error` gets the server's message.
#[component]
pub fn AttachButton(
    /// The session the files belong to, or `api::DRAFT_SCOPE`.
    scope: String,
    on_uploaded: EventHandler<()>,
    on_error: EventHandler<String>,
) -> Element {
    let mut is_uploading = use_signal(|| false);
    let upload = move |_| {
        let scope = scope.clone();
        spawn(async move {
            is_uploading.set(true);
            let outcome = upload_selected_files(&scope).await;
            // Part of a batched upload can land before a failure, so
            // the list is refreshed either way.
            on_uploaded.call(());
            if let Err(message) = outcome {
                on_error.call(message);
            }
            is_uploading.set(false);
        });
    };
    rsx! {
        label {
            class: if is_uploading() { "attach-button busy" } else { "attach-button" },
            title: "Attach files as sources: images, PDFs, text. Up to 50 MB each. You can also paste a file or a folder.",
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

/// From this many files the chip list collapses behind a summary
/// button, so a big paste does not fill the composer.
const ATTACHMENT_COLLAPSE_THRESHOLD: usize = 4;

/// True when `count` files render collapsed behind the summary button.
fn is_collapsed(count: usize) -> bool {
    count >= ATTACHMENT_COLLAPSE_THRESHOLD
}

/// True when the list offers the `×` that removes every file at once.
/// One file has its own `×` already.
fn has_remove_all(count: usize) -> bool {
    count > 1
}

/// The uploads no sent prompt has carried yet. The composer chips show
/// only these, so a sent prompt leaves the composer clean.
pub(crate) fn pending_uploads(
    uploads: &[api::UploadSummary],
    sent_names: &[String],
) -> Vec<api::UploadSummary> {
    uploads
        .iter()
        .filter(|upload| !sent_names.contains(&upload.name))
        .cloned()
        .collect()
}

/// A `×` that deletes every attached file in one click. A failure
/// stops the sweep and surfaces the server's message; the files
/// already deleted stay deleted.
#[component]
fn RemoveAllButton(
    names: Vec<String>,
    on_changed: EventHandler<()>,
    on_error: EventHandler<String>,
) -> Element {
    rsx! {
        button {
            class: "attachment-remove attachment-remove-all",
            title: "Remove all files",
            onclick: move |_| {
                let names = names.clone();
                spawn(async move {
                    for name in names {
                        if let Err(message) = api::delete_upload(&name).await {
                            on_error.call(message);
                            break;
                        }
                    }
                    on_changed.call(());
                });
            },
            "×"
        }
    }
}

/// The attached source files as chips: name, size, and a `×` that
/// deletes the upload. `on_changed` fires after a delete so the caller
/// refreshes its list. Renders nothing for an empty list. From
/// `ATTACHMENT_COLLAPSE_THRESHOLD` files the list collapses behind a
/// paperclip-and-count button; a click shows the chips.
#[component]
pub fn AttachmentChips(
    uploads: Vec<api::UploadSummary>,
    on_changed: EventHandler<()>,
    on_error: EventHandler<String>,
) -> Element {
    let mut is_open = use_signal(|| false);
    if uploads.is_empty() {
        return rsx! {};
    }
    let names: Vec<String> = uploads.iter().map(|upload| upload.name.clone()).collect();
    if is_collapsed(uploads.len()) {
        return rsx! {
            div { class: "brief-attachments",
                button {
                    class: if is_open() { "attachment-summary open" } else { "attachment-summary" },
                    "aria-expanded": "{is_open()}",
                    title: "Show the attached files",
                    onclick: move |_| is_open.set(!is_open()),
                    span { dangerous_inner_html: icons::PAPERCLIP }
                    span { class: "attachment-summary-count", "{uploads.len()} files" }
                    span {
                        class: "attachment-summary-chevron",
                        dangerous_inner_html: icons::CHEVRON_DOWN,
                    }
                }
                RemoveAllButton { names, on_changed, on_error }
            }
            if is_open() {
                AttachmentList {
                    uploads,
                    has_remove_all: false,
                    on_changed,
                    on_error,
                }
            }
        };
    }
    rsx! {
        AttachmentList {
            uploads,
            has_remove_all: has_remove_all(names.len()),
            on_changed,
            on_error,
        }
    }
}

/// The chip list itself, one chip per upload. With `has_remove_all`
/// the list ends in the `×` that deletes every file; the collapsed
/// view keeps that `×` beside its summary button instead.
#[component]
fn AttachmentList(
    uploads: Vec<api::UploadSummary>,
    has_remove_all: bool,
    on_changed: EventHandler<()>,
    on_error: EventHandler<String>,
) -> Element {
    let names: Vec<String> = uploads.iter().map(|upload| upload.name.clone()).collect();
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
            if has_remove_all {
                li { class: "attachment-clear-all",
                    RemoveAllButton { names, on_changed, on_error }
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
    fn four_files_collapse_and_three_do_not() {
        assert!(!is_collapsed(0));
        assert!(!is_collapsed(3));
        assert!(is_collapsed(4));
        assert!(is_collapsed(40));
    }

    #[test]
    fn remove_all_needs_more_than_one_file() {
        assert!(!has_remove_all(0));
        assert!(!has_remove_all(1));
        assert!(has_remove_all(2));
    }

    #[test]
    fn pending_uploads_drop_the_files_a_prompt_carried() {
        let upload = |name: &str| api::UploadSummary {
            name: name.to_owned(),
            size_bytes: 1,
            content_type: String::new(),
            is_image: false,
        };
        let uploads = vec![upload("brief.pdf"), upload("logo.png")];
        let sent = vec!["brief.pdf".to_owned()];
        let pending = pending_uploads(&uploads, &sent);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].name, "logo.png");
        assert!(pending_uploads(&uploads, &[]).len() == 2);
    }

    #[test]
    fn sizes_format_in_bytes_kilobytes_and_megabytes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(3 * 1024 + 410), "3.4 KB");
        assert_eq!(format_size(1_258_291), "1.2 MB");
    }

    #[test]
    fn every_upload_script_runs_with_the_shared_helpers() {
        for script in [UPLOAD_SELECTED_FILES, LISTEN_FOR_FILES] {
            assert!(script.contains("attach("), "{script}");
        }
        assert!(UPLOAD_HELPERS.contains("async function attach(roots)"));
        assert!(UPLOAD_HELPERS.contains("collectEntry"));
    }
}
