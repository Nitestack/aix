use super::json_usage::JsonUsageFieldExtractor;
use super::sse::{SseDecoder, SseFrame};
use crate::local_gateway::{ResponseBodyObserver, ResponseStreamEnd};
use crate::usage_event::{TokenUsage, UsageEventRecorder, UsageOutcome};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct AnthropicUsageRaw {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct AnthropicEnvelope {
    usage: Option<AnthropicUsageRaw>,
    message: Option<AnthropicMessage>,
    #[serde(rename = "type")]
    event_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessage {
    usage: Option<AnthropicUsageRaw>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalEvent {
    Stopped,
    Failed,
}

#[derive(Default)]
struct AnthropicSseUsage {
    decoder: SseDecoder,
    usage: TokenUsage,
    terminal: Option<TerminalEvent>,
}

impl AnthropicSseUsage {
    fn push(&mut self, bytes: &[u8]) {
        let (decoder, usage, terminal) = (&mut self.decoder, &mut self.usage, &mut self.terminal);
        decoder.push(bytes, |frame| {
            observe_anthropic_frame(frame, usage, terminal)
        });
    }

    fn finish(&mut self) {
        let (decoder, usage, terminal) = (&mut self.decoder, &mut self.usage, &mut self.terminal);
        decoder.finish(|frame| observe_anthropic_frame(frame, usage, terminal));
    }
}

fn observe_anthropic_frame(
    frame: SseFrame,
    usage: &mut TokenUsage,
    terminal: &mut Option<TerminalEvent>,
) {
    let frame_type = frame.event.as_deref();
    match frame_type {
        Some("message_stop") => *terminal = Some(TerminalEvent::Stopped),
        Some("error") => *terminal = Some(TerminalEvent::Failed),
        _ => {}
    }
    let Ok(envelope) = serde_json::from_slice::<AnthropicEnvelope>(&frame.data) else {
        return;
    };
    if let Some(event_type) = envelope.event_type.as_deref() {
        match event_type {
            "message_stop" => *terminal = Some(TerminalEvent::Stopped),
            "error" => *terminal = Some(TerminalEvent::Failed),
            _ => {}
        }
    }
    let event_type = frame_type.or(envelope.event_type.as_deref());
    let update = match event_type {
        Some("message_start") => envelope.message.and_then(|message| message.usage),
        Some("message_delta") => envelope.usage,
        _ => envelope.usage,
    };
    if let Some(mut update) = update.map(normalize_anthropic_usage) {
        if event_type == Some("message_start") {
            update.output_tokens = None;
            update.total_tokens = None;
        }
        usage.merge(&update);
    }
}

pub(crate) fn messages_json_usage(body: &[u8]) -> Option<TokenUsage> {
    serde_json::from_slice::<AnthropicEnvelope>(body)
        .ok()?
        .usage
        .map(normalize_anthropic_usage)
}

pub(crate) fn messages_sse_usage<'a>(
    chunks: impl IntoIterator<Item = &'a [u8]>,
) -> Option<TokenUsage> {
    let mut stream = AnthropicSseUsage::default();
    for chunk in chunks {
        stream.push(chunk);
    }
    stream.finish();
    (stream.usage.completeness() != crate::usage_event::UsageCompleteness::Unavailable)
        .then_some(stream.usage)
}

pub(crate) struct AnthropicMessagesObserver {
    stream: AnthropicSseUsage,
    recorder: Option<UsageEventRecorder>,
    status: u16,
    is_sse: bool,
    json_usage: JsonUsageFieldExtractor,
    finished: bool,
}

impl AnthropicMessagesObserver {
    pub(crate) fn new(recorder: UsageEventRecorder, status: u16, is_sse: bool) -> Self {
        Self {
            stream: AnthropicSseUsage::default(),
            recorder: Some(recorder),
            status,
            is_sse,
            json_usage: JsonUsageFieldExtractor::new(false),
            finished: false,
        }
    }
}

impl ResponseBodyObserver for AnthropicMessagesObserver {
    fn observe(&mut self, bytes: &[u8]) {
        if self.is_sse {
            self.stream.push(bytes);
        } else {
            self.json_usage.push(bytes);
        }
        if let Some(recorder) = &mut self.recorder {
            recorder.set_usage(&self.stream.usage);
        }
    }

    fn finish(&mut self, stream_end: ResponseStreamEnd) {
        if self.is_sse {
            self.stream.finish();
        } else {
            self.json_usage.finish();
            if let Some(usage) = self
                .json_usage
                .root_usage()
                .and_then(|bytes| serde_json::from_slice::<AnthropicUsageRaw>(bytes).ok())
                .map(normalize_anthropic_usage)
            {
                self.stream.usage.merge(&usage);
            }
        }
        let (outcome, category) = if !(200..300).contains(&self.status) {
            (UsageOutcome::Failed, Some("upstream_http"))
        } else if matches!(self.stream.terminal, Some(TerminalEvent::Failed)) {
            (UsageOutcome::Failed, Some("upstream_response_error"))
        } else if matches!(stream_end, ResponseStreamEnd::Error) {
            (UsageOutcome::Failed, Some("upstream_stream"))
        } else if !self.is_sse {
            (UsageOutcome::Succeeded, None)
        } else {
            match self.stream.terminal {
                Some(TerminalEvent::Stopped) => (UsageOutcome::Succeeded, None),
                _ => (
                    UsageOutcome::Incomplete,
                    Some("stream_ended_without_message_stop"),
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

impl Drop for AnthropicMessagesObserver {
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

fn normalize_anthropic_usage(raw: AnthropicUsageRaw) -> TokenUsage {
    let input_tokens_total = raw.input_tokens.and_then(|input| {
        input
            .checked_add(raw.cache_creation_input_tokens.unwrap_or_default())?
            .checked_add(raw.cache_read_input_tokens.unwrap_or_default())
    });
    TokenUsage::canonical(
        input_tokens_total,
        raw.input_tokens,
        raw.cache_read_input_tokens,
        raw.cache_creation_input_tokens,
        raw.output_tokens,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_gateway::LaunchContext;
    use crate::usage_event::UsageCompleteness;
    use crate::usage_store::{UsageEventFilter, UsageStore};
    use assert_fs::TempDir;

    #[test]
    fn messages_usage_includes_cache_counters_in_total_input_exactly_once() {
        let usage = messages_json_usage(
            br#"{"id":"msg-secret","content":[{"text":"private response"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":5,"output_tokens":7}}"#,
        )
        .unwrap();

        assert_eq!(usage.input_tokens_total, Some(18));
        assert_eq!(usage.input_tokens_uncached, Some(10));
        assert_eq!(usage.cache_read_input_tokens, Some(5));
        assert_eq!(usage.cache_write_input_tokens, Some(3));
        assert_eq!(usage.total_tokens, Some(25));
        assert_eq!(usage.completeness(), UsageCompleteness::Complete);
    }

    #[test]
    fn messages_sse_merges_start_cache_usage_and_cumulative_output_usage() {
        let mut stream = AnthropicSseUsage::default();
        let bytes = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":8,\"cache_creation_input_tokens\":2,\"cache_read_input_tokens\":4,\"output_tokens\":0}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"never persist this\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":9}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        for chunk in bytes.chunks(7) {
            stream.push(chunk);
        }
        stream.finish();

        assert_eq!(stream.usage.input_tokens_total, Some(14));
        assert_eq!(stream.usage.input_tokens_uncached, Some(8));
        assert_eq!(stream.usage.cache_read_input_tokens, Some(4));
        assert_eq!(stream.usage.cache_write_input_tokens, Some(2));
        assert_eq!(stream.usage.output_tokens, Some(9));
        assert_eq!(stream.usage.total_tokens, Some(23));
        assert_eq!(stream.terminal, Some(TerminalEvent::Stopped));
    }

    #[test]
    fn messages_sse_helper_preserves_cumulative_usage_across_arbitrary_chunks() {
        let bytes = b"event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":4,\"cache_read_input_tokens\":2}}}\n\nevent: message_delta\ndata: {\"usage\":{\"output_tokens\":5}}\n\nevent: message_stop\ndata: {}\n\n";
        let usage = messages_sse_usage(bytes.chunks(4)).unwrap();

        assert_eq!(usage.input_tokens_total, Some(6));
        assert_eq!(usage.cache_read_input_tokens, Some(2));
        assert_eq!(usage.output_tokens, Some(5));
        assert_eq!(usage.total_tokens, Some(11));
    }

    #[test]
    fn non_sse_usage_extracts_cache_counters_across_chunk_splits_without_content() {
        let state = TempDir::new().unwrap();
        let store = UsageStore::new(state.path());
        let body = format!(
            r#"{{"content":[{{"type":"text","text":"ANTHROPIC_RESPONSE_MARKER_DO_NOT_PERSIST{}"}},{{"type":"tool_use","input":{{"private":"TOOL_INPUT_MARKER_DO_NOT_PERSIST"}}}}],"usage":{{"input_tokens":11,"cache_creation_input_tokens":3,"cache_read_input_tokens":5,"output_tokens":7}}}}"#,
            "z".repeat(64 * 1024)
        );
        let context = LaunchContext::new("work".to_string(), "claude".to_string(), None, None);
        let recorder = UsageEventRecorder::new(Some(store.clone()), &context, "anthropic_messages");
        let mut observer = AnthropicMessagesObserver::new(recorder, 200, false);
        let mut offset = 0;
        let mut chunk_size = 1;
        while offset < body.len() {
            let end = (offset + chunk_size).min(body.len());
            observer.observe(&body.as_bytes()[offset..end]);
            offset = end;
            chunk_size = (chunk_size * 11 % 37).max(1);
        }
        observer.finish(ResponseStreamEnd::Complete);

        let events = store
            .events(&UsageEventFilter {
                start_unix_ms: 0,
                end_unix_ms: u64::MAX,
                ..UsageEventFilter::default()
            })
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens_total, Some(19));
        assert_eq!(events[0].input_tokens_uncached, Some(11));
        assert_eq!(events[0].cache_read_input_tokens, Some(5));
        assert_eq!(events[0].cache_write_input_tokens, Some(3));
        assert_eq!(events[0].output_tokens, Some(7));
        assert_eq!(events[0].total_tokens, Some(26));
        assert_eq!(events[0].usage_completeness, UsageCompleteness::Complete);
        assert_eq!(events[0].outcome, UsageOutcome::Succeeded);
        let event_dir = std::fs::read_dir(state.path().join("usage/events"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let persisted = std::fs::read_to_string(
            std::fs::read_dir(event_dir)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        assert!(!persisted.contains("ANTHROPIC_RESPONSE_MARKER_DO_NOT_PERSIST"));
        assert!(!persisted.contains("TOOL_INPUT_MARKER_DO_NOT_PERSIST"));
    }
}
