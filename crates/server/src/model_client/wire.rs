use super::*;

pub(super) fn http_failure(
    status: u16,
    message: String,
    retry_after: Option<std::time::Duration>,
) -> RequestFailure {
    let is_temporary = (matches!(status, 408 | 429)
        || ((500..=599).contains(&status) && !matches!(status, 501 | 505)))
        && !is_account_failure(&message);
    if is_temporary {
        RequestFailure::Temporary {
            message,
            retry_after,
        }
    } else {
        RequestFailure::Permanent(message)
    }
}

/// Reads a `usage` object in OpenAI or Anthropic shape. Cached input
/// tokens count toward the context size. `None` when absent.
pub(super) fn parse_usage(usage: &serde_json::Value) -> Option<TokenUsage> {
    let input = usage["prompt_tokens"]
        .as_u64()
        .or_else(|| usage["input_tokens"].as_u64())?;
    let cached = usage["cache_read_input_tokens"].as_u64().unwrap_or(0)
        + usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
    let output = usage["completion_tokens"]
        .as_u64()
        .or_else(|| usage["output_tokens"].as_u64())
        .unwrap_or(0);
    Some(TokenUsage {
        input_tokens: input + cached,
        output_tokens: output,
    })
}

/// Replaces every `file` part in `messages` with a text note, for
/// providers that reject file parts.
pub(crate) fn without_file_parts(messages: &[serde_json::Value]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .map(|message| {
            let Some(parts) = message["content"].as_array() else {
                return message.clone();
            };
            let replaced: Vec<serde_json::Value> = parts
                .iter()
                .map(|part| match part["type"].as_str() {
                    Some("file") => serde_json::json!({
                        "type": "text",
                        "text": format!(
                            "(the file {} cannot be sent to this provider)",
                            part["file"]["filename"].as_str().unwrap_or("?")
                        ),
                    }),
                    _ => part.clone(),
                })
                .collect();
            let mut message = message.clone();
            message["content"] = serde_json::Value::Array(replaced);
            message
        })
        .collect()
}

/// Splits a `data:{media};base64,{data}` URL into its media type and
/// data. `None` for any other URL.
pub(super) fn split_data_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (media_type, data) = rest.split_once(";base64,")?;
    Some((media_type, data))
}

/// Converts OpenAI-style content (a string or parts) to Anthropic
/// content blocks.
pub(super) fn anthropic_content(content: &serde_json::Value) -> serde_json::Value {
    let Some(parts) = content.as_array() else {
        return content.clone();
    };
    let blocks: Vec<serde_json::Value> = parts
        .iter()
        .filter_map(|part| match part["type"].as_str() {
            Some("text") => Some(serde_json::json!({ "type": "text", "text": part["text"] })),
            Some("image_url") => {
                let (media_type, data) = split_data_url(part["image_url"]["url"].as_str()?)?;
                Some(serde_json::json!({
                    "type": "image",
                    "source": { "type": "base64", "media_type": media_type, "data": data },
                }))
            }
            Some("file") => {
                let (media_type, data) = split_data_url(part["file"]["file_data"].as_str()?)?;
                Some(serde_json::json!({
                    "type": "document",
                    "source": { "type": "base64", "media_type": media_type, "data": data },
                    "title": part["file"]["filename"],
                }))
            }
            _ => None,
        })
        .collect();
    serde_json::Value::Array(blocks)
}

/// Converts OpenAI-style content (a string or parts) to ChatGPT
/// responses-API input parts. `text_type` is `input_text` or
/// `output_text`.
pub(super) fn chatgpt_content(content: &serde_json::Value, text_type: &str) -> serde_json::Value {
    let Some(parts) = content.as_array() else {
        let text = content.as_str().unwrap_or_default();
        return serde_json::json!([{ "type": text_type, "text": text }]);
    };
    let items: Vec<serde_json::Value> = parts
        .iter()
        .filter_map(|part| match part["type"].as_str() {
            Some("text") => Some(serde_json::json!({ "type": text_type, "text": part["text"] })),
            Some("image_url") => Some(serde_json::json!({
                "type": "input_image",
                "image_url": part["image_url"]["url"],
                "detail": "auto",
            })),
            Some("file") => Some(serde_json::json!({
                "type": "input_file",
                "filename": part["file"]["filename"],
                "file_data": part["file"]["file_data"],
            })),
            _ => None,
        })
        .collect();
    serde_json::Value::Array(items)
}

