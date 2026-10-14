use super::*;

/// The campaign system prompt: role, campaign rules, the campaign
/// schema, the clarification protocol, and one example campaign.
pub(super) fn campaign_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Campaign)).unwrap_or_default();
    format!(
        "You build display ad campaigns as JSON campaigns: banners and display units read on a page. \
         Each ad is one HTML fragment plus its own CSS, for the px canvas of the campaign's size: \
         300 by 250 px for medium_rectangle, 728 by 90 px for leaderboard, 300 by 600 px for \
         half_page, 160 by 600 px for skyscraper, 320 by 100 px for mobile_banner. \
         One ad is a single placement. Two or more ads are A/B variants of the same size, in priority order.\n\
         Follow these rules:\n{rules}\n\
         The campaign must conform to this JSON Schema:\n{schema}\n\
         Example campaign:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one campaign JSON. No prose, no code fences.",
        rules = CAMPAIGN_RULES.join("\n"),
        example = include_str!("../../../../fixtures/sample-campaign.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` ads and
/// the full outline.
pub(super) fn campaign_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} ads of the campaign, in order, starting with \
         the first ad. Put the ad titles of the complete campaign in `outline`, in order, \
         every ad title of the complete campaign. The app asks you for the remaining ads \
         later. Make these {count} ads show the theme, the layout language, and the text \
         density of the whole campaign.\n"
    )
}

/// The prompt line for the app's size choice. Empty when the agent
/// decides it, or when the user typed a value the JSON does not carry.
pub(super) fn size_note(size: Option<&str>) -> String {
    match size.and_then(AdSize::from_name) {
        Some(size) => {
            let viewport = size.viewport();
            format!(
                "Lay the ads out on the {} size: {} by {} px. Set `size` to `{}`.\n",
                size.as_str(),
                viewport.width,
                viewport.height,
                size.as_str()
            )
        }
        None => String::new(),
    }
}

/// The prompt line that holds the campaign to the length the user
/// asked for. Empty when the user set no length. A preview writes fewer
/// ads than the length, so the count goes to the outline instead.
pub(super) fn ad_count_note(ad_count: Option<u32>, preview_ads: Option<usize>) -> String {
    let Some(count) = ad_count else {
        return String::new();
    };
    match preview_ads {
        Some(_) => {
            format!("The user asked for {count} ads. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} ads. Write exactly {count} ads.\n"),
    }
}

/// The user prompt for one campaign candidate: the request and the
/// answers are authoritative, plus the template, preview, concept, and
/// effort notes.
pub(super) fn campaign_candidate_prompt(request: &CampaignCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a campaign for this request. The request and the answers are \
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
    if let Some(count) = request.preview_ads {
        prompt.push_str(&campaign_preview_note(count));
    }
    prompt.push_str(&ad_count_note(options.ad_count, request.preview_ads));
    prompt.push_str(&size_note(options.ad_size.as_deref()));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("campaign", merge));
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
        "low" => prompt.push_str("Keep the campaign concise: fewer ads, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the campaign JSON.");
    prompt
}

/// The user prompt for a campaign edit: the campaign as it is, the
/// request, and the change the user asked for.
pub(super) fn campaign_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the campaign to change:\n{campaign_json}\n{note}\
         The campaign is for this request:\n{request}\n{conversation}\
         Apply this change: {critique}\n{findings}\
         A reference like [ad 3, node 0/1 <h2.title>: What changed] names an ad \
         (1-based) and one element in that ad's html by its index path from the ad root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [ad 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one ad the same way, without their text. A reference like [ad 3] \
         names the ad alone: the change is about that ad. Change only what the critique asks for. Keep every other ad and \
         value as it is. Return every changed ad complete: html, css, and notes.\n{format}",
        campaign_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        conversation = conversation_block(input.conversation),
        findings = findings_note(input.findings),
        format = PATCH_FORMAT
    )
}

/// The campaign as a focused edit sees it: the title, the theme, the
/// size, the ad count, and only the ads at `indexes`, each
/// with its index.
pub(super) fn focused_campaign_json(
    campaign: &Campaign,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let ads: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            campaign.ads.get(*index).map(|ad| {
                let ad = if is_fresh { fresh_ad(ad) } else { ad.clone() };
                serde_json::json!({ "index": index, "ad": ad })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": campaign.title,
        "theme": campaign.theme,
        "size": campaign.size,
        "ad_count": campaign.ads.len(),
        "ads": ads,
    }))
}

/// The ad as a regenerate shows it: its notes, without its markup, so
/// the model writes it anew instead of tweaking it.
pub(super) fn fresh_ad(ad: &Ad) -> Ad {
    Ad {
        html: String::new(),
        css: None,
        ..ad.clone()
    }
}

/// The user prompt for one campaign continuation chunk: the preview
/// campaign and the chunk's ads to add, as a patch of inserts.
pub(super) fn campaign_continue_prompt(
    request: &SessionRequest,
    campaign: &Campaign,
    campaign_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = campaign.ads.len();
    let planned = campaign.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = campaign
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a campaign in progress: its theme, its size, its first {written} \
         ads, and `outline`, the ad titles of the complete campaign:\n{campaign_json}\n\
         The campaign is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} ads: outline titles {} to {last} of {planned}, in order, one ad per \
         title:\n{}\n\
         Keep the theme. Match the existing ads in CSS style, font sizes, spacing, colors, \
         and visual language, so the campaign reads as one piece. Do not change or repeat \
         the existing ads.\n",
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
        "Reply with only a JSON patch that appends the new ads, not the whole campaign:\n\
         {{\"ads\":[{{\"index\":{written},\"insert\":true,\"ad\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new ad index {written} and insert true, in priority order. Each ad \
         carries html, css, and notes. Omit title, theme, size, outline, and the existing ads."
    ));
    prompt
}

/// Extracts and parses the campaign JSON from a model reply.
pub(super) fn parse_campaign(content: &str) -> Result<Campaign, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end])
        .map_err(|error| format!("invalid campaign: {error}"))
}
