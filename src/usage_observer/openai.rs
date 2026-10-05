use super::json_usage::JsonUsageFieldExtractor;
use super::sse::{SseDecoder, SseFrame};
use crate::local_gateway::{ResponseBodyObserver, ResponseStreamEnd};
use crate::usage_event::{TokenUsage, UsageEventRecorder, UsageOutcome};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct OpenAiUsageRaw {
    input_tokens: Option<u64>,
    prompt_tokens: Option<u64>,
    output_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
    input_tokens_details: Option<OpenAiInputDetails>,
    prompt_tokens_details: Option<OpenAiInputDetails>,
}

#[derive(Debug, Deserialize)]
struct OpenAiInputDetails {
    cached_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponseEnvelope {
    usage: Option<OpenAiUsageRaw>,
    response: Option<OpenAiResponseBody>,
    #[serde(rename = "type")]
    event_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponseBody {
    usage: Option<OpenAiUsageRaw>,
}

#[derive(Clone, Copy)]
pub(crate) enum OpenAiProtocol {
    Responses,
    ChatCompletions,
}

#[derive(Default)]
struct OpenAiSseUsage {
    decoder: SseDecoder,
    usage: TokenUsage,
    terminal: Option<TerminalEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalEvent {
    Completed,
    Incomplete,
    Failed,
}

impl OpenAiSseUsage {
    fn push(&mut self, bytes: &[u8], protocol: OpenAiProtocol) {
        let (decoder, usage, terminal) = (&mut self.decoder, &mut self.usage, &mut self.terminal);
        decoder.push(bytes, |frame| {
            observe_openai_frame(frame, protocol, usage, terminal)
        });
    }

    fn finish(&mut self, protocol: OpenAiProtocol) {
        let (decoder, usage, terminal) = (&mut self.decoder, &mut self.usage, &mut self.terminal);
        decoder.finish(|frame| observe_openai_frame(frame, protocol, usage, terminal));
    }
}

fn observe_openai_frame(
    frame: SseFrame,
    protocol: OpenAiProtocol,
    usage: &mut TokenUsage,
    terminal: &mut Option<TerminalEvent>,
) {
    if frame.data == b"[DONE]" {
        *terminal = Some(TerminalEvent::Completed);
        return;
    }
    let envelope = serde_json::from_slice::<OpenAiResponseEnvelope>(&frame.data).ok();
    let event_type = frame.event.as_deref().or_else(|| {
        envelope
            .as_ref()
            .and_then(|envelope| envelope.event_type.as_deref())
    });
    *terminal = match (protocol, event_type) {
        (OpenAiProtocol::Responses, Some("response.completed")) => Some(TerminalEvent::Completed),
        (OpenAiProtocol::Responses, Some("response.incomplete")) => Some(TerminalEvent::Incomplete),
        (OpenAiProtocol::Responses, Some("response.failed" | "error"))
        | (OpenAiProtocol::ChatCompletions, Some("error")) => Some(TerminalEvent::Failed),
        _ => *terminal,
    };
    let Some(envelope) = envelope else {
        return;
    };
    let update = match protocol {
        OpenAiProtocol::Responses => envelope
            .response
            .and_then(|response| response.usage)
            .or(envelope.usage),
        OpenAiProtocol::ChatCompletions => envelope.usage,
    };
    if let Some(update) = update.map(normalize_openai_usage) {
        usage.merge(&update);
    }
}

#[allow(dead_code)]
pub(crate) fn responses_json_usage(body: &[u8]) -> Option<TokenUsage> {
    let envelope = serde_json::from_slice::<OpenAiResponseEnvelope>(body).ok()?;
    envelope
        .response
        .and_then(|response| response.usage)
        .or(envelope.usage)
        .map(normalize_openai_usage)
}

#[allow(dead_code)]
pub(crate) fn chat_completions_json_usage(body: &[u8]) -> Option<TokenUsage> {
    serde_json::from_slice::<OpenAiResponseEnvelope>(body)
        .ok()?
        .usage
        .map(normalize_openai_usage)
}

#[allow(dead_code)]
pub(crate) fn responses_sse_usage<'a>(
    chunks: impl IntoIterator<Item = &'a [u8]>,
) -> Option<TokenUsage> {
    sse_usage(chunks, OpenAiProtocol::Responses)
}

#[allow(dead_code)]
pub(crate) fn chat_completions_sse_usage<'a>(
    chunks: impl IntoIterator<Item = &'a [u8]>,
) -> Option<TokenUsage> {
    sse_usage(chunks, OpenAiProtocol::ChatCompletions)
}

#[allow(dead_code)]
fn sse_usage<'a>(
    chunks: impl IntoIterator<Item = &'a [u8]>,
    protocol: OpenAiProtocol,
) -> Option<TokenUsage> {
    let mut stream = OpenAiSseUsage::default();
    for chunk in chunks {
        stream.push(chunk, protocol);
    }
    stream.finish(protocol);
    (stream.usage.completeness() != crate::usage_event::UsageCompleteness::Unavailable)
        .then_some(stream.usage)
}