/// Builds an Anthropic messages-API body from OpenAI-style messages.
/// Claude login tokens require the Claude Code identity as the first
/// system block.
pub(crate) fn anthropic_messages_body(
    model: &str,
    messages: &[serde_json::Value],
) -> serde_json::Value {
    let mut system_blocks = vec![serde_json::json!({
        "type": "text",
        "text": "You are Claude Code, Anthropic's official CLI for Claude.",
    })];
    let mut chat_messages = Vec::new();
    for message in messages {
        if message["role"] == "system" {
            system_blocks.push(serde_json::json!({
                "type": "text",
                "text": message["content"],
            }));
        } else {
            chat_messages.push(serde_json::json!({
                "role": message["role"],
                "content": anthropic_content(&message["content"]),
            }));
        }
    }
    serde_json::json!({
        "model": model,
        "max_tokens": 16000,
        "stream": true,
        "system": system_blocks,
        "messages": chat_messages,
    })
}

/// Builds a ChatGPT responses-API body from OpenAI-style messages.
pub(crate) fn chatgpt_responses_body(
    model: &str,
    messages: &[serde_json::Value],
    effort: &str,
) -> serde_json::Value {
    let mut instructions = String::new();
    let mut input_items = Vec::new();
    for message in messages {
        if message["role"] == "system" {
            instructions.push_str(message["content"].as_str().unwrap_or_default());
            instructions.push('\n');
        } else {
            let content_type = if message["role"] == "assistant" {
                "output_text"
            } else {
                "input_text"
            };
            input_items.push(serde_json::json!({
                "type": "message",
                "role": message["role"],
                "content": chatgpt_content(&message["content"], content_type),
            }));
        }
    }
    serde_json::json!({
        "model": model,
        "instructions": instructions,
        "input": input_items,
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "store": false,
        "stream": true,
        "reasoning": { "effort": effort },
    })
}

/// What one event stream has delivered so far.
#[derive(Debug, Default)]
pub(super) struct StreamState {
    /// The assistant text, in order.
    pub(super) collected: String,
    /// Token usage, when the stream reported it.
    pub(super) usage: Option<TokenUsage>,
    /// The failure the stream reported, when it did.
    pub(super) failure: Option<String>,
    /// The structured failure code, when the provider supplied one.
    pub(super) failure_code: Option<String>,
    /// True once one `data:` event parsed: the reply is a stream.
    pub(super) saw_event: bool,
}

/// Folds one line of an event stream into `state`. Returns true when
/// the line added assistant text.
pub(super) fn stream_line(wire: WireFormat, line: &str, state: &mut StreamState) -> bool {
    let Some(data) = line.strip_prefix("data:") else {
        return false;
    };
    let data = data.trim();
    if data == "[DONE]" {
        return false;
    }
    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
        return false;
    };
    state.saw_event = true;
    let (text, usage, failure) = match wire {
        WireFormat::OpenAiChat => (
            event["choices"][0]["delta"]["content"].as_str(),
            parse_usage(&event["usage"]),
            event["error"]["message"].as_str(),
        ),
        WireFormat::AnthropicMessages => match event["type"].as_str() {
            Some("content_block_delta") => (event["delta"]["text"].as_str(), None, None),
            Some("message_start") => (None, parse_usage(&event["message"]["usage"]), None),
            // The closing delta carries output tokens only.
            Some("message_delta") => (
                None,
                event["usage"]["output_tokens"]
                    .as_u64()
                    .map(|output_tokens| TokenUsage {
                        input_tokens: event["usage"]["input_tokens"].as_u64().unwrap_or(0),
                        output_tokens,
                    }),
                None,
            ),
            Some("error") => (None, None, event["error"]["message"].as_str()),
            _ => (None, None, None),
        },
        WireFormat::ChatGptResponses => match event["type"].as_str() {
            Some("response.output_text.delta") => (event["delta"].as_str(), None, None),
            Some("response.completed") => (None, parse_usage(&event["response"]["usage"]), None),
            Some("response.failed") | Some("error") => (
                None,
                None,
                Some(
                    event["response"]["error"]["message"]
                        .as_str()
                        .or_else(|| event["message"].as_str())
                        .unwrap_or("the stream reported a failure"),
                ),
            ),
            _ => (None, None, None),
        },
    };
    if let Some(usage) = usage {
        // Anthropic reports input tokens at the start and output
        // tokens at the end; keep the larger of each.
        let previous = state.usage.unwrap_or_default();
        state.usage = Some(TokenUsage {
            input_tokens: previous.input_tokens.max(usage.input_tokens),
            output_tokens: previous.output_tokens.max(usage.output_tokens),
        });
    }
    if let Some(failure) = failure {
        state.failure = Some(failure.to_owned());
        state.failure_code = event["response"]["error"]["code"]
            .as_str()
            .or_else(|| event["error"]["code"].as_str())
            .or_else(|| event["error"]["type"].as_str())
            .or_else(|| event["code"].as_str())
            .map(str::to_owned);
    }
    match text {
        Some(text) if !text.is_empty() => {
            state.collected.push_str(text);
            true
        }
        _ => false,
    }
}
