//! The editor's chat column: the conversation, scoped to the open design.
//!
//! The user asks for changes in words. Each message carries the design id,
//! so the engine applies the request to that design. A context chip names
//! the node the user last clicked or right-clicked, and is prepended to
//! the next message as `[screen N, node a/b <tag.class>: text]`.

use dioxus::document;
use dioxus::prelude::*;

use crate::api;
use crate::chat_controls::{ModelChip, SendButton, with_effort};
use crate::prompt_history::{PromptHistory, prompt_entries};
use crate::settings::{SettingsPanel, artifact_project, pause_briefly};
use crate::status::{RunStatusCard, phase_name};
use crate::uploads::{AttachButton, AttachmentChips, PasteUploads};

/// Scrolls the conversation to its newest message. Runs after the
/// thread renders, when the message count changes.
const SCROLL_TO_LATEST: &str = "\
const scrollThread = () => {
  const thread = document.querySelector('.thread');
  if (thread) { thread.scrollTop = thread.scrollHeight; }
};
scrollThread();
setTimeout(scrollThread, 120);
";

/// The chat column for one design.
#[component]
pub fn DesignChat(
    design_id: String,
    /// The element reference to prepend to the next message.
    context: Signal<Option<String>>,
    /// The pinned pages' reference, like `[slide 2] [slide 4]`, when
    /// the user pinned any. With none, a change is about the whole
    /// artifact.
    #[props(default)]
    page: Option<String>,
    /// True when `page` lists pinned pages. The chip's × then clears
    /// the pins first; a second × drops the open page.
    #[props(default)]
    is_pinned: bool,
    /// Called when the user drops the page chip while pages are pinned,
    /// so the editor clears its pins.
    #[props(default)]
    on_drop_page: Option<EventHandler<()>>,
    /// The pages the user can mention with `@`, in order: one label
    /// each. Empty outside the editor.
    #[props(default)]
    pages: Vec<String>,
    /// The unit a mention names: `slide` or `screen`.
    #[props(default)]
    page_unit: Option<String>,
    /// Called with the page index the user picked from the `@` menu.
    /// The editor pins that page.
    #[props(default)]
    on_pin_page: Option<EventHandler<usize>>,
    /// Called before a message is sent, so the editor can save local
    /// edits first.
    on_before_send: EventHandler<()>,
    /// Called when a run finishes, so the editor can reload the design.
    on_run_finished: EventHandler<()>,
) -> Element {
    let session_id = artifact_project(&design_id);
    let mut messages = use_signal(Vec::<api::ChatMessage>::new);
    let mut agent_run = use_signal(|| Option::<api::AgentRun>::None);
    let mut settings = use_signal(|| Option::<api::SettingsView>::None);
    let mut is_configuring = use_signal(|| false);
    let mut draft = use_signal(String::new);
    // The comments kept for one message: each a node or page reference
    // and a note, so several changes go out as one turn.
    let mut comments = use_signal(Vec::<String>::new);
    let mut error = use_signal(|| Option::<String>::None);
    // The run options, for the effort pick on the model chip.
    let mut options = use_signal(|| Option::<api::SessionOptions>::None);
    // The page the user dropped with the chip's ×. A new page brings
    // the chip back.
    let mut dropped_page = use_signal(|| Option::<String>::None);
    let page_for_send = page.clone();
    let page_for_comment = page.clone();
    // ↑ and ↓ walk the prompts sent before, as a shell does, when the
    // caret is on the first or the last line.
    let mut history = use_signal(PromptHistory::default);
    let prompts = prompt_entries(&messages());
    // The `@` menu: the highlighted row, and the draft the user closed
    // it on, so Escape keeps it closed until the text changes.
    let mut mention_row = use_signal(|| 0usize);
    let mut mention_closed_on = use_signal(|| Option::<String>::None);
    // The caret, read from the page after every edit and move, so a
    // mention typed in the middle of the text is found too.
    let caret = use_signal(|| 0usize);
    let mention = page_unit
        .as_deref()
        .filter(|_| !pages.is_empty() && mention_closed_on() != Some(draft()))
        .and_then(|_| mention_at(&draft(), caret()));
    let mention_rows: Vec<(usize, String)> = mention
        .as_ref()
        .map(|(_, query)| mention_matches(&pages, query))
        .unwrap_or_default();
    let unit_for_send = page_unit.clone().unwrap_or_default();
    let unit_for_comment = unit_for_send.clone();
    // Picks row `row` of the menu: the page is pinned, and the `@` and
    // its query leave the draft.
    let pick_mention = {
        let rows = mention_rows.clone();
        let start = mention.as_ref().map(|(start, _)| *start);
        move |row: usize| {
            if let (Some(start), Some((index, _))) = (start, rows.get(row)) {
                draft.set(remove_mention(&draft(), start, caret()));
                mention_row.set(0);
                if let Some(on_pin_page) = &on_pin_page {
                    on_pin_page.call(*index);
                }
            }
        }
    };
    let mut was_running = use_signal(|| false);
    let mut uploads = use_signal(Vec::<api::UploadSummary>::new);
    let refresh_uploads = use_callback({
        // The chat belongs to one session, so it shows that session's
        // source files and no other's.
        let session_id = session_id.clone();
        move |_: ()| {
            let session_id = session_id.clone();
            spawn(async move {
                if let Ok(listing) = api::fetch_uploads(&session_id).await {
                    uploads.set(listing);
                }
            });
        }
    });

    // The live loop: refresh, then wait for the next change.
    {
        let session_id = session_id.clone();
        use_future(move || {
            let session_id = session_id.clone();
            async move {
                let mut seen = 0u64;
                loop {
                    if let Ok(view) = api::fetch_session(&session_id).await {
                        options.set(Some(view.session.options));
                        messages.set(view.messages);
                    }
                    refresh_uploads.call(());
                    if let Ok(fetched) = api::fetch_agent_run().await {
                        if was_running() && !fetched.is_running {
                            on_run_finished.call(());
                        }
                        was_running.set(fetched.is_running);
                        agent_run.set(Some(fetched));
                    }
                    if let Ok(fetched) = api::fetch_settings().await {
                        settings.set(Some(fetched));
                    }
                    match api::wait_for_change(seen).await {
                        Ok(current) => seen = current,
                        Err(_) => pause_briefly().await,
                    }
                }
            }
        });
    }

    // Open the setup panel once when no model is chosen yet. The user
    // can close it and come back through the model chip.
    let mut has_auto_opened_setup = use_signal(|| false);
    use_effect(move || {
        if let Some(view) = settings()
            && view.current.is_none()
            && !has_auto_opened_setup()
        {
            has_auto_opened_setup.set(true);
            is_configuring.set(true);
        }
    });
    // Follow the conversation: jump to the newest message on load and
    // whenever a message arrives.
    let mut followed_count = use_signal(|| usize::MAX);
    use_effect(move || {
        let count = messages().len();
        if count != *followed_count.peek() {
            followed_count.set(count);
            document::eval(SCROLL_TO_LATEST);
        }
    });

    // Keeps the draft as one comment on the referenced node or page,
    // for a message that carries several. The composer clears for the
    // next one.
    let queue_comment = use_callback(move |_: ()| {
        let text = draft().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let page = page_for_comment
            .as_deref()
            .filter(|_| !has_page_reference(&text, &unit_for_comment));
        let Some(reference) = reference_for(&context(), page, &dropped_page()) else {
            return;
        };
        comments.write().push(comment_line(&reference, &text));
        draft.set(String::new());
        history.write().reset();
        context.set(None);
        if is_pinned && let Some(on_drop_page) = &on_drop_page {
            on_drop_page.call(());
        }
    });

    let send = use_callback({
        let design_id = design_id.clone();
        let session_id = session_id.clone();
        move |_: ()| {
            let text = draft().trim().to_owned();
            let queued = comments();
            if text.is_empty() && queued.is_empty() {
                return;
            }
            // A page the text names itself needs no chip in front of it.
            let page = page_for_send
                .as_deref()
                .filter(|_| !has_page_reference(&text, &unit_for_send));
            let tail = (!text.is_empty()).then(|| {
                match reference_for(&context(), page, &dropped_page()) {
                    Some(reference) => format!("{reference} {text}"),
                    None => text,
                }
            });
            let content = message_with_comments(&queued, tail);
            let design_id = design_id.clone();
            let session_id = session_id.clone();
            draft.set(String::new());
            comments.set(Vec::new());
            history.write().reset();
            context.set(None);
            // The pins were for this message. The next one starts clean.
            if is_pinned && let Some(on_drop_page) = &on_drop_page {
                on_drop_page.call(());
            }
            on_before_send.call(());
            spawn(async move {
                match api::send_session_message(&session_id, &content, Some(&design_id)).await {
                    Ok(()) => {
                        error.set(None);
                        if let Err(message) = api::start_agent_run(&session_id).await
                            && !message.contains("already active")
                        {
                            error.set(Some(message));
                        }
                    }
                    Err(message) => error.set(Some(message)),
                }
            });
        }
    });

    let current_settings = settings().and_then(|view| view.current);
    let is_model_chosen = current_settings.is_some();
    let show_setup = settings().is_some() && is_configuring();
    let is_running = agent_run().is_some_and(|run| run.is_running);
    let running_placeholder = agent_run()
        .map(|run| format!("{} Your next request queues up.", phase_name(&run)))
        .unwrap_or_default();
    let has_draft = !draft().trim().is_empty();
    let can_send = is_model_chosen && (has_draft || !comments().is_empty());
    rsx! {
        section { class: "conversation editor-chat",
            div { class: "thread",
                if messages().is_empty() {
                    p { class: "chat-note",
                        "No conversation yet. Ask for a change below, for example "
                        "“make the title on screen 2 bigger”."
                    }
                }
                for (index, message) in messages().iter().enumerate() {
                    div {
                        key: "{index}",
                        class: if message.role == "user" { "bubble user" } else { "bubble agent" },
                        p { "{message.content}" }
                    }
                }
                if let Some(run) = agent_run() {
                    RunStatusCard { run }
                }
                if let Some(message) = error() {
                    p { class: "error", "{message}" }
                }
            }
            div { class: "chat-box",
                if show_setup {
                    div { class: "chat-settings",
                        SettingsPanel { settings, is_configuring }
                    }
                }
                // The comments kept so far. They go out as one message
                // with the next Send.
                if !comments().is_empty() {
                    div { class: "comment-list",
                        div { class: "comment-head", {comment_summary(comments().len())} }
                        for (index, line) in comments().iter().enumerate() {
                            div { key: "{index}", class: "comment-row mono",
                                span { class: "comment-text", "{line}" }
                                button {
                                    title: "Drop this comment",
                                    onclick: move |_| {
                                        comments.write().remove(index);
                                    },
                                    "×"
                                }
                            }
                        }
                    }
                }
                if let Some(reference) = reference_for(
                    &context(),
                    page
                        .as_deref()
                        .filter(|_| {
                            !has_page_reference(&draft(), page_unit.as_deref().unwrap_or_default())
                        }),
                    &dropped_page(),
                )
                {
                    div { class: "context-row",
                        div { class: "context-chip mono",
                            span { "{reference}" }
                            button {
                                title: "Drop this reference",
                                onclick: {
                                    let page = page.clone();
                                    move |_| {
                                        context.set(None);
                                        if is_pinned {
                                            if let Some(on_drop_page) = &on_drop_page {
                                                on_drop_page.call(());
                                            }
                                        } else {
                                            dropped_page.set(page.clone());
                                        }
                                    }
                                },
                                "×"
                            }
                        }
                        // A note on the referenced node can wait for more
                        // notes, so several changes go out as one turn.
                        button {
                            class: "queue-comment",
                            disabled: !has_draft,
                            title: "Keep this note as a comment and add more before sending (⌘Enter)",
                            onclick: move |_| queue_comment.call(()),
                            "+ Comment"
                        }
                    }
                }
                // The `@` menu: the pages that match what follows the `@`.
                if !mention_rows.is_empty() {
                    div { class: "mention-menu", role: "listbox",
                        for (row, (index, label)) in mention_rows.iter().enumerate() {
                            button {
                                key: "{index}",
                                class: if row == mention_row() { "mention-item active" } else { "mention-item" },
                                role: "option",
                                onmousedown: move |event: Event<MouseData>| event.prevent_default(),
                                onclick: {
                                    let mut pick_mention = pick_mention.clone();
                                    move |_| pick_mention(row)
                                },
                                span { class: "mono", {format!("{:02}", index + 1)} }
                                span { "{page_title(label)}" }
                            }
                        }
                    }
                }
                textarea {
                    placeholder: if is_running { running_placeholder } else if page_unit.is_some() { "Ask for a change: “make this title bigger”, “@3 more margin”…" } else { "Ask for a change: “make this title bigger”, “swap screens 2 and 4”…" },
                    value: "{draft()}",
                    oninput: move |event| {
                        draft.set(event.value());
                        mention_row.set(0);
                        history.write().reset();
                        watch_caret(caret);
                    },
                    onkeyup: move |_| watch_caret(caret),
                    onmouseup: move |_| watch_caret(caret),
                    onkeydown: {
                        let mut pick_mention = pick_mention.clone();
                        let row_count = mention_rows.len();
                        let prompts = prompts.clone();
                        move |event: Event<KeyboardData>| {
                            if row_count == 0
                                && recall_prompt(&event, &prompts, &mut history.write(), &mut draft)
                            {
                                return;
                            }
                            if row_count > 0 {
                                match event.key() {
                                    Key::ArrowDown => {
                                        event.prevent_default();
                                        mention_row.set((mention_row() + 1) % row_count);
                                        return;
                                    }
                                    Key::ArrowUp => {
                                        event.prevent_default();
                                        mention_row.set((mention_row() + row_count - 1) % row_count);
                                        return;
                                    }
                                    Key::Enter | Key::Tab => {
                                        event.prevent_default();
                                        pick_mention(mention_row());
                                        return;
                                    }
                                    Key::Escape => {
                                        mention_closed_on.set(Some(draft()));
                                        return;
                                    }
                                    _ => {}
                                }
                            }
                            if event.key() == Key::Enter && !event.modifiers().shift() {
                                event.prevent_default();
                                // ⌘Enter keeps the note as a comment;
                                // Enter sends everything.
                                if event.modifiers().meta() || event.modifiers().ctrl() {
                                    queue_comment.call(());
                                } else {
                                    send.call(());
                                }
                            }
                        }
                    },
                }
                PasteUploads {
                    scope: session_id.clone(),
                    on_uploaded: move |_| {
                        error.set(None);
                        refresh_uploads.call(());
                    },
                    on_error: move |message| error.set(Some(message)),
                }
                AttachmentChips {
                    uploads: uploads(),
                    on_changed: move |_| refresh_uploads.call(()),
                    on_error: move |message| error.set(Some(message)),
                }
                div { class: "chat-box-row",
                    div { class: "chat-box-left",
                        AttachButton {
                            scope: session_id.clone(),
                            on_uploaded: move |_| {
                                error.set(None);
                                refresh_uploads.call(());
                            },
                            on_error: move |message| error.set(Some(message)),
                        }
                    }
                    div { class: "chat-box-right",
                        ModelChip {
                            settings,
                            is_configuring,
                            effort: options().map(|options| options.effort),
                            on_effort: {
                                let session_id = session_id.clone();
                                move |level: String| {
                                    let Some(current) = options() else {
                                        return;
                                    };
                                    let next = with_effort(&current, &level);
                                    options.set(Some(next.clone()));
                                    let session_id = session_id.clone();
                                    spawn(async move {
                                        let saved = api::save_session_options(&session_id, &next).await;
                                        if let Err(message) = saved {
                                            error.set(Some(message));
                                        }
                                    });
                                }
                            },
                        }
                        SendButton {
                            label: "Send",
                            is_enabled: can_send,
                            on_send: move |_| send.call(()),
                        }
                    }
                }
            }
        }
    }
}

