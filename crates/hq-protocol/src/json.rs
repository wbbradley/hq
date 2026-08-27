//! Small canonical JSON cursor shared by signed-event framing and prefix dispatch.

use crate::{FailureClass, ProtocolError};

pub(crate) const MAX_JSON_DEPTH: usize = 16;
pub(crate) const MAX_OBJECT_MEMBERS: usize = 16;
pub(crate) const MAX_COLLECTION_ITEMS: usize = 64;

#[derive(Clone, Copy)]
enum Context {
    Outer,
    Content,
}

pub(crate) struct JsonCursor<'a> {
    input: &'a [u8],
    position: usize,
    context: Context,
}

impl<'a> JsonCursor<'a> {
    pub(crate) const fn outer(input: &'a [u8]) -> Self {
        Self {
            input,
            position: 0,
            context: Context::Outer,
        }
    }

    pub(crate) const fn content(input: &'a [u8]) -> Self {
        Self {
            input,
            position: 0,
            context: Context::Content,
        }
    }

    pub(crate) fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    pub(crate) fn remaining(&self) -> &'a [u8] {
        self.input.get(self.position..).unwrap_or_default()
    }

    pub(crate) fn expect(
        &mut self,
        literal: &[u8],
        failure: FailureClass,
    ) -> Result<(), ProtocolError> {
        if self.remaining().starts_with(literal) {
            self.position += literal.len();
            Ok(())
        } else if first_mismatch_is_whitespace(self.remaining(), literal) {
            Err(self.noncanonical())
        } else {
            Err(ProtocolError::new(failure))
        }
    }

    pub(crate) fn finish_outer(&self) -> Result<(), ProtocolError> {
        if self.position == self.input.len() {
            Ok(())
        } else {
            Err(ProtocolError::new(FailureClass::OuterTrailingData))
        }
    }

    pub(crate) fn finish_content(&self) -> Result<(), ProtocolError> {
        if self.position == self.input.len() {
            Ok(())
        } else if self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            Err(self.noncanonical())
        } else {
            Err(ProtocolError::new(FailureClass::ContentMalformed))
        }
    }

    pub(crate) fn parse_u64(&mut self) -> Result<u64, ProtocolError> {
        let start = self.position;
        let Some(first) = self.peek() else {
            return Err(self.malformed());
        };
        if !first.is_ascii_digit() {
            return Err(self.malformed());
        }
        self.position += 1;
        if first == b'0' && self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            return Err(self.noncanonical());
        }
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.position += 1;
        }
        let digits = self
            .input
            .get(start..self.position)
            .ok_or_else(|| self.malformed())?;
        let mut value = 0_u64;
        for digit in digits {
            value = value
                .checked_mul(10)
                .and_then(|value| value.checked_add(u64::from(*digit - b'0')))
                .ok_or_else(|| self.malformed())?;
        }
        Ok(value)
    }

    pub(crate) fn parse_hex<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        self.expect(b"\"", FailureClass::OuterFieldShape)?;
        let encoded_length = N
            .checked_mul(2)
            .ok_or_else(|| ProtocolError::new(FailureClass::OuterFieldShape))?;
        let encoded = self
            .input
            .get(self.position..self.position.saturating_add(encoded_length))
            .ok_or_else(|| ProtocolError::new(FailureClass::OuterFieldShape))?;
        let mut decoded = [0_u8; N];
        let (pairs, _) = encoded.as_chunks::<2>();
        for (index, pair) in pairs.iter().enumerate() {
            let high = decode_lower_hex(pair[0])?;
            let low = decode_lower_hex(pair[1])?;
            decoded[index] = (high << 4) | low;
        }
        self.position += encoded_length;
        self.expect(b"\"", FailureClass::OuterFieldShape)?;
        Ok(decoded)
    }

    pub(crate) fn parse_string(&mut self, maximum: usize) -> Result<Vec<u8>, ProtocolError> {
        self.expect(b"\"", self.malformed().class())?;
        let mut decoded = Vec::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.malformed());
            };
            match byte {
                b'\"' => {
                    self.position += 1;
                    return Ok(decoded);
                }
                b'\\' => {
                    self.position += 1;
                    self.decode_escape(&mut decoded)?;
                }
                0x00..=0x1f => return Err(self.noncanonical()),
                _ => {
                    self.position += 1;
                    decoded.push(byte);
                }
            }
            if decoded.len() > maximum {
                return Err(ProtocolError::new(FailureClass::ContentTooLarge));
            }
        }
    }

    fn decode_escape(&mut self, decoded: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let Some(escape) = self.peek() else {
            return Err(self.malformed());
        };
        self.position += 1;
        match escape {
            b'\"' => decoded.push(b'\"'),
            b'\\' => decoded.push(b'\\'),
            b'b' => decoded.push(0x08),
            b'f' => decoded.push(0x0c),
            b'n' => decoded.push(b'\n'),
            b'r' => decoded.push(b'\r'),
            b't' => decoded.push(b'\t'),
            b'u' => self.decode_control_escape(decoded)?,
            _ => return Err(self.noncanonical()),
        }
        Ok(())
    }

    fn decode_control_escape(&mut self, decoded: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let digits = self
            .input
            .get(self.position..self.position.saturating_add(4))
            .ok_or_else(|| self.malformed())?;
        if digits.len() != 4 || digits[0] != b'0' || digits[1] != b'0' {
            return Err(self.noncanonical());
        }
        let value = (decode_lower_hex(digits[2])? << 4) | decode_lower_hex(digits[3])?;
        self.position += 4;
        if value == 0 || value > 0x1f || matches!(value, 0x08 | 0x09 | 0x0a | 0x0c | 0x0d) {
            return Err(self.noncanonical());
        }
        decoded.push(value);
        Ok(())
    }

    pub(crate) fn validate_value(&mut self, depth: usize) -> Result<(), ProtocolError> {
        if depth > MAX_JSON_DEPTH {
            return Err(ProtocolError::new(FailureClass::ContentTooDeep));
        }
        match self.peek() {
            Some(b'{') => self.validate_object(depth),
            Some(b'[') => self.validate_array(depth),
            Some(b'\"') => self.skip_string(),
            Some(b't') => self.expect(b"true", FailureClass::ContentMalformed),
            Some(b'f') => self.expect(b"false", FailureClass::ContentMalformed),
            Some(b'n') => self.expect(b"null", FailureClass::ContentMalformed),
            Some(byte) if byte.is_ascii_digit() => self.parse_u64().map(|_| ()),
            Some(byte) if byte.is_ascii_whitespace() => Err(self.noncanonical()),
            _ => Err(self.malformed()),
        }
    }

    fn validate_object(&mut self, depth: usize) -> Result<(), ProtocolError> {
        self.position += 1;
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(());
        }
        let mut members = 0_usize;
        loop {
            members += 1;
            if members > MAX_OBJECT_MEMBERS {
                return Err(ProtocolError::new(FailureClass::ContentTooManyItems));
            }
            self.skip_string()?;
            self.expect(b":", FailureClass::ContentMalformed)?;
            self.validate_value(depth + 1)?;
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b'}') => {
                    self.position += 1;
                    return Ok(());
                }
                Some(byte) if byte.is_ascii_whitespace() => return Err(self.noncanonical()),
                _ => return Err(self.malformed()),
            }
        }
    }

    fn validate_array(&mut self, depth: usize) -> Result<(), ProtocolError> {
        self.position += 1;
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(());
        }
        let mut items = 0_usize;
        loop {
            items += 1;
            if items > MAX_COLLECTION_ITEMS {
                return Err(ProtocolError::new(FailureClass::ContentTooManyItems));
            }
            self.validate_value(depth + 1)?;
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b']') => {
                    self.position += 1;
                    return Ok(());
                }
                Some(byte) if byte.is_ascii_whitespace() => return Err(self.noncanonical()),
                _ => return Err(self.malformed()),
            }
        }
    }

    fn skip_string(&mut self) -> Result<(), ProtocolError> {
        self.expect(b"\"", FailureClass::ContentMalformed)?;
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.malformed());
            };
            match byte {
                b'\"' => {
                    self.position += 1;
                    return Ok(());
                }
                b'\\' => {
                    self.position += 1;
                    let mut ignored = Vec::with_capacity(1);
                    self.decode_escape(&mut ignored)?;
                }
                0x00..=0x1f => return Err(self.noncanonical()),
                _ => self.position += 1,
            }
        }
    }

    const fn malformed(&self) -> ProtocolError {
        ProtocolError::new(match self.context {
            Context::Outer => FailureClass::OuterFieldShape,
            Context::Content => FailureClass::ContentMalformed,
        })
    }

    const fn noncanonical(&self) -> ProtocolError {
        ProtocolError::new(match self.context {
            Context::Outer => FailureClass::OuterNonCanonical,
            Context::Content => FailureClass::ContentNonCanonical,
        })
    }
}

fn first_mismatch_is_whitespace(actual: &[u8], expected: &[u8]) -> bool {
    let mismatch = actual
        .iter()
        .zip(expected)
        .position(|(actual, expected)| actual != expected)
        .unwrap_or_else(|| actual.len().min(expected.len()));
    actual.get(mismatch).is_some_and(u8::is_ascii_whitespace)
}

pub(crate) fn validate_content_json(input: &[u8]) -> Result<(), ProtocolError> {
    if std::str::from_utf8(input).is_err() {
        return Err(ProtocolError::new(FailureClass::ContentMalformed));
    }
    let mut cursor = JsonCursor::content(input);
    cursor.validate_value(1)?;
    cursor.finish_content()
}

fn decode_lower_hex(byte: u8) -> Result<u8, ProtocolError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(ProtocolError::new(FailureClass::OuterFieldShape)),
    }
}
