use crate::providers::ProviderError;

const MAX_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Default)]
pub(super) struct SseDecoder {
    buffer: Vec<u8>,
}

impl SseDecoder {
    pub(super) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, ProviderError> {
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() > MAX_BUFFER_BYTES {
            return Err(ProviderError::InvalidResponse(
                "SSE event exceeded the 1 MiB limit".to_string(),
            ));
        }

        let mut events = Vec::new();
        while let Some((index, delimiter_len)) = find_delimiter(&self.buffer) {
            let frame = self.buffer[..index].to_vec();
            self.buffer.drain(..index + delimiter_len);
            if let Some(data) = decode_frame(&frame)? {
                events.push(data);
            }
        }
        Ok(events)
    }

    pub(super) fn finish(self) -> Result<(), ProviderError> {
        if self.buffer.iter().all(u8::is_ascii_whitespace) {
            Ok(())
        } else {
            Err(ProviderError::Interrupted)
        }
    }
}

fn find_delimiter(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, 2)),
        (Some(_), Some(right)) => Some((right, 4)),
        (Some(left), None) => Some((left, 2)),
        (None, Some(right)) => Some((right, 4)),
        (None, None) => None,
    }
}

fn decode_frame(frame: &[u8]) -> Result<Option<String>, ProviderError> {
    let frame = std::str::from_utf8(frame)
        .map_err(|_| ProviderError::InvalidResponse("SSE data was not valid UTF-8".to_string()))?;
    let data = frame
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            line.strip_prefix("data:")
                .map(|value| value.strip_prefix(' ').unwrap_or(value))
        })
        .collect::<Vec<_>>();

    if data.is_empty() {
        Ok(None)
    } else {
        Ok(Some(data.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_events_split_at_arbitrary_byte_boundaries() {
        let input =
            "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\r\n\r\ndata: [DONE]\n\n";
        let mut decoder = SseDecoder::default();
        let mut events = Vec::new();

        for byte in input.as_bytes().chunks(1) {
            events.extend(decoder.push(byte).expect("chunk should decode"));
        }

        decoder.finish().expect("stream should end cleanly");
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("你好"));
        assert_eq!(events[1], "[DONE]");
    }

    #[test]
    fn ignores_comments_and_joins_multiple_data_lines() {
        let mut decoder = SseDecoder::default();
        let events = decoder
            .push(b": keepalive\ndata: first\ndata: second\n\n")
            .expect("frame should decode");

        assert_eq!(events, vec!["first\nsecond"]);
    }

    #[test]
    fn reports_an_interrupted_trailing_frame() {
        let mut decoder = SseDecoder::default();
        decoder
            .push(b"data: {\"incomplete\":")
            .expect("chunk buffers");

        assert_eq!(decoder.finish(), Err(ProviderError::Interrupted));
    }
}
