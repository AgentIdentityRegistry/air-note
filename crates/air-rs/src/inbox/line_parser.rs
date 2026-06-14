//! Newline-delimited JSON framing (PROTOCOL §1; ports daemon-ipc.mjs `makeLineParser`).
use serde_json::Value;

/// 1 MiB line ceiling (PROTOCOL §1). A line exceeding this with no newline is a protocol error.
pub const MAX_FRAME: usize = 1 << 20;

#[derive(Debug)]
/// Events produced by [`LineParser::feed`].
pub enum FrameEvent {
    /// A parsed JSON object frame.
    Frame(Value),
    /// A line failed to parse (bad JSON) or the ceiling was exceeded — non-fatal to the parser;
    /// the CLIENT decides what to do (the daemon closes the socket on its side).
    ParseError(String),
}

/// Stateful accumulator: feed raw bytes, get back zero or more events. Mirrors the JS parser's
/// semantics exactly: skip blank lines, surface parse errors, and on an over-ceiling line with no
/// newline yet, emit one ParseError and RESET the buffer (drop the garbage).
#[derive(Default)]
pub struct LineParser {
    buf: String,
}

impl LineParser {
    /// Create a new, empty [`LineParser`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `chunk` to the internal buffer and return any complete frames (or errors) found.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<FrameEvent> {
        let mut out = Vec::new();
        self.buf.push_str(&String::from_utf8_lossy(chunk));
        if self.buf.len() > MAX_FRAME && !self.buf.contains('\n') {
            out.push(FrameEvent::ParseError(format!("line exceeds {MAX_FRAME} bytes")));
            self.buf.clear();
            return out;
        }
        while let Some(nl) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=nl).collect();
            let line = line.trim_end_matches('\n');
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(v) => out.push(FrameEvent::Frame(v)),
                Err(e) => out.push(FrameEvent::ParseError(e.to_string())),
            }
        }
        out
    }
}
