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
    finished: bool,
}

impl AnthropicMessagesObserver {
    pub(crate) fn new(recorder: UsageEventRecorder, status: u16) -> Self {
        Self {
            stream: AnthropicSseUsage::default(),
            recorder: Some(recorder),
            status,
            finished: false,
        }
    }
}

impl ResponseBodyObserver for AnthropicMessagesObserver {
    fn observe(&mut self, bytes: &[u8]) {
        self.stream.push(bytes);
        if let Some(recorder) = &mut self.recorder {
            recorder.set_usage(&self.stream.usage);
        }
    }

    fn finish(&mut self, stream_end: ResponseStreamEnd) {
        self.stream.finish();
        let (outcome, category) = if !(200..300).contains(&self.status) {
            (UsageOutcome::Failed, Some("upstream_http"))
        } else {
            match (self.stream.terminal, stream_end) {
                (Some(TerminalEvent::Failed), _) => {
                    (UsageOutcome::Failed, Some("upstream_response_error"))
                }
                (Some(TerminalEvent::Stopped), ResponseStreamEnd::Complete) => {
                    (UsageOutcome::Succeeded, None)
                }
                (_, ResponseStreamEnd::Error) => (UsageOutcome::Failed, Some("upstream_stream")),
                (_, ResponseStreamEnd::Complete) => (
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
    use crate::usage_event::UsageCompleteness;

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
}
