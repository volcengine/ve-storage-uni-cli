//! Incremental SSE framing; every byte, line and frame shares strict resource limits.
use super::{http_error, protocol::ReplyState, Result, SessionError};

const MAX_STREAM_BYTES: usize = 2 * 1024 * 1024;
const MAX_FRAME_BYTES: usize = 64 * 1024;
const MAX_FRAMES: usize = 4096;

#[derive(Default)]
struct Decoder {
    line: Vec<u8>,
    event: String,
    id: Option<String>,
    payload: String,
    has_data: bool,
    is_after_cr: bool,
    is_first_line: bool,
    frame_bytes: usize,
    frames: usize,
    total_bytes: usize,
}

pub(super) async fn read(
    mut response: reqwest::Response,
    session: &str,
    user: &str,
) -> Result<String> {
    let mut decoder = Decoder {
        is_first_line: true,
        ..Decoder::default()
    };
    let mut reply = ReplyState::new(session, user);
    while let Some(chunk) = response.chunk().await.map_err(http_error)? {
        decoder.feed(&chunk, &mut reply)?;
    }
    decoder.finish(&mut reply)?;
    reply.finish()
}

impl Decoder {
    fn finish(&mut self, reply: &mut ReplyState<'_>) -> Result<()> {
        // [Review Fix #2] SSE discards tails without data; only pending data blocks success.
        if !self.line.is_empty() {
            self.end_line(reply)?;
        }
        // [Review Fix #1] An unterminated terminal frame is not proof of successful completion.
        if self.has_data {
            return Err(SessionError::Incomplete);
        }
        Ok(())
    }

    fn feed(&mut self, bytes: &[u8], reply: &mut ReplyState<'_>) -> Result<()> {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        if self.total_bytes > MAX_STREAM_BYTES {
            return Err(SessionError::Response);
        }
        for &byte in bytes {
            if self.is_after_cr && byte == b'\n' {
                self.is_after_cr = false;
                continue;
            }
            self.is_after_cr = byte == b'\r';
            self.frame_bytes += 1;
            if self.frame_bytes > MAX_FRAME_BYTES {
                return Err(SessionError::Response);
            }
            if matches!(byte, b'\n' | b'\r') {
                self.end_line(reply)?;
            } else {
                self.line.push(byte);
            }
        }
        Ok(())
    }

    fn end_line(&mut self, reply: &mut ReplyState<'_>) -> Result<()> {
        let bytes = std::mem::take(&mut self.line);
        let mut line = std::str::from_utf8(&bytes).map_err(|_| SessionError::Response)?;
        if self.is_first_line {
            line = line.strip_prefix('\u{feff}').unwrap_or(line);
            self.is_first_line = false;
        }
        if line.is_empty() {
            return self.dispatch(reply);
        }
        if line.starts_with(':') {
            return Ok(());
        }
        let (key, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match key {
            "event" => self.event = value.to_owned(),
            "id" if !value.contains('\0') => self.id = Some(value.to_owned()),
            "data" => {
                self.payload.push_str(value);
                self.payload.push('\n');
                self.has_data = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self, reply: &mut ReplyState<'_>) -> Result<()> {
        self.frames += 1;
        if self.frames > MAX_FRAMES {
            return Err(SessionError::Response);
        }
        if self.has_data {
            self.payload.pop();
            reply.accept(&self.event, self.id.as_deref(), &self.payload)?;
        }
        self.event.clear();
        self.id = None;
        self.payload.clear();
        self.has_data = false;
        self.frame_bytes = 0;
        Ok(())
    }
}