/// One kept comment: the reference, then the note.
pub(crate) fn comment_line(reference: &str, text: &str) -> String {
    format!("{reference}: {text}")
}

/// The message the kept comments make, one per line, with the text
/// typed last after them.
pub(crate) fn message_with_comments(comments: &[String], tail: Option<String>) -> String {
    let mut lines: Vec<&str> = comments.iter().map(String::as_str).collect();
    if let Some(tail) = &tail {
        lines.push(tail);
    }
    lines.join("\n")
}

/// The head of the comment list: `1 comment for the next message`.
pub(crate) fn comment_summary(count: usize) -> String {
    if count == 1 {
        "1 comment for the next message".to_owned()
    } else {
        format!("{count} comments for the next message")
    }
}

/// The reference the next message carries: the picked element, else
/// the open page unless the user dropped that page.
pub(crate) fn reference_for(
    context: &Option<String>,
    page: Option<&str>,
    dropped_page: &Option<String>,
) -> Option<String> {
    if let Some(reference) = context {
        return Some(reference.clone());
    }
    page.filter(|page| dropped_page.as_deref() != Some(*page))
        .map(str::to_owned)
}

/// Handles ↑ and ↓ in a composer: ↑ recalls an earlier prompt, ↓ walks
/// forward again. `ARROW_GUARD` lets only an arrow on the first or the
/// last line through, so elsewhere the arrows move the caret. Returns
/// true when the key was taken.
pub(crate) fn recall_prompt(
    event: &Event<KeyboardData>,
    prompts: &[String],
    history: &mut PromptHistory,
    draft: &mut Signal<String>,
) -> bool {
    let text = draft.peek().clone();
    let recalled = match event.key() {
        Key::ArrowUp => history.older(prompts, &text),
        Key::ArrowDown if history.is_walking() => history.newer(prompts),
        _ => None,
    };
    let Some(text) = recalled else {
        return false;
    };
    event.prevent_default();
    draft.set(text);
    true
}

