use serde_json::{Value, json};

#[derive(Debug, Clone, Copy)]
pub enum SftpResult {
    Ok,
    Error,
}

impl SftpResult {
    fn as_str(self) -> &'static str {
        match self {
            SftpResult::Ok => "ok",
            SftpResult::Error => "error",
        }
    }
}

pub fn encode_event(t_secs: f64, op: &str, result: SftpResult, extra: Value) -> Vec<u8> {
    let t_rounded = (t_secs * 1_000_000.0).round() / 1_000_000.0;
    let mut obj = serde_json::Map::new();
    obj.insert("t".to_string(), json!(t_rounded));
    obj.insert("op".to_string(), json!(op));
    if let Value::Object(extras) = extra {
        for (k, v) in extras {
            obj.insert(k, v);
        }
    }
    obj.insert("result".to_string(), json!(result.as_str()));
    let mut out = serde_json::to_vec(&Value::Object(obj)).expect("sftp event serializes");
    out.push(b'\n');
    out
}

pub fn upload_ok(t: f64, src: &str, dst: &str, bytes: u64) -> Vec<u8> {
    encode_event(
        t,
        "upload",
        SftpResult::Ok,
        json!({ "src": src, "dst": dst, "bytes": bytes }),
    )
}

pub fn op_error(t: f64, op: &str, path: &str, error: &str) -> Vec<u8> {
    encode_event(
        t,
        op,
        SftpResult::Error,
        json!({ "path": path, "error": error }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn encode_upload_ok_shape() {
        let bytes = upload_ok(0.0, "/a", "/b", 42);
        assert!(bytes.ends_with(b"\n"));
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["t"], json!(0.0));
        assert_eq!(v["op"], "upload");
        assert_eq!(v["src"], "/a");
        assert_eq!(v["dst"], "/b");
        assert_eq!(v["bytes"], 42);
        assert_eq!(v["result"], "ok");
    }

    #[test]
    fn encode_op_error_shape() {
        let bytes = op_error(0.0, "delete", "/x", "denied");
        assert!(bytes.ends_with(b"\n"));
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["t"], json!(0.0));
        assert_eq!(v["op"], "delete");
        assert_eq!(v["path"], "/x");
        assert_eq!(v["error"], "denied");
        assert_eq!(v["result"], "error");
    }

    #[test]
    fn encode_event_monotonic_t() {
        let start = Instant::now();
        let mut tstamps = Vec::new();
        for _ in 0..5 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let t = start.elapsed().as_secs_f64();
            let bytes = encode_event(t, "list", SftpResult::Ok, json!({"path": "/"}));
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            tstamps.push(v["t"].as_f64().unwrap());
        }
        for w in tstamps.windows(2) {
            assert!(w[1] > w[0], "timestamps must strictly increase: {:?}", tstamps);
        }
    }
}
