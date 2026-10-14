#![allow(clippy::unwrap_used)]

/// Collects the assistant text and token usage from a whole ChatGPT
/// responses event stream.
#[cfg(test)]
fn parse_chatgpt_stream(stream_text: &str) -> Result<(String, Option<TokenUsage>), String> {
    let mut state = StreamState::default();
    for line in stream_text.lines() {
        stream_line(WireFormat::ChatGptResponses, line, &mut state);
    }
    if state.collected.is_empty() {
        return Err(state
            .failure
            .unwrap_or_else(|| "the stream carried no output text".to_owned()));
    }
    Ok((state.collected, state.usage))
}
use axum::http::StatusCode;

use crate::model_retry::RequestFailure;
use crate::test_support::FakeModelServer;

use super::{
    ModelClient, StreamState, TokenUsage, WireFormat, chatgpt_responses_body, context_window,
    http_failure, parse_usage, stream_line,
};

#[tokio::test]
async fn retry_after_headers_are_read_without_retrying_before_the_requested_time() {
    for header in ["61", "Fri, 01 Jan 9999 00:00:00 GMT"] {
        let fake = FakeModelServer::start().await;
        fake.push_error_with_retry_after(StatusCode::SERVICE_UNAVAILABLE, "overloaded", header);
        let model = ModelClient::new(fake.configuration(), None);
        let error = model
            .chat_with(&reqwest::Client::new(), serde_json::json!({}), None)
            .await
            .unwrap_err();
        assert!(error.contains("above the automatic limit"));
        assert_eq!(fake.requests().len(), 1);
    }
}

#[tokio::test]
async fn structured_quota_errors_never_retry_even_when_the_message_says_try_again() {
    for body in [
        r#"data: {"error":{"message":"Please try again later.","code":"insufficient_quota"}}

"#,
        r#"{"error":{"message":"Please try again later.","code":"insufficient_quota"}}"#,
    ] {
        let fake = FakeModelServer::start().await;
        fake.push_stream(body);
        let model = ModelClient::new(fake.configuration(), None);
        assert!(
            model
                .chat_with(&reqwest::Client::new(), serde_json::json!({}), None)
                .await
                .is_err()
        );
        assert_eq!(fake.requests().len(), 1);
    }
}

#[tokio::test]
async fn streamed_errors_discard_partial_text_and_retry_the_same_body() {
    let fake = FakeModelServer::start().await;
    fake.push_stream("data: {\"choices\":[{\"delta\":{\"content\":\"discard partial text\"}}]}\n\ndata: {\"error\":{\"message\":\"Our servers are currently overloaded.\"}}\n\n");
    fake.push_text("complete reply");
    let model = ModelClient::new(fake.configuration(), None);
    let result = model
        .chat_with(
            &reqwest::Client::new(),
            serde_json::json!({"model":"fake-model","messages":[]}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(result, "complete reply");
    let requests = fake.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
}

#[test]
fn http_failures_retry_transient_statuses_but_not_account_or_request_errors() {
    for status in [408, 429, 500, 502, 503, 504] {
        assert!(matches!(
            http_failure(status, "temporarily unavailable".to_owned(), None),
            RequestFailure::Temporary { .. }
        ));
    }
    for status in [400, 401, 403, 404, 501, 505] {
        assert!(matches!(
            http_failure(status, "overloaded".to_owned(), None),
            RequestFailure::Permanent(_)
        ));
    }
    assert!(matches!(
        http_failure(429, "insufficient_quota".to_owned(), None),
        RequestFailure::Permanent(_)
    ));
}

#[test]
fn usage_parses_openai_and_anthropic_shapes() {
    let openai = serde_json::json!({"prompt_tokens": 50, "completion_tokens": 5});
    assert_eq!(
        parse_usage(&openai),
        Some(TokenUsage {
            input_tokens: 50,
            output_tokens: 5
        })
    );
    let anthropic = serde_json::json!({
        "input_tokens": 10, "cache_read_input_tokens": 30, "output_tokens": 4
    });
    assert_eq!(
        parse_usage(&anthropic),
        Some(TokenUsage {
            input_tokens: 40,
            output_tokens: 4
        })
    );
    assert_eq!(parse_usage(&serde_json::json!(null)), None);
}

#[test]
fn chatgpt_bodies_split_instructions_from_input() {
    let messages = vec![
        serde_json::json!({"role": "system", "content": "sys"}),
        serde_json::json!({"role": "user", "content": "hello"}),
        serde_json::json!({"role": "assistant", "content": "draft"}),
    ];
    let body = chatgpt_responses_body("gpt-5.5", &messages, "high");
    assert_eq!(body["instructions"], "sys\n");
    assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(body["input"][1]["content"][0]["type"], "output_text");
    assert_eq!(body["stream"], true);
    assert_eq!(body["reasoning"]["effort"], "high");
}

#[test]
fn chatgpt_streams_collect_deltas_and_report_failures() {
    let stream = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"a\\\"\"}\n\
                  data: {\"type\":\"response.output_text.delta\",\"delta\":\":1}\"}\n\
                  data: {\"type\":\"response.completed\",\
                  \"response\":{\"usage\":{\"input_tokens\":120,\"output_tokens\":7}}}\n";
    let (content, usage) = parse_chatgpt_stream(stream).unwrap();
    assert_eq!(content, "{\"a\":1}");
    assert_eq!(
        usage,
        Some(TokenUsage {
            input_tokens: 120,
            output_tokens: 7
        })
    );
    let failed = "data: {\"type\":\"response.failed\",\
                  \"response\":{\"error\":{\"message\":\"quota\"}}}\n";
    assert!(parse_chatgpt_stream(failed).unwrap_err().contains("quota"));
}

#[test]
fn stream_lines_collect_text_and_usage_per_wire_format() {
    let mut state = StreamState::default();
    assert!(stream_line(
        WireFormat::OpenAiChat,
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}",
        &mut state
    ));
    assert!(!stream_line(
        WireFormat::OpenAiChat,
        "data: {\"choices\":[{\"delta\":{}}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":2}}",
        &mut state
    ));
    assert!(!stream_line(
        WireFormat::OpenAiChat,
        "data: [DONE]",
        &mut state
    ));
    assert_eq!(state.collected, "Hel");
    assert_eq!(state.usage.unwrap().input_tokens, 9);
    assert!(state.saw_event);
    let mut anthropic = StreamState::default();
    stream_line(
        WireFormat::AnthropicMessages,
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":30,\"output_tokens\":1}}}",
        &mut anthropic,
    );
    assert!(stream_line(
        WireFormat::AnthropicMessages,
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"{\\\"a\\\"\"}}",
        &mut anthropic,
    ));
    stream_line(
        WireFormat::AnthropicMessages,
        "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":12}}",
        &mut anthropic,
    );
    assert_eq!(anthropic.collected, "{\"a\"");
    assert_eq!(
        anthropic.usage,
        Some(TokenUsage {
            input_tokens: 30,
            output_tokens: 12
        })
    );
    let mut plain = StreamState::default();
    assert!(!stream_line(
        WireFormat::OpenAiChat,
        "{\"choices\":[]}",
        &mut plain
    ));
    assert!(!plain.saw_event);
}

#[test]
fn context_windows_follow_the_model_family() {
    assert_eq!(context_window("gemini-2.5-pro"), 1_048_576);
    assert_eq!(context_window("gpt-5-mini"), 400_000);
    assert_eq!(context_window("claude-sonnet-5"), 200_000);
    assert_eq!(context_window("llama3.1"), 128_000);
}