/// Reads the caret of the focused textarea, in UTF-16 units.
const CARET_SCRIPT: &str = "const box = document.activeElement; \
dioxus.send(box && box.tagName === 'TEXTAREA' ? box.selectionStart : null);";

/// Reads the focused textarea's caret into `caret`, as a byte offset
/// into its text.
pub(crate) fn watch_caret(mut caret: Signal<usize>) {
    spawn(async move {
        let mut channel = document::eval(CARET_SCRIPT);
        if let Ok(Some(units)) = channel.recv::<Option<usize>>().await {
            caret.set(units);
        }
    });
}

/// The byte offset that `units` UTF-16 units into `text` reach.
fn byte_offset(text: &str, units: usize) -> usize {
    let mut seen = 0usize;
    for (offset, character) in text.char_indices() {
        if seen >= units {
            return offset;
        }
        seen += character.len_utf16();
    }
    text.len()
}

/// The `@` the user is typing: the byte offset of an `@` before the
/// caret, at the start of the text or after whitespace, with the text
/// between it and the caret as the query. `None` when no such `@`
/// precedes the caret or the query spans a line. `caret` counts UTF-16
/// units, as the browser does.
pub(crate) fn mention_at(text: &str, caret: usize) -> Option<(usize, String)> {
    let caret = byte_offset(text, caret);
    let before = &text[..caret];
    let start = before.rfind('@')?;
    let is_word_start = start == 0
        || before[..start]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace);
    let query = &before[start + 1..];
    if !is_word_start || query.contains('\n') {
        return None;
    }
    Some((start, query.to_owned()))
}

