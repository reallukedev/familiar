//! JSON-RPC 2.0 messages shared by the stdio and HTTP clients.
use dot_wire::GuestError;
use serde_json::{Value, json};

/// The MCP revision the guest speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Largest message or HTTP body accepted from a server.
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// Why a request failed.
#[derive(Debug)]
pub enum Fail {
    /// The server answered with a JSON-RPC error. The session is still good.
    Rpc(String),
    /// The request took too long and was given up on, but the session is still good.
    Timeout(GuestError),
    /// The session can't be trusted any more (it stopped, timed out or sent garbage).
    Broken(GuestError),
}

/// A request with a numeric id.
pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// A notification (no id, no answer).
pub fn notification(method: &str) -> Value {
    json!({"jsonrpc": "2.0", "method": method})
}

/// Tell the server we no longer want the answer to request `id`.
pub fn cancelled(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": {"requestId": id, "reason": "The request took too long."},
    })
}

/// `initialize` params.
pub fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {"name": "Familiar", "version": env!("CARGO_PKG_VERSION")},
    })
}

/// What a server sent.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    /// An answer to one of our requests: its `result`, or the error's message.
    Response {
        /// The request's id.
        id: Value,
        /// `result`, or `error.message`.
        outcome: Result<Value, String>,
    },
    /// A request from the server to us.
    Request {
        /// Its id, echoed in our answer.
        id: Value,
        /// Its method.
        method: String,
    },
    /// A notification or something unrecognised.
    Other,
}

/// Sort one message.
pub fn classify(mut msg: Value) -> Incoming {
    let Some(obj) = msg.as_object_mut() else {
        return Incoming::Other;
    };
    let id = obj.remove("id").filter(|id| !id.is_null());
    match (id, obj.get("method").and_then(Value::as_str)) {
        (Some(id), Some(method)) => Incoming::Request {
            id,
            method: method.to_owned(),
        },
        (Some(id), None) => {
            let outcome = match (obj.remove("result"), obj.get("error")) {
                (_, Some(error)) => Err(error_message(error)),
                (Some(result), None) => Ok(result),
                (None, None) => Ok(Value::Null),
            };
            Incoming::Response { id, outcome }
        }
        _ => Incoming::Other,
    }
}

fn error_message(error: &Value) -> String {
    match error.get("message").and_then(Value::as_str) {
        Some(message) if !message.trim().is_empty() => message.trim().to_owned(),
        _ => match error.get("code").and_then(Value::as_i64) {
            Some(code) => format!("error {code} with no message"),
            None => "an error with no message".to_owned(),
        },
    }
}

/// Whether a response id is the one we sent (some servers echo it as a string).
pub fn same_id(got: &Value, sent: u64) -> bool {
    got.as_u64() == Some(sent) || got.as_str() == Some(sent.to_string().as_str())
}

/// Our answer to a server's request: `ping` is answered, anything else is refused.
pub fn answer(id: Value, method: &str) -> Value {
    if method == "ping" {
        json!({"jsonrpc": "2.0", "id": id, "result": {}})
    } else {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": format!("The client does not support {method}.")},
        })
    }
}

/// Find the answer to `sent` in a body that is one message or a batch.
pub fn pick(body: Value, sent: u64) -> Option<Result<Value, String>> {
    let messages = match body {
        Value::Array(items) => items,
        other => vec![other],
    };
    messages.into_iter().find_map(|msg| match classify(msg) {
        Incoming::Response { id, outcome } if same_id(&id, sent) => Some(outcome),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Plain answers, pings and notifications are covered by the stdio and HTTP tests;
    // these are the edge cases real servers produce that those fakes don't.
    #[test]
    fn batches_string_ids_and_errors_without_a_message_still_resolve() {
        let batch = json!([
            {"jsonrpc": "2.0", "method": "notifications/message"},
            {"jsonrpc": "2.0", "id": 1, "result": "other"},
            {"jsonrpc": "2.0", "id": "2", "result": "mine"},
        ]);
        assert_eq!(pick(batch, 2), Some(Ok(json!("mine"))));
        assert_eq!(
            pick(json!({"jsonrpc": "2.0", "id": 7, "error": {"code": -1}}), 7),
            Some(Err("error -1 with no message".into()))
        );
        assert_eq!(pick(json!({"jsonrpc": "2.0", "id": 1}), 2), None);
    }
}
