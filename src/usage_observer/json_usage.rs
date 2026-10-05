const MAX_USAGE_FIELD_BYTES: usize = 4096;
const MAX_JSON_KEY_BYTES: usize = 64;
const MAX_JSON_NESTING: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ObjectRole {
    Root,
    Response,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureTarget {
    Root,
    Response,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ObjectState {
    KeyOrEnd,
    Key,
    Colon,
    Value,
    CommaOrEnd,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArrayState {
    ValueOrEnd,
    Value,
    CommaOrEnd,
}

enum Frame {
    Object {
        role: ObjectRole,
        state: ObjectState,
        key: Option<String>,
    },
    Array {
        state: ArrayState,
    },
}

enum RootState {
    ExpectValue,
    InProgress,
    Done,
}

enum StringPurpose {
    Key,
    Value,
}

struct StringState {
    purpose: StringPurpose,
    escaped: bool,
    unicode_remaining: u8,
    raw_key: Vec<u8>,
    key_overflowed: bool,
}

#[derive(Clone, Copy)]
enum NumberState {
    Minus,
    Zero,
    Integer,
    FractionStart,
    Fraction,
    ExponentStart,
    ExponentSign,
    Exponent,
}

impl NumberState {
    fn is_complete(self) -> bool {
        matches!(
            self,
            Self::Zero | Self::Integer | Self::Fraction | Self::Exponent
        )
    }

    fn consume(&mut self, byte: u8) -> bool {
        *self = match (*self, byte) {
            (Self::Minus, b'0') => Self::Zero,
            (Self::Minus, b'1'..=b'9') => Self::Integer,
            (Self::Zero, b'.') => Self::FractionStart,
            (Self::Zero, b'e' | b'E') => Self::ExponentStart,
            (Self::Integer, b'0'..=b'9') => Self::Integer,
            (Self::Integer, b'.') => Self::FractionStart,
            (Self::Integer, b'e' | b'E') => Self::ExponentStart,
            (Self::FractionStart, b'0'..=b'9') => Self::Fraction,
            (Self::Fraction, b'0'..=b'9') => Self::Fraction,
            (Self::Fraction, b'e' | b'E') => Self::ExponentStart,
            (Self::ExponentStart, b'+' | b'-') => Self::ExponentSign,
            (Self::ExponentStart, b'0'..=b'9') => Self::Exponent,
            (Self::ExponentSign, b'0'..=b'9') => Self::Exponent,
            (Self::Exponent, b'0'..=b'9') => Self::Exponent,
            _ => return false,
        };
        true
    }
}

enum LexicalState {
    Normal,
    String(StringState),
    Number(NumberState),
    Literal {
        expected: &'static [u8],
        next: usize,
    },
}

struct Capture {
    target: CaptureTarget,
    base_depth: usize,
    bytes: Vec<u8>,
}

/// Incrementally validates JSON structure while retaining only a bounded `usage` value.
/// It recognizes `$.usage` and, when enabled, `$.response.usage`.
pub(crate) struct JsonUsageFieldExtractor {
    nested_response_usage: bool,
    root_state: RootState,
    stack: Vec<Frame>,
    lexical: LexicalState,
    capture: Option<Capture>,
    root_usage: Option<Vec<u8>>,
    response_usage: Option<Vec<u8>>,
    retained_usage_bytes: usize,
    failed: bool,
    overflowed: bool,
    finished: bool,
}

impl JsonUsageFieldExtractor {
    pub(crate) fn new(nested_response_usage: bool) -> Self {
        Self {
            nested_response_usage,
            root_state: RootState::ExpectValue,
            stack: Vec::new(),
            lexical: LexicalState::Normal,
            capture: None,
            root_usage: None,
            response_usage: None,
            retained_usage_bytes: 0,
            failed: false,
            overflowed: false,
            finished: false,
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) {
        if self.finished || self.failed || self.overflowed {
            return;
        }
        for byte in bytes {
            self.process_byte(*byte);
            if self.failed || self.overflowed {
                break;
            }
        }
    }

    pub(crate) fn finish(&mut self) {
        if self.finished {
            return;
        }
        if !self.failed && !self.overflowed {
            match &self.lexical {
                LexicalState::Number(state) if state.is_complete() => {
                    self.lexical = LexicalState::Normal;
                    self.complete_value();
                }
                LexicalState::Normal => {}
                _ => self.failed = true,
            }
            if !self.stack.is_empty() || !matches!(self.root_state, RootState::Done) {
                self.failed = true;
            }
        }
        if self.failed || self.overflowed {
            self.root_usage = None;
            self.response_usage = None;
            self.capture = None;
            self.retained_usage_bytes = 0;
        }
        self.finished = true;
    }

    pub(crate) fn root_usage(&self) -> Option<&[u8]> {
        (self.finished && !self.failed && !self.overflowed)
            .then_some(self.root_usage.as_deref())
            .flatten()
    }

    pub(crate) fn response_usage(&self) -> Option<&[u8]> {
        (self.finished && !self.failed && !self.overflowed)
            .then_some(self.response_usage.as_deref())
            .flatten()
    }

    #[cfg(test)]
    fn retained_usage_bytes(&self) -> usize {
        self.retained_usage_bytes
    }

    fn process_byte(&mut self, byte: u8) {
        loop {
            if let LexicalState::Number(state) = &self.lexical {
                if is_value_delimiter(byte) {
                    if !state.is_complete() {
                        self.failed = true;
                        return;
                    }
                    self.lexical = LexicalState::Normal;
                    self.complete_value();
                    if self.failed || self.overflowed {
                        return;
                    }
                    continue;
                }
            }

            self.append_capture(byte);
            if self.failed || self.overflowed {
                return;
            }
            let lexical = std::mem::replace(&mut self.lexical, LexicalState::Normal);
            match lexical {
                LexicalState::Normal => self.process_normal_byte(byte),
                LexicalState::String(state) => self.process_string_byte(byte, state),
                LexicalState::Number(mut state) => {
                    if state.consume(byte) {
                        self.lexical = LexicalState::Number(state);
                    } else {
                        self.failed = true;
                    }
                }
                LexicalState::Literal { expected, next } => {
                    if expected.get(next) != Some(&byte) {
                        self.failed = true;
                    } else if next + 1 == expected.len() {
                        self.complete_value();
                    } else {
                        self.lexical = LexicalState::Literal {
                            expected,
                            next: next + 1,
                        };
                    }
                }
            }
            return;
        }
    }

    fn process_normal_byte(&mut self, byte: u8) {
        if is_json_whitespace(byte) {
            return;
        }

        let state = self.stack.last().map(|frame| match frame {
            Frame::Object { state, .. } => match state {
                ObjectState::KeyOrEnd => 0,
                ObjectState::Key => 1,
                ObjectState::Colon => 2,
                ObjectState::Value => 3,
                ObjectState::CommaOrEnd => 4,
            },
            Frame::Array { state } => match state {
                ArrayState::ValueOrEnd => 5,
                ArrayState::Value => 6,
                ArrayState::CommaOrEnd => 7,
            },
        });

        match state {
            None => match self.root_state {
                RootState::ExpectValue => self.start_value(byte),
                RootState::InProgress | RootState::Done => self.failed = true,
            },
            Some(0 | 1) if byte == b'"' => {
                self.lexical = LexicalState::String(StringState {
                    purpose: StringPurpose::Key,
                    escaped: false,
                    unicode_remaining: 0,
                    raw_key: Vec::new(),
                    key_overflowed: false,
                });
            }
            Some(0) if byte == b'}' => self.close_container(false),
            Some(2) if byte == b':' => {
                if let Some(Frame::Object { state, .. }) = self.stack.last_mut() {
                    *state = ObjectState::Value;
                }
            }
            Some(5) if byte == b']' => self.close_container(true),
            Some(3 | 5 | 6) => self.start_value(byte),
            Some(4) if byte == b',' => {
                if let Some(Frame::Object { state, key, .. }) = self.stack.last_mut() {
                    *state = ObjectState::Key;
                    *key = None;
                }
            }
            Some(4) if byte == b'}' => self.close_container(false),
            Some(7) if byte == b',' => {
                if let Some(Frame::Array { state }) = self.stack.last_mut() {
                    *state = ArrayState::Value;
                }
            }
            Some(7) if byte == b']' => self.close_container(true),
            _ => self.failed = true,
        }
    }

    fn process_string_byte(&mut self, byte: u8, mut state: StringState) {
        let closes_string = byte == b'"' && !state.escaped && state.unicode_remaining == 0;
        if matches!(state.purpose, StringPurpose::Key) && !closes_string {
            if state.raw_key.len() < MAX_JSON_KEY_BYTES {
                state.raw_key.push(byte);
            } else {
                state.key_overflowed = true;
                state.raw_key.clear();
            }
        }

        if state.unicode_remaining > 0 {
            if !byte.is_ascii_hexdigit() {
                self.failed = true;
                return;
            }
            state.unicode_remaining -= 1;
        } else if state.escaped {
            state.escaped = false;
            match byte {
                b'u' => state.unicode_remaining = 4,
                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                _ => self.failed = true,
            }
        } else {
            match byte {
                b'\\' => state.escaped = true,
                b'"' => match state.purpose {
                    StringPurpose::Key => self.finish_key(&state),
                    StringPurpose::Value => self.complete_value(),
                },
                0..=0x1f => self.failed = true,
                _ => {}
            }
        }
        if self.failed || closes_string {
            return;
        }
        self.lexical = LexicalState::String(state);
    }

    fn finish_key(&mut self, state: &StringState) {
        let key = if state.key_overflowed {
            None
        } else {
            let mut encoded = Vec::with_capacity(state.raw_key.len() + 2);
            encoded.push(b'"');
            encoded.extend_from_slice(&state.raw_key);
            encoded.push(b'"');
            match serde_json::from_slice::<String>(&encoded) {
                Ok(key) => Some(key),
                Err(_) => {
                    self.failed = true;
                    return;
                }
            }
        };
        match self.stack.last_mut() {
            Some(Frame::Object {
                state: object_state,
                key: current_key,
                ..
            }) if matches!(object_state, ObjectState::KeyOrEnd | ObjectState::Key) => {
                *current_key = key;
                *object_state = ObjectState::Colon;
            }
            _ => self.failed = true,
        }
    }

    fn start_value(&mut self, byte: u8) {
        if self.stack.is_empty() {
            if !matches!(self.root_state, RootState::ExpectValue) {
                self.failed = true;
                return;
            }
            self.root_state = RootState::InProgress;
        }

        let capture_target = self.capture_target_for_value();
        if let Some(target) = capture_target {
            self.start_capture(target, byte);
            if self.failed || self.overflowed {
                return;
            }
        }

        match byte {
            b'{' => {
                if self.stack.len() >= MAX_JSON_NESTING {
                    self.failed = true;
                    return;
                }
                let role = self.container_role_for_value();
                self.stack.push(Frame::Object {
                    role,
                    state: ObjectState::KeyOrEnd,
                    key: None,
                });
            }
            b'[' => {
                if self.stack.len() >= MAX_JSON_NESTING {
                    self.failed = true;
                    return;
                }
                self.stack.push(Frame::Array {
                    state: ArrayState::ValueOrEnd,
                });
            }
            b'"' => {
                self.lexical = LexicalState::String(StringState {
                    purpose: StringPurpose::Value,
                    escaped: false,
                    unicode_remaining: 0,
                    raw_key: Vec::new(),
                    key_overflowed: false,
                });
            }
            b'-' => self.lexical = LexicalState::Number(NumberState::Minus),
            b'0' => self.lexical = LexicalState::Number(NumberState::Zero),
            b'1'..=b'9' => self.lexical = LexicalState::Number(NumberState::Integer),
            b't' => {
                self.lexical = LexicalState::Literal {
                    expected: b"true",
                    next: 1,
                }
            }
            b'f' => {
                self.lexical = LexicalState::Literal {
                    expected: b"false",
                    next: 1,
                }
            }
            b'n' => {
                self.lexical = LexicalState::Literal {
                    expected: b"null",
                    next: 1,
                }
            }
            _ => self.failed = true,
        }
    }

    fn capture_target_for_value(&self) -> Option<CaptureTarget> {
        match self.stack.last() {
            Some(Frame::Object {
                role: ObjectRole::Root,
                key: Some(key),
                ..
            }) if key == "usage" => Some(CaptureTarget::Root),
            Some(Frame::Object {
                role: ObjectRole::Response,
                key: Some(key),
                ..
            }) if key == "usage" => Some(CaptureTarget::Response),
            _ => None,
        }
    }

    fn container_role_for_value(&self) -> ObjectRole {
        if self.stack.is_empty() {
            return ObjectRole::Root;
        }
        if self.nested_response_usage
            && matches!(
                self.stack.last(),
                Some(Frame::Object {
                    role: ObjectRole::Root,
                    key: Some(key),
                    ..
                }) if key == "response"
            )
        {
            ObjectRole::Response
        } else {
            ObjectRole::Other
        }
    }

    fn start_capture(&mut self, target: CaptureTarget, first_byte: u8) {
        if self.overflowed {
            return;
        }
        let already_captured = match target {
            CaptureTarget::Root => self.root_usage.is_some(),
            CaptureTarget::Response => self.response_usage.is_some(),
        };
        if already_captured || self.capture.is_some() {
            self.failed = true;
            return;
        }
        self.capture = Some(Capture {
            target,
            base_depth: self.stack.len(),
            bytes: Vec::new(),
        });
        self.append_capture(first_byte);
    }

    fn append_capture(&mut self, byte: u8) {
        let Some(capture) = &self.capture else {
            return;
        };
        if self
            .retained_usage_bytes
            .checked_add(capture.bytes.len())
            .and_then(|length| length.checked_add(1))
            .is_none_or(|length| length > MAX_USAGE_FIELD_BYTES)
        {
            self.overflowed = true;
            self.capture = None;
            self.root_usage = None;
            self.response_usage = None;
            self.retained_usage_bytes = 0;
            return;
        }
        self.capture
            .as_mut()
            .expect("capture was checked above")
            .bytes
            .push(byte);
    }

    fn close_container(&mut self, is_array: bool) {
        let can_close = match self.stack.last() {
            Some(Frame::Object { state, .. }) if !is_array => {
                matches!(state, ObjectState::KeyOrEnd | ObjectState::CommaOrEnd)
            }
            Some(Frame::Array { state }) if is_array => {
                matches!(state, ArrayState::ValueOrEnd | ArrayState::CommaOrEnd)
            }
            _ => false,
        };
        if !can_close {
            self.failed = true;
            return;
        }
        self.stack.pop();
        self.complete_value();
    }

    fn complete_value(&mut self) {
        if self
            .capture
            .as_ref()
            .is_some_and(|capture| self.stack.len() == capture.base_depth)
        {
            self.finish_capture();
        }
        match self.stack.last_mut() {
            Some(Frame::Object { state, key, .. }) if *state == ObjectState::Value => {
                *state = ObjectState::CommaOrEnd;
                *key = None;
            }
            Some(Frame::Array { state })
                if matches!(state, ArrayState::Value | ArrayState::ValueOrEnd) =>
            {
                *state = ArrayState::CommaOrEnd;
            }
            Some(_) => self.failed = true,
            None if matches!(self.root_state, RootState::InProgress) => {
                self.root_state = RootState::Done;
            }
            None => self.failed = true,
        }
    }

    fn finish_capture(&mut self) {
        let Some(capture) = self.capture.take() else {
            return;
        };
        self.retained_usage_bytes += capture.bytes.len();
        let slot = match capture.target {
            CaptureTarget::Root => &mut self.root_usage,
            CaptureTarget::Response => &mut self.response_usage,
        };
        if slot.replace(capture.bytes).is_some() {
            self.failed = true;
        }
    }
}

fn is_value_delimiter(byte: u8) -> bool {
    is_json_whitespace(byte) || matches!(byte, b',' | b']' | b'}')
}

fn is_json_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_root_and_response_usage_across_single_byte_chunks() {
        let body = br#"{"id":"private \"usage\": marker","output":[{"text":"response marker","tool":{"usage":{"input_tokens":900}}}],"response":{"output_text":"private response","usage":{"input_tokens":12,"input_tokens_details":{"cached_tokens":4},"output_tokens":5}},"usage":{"input_tokens":99,"output_tokens":99}}"#;
        let mut extractor = JsonUsageFieldExtractor::new(true);
        for byte in body.chunks(1) {
            extractor.push(byte);
        }
        extractor.finish();

        let response: serde_json::Value =
            serde_json::from_slice(extractor.response_usage().unwrap()).unwrap();
        let root: serde_json::Value =
            serde_json::from_slice(extractor.root_usage().unwrap()).unwrap();
        assert_eq!(response["input_tokens"], 12);
        assert_eq!(response["input_tokens_details"]["cached_tokens"], 4);
        assert_eq!(root["input_tokens"], 99);
        assert!(extractor.retained_usage_bytes() < body.len());
    }

    #[test]
    fn large_content_is_discarded_while_bounded_usage_is_retained() {
        let marker = "LARGE_RESPONSE_CONTENT_MARKER";
        let large_content = format!("{marker}{}", "x".repeat(256 * 1024));
        let body = serde_json::json!({
            "output": [{ "text": large_content }],
            "tool_result": "tool-result-marker",
            "usage": { "input_tokens": 8, "output_tokens": 3 }
        })
        .to_string();
        let mut extractor = JsonUsageFieldExtractor::new(false);
        for chunk in body.as_bytes().chunks(113) {
            extractor.push(chunk);
        }
        extractor.finish();

        let usage = extractor.root_usage().unwrap();
        assert!(!usage
            .windows(marker.len())
            .any(|window| window == marker.as_bytes()));
        assert!(extractor.retained_usage_bytes() <= MAX_USAGE_FIELD_BYTES);
        assert!(extractor.retained_usage_bytes() < body.len());
    }

    #[test]
    fn malformed_json_or_oversized_usage_fails_without_retaining_a_body() {
        let mut malformed = JsonUsageFieldExtractor::new(false);
        malformed.push(br#"{"usage":{"input_tokens":4},"bad"#);
        malformed.finish();
        assert!(malformed.root_usage().is_none());

        let oversized = format!(
            r#"{{"usage":{{"unknown":"{}","input_tokens":4}}}}"#,
            "x".repeat(MAX_USAGE_FIELD_BYTES)
        );
        let mut bounded = JsonUsageFieldExtractor::new(false);
        for chunk in oversized.as_bytes().chunks(257) {
            bounded.push(chunk);
        }
        bounded.finish();
        assert!(bounded.root_usage().is_none());
        assert_eq!(bounded.retained_usage_bytes(), 0);
    }
}