pub(crate) struct OpenAiResponsesObserver {
    stream: OpenAiSseUsage,
    recorder: Option<UsageEventRecorder>,
    status: u16,
    protocol: OpenAiProtocol,
    is_sse: bool,
    json_usage: JsonUsageFieldExtractor,
    finished: bool,
}

impl OpenAiResponsesObserver {
    pub(crate) fn new(recorder: UsageEventRecorder, status: u16) -> Self {
        Self::for_protocol(recorder, status, OpenAiProtocol::Responses, true)
    }

    pub(crate) fn for_protocol(
        recorder: UsageEventRecorder,
        status: u16,
        protocol: OpenAiProtocol,
        is_sse: bool,
    ) -> Self {
        Self {
            stream: OpenAiSseUsage::default(),
            recorder: Some(recorder),
            status,
            protocol,
            is_sse,
            json_usage: JsonUsageFieldExtractor::new(matches!(protocol, OpenAiProtocol::Responses)),
            finished: false,
        }
    }
}

impl ResponseBodyObserver for OpenAiResponsesObserver {
    fn observe(&mut self, bytes: &[u8]) {
        if self.is_sse {
            self.stream.push(bytes, self.protocol);
        } else {
            self.json_usage.push(bytes);
        }
        if let Some(recorder) = &mut self.recorder {
            recorder.set_usage(&self.stream.usage);
        }
    }

    fn finish(&mut self, stream_end: ResponseStreamEnd) {
        if self.is_sse {
            self.stream.finish(self.protocol);
        } else {
            self.json_usage.finish();
            let usage = non_sse_json_usage(&self.json_usage, self.protocol);
            if let Some(usage) = usage {
                self.stream.usage.merge(&usage);
            }
        }
        let (outcome, category) = if !(200..300).contains(&self.status) {
            (UsageOutcome::Failed, Some("upstream_http"))
        } else if matches!(stream_end, ResponseStreamEnd::Error) {
            (UsageOutcome::Failed, Some("upstream_stream"))
        } else if !self.is_sse {
            (UsageOutcome::Succeeded, None)
        } else {
            match self.stream.terminal {
                Some(TerminalEvent::Failed) => {
                    (UsageOutcome::Failed, Some("upstream_response_error"))
                }
                Some(TerminalEvent::Incomplete) => {
                    (UsageOutcome::Incomplete, Some("incomplete_response"))
                }
                Some(TerminalEvent::Completed) => (UsageOutcome::Succeeded, None),
                None => (
                    UsageOutcome::Incomplete,
                    Some("stream_ended_without_terminal_event"),
                ),
            }
        };
        if let Some(mut recorder) = self.recorder.take() {
            recorder.set_usage(&self.stream.usage);
            recorder.finish(outcome, Some(self.status), category);
        }
        self.finished = true;
    }
}

impl Drop for OpenAiResponsesObserver {
    fn drop(&mut self) {
        if !self.finished {
            if let Some(mut recorder) = self.recorder.take() {
                recorder.set_usage(&self.stream.usage);
                let category = if (200..300).contains(&self.status) {
                    "stream_terminated"
                } else {
                    "upstream_http"
                };
                recorder.finish(UsageOutcome::Failed, Some(self.status), Some(category));
            }
        }
    }
}

