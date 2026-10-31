//! A2A v1.0 wire shapes (JSON-RPC binding), built as plain JSON so the format is visible in one place.
//! Field names and enum strings follow https://a2a-protocol.org/latest/specification/ (camelCase, `TASK_STATE_*`).

use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub const VERSION: &str = "1.0";
pub const DOMAIN: &str = "a2a-protocol.org";

pub const SUBMITTED: &str = "TASK_STATE_SUBMITTED";
pub const WORKING: &str = "TASK_STATE_WORKING";
pub const COMPLETED: &str = "TASK_STATE_COMPLETED";
pub const FAILED: &str = "TASK_STATE_FAILED";
pub const CANCELED: &str = "TASK_STATE_CANCELED";
pub const REJECTED: &str = "TASK_STATE_REJECTED";
pub const INPUT_REQUIRED: &str = "TASK_STATE_INPUT_REQUIRED";

pub fn is_terminal(state: &str) -> bool {
    matches!(state, COMPLETED | FAILED | CANCELED | REJECTED)
}

/// Terminal, or paused waiting for the caller: a blocking SendMessage returns in either case.
pub fn is_final_for_blocking(state: &str) -> bool {
    is_terminal(state) || state == INPUT_REQUIRED
}

/// RFC 3339 UTC timestamp without pulling in a date crate.
pub fn now_rfc3339() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let (secs, ms) = (d.as_secs() as i64, d.subsec_millis());
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

pub fn text_part(t: &str) -> Value {
    json!({ "text": t })
}

pub fn data_part(v: Value) -> Value {
    json!({ "data": v, "mediaType": "application/json" })
}

pub fn agent_message(text: &str, context_id: &str, task_id: Option<&str>) -> Value {
    let mut m = json!({
        "messageId": uuid::Uuid::new_v4().to_string(),
        "role": "ROLE_AGENT",
        "parts": [text_part(text)],
        "contextId": context_id,
    });
    if let Some(t) = task_id {
        m["taskId"] = json!(t);
    }
    m
}

/// JSON-RPC error with the A2A `google.rpc.ErrorInfo` detail the spec asks for.
pub fn rpc_error(id: &Value, code: i64, message: &str, reason: &str, meta: Value) -> Value {
    json!({
        "jsonrpc": "2.0", "id": id,
        "error": {
            "code": code, "message": message,
            "data": [{
                "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                "reason": reason, "domain": DOMAIN, "metadata": meta,
            }]
        }
    })
}

pub fn rpc_ok(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

// Standard JSON-RPC and A2A error codes (spec section 5.4 / 9.5).
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL: i64 = -32603;
pub const TASK_NOT_FOUND: i64 = -32001;
pub const TASK_NOT_CANCELABLE: i64 = -32002;
pub const PUSH_NOT_SUPPORTED: i64 = -32003;
pub const UNSUPPORTED_OPERATION: i64 = -32004;
pub const EXTENDED_CARD_NOT_CONFIGURED: i64 = -32007;
pub const VERSION_NOT_SUPPORTED: i64 = -32009;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_rfc3339() {
        let t = now_rfc3339();
        assert_eq!(t.len(), 24, "{t}");
        assert!(t.ends_with('Z') && &t[10..11] == "T", "{t}");
        assert!(t.starts_with("20"), "{t}");
    }

    #[test]
    fn terminal_states() {
        assert!(is_terminal(COMPLETED) && is_terminal(CANCELED));
        assert!(!is_terminal(INPUT_REQUIRED) && !is_terminal(WORKING));
        assert!(is_final_for_blocking(INPUT_REQUIRED));
    }
}
