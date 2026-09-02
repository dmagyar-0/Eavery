//! Newline-delimited JSON-RPC 2.0, hand-rolled.
//!
//! The fake agent deliberately does not use the ACP SDK: it is the yardstick
//! the SDK is measured against, so an SDK bug must not be able to hide by
//! appearing on both sides of the wire
//! (`docs/plan/10-task-breakdown.md`, M0-T05).

use serde_json::{Value, json};

/// A parsed JSON-RPC message. Which of the three it is follows from which
/// fields are present, not from anything the sender declares.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// `method` and `id`: expects a response.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// `method`, no `id`: expects nothing.
    Notification { method: String, params: Value },
    /// `id` and one of `result` / `error`.
    Response {
        id: Value,
        outcome: Result<Value, RpcError>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ParseError {
    #[error("not JSON: {0}")]
    NotJson(String),
    #[error("not a JSON-RPC message: {0}")]
    NotAMessage(String),
}

/// Standard JSON-RPC error codes (`docs/plan/04-acp-engines.md` §7).
pub const METHOD_NOT_FOUND: i64 = -32601;
/// The implementation-defined range. Eavery uses -32000 for "refused by
/// Eavery", such as a write during the plan phase.
pub const REFUSED: i64 = -32000;

/// Parses one line. Blank lines are not messages and are not errors: agents and
/// clients both flush newlines liberally.
pub fn parse_line(line: &str) -> Result<Option<Message>, ParseError> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let value: Value =
        serde_json::from_str(line).map_err(|e| ParseError::NotJson(e.to_string()))?;

    let id = value.get("id").cloned();
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let params = value.get("params").cloned().unwrap_or(Value::Null);

    match (method, id) {
        (Some(method), Some(id)) if !id.is_null() => {
            Ok(Some(Message::Request { id, method, params }))
        }
        (Some(method), _) => Ok(Some(Message::Notification { method, params })),
        (None, Some(id)) => {
            let outcome = match value.get("error") {
                Some(error) => Err(RpcError {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    data: error.get("data").cloned(),
                }),
                None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
            };
            Ok(Some(Message::Response { id, outcome }))
        }
        (None, None) => Err(ParseError::NotAMessage(line.to_owned())),
    }
}

pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

pub fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

pub fn ok_response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_lines_are_not_messages_and_not_errors() {
        assert_eq!(parse_line(""), Ok(None));
        assert_eq!(parse_line("   \t "), Ok(None));
    }

    #[test]
    fn a_method_with_an_id_is_a_request() {
        let msg =
            parse_line(r#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"/p"}}"#)
                .unwrap()
                .unwrap();
        match msg {
            Message::Request { id, method, params } => {
                assert_eq!(id, json!(1));
                assert_eq!(method, "session/new");
                assert_eq!(params["cwd"], "/p");
            }
            other => panic!("expected a request, got {other:?}"),
        }
    }

    #[test]
    fn a_method_without_an_id_is_a_notification() {
        let msg = parse_line(r#"{"jsonrpc":"2.0","method":"session/cancel","params":{}}"#)
            .unwrap()
            .unwrap();
        assert!(
            matches!(msg, Message::Notification { ref method, .. } if method == "session/cancel")
        );
    }

    /// A null id is how some implementations spell "no id". It must not turn a
    /// notification into a request the agent then tries to answer.
    #[test]
    fn a_null_id_is_still_a_notification() {
        let msg = parse_line(r#"{"jsonrpc":"2.0","id":null,"method":"session/cancel"}"#)
            .unwrap()
            .unwrap();
        assert!(matches!(msg, Message::Notification { .. }));
    }

    #[test]
    fn string_ids_survive_the_round_trip() {
        let msg = parse_line(r#"{"jsonrpc":"2.0","id":"abc","method":"initialize"}"#)
            .unwrap()
            .unwrap();
        match msg {
            Message::Request { id, .. } => assert_eq!(id, json!("abc")),
            other => panic!("expected a request, got {other:?}"),
        }
    }

    #[test]
    fn results_and_errors_are_responses() {
        let ok =
            parse_line(r#"{"jsonrpc":"2.0","id":7,"result":{"outcome":{"outcome":"cancelled"}}}"#)
                .unwrap()
                .unwrap();
        match ok {
            Message::Response {
                id,
                outcome: Ok(result),
            } => {
                assert_eq!(id, json!(7));
                assert_eq!(result["outcome"]["outcome"], "cancelled");
            }
            other => panic!("expected an ok response, got {other:?}"),
        }

        let err = parse_line(r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32000,"message":"no"}}"#)
            .unwrap()
            .unwrap();
        match err {
            Message::Response {
                outcome: Err(e), ..
            } => {
                assert_eq!(e.code, REFUSED);
                assert_eq!(e.message, "no");
            }
            other => panic!("expected an error response, got {other:?}"),
        }
    }

    /// A response whose `result` is absent but which carries no `error` either
    /// is still a response: `fs/write_text_file` legitimately returns nothing.
    #[test]
    fn a_response_with_no_result_is_an_empty_ok() {
        let msg = parse_line(r#"{"jsonrpc":"2.0","id":3}"#).unwrap().unwrap();
        assert!(matches!(
            msg,
            Message::Response {
                outcome: Ok(Value::Null),
                ..
            }
        ));
    }

    #[test]
    fn junk_is_reported_rather_than_guessed_at() {
        assert!(matches!(
            parse_line("not json at all"),
            Err(ParseError::NotJson(_))
        ));
        assert!(matches!(
            parse_line(r#"{"hello":"world"}"#),
            Err(ParseError::NotAMessage(_))
        ));
    }

    #[test]
    fn constructors_produce_well_formed_messages() {
        let req = request(4, "session/request_permission", json!({"sessionId": "s"}));
        assert_eq!(req["jsonrpc"], "2.0");
        assert_eq!(req["id"], 4);
        assert!(matches!(
            parse_line(&req.to_string()).unwrap().unwrap(),
            Message::Request { .. }
        ));

        let note = notification("session/update", json!({}));
        assert!(note.get("id").is_none());
        assert!(matches!(
            parse_line(&note.to_string()).unwrap().unwrap(),
            Message::Notification { .. }
        ));

        let ok = ok_response(&json!(1), json!({"sessionId": "s"}));
        assert!(matches!(
            parse_line(&ok.to_string()).unwrap().unwrap(),
            Message::Response { outcome: Ok(_), .. }
        ));

        let err = error_response(&json!(1), METHOD_NOT_FOUND, "no such method");
        assert!(matches!(
            parse_line(&err.to_string()).unwrap().unwrap(),
            Message::Response {
                outcome: Err(_),
                ..
            }
        ));
    }
}
