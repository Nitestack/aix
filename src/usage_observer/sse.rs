const MAX_SSE_LINE_BYTES: usize = 64 * 1024;
const MAX_SSE_DATA_BYTES: usize = 64 * 1024;
const MAX_SSE_EVENT_NAME_BYTES: usize = 256;

pub(crate) struct SseFrame {
    pub event: Option<String>,
    pub data: Vec<u8>,
}

/// A bounded SSE decoder. It retains only the current line and event, never the response stream.
#[derive(Default)]
pub(crate) struct SseDecoder {
    line: Vec<u8>,
    event: Option<String>,
    data: Vec<u8>,
    saw_data_line: bool,
    line_overflow: bool,
    discard_event: bool,
    skip_lf: bool,
}

impl SseDecoder {
    pub(crate) fn push(&mut self, bytes: &[u8], mut observe: impl FnMut(SseFrame)) {
        for byte in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if *byte == b'\n' {
                    continue;
                }
            }
            if *byte == b'\r' || *byte == b'\n' {
                self.process_line(&mut observe);
                self.skip_lf = *byte == b'\r';
            } else if self.line.len() < MAX_SSE_LINE_BYTES {
                self.line.push(*byte);
            } else {
                self.line_overflow = true;
            }
        }
    }

    pub(crate) fn finish(&mut self, mut observe: impl FnMut(SseFrame)) {
        if !self.line.is_empty() || self.line_overflow {
            self.process_line(&mut observe);
        }
        self.dispatch(&mut observe);
    }

    fn process_line(&mut self, observe: &mut impl FnMut(SseFrame)) {
        if self.line_overflow {
            self.line.clear();
            self.line_overflow = false;
            self.discard_event = true;
            return;
        }
        let line = std::mem::take(&mut self.line);
        if line.is_empty() {
            self.dispatch(observe);
            return;
        }
        if line.first() == Some(&b':') {
            return;
        }
        let (field, value) = match line.iter().position(|byte| *byte == b':') {
            Some(index) => {
                let mut value = &line[index + 1..];
                if value.first() == Some(&b' ') {
                    value = &value[1..];
                }
                (&line[..index], value)
            }
            None => (line.as_slice(), &[][..]),
        };
        match field {
            b"event" if value.len() <= MAX_SSE_EVENT_NAME_BYTES => {
                self.event = std::str::from_utf8(value).ok().map(str::to_owned);
            }
            b"event" => self.discard_event = true,
            b"data" => {
                let additional = value.len() + usize::from(self.saw_data_line);
                if self.data.len().saturating_add(additional) > MAX_SSE_DATA_BYTES {
                    self.data.clear();
                    self.discard_event = true;
                } else {
                    if self.saw_data_line {
                        self.data.push(b'\n');
                    }
                    self.data.extend_from_slice(value);
                    self.saw_data_line = true;
                }
            }
            _ => {}
        }
    }

    fn dispatch(&mut self, observe: &mut impl FnMut(SseFrame)) {
        if !self.discard_event && self.saw_data_line {
            observe(SseFrame {
                event: self.event.take(),
                data: std::mem::take(&mut self.data),
            });
        } else {
            self.event = None;
            self.data.clear();
        }
        self.event = None;
        self.data.clear();
        self.saw_data_line = false;
        self.discard_event = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_sse_across_arbitrary_chunk_boundaries() {
        let bytes = b"event: usage\r\ndata: {\"usage\":\r\ndata: 7}\r\n\r\n";
        let mut decoder = SseDecoder::default();
        let mut frames = Vec::new();
        for chunk in bytes.chunks(3) {
            decoder.push(chunk, |frame| frames.push(frame));
        }
        decoder.finish(|frame| frames.push(frame));

        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event.as_deref(), Some("usage"));
        assert_eq!(frames[0].data, b"{\"usage\":\n7}");
    }

    #[test]
    fn skips_oversized_or_unrelated_events_and_recovers_for_following_frames() {
        let mut decoder = SseDecoder::default();
        let oversized = vec![b'x'; MAX_SSE_DATA_BYTES + 10];
        let mut input = b"data: ".to_vec();
        input.extend_from_slice(&oversized);
        input.extend_from_slice(b"\n\ndata: [DONE]\n\n");

        let mut frames = Vec::new();
        decoder.push(&input, |frame| frames.push(frame));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, b"[DONE]");
    }
}
