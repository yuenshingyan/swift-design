//! Recalling earlier prompts with the arrow keys, as a shell does: ↑
//! walks back through what the user sent, ↓ walks forward, and past
//! the newest entry the draft the user was typing comes back.

use crate::api;

/// Installed once on the page. At keydown, before the app sees the
/// key, it finds the visual line the caret is on: a mirror of the
/// textarea wraps the text the same way, and a marker at the caret
/// tells how far from the top and the bottom the caret sits. A wrapped
/// long line counts as the lines it shows. ↑ with a line above the
/// caret, or ↓ with a line below it, is stopped here, so the browser
/// moves the caret and the app never recalls a prompt. Only an arrow
/// on the first or the last visual line reaches the app.
pub(crate) const ARROW_GUARD: &str = "\
const caretEdges = (box) => { \
  const style = getComputedStyle(box); \
  const mirror = document.createElement('div'); \
  for (const property of ['fontFamily', 'fontSize', 'fontWeight', 'fontStyle', 'letterSpacing', 'lineHeight', 'paddingTop', 'paddingRight', 'paddingBottom', 'paddingLeft', 'borderTopWidth', 'borderRightWidth', 'borderBottomWidth', 'borderLeftWidth', 'borderStyle', 'textIndent', 'wordSpacing', 'tabSize', 'textTransform']) { mirror.style[property] = style[property]; } \
  mirror.style.position = 'absolute'; mirror.style.visibility = 'hidden'; mirror.style.top = '0'; mirror.style.left = '-9999px'; \
  mirror.style.boxSizing = 'border-box'; mirror.style.width = (box.clientWidth + parseFloat(style.borderLeftWidth) + parseFloat(style.borderRightWidth)) + 'px'; \
  mirror.style.whiteSpace = 'pre-wrap'; mirror.style.overflowWrap = 'break-word'; mirror.style.height = 'auto'; \
  mirror.appendChild(document.createTextNode(box.value.slice(0, box.selectionStart))); \
  const marker = document.createElement('span'); marker.textContent = '\\u200b'; mirror.appendChild(marker); \
  mirror.appendChild(document.createTextNode(box.value.slice(box.selectionEnd) + '\\u200b')); \
  document.body.appendChild(mirror); \
  const lineHeight = marker.getBoundingClientRect().height || parseFloat(style.lineHeight) || 16; \
  const outer = mirror.getBoundingClientRect(); const mark = marker.getBoundingClientRect(); \
  const top = outer.top + parseFloat(style.borderTopWidth) + parseFloat(style.paddingTop); \
  const bottom = outer.bottom - parseFloat(style.borderBottomWidth) - parseFloat(style.paddingBottom); \
  mirror.remove(); \
  return { isFirst: mark.top - top < lineHeight / 2, isLast: bottom - mark.bottom < lineHeight / 2 }; \
}; \
document.addEventListener('keydown', (event) => { \
  const box = event.target; \
  if (!box || box.tagName !== 'TEXTAREA') { return; } \
  if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') { return; } \
  const edges = caretEdges(box); \
  if ((event.key === 'ArrowUp' && !edges.isFirst) || (event.key === 'ArrowDown' && !edges.isLast)) { event.stopPropagation(); } \
}, true);";

/// Where a walk through the earlier prompts is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PromptHistory {
    /// The entry shown, or `None` while the draft is the user's own.
    position: Option<usize>,
    /// The draft the walk started from, given back past the newest entry.
    stash: String,
}

impl PromptHistory {
    /// ↑: the entry before the one shown, oldest first. `None` when
    /// nothing older exists, so the caller leaves the draft alone.
    pub(crate) fn older(&mut self, entries: &[String], draft: &str) -> Option<String> {
        let next = match self.position {
            None => {
                self.stash = draft.to_owned();
                entries.len().checked_sub(1)?
            }
            Some(0) => return None,
            Some(position) => position - 1,
        };
        self.position = Some(next);
        entries.get(next).cloned()
    }

    /// ↓: the entry after the one shown, or the stashed draft past the
    /// newest. `None` when no walk is on.
    pub(crate) fn newer(&mut self, entries: &[String]) -> Option<String> {
        let position = self.position?;
        if position + 1 < entries.len() {
            self.position = Some(position + 1);
            return entries.get(position + 1).cloned();
        }
        self.position = None;
        Some(std::mem::take(&mut self.stash))
    }

    /// True while an entry is shown instead of the user's own draft.
    pub(crate) fn is_walking(&self) -> bool {
        self.position.is_some()
    }

    /// Ends the walk: the user typed, or sent.
    pub(crate) fn reset(&mut self) {
        self.position = None;
        self.stash.clear();
    }
}

/// The prompts to walk: the user's own messages, oldest first, without
/// the same text twice in a row. A Finish press is not a prompt.
pub(crate) fn prompt_entries(messages: &[api::ChatMessage]) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    for message in messages {
        if message.role != "user" || message.is_continue || message.content.trim().is_empty() {
            continue;
        }
        if entries.last() != Some(&message.content) {
            entries.push(message.content.clone());
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(content: &str, is_continue: bool) -> api::ChatMessage {
        api::ChatMessage {
            role: "user".to_owned(),
            content: content.to_owned(),
            design: None,
            question_set: None,
            is_continue,
            at: None,
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn the_entries_are_the_users_prompts_without_repeats_or_finishes() {
        let mut messages = vec![user("A", false), user("A", false), user("Finish.", true)];
        messages.push(api::ChatMessage {
            role: "assistant".to_owned(),
            ..user("Done.", false)
        });
        messages.push(user("B", false));
        assert_eq!(
            prompt_entries(&messages),
            vec!["A".to_owned(), "B".to_owned()]
        );
    }

    #[test]
    fn the_guard_stops_arrows_that_have_a_line_to_move_to() {
        assert!(ARROW_GUARD.contains("event.stopPropagation()"));
        assert!(ARROW_GUARD.contains("box.selectionStart"));
        // The visual line, not the `\n` count: a wrapped line is several.
        assert!(ARROW_GUARD.contains("mirror.style.whiteSpace = 'pre-wrap'"));
        assert!(ARROW_GUARD.ends_with("}, true);"));
    }

    #[test]
    fn the_arrows_walk_back_then_forward_to_the_draft() {
        let entries = vec!["A".to_owned(), "B".to_owned()];
        let mut history = PromptHistory::default();
        assert_eq!(history.older(&entries, "typing").as_deref(), Some("B"));
        assert_eq!(history.older(&entries, "").as_deref(), Some("A"));
        assert_eq!(history.older(&entries, ""), None);
        assert!(history.is_walking());
        assert_eq!(history.newer(&entries).as_deref(), Some("B"));
        assert_eq!(history.newer(&entries).as_deref(), Some("typing"));
        assert!(!history.is_walking());
        assert_eq!(history.newer(&entries), None);
        assert_eq!(PromptHistory::default().older(&[], ""), None);
    }
}