/// The pages a query matches: by number, or by a word of the label.
/// Every page for an empty query. At most eight.
pub(crate) fn mention_matches(pages: &[String], query: &str) -> Vec<(usize, String)> {
    let query = query.trim().to_lowercase();
    pages
        .iter()
        .enumerate()
        .filter(|(index, label)| {
            query.is_empty()
                || (index + 1).to_string().starts_with(&query)
                || label.to_lowercase().contains(&query)
        })
        .map(|(index, label)| (index, label.clone()))
        .take(8)
        .collect()
}

/// The draft without the `@` query between `start` and the caret.
pub(crate) fn remove_mention(text: &str, start: usize, caret: usize) -> String {
    let caret = byte_offset(text, caret).max(start);
    format!("{}{}", &text[..start], &text[caret..])
}

/// A page label without its leading number: `3. Roadmap` reads
/// `Roadmap`. The menu shows the number itself.
pub(crate) fn page_title(label: &str) -> &str {
    match label.split_once(". ") {
        Some((number, title))
            if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) =>
        {
            title
        }
        _ => label,
    }
}

/// True when the text names a page itself, like `[slide 3]`.
pub(crate) fn has_page_reference(text: &str, unit: &str) -> bool {
    !unit.is_empty() && text.contains(&format!("[{unit} "))
}

