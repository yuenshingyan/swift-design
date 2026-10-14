use super::*;

/// The mailing system prompt: role, mailing rules, the mailing
/// schema, the clarification protocol, and one example mailing.
pub(super) fn mailing_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Mailing)).unwrap_or_default();
    format!(
        "You build mailings as JSON mailings: emails, newsletters, and email sequences read in an inbox. \
         Each email is one HTML fragment plus its own CSS, for the px canvas of the mailing's format, \
         600 px wide: 600 by 800 px for short, 600 by 1200 px for standard, \
         600 by 1800 px for long. \
         One email is a single send. Two or more emails are a sequence, in send order.\n\
         Follow these rules:\n{rules}\n\
         The mailing must conform to this JSON Schema:\n{schema}\n\
         Example mailing:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one mailing JSON. No prose, no code fences.",
        rules = MAILING_RULES.join("\n"),
        example = include_str!("../../../../fixtures/sample-mailing.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` emails and
/// the full outline.
pub(super) fn mailing_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} emails of the mailing, in order, starting with \
         the first email. Put the email titles of the complete mailing in `outline`, in order, \
         every email title of the complete mailing. The app asks you for the remaining emails \
         later. Make these {count} emails show the theme, the layout language, and the text \
         density of the whole mailing.\n"
    )
}

/// The prompt line for the app's format choice. Empty when the agent
/// decides it, or when the user typed a value the JSON does not carry.
pub(super) fn format_note(format: Option<&str>) -> String {
    match format.and_then(EmailFormat::from_name) {
        Some(format) => {
            let viewport = format.viewport();
            format!(
                "Lay the emails out on the {} format: {} by {} px. Set `format` to `{}`.\n",
                format.as_str(),
                viewport.width,
                viewport.height,
                format.as_str()
            )
        }
        None => String::new(),
    }
}

/// The prompt line that holds the mailing to the length the user
/// asked for. Empty when the user set no length. A preview writes fewer
/// emails than the length, so the count goes to the outline instead.
pub(super) fn email_count_note(email_count: Option<u32>, preview_emails: Option<usize>) -> String {
    let Some(count) = email_count else {
        return String::new();
    };
    match preview_emails {
        Some(_) => {
            format!("The user asked for {count} emails. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} emails. Write exactly {count} emails.\n"),
    }
}

/// The user prompt for one mailing candidate: the request and the
/// answers are authoritative, plus the template, preview, concept, and
/// effort notes.
pub(super) fn mailing_candidate_prompt(request: &MailingCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a mailing for this request. The request and the answers are \
         authoritative; do not override an answer.\n{}\n",
        request_input(&request.context.request)
    );
    if let Some(template) = request.template {
        prompt.push_str(&template_note(template));
        prompt.push_str(
            "The template screens are screens, slides, or pages of another artifact. Use them \
             for the look only.\n",
        );
    }
    if let Some(count) = request.preview_emails {
        prompt.push_str(&mailing_preview_note(count));
    }
    prompt.push_str(&email_count_note(
        options.email_count,
        request.preview_emails,
    ));
    prompt.push_str(&format_note(options.email_format.as_deref()));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("mailing", merge));
    }
    let count = options.variation_count();
    // A merge is one candidate on its own, not one of the run's set.
    if count > 1 && request.merge.is_none() {
        prompt.push_str(&format!(
            "This is candidate {candidate_number} of {count}. Make it distinct from the other \
             candidates in theme, structure, and angle.\n"
        ));
        prompt.push_str(&concept_note(request.concepts, candidate_number - 1));
    }
    match options.effort.as_str() {
        "low" => prompt.push_str("Keep the mailing concise: fewer emails, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the mailing JSON.");
    prompt
}

/// The user prompt for a mailing edit: the mailing as it is, the
/// request, and the change the user asked for.
pub(super) fn mailing_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the mailing to change:\n{mailing_json}\n{note}\
         The mailing is for this request:\n{request}\n{conversation}\
         Apply this change: {critique}\n{findings}\
         A reference like [email 3, node 0/1 <h2.title>: What changed] names an email \
         (1-based) and one element in that email's html by its index path from the email root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [email 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one email the same way, without their text. A reference like [email 3] \
         names the email alone: the change is about that email. Change only what the critique asks for. Keep every other email and \
         value as it is. Return every changed email complete: html, css, and notes.\n{format}",
        mailing_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        conversation = conversation_block(input.conversation),
        findings = findings_note(input.findings),
        format = PATCH_FORMAT
    )
}

/// The mailing as a focused edit sees it: the title, the theme, the
/// format, the email count, and only the emails at `indexes`, each
/// with its index.
pub(super) fn focused_mailing_json(
    mailing: &Mailing,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let emails: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            mailing.emails.get(*index).map(|email| {
                let email = if is_fresh {
                    fresh_email(email)
                } else {
                    email.clone()
                };
                serde_json::json!({ "index": index, "email": email })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": mailing.title,
        "theme": mailing.theme,
        "format": mailing.format,
        "email_count": mailing.emails.len(),
        "emails": emails,
    }))
}

/// The email as a regenerate shows it: its notes, without its markup, so
/// the model writes it anew instead of tweaking it.
pub(super) fn fresh_email(email: &Email) -> Email {
    Email {
        html: String::new(),
        css: None,
        ..email.clone()
    }
}

/// The user prompt for one mailing continuation chunk: the preview
/// mailing and the chunk's emails to add, as a patch of inserts.
pub(super) fn mailing_continue_prompt(
    request: &SessionRequest,
    mailing: &Mailing,
    mailing_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = mailing.emails.len();
    let planned = mailing.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = mailing
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a mailing in progress: its theme, its format, its first {written} \
         emails, and `outline`, the email titles of the complete mailing:\n{mailing_json}\n\
         The mailing is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} emails: outline titles {} to {last} of {planned}, in order, one email per \
         title:\n{}\n\
         Keep the theme. Match the existing emails in CSS style, font sizes, spacing, colors, \
         and visual language, so the mailing reads as one piece. Do not change or repeat \
         the existing emails.\n",
        next_titles.len(),
        first + 1,
        next_titles.join("\n")
    ));
    if first > written || last < planned {
        prompt.push_str(
            "Other requests write the other outline titles at the same time. Write only these.\n",
        );
    }
    prompt.push_str(&format!(
        "Reply with only a JSON patch that appends the new emails, not the whole mailing:\n\
         {{\"emails\":[{{\"index\":{written},\"insert\":true,\"email\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new email index {written} and insert true, in send order. Each email \
         carries html, css, and notes. Omit title, theme, format, outline, and the existing emails."
    ));
    prompt
}

/// Extracts and parses the mailing JSON from a model reply.
pub(super) fn parse_mailing(content: &str) -> Result<Mailing, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end]).map_err(|error| format!("invalid mailing: {error}"))
}
