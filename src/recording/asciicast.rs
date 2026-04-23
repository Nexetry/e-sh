use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AsciicastHeader {
    pub version: u8,
    pub width: u16,
    pub height: u16,
    pub timestamp: i64,
    pub env: AsciicastEnv,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AsciicastEnv {
    #[serde(rename = "TERM")]
    pub term: String,
}

pub fn encode_header(width: u16, height: u16, unix_ts: i64, term: &str, title: &str) -> Vec<u8> {
    let header = AsciicastHeader {
        version: 2,
        width,
        height,
        timestamp: unix_ts,
        env: AsciicastEnv { term: term.to_string() },
        title: title.to_string(),
    };
    let mut out = serde_json::to_vec(&header).expect("asciicast header serializes");
    out.push(b'\n');
    out
}

pub fn encode_event(t_secs: f64, event_type: &str, data: &[u8]) -> Vec<u8> {
    let t_rounded = (t_secs * 1_000_000.0).round() / 1_000_000.0;
    let data_str = String::from_utf8_lossy(data).into_owned();
    let value = serde_json::json!([t_rounded, event_type, data_str]);
    let mut out = serde_json::to_vec(&value).expect("asciicast event serializes");
    out.push(b'\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_header_matches_spec() {
        let bytes = encode_header(120, 40, 1745448000, "xterm-256color", "prod");
        let s = std::str::from_utf8(&bytes).unwrap();
        assert_eq!(
            s,
            "{\"version\":2,\"width\":120,\"height\":40,\"timestamp\":1745448000,\"env\":{\"TERM\":\"xterm-256color\"},\"title\":\"prod\"}\n"
        );
    }

    #[test]
    fn encode_event_six_decimals() {
        let bytes = encode_event(0.123456789, "o", b"hi");
        let s = std::str::from_utf8(&bytes).unwrap();
        assert_eq!(s, "[0.123457,\"o\",\"hi\"]\n");
    }

    #[test]
    fn encode_event_handles_invalid_utf8() {
        let bytes = encode_event(0.0, "o", &[0xff, 0xfe, b'a']);
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains('\u{fffd}'), "invalid utf8 becomes replacement char, got: {s}");
        assert!(s.contains('a'));
    }

    #[test]
    fn encode_event_zero_time() {
        let bytes = encode_event(0.0, "o", b"x");
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), "[0.0,\"o\",\"x\"]\n");
    }
}
