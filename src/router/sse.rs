use serde_json::Value;

use crate::stats::Usage;

/// 增量解析 SSE 的 usage：
/// - `message_start.message.usage` 给输入与缓存
/// - `message_delta.usage` 给累计输出
///
/// 同一个字段出现多次时取最大值（Anthropic 的 delta 是增量语义）。
#[derive(Default)]
pub struct Parser {
    buf: Vec<u8>,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_create: u64,
    seen: bool,
}

impl Parser {
    pub fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
        if self.buf.contains(&b'\r') {
            self.buf.retain(|byte| *byte != b'\r');
        }
        while let Some(pos) = find(&self.buf, b"\n\n") {
            let block: Vec<u8> = self.buf.drain(..pos + 2).collect();
            self.parse_block(&block[..pos]);
        }
        // 单块过大说明不是标准 SSE，丢弃避免内存无限增长。
        if self.buf.len() > 1 << 20 {
            self.buf.clear();
        }
    }

    fn parse_block(&mut self, block: &[u8]) {
        let Ok(text) = std::str::from_utf8(block) else {
            return;
        };
        for line in text.lines() {
            let Some(payload) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            let Ok(event) = serde_json::from_str::<Value>(payload) else {
                continue;
            };
            let usage = match event.get("type").and_then(|t| t.as_str()) {
                Some("message_start") => event.get("message").and_then(|m| m.get("usage")),
                Some("message_delta") => event.get("usage"),
                _ => None,
            };
            if let Some(usage) = usage {
                self.merge(usage);
            }
        }
    }

    fn merge(&mut self, usage: &Value) {
        let mut took = false;
        for (field, slot) in [
            ("input_tokens", &mut self.input),
            ("output_tokens", &mut self.output),
            ("cache_read_input_tokens", &mut self.cache_read),
            ("cache_creation_input_tokens", &mut self.cache_create),
        ] {
            if let Some(value) = usage.get(field).and_then(|v| v.as_u64()) {
                *slot = (*slot).max(value);
                took = true;
            }
        }
        self.seen |= took;
    }

    pub fn usage(&self) -> Option<Usage> {
        self.seen.then_some(Usage {
            input: self.input,
            output: self.output,
            cache_read: self.cache_read,
            cache_create: self.cache_create,
        })
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usage_across_arbitrary_chunks() {
        let stream = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"cache_read_input_tokens\":3}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"hi\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":9}}\n\n",
        );
        let mut parser = Parser::default();
        for byte in stream.as_bytes() {
            parser.feed(&[*byte]);
        }
        let usage = parser.usage().expect("应解析出 usage");
        assert_eq!(usage.input, 12);
        assert_eq!(usage.output, 9);
        assert_eq!(usage.cache_read, 3);
    }

    #[test]
    fn handles_crlf_and_ignores_garbage() {
        let mut parser = Parser::default();
        parser.feed(b"data: not-json\r\n\r\n");
        parser.feed(b"data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":5}}\r\n\r\n");
        assert_eq!(parser.usage().unwrap().output, 5);
    }

    #[test]
    fn keeps_max_value() {
        let mut parser = Parser::default();
        parser.feed(b"data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":5}}\n\n");
        parser.feed(b"data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":9}}\n\n");
        parser.feed(b"data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":7}}\n\n");
        assert_eq!(parser.usage().unwrap().output, 9);
    }
}