#[cfg(test)]
mod tests {
    use super::{
        comment_line, comment_summary, has_page_reference, mention_at, mention_matches,
        message_with_comments, page_title, reference_for, remove_mention,
    };

    #[test]
    fn kept_comments_go_out_as_one_message_one_per_line() {
        let first = comment_line("[screen 2, node 0/1 <h2.title>: Plans]", "make it bigger");
        let second = comment_line("[screen 3]", "more margin");
        assert_eq!(
            first,
            "[screen 2, node 0/1 <h2.title>: Plans]: make it bigger"
        );
        assert_eq!(
            message_with_comments(&[first.clone(), second.clone()], None),
            format!("{first}\n{second}")
        );
        // The text typed last follows the comments.
        assert_eq!(
            message_with_comments(
                std::slice::from_ref(&first),
                Some("and a footer".to_owned())
            ),
            format!("{first}\nand a footer")
        );
        assert_eq!(
            message_with_comments(&[], Some("plain".to_owned())),
            "plain"
        );
        assert_eq!(comment_summary(1), "1 comment for the next message");
        assert_eq!(comment_summary(3), "3 comments for the next message");
    }

    #[test]
    fn an_at_sign_opens_a_page_mention() {
        assert_eq!(mention_at("@", 1), Some((0, String::new())));
        assert_eq!(mention_at("fix @3", 6), Some((4, "3".to_owned())));
        // The caret decides: the `@` behind it counts, the text after
        // it does not.
        assert_eq!(
            mention_at("fix @3 on the left", 6),
            Some((4, "3".to_owned()))
        );
        // Past the text, the query runs on and matches no page.
        assert_eq!(
            mention_at("fix @3 on the left", 18),
            Some((4, "3 on the left".to_owned()))
        );
        assert_eq!(mention_at("fix @3\nmore", 11), None);
        assert_eq!(mention_at("mail@example.com", 16), None);
        assert_eq!(mention_at("no mention", 10), None);
        // Units, not bytes: an emoji is two.
        assert_eq!(mention_at("😀 @2", 5), Some((5, "2".to_owned())));
    }