fn normalize_openai_usage(raw: OpenAiUsageRaw) -> TokenUsage {
    let input_tokens_total = raw.input_tokens.or(raw.prompt_tokens);
    let output_tokens = raw.output_tokens.or(raw.completion_tokens);
    let cached = raw
        .input_tokens_details
        .and_then(|details| details.cached_tokens)
        .or_else(|| {
            raw.prompt_tokens_details
                .and_then(|details| details.cached_tokens)
        });
    let input_tokens_uncached = input_tokens_total
        .zip(cached)
        .and_then(|(total, cached)| total.checked_sub(cached));
    TokenUsage::canonical(
        input_tokens_total,
        input_tokens_uncached,
        cached,
        None,
        output_tokens,
        raw.total_tokens,
    )
}

fn non_sse_json_usage(
    extractor: &JsonUsageFieldExtractor,
    protocol: OpenAiProtocol,
) -> Option<TokenUsage> {
    let parse = |bytes: &[u8]| {
        serde_json::from_slice::<Option<OpenAiUsageRaw>>(bytes)
            .ok()
            .flatten()
            .map(normalize_openai_usage)
    };
    match protocol {
        OpenAiProtocol::Responses => match extractor.response_usage() {
            Some(bytes) => match serde_json::from_slice::<Option<OpenAiUsageRaw>>(bytes) {
                Ok(Some(raw)) => Some(normalize_openai_usage(raw)),
                Ok(None) => extractor.root_usage().and_then(parse),
                Err(_) => None,
            },
            None => extractor.root_usage().and_then(parse),
        },
        OpenAiProtocol::ChatCompletions => extractor.root_usage().and_then(parse),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_gateway::LaunchContext;
    use crate::usage_event::UsageCompleteness;
    use crate::usage_store::{UsageEventFilter, UsageStore};
    use assert_fs::TempDir;

    #[test]
    fn responses_usage_captures_cached_input_and_calculates_total() {
        let usage = responses_json_usage(
            br#"{"id":"resp-secret","output":[{"text":"response secret"}],"usage":{"input_tokens":15,"input_tokens_details":{"cached_tokens":4},"output_tokens":6}}"#,
        )
        .unwrap();

        assert_eq!(usage.input_tokens_total, Some(15));
        assert_eq!(usage.input_tokens_uncached, Some(11));
        assert_eq!(usage.cache_read_input_tokens, Some(4));
        assert_eq!(usage.total_tokens, Some(21));
        assert_eq!(usage.completeness(), UsageCompleteness::Complete);
    }

    #[test]
    fn chat_completions_usage_accepts_prompt_token_names() {
        let usage = chat_completions_json_usage(
            br#"{"choices":[{"message":{"content":"private output"}}],"usage":{"prompt_tokens":8,"completion_tokens":3,"total_tokens":11,"prompt_tokens_details":{"cached_tokens":2}}}"#,
        )
        .unwrap();

        assert_eq!(usage.input_tokens_total, Some(8));
        assert_eq!(usage.input_tokens_uncached, Some(6));
        assert_eq!(usage.cache_read_input_tokens, Some(2));
        assert_eq!(usage.total_tokens, Some(11));
    }

    #[test]
    fn responses_sse_tolerates_chunk_boundaries_and_missing_usage() {
        let mut stream = OpenAiSseUsage::default();
        let bytes = b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":9,\"output_tokens\":5}}}\n\n";
        for chunk in bytes.chunks(5) {
            stream.push(chunk, OpenAiProtocol::Responses);
        }
        stream.finish(OpenAiProtocol::Responses);
        assert_eq!(stream.terminal, Some(TerminalEvent::Completed));
        assert_eq!(stream.usage.total_tokens, Some(14));

        let mut missing = OpenAiSseUsage::default();
        missing.push(
            b"event: response.completed\ndata: {}\n\n",
            OpenAiProtocol::Responses,
        );
        assert_eq!(missing.usage.completeness(), UsageCompleteness::Unavailable);
    }

    #[test]
    fn chat_sse_extraction_does_not_require_a_usage_request_mutation() {
        let mut stream = OpenAiSseUsage::default();
        stream.push(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"secret\"}}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n",
            OpenAiProtocol::ChatCompletions,
        );
        assert_eq!(stream.usage.input_tokens_total, Some(4));
        assert_eq!(stream.usage.output_tokens, Some(2));
    }

    #[test]
    fn public_sse_helpers_accept_chunked_openai_responses_and_chat_streams() {
        let responses = [
            &b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":6,"[..],
            &b"\"output_tokens\":2}}}\n\n"[..],
        ];
        let response_usage = responses_sse_usage(responses).unwrap();
        assert_eq!(response_usage.total_tokens, Some(8));

        let chat = [
            &b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,"[..],
            &b"\"completion_tokens\":1}}\n\ndata: [DONE]\n\n"[..],
        ];
        let chat_usage = chat_completions_sse_usage(chat).unwrap();
        assert_eq!(chat_usage.total_tokens, Some(6));
    }

    #[test]
    fn non_sse_usage_extracts_across_chunk_splits_without_persisting_content() {
        let state = TempDir::new().unwrap();
        let store = UsageStore::new(state.path());
        let content_marker = "OPENAI_RESPONSE_CONTENT_MARKER_DO_NOT_PERSIST";
        let cases = [
            (
                OpenAiProtocol::Responses,
                "openai_responses",
                "responses-large",
                format!(
                    r#"{{"output":[{{"text":"{content_marker}{}"}},{{"type":"function_call_output","output":"TOOL_RESULT_MARKER_DO_NOT_PERSIST"}}],"response":{{"usage":{{"input_tokens":13,"input_tokens_details":{{"cached_tokens":4}},"output_tokens":6}}}}}}"#,
                    "x".repeat(64 * 1024)
                ),
                (Some(13), Some(9), Some(4), Some(6), Some(19)),
            ),
            (
                OpenAiProtocol::ChatCompletions,
                "openai_chat_completions",
                "chat-completions-large",
                format!(
                    r#"{{"choices":[{{"message":{{"content":"{content_marker}{}"}}}}],"usage":{{"prompt_tokens":10,"prompt_tokens_details":{{"cached_tokens":2}},"completion_tokens":4,"total_tokens":14}}}}"#,
                    "y".repeat(32 * 1024)
                ),
                (Some(10), Some(8), Some(2), Some(4), Some(14)),
            ),
            (
                OpenAiProtocol::Responses,
                "openai_responses",
                "responses-null-fallback",
                r#"{"usage":{"input_tokens":2,"output_tokens":1},"response":{"usage":null}}"#
                    .to_string(),
                (Some(2), None, None, Some(1), Some(3)),
            ),
        ];

        for (protocol, protocol_name, model, body, expected) in cases {
            let context = LaunchContext::new("work".to_string(), "review".to_string(), None, None);
            let mut recorder =
                UsageEventRecorder::new(Some(store.clone()), &context, protocol_name);
            recorder.set_model(Some(model));
            let mut observer =
                OpenAiResponsesObserver::for_protocol(recorder, 200, protocol, false);
            let mut offset = 0;
            let mut chunk_size = 1;
            while offset < body.len() {
                let end = (offset + chunk_size).min(body.len());
                observer.observe(&body.as_bytes()[offset..end]);
                offset = end;
                chunk_size = (chunk_size * 7 % 29).max(1);
            }
            observer.finish(ResponseStreamEnd::Complete);

            let events = store
                .events(&UsageEventFilter {
                    start_unix_ms: 0,
                    end_unix_ms: u64::MAX,
                    ..UsageEventFilter::default()
                })
                .unwrap();
            let event = events
                .iter()
                .find(|event| event.model.as_deref() == Some(model))
                .unwrap();
            assert_eq!(event.input_tokens_total, expected.0);
            assert_eq!(event.input_tokens_uncached, expected.1);
            assert_eq!(event.cache_read_input_tokens, expected.2);
            assert_eq!(event.output_tokens, expected.3);
            assert_eq!(event.total_tokens, expected.4);
            assert_eq!(event.outcome, UsageOutcome::Succeeded);
            assert_eq!(event.usage_completeness, UsageCompleteness::Complete);
        }

        for day in std::fs::read_dir(state.path().join("usage/events")).unwrap() {
            let day = day.unwrap();
            for event in std::fs::read_dir(day.path()).unwrap() {
                let persisted = std::fs::read_to_string(event.unwrap().path()).unwrap();
                assert!(!persisted.contains(content_marker));
                assert!(!persisted.contains("TOOL_RESULT_MARKER_DO_NOT_PERSIST"));
            }
        }
    }
}