    #[test]
    fn a_mention_matches_pages_by_number_or_word_and_inserts_a_reference() {
        let pages = vec![
            "Why It Exists".to_owned(),
            "Architecture".to_owned(),
            "Roadmap".to_owned(),
        ];
        assert_eq!(mention_matches(&pages, "").len(), 3);
        assert_eq!(
            mention_matches(&pages, "2"),
            vec![(1, "Architecture".to_owned())]
        );
        assert_eq!(
            mention_matches(&pages, "road"),
            vec![(2, "Roadmap".to_owned())]
        );
        assert_eq!(remove_mention("fix @roa", 4, 8), "fix ");
        assert_eq!(
            remove_mention("fix @roa on the left", 4, 8),
            "fix  on the left"
        );
        assert!(has_page_reference("[slide 3] more margin", "slide"));
        assert_eq!(page_title("3. Roadmap"), "Roadmap");
        assert_eq!(page_title("Roadmap"), "Roadmap");
        assert!(!has_page_reference("more margin", "slide"));
    }

    #[test]
    fn the_open_page_is_the_reference_until_the_user_drops_it() {
        let none: Option<String> = None;
        assert_eq!(
            reference_for(&none, Some("[slide 4]"), &None),
            Some("[slide 4]".to_owned())
        );
        assert_eq!(
            reference_for(&none, Some("[slide 4]"), &Some("[slide 4]".to_owned())),
            None
        );
        // A new page brings the chip back.
        assert_eq!(
            reference_for(&none, Some("[slide 5]"), &Some("[slide 4]".to_owned())),
            Some("[slide 5]".to_owned())
        );
        // A picked element wins over the page.
        assert_eq!(
            reference_for(
                &Some("[slide 4, node 1 <p>]".to_owned()),
                Some("[slide 4]"),
                &None
            ),
            Some("[slide 4, node 1 <p>]".to_owned())
        );
    }
}
