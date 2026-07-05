//! Wire format exchanged over the `/agent/desktop` WebSocket.
//! Conforms to Clanker's typed frame schemas (see design spec §5).

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum IncomingFrame {
    Ready,
    Pong,
    Task {
        #[serde(rename = "taskId")]
        task_id: String,
        tool: String,
        #[serde(default)]
        params: Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskErrorCode {
    UnknownTool,
    BadParams,
    ToolTimeout,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskErrorBody {
    pub code: TaskErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutgoingMessage {
    Auth {
        #[serde(rename = "pairingToken")]
        pairing_token: String,
    },
    TaskResult {
        #[serde(rename = "taskId")]
        task_id: String,
        result: Value,
    },
    TaskError {
        #[serde(rename = "taskId")]
        task_id: String,
        error: TaskErrorBody,
    },
    Ping,
}

/// Classify a dispatch error into the CT-side error-code taxonomy (design spec §6).
pub fn classify_dispatch_error(err: &anyhow::Error) -> TaskErrorCode {
    let msg = err.to_string();
    if msg.contains("unknown tool") {
        TaskErrorCode::UnknownTool
    } else if err.downcast_ref::<serde_json::Error>().is_some() {
        TaskErrorCode::BadParams
    } else {
        TaskErrorCode::Internal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incoming_ready_parses() {
        let frame: IncomingFrame = serde_json::from_str(r#"{"type":"ready"}"#).unwrap();
        assert_eq!(frame, IncomingFrame::Ready);
    }

    #[test]
    fn incoming_pong_parses() {
        let frame: IncomingFrame = serde_json::from_str(r#"{"type":"pong"}"#).unwrap();
        assert_eq!(frame, IncomingFrame::Pong);
    }

    #[test]
    fn incoming_task_parses_camel_case_task_id() {
        let raw = r#"{"type":"task","taskId":"t1","tool":"wiki_search","params":{"query":"x"}}"#;
        let frame: IncomingFrame = serde_json::from_str(raw).unwrap();
        assert_eq!(
            frame,
            IncomingFrame::Task {
                task_id: "t1".into(),
                tool: "wiki_search".into(),
                params: serde_json::json!({"query": "x"}),
            }
        );
    }

    #[test]
    fn incoming_task_defaults_params_when_absent() {
        let raw = r#"{"type":"task","taskId":"t1","tool":"wiki_get_ontology"}"#;
        let frame: IncomingFrame = serde_json::from_str(raw).unwrap();
        match frame {
            IncomingFrame::Task { params, .. } => assert_eq!(params, Value::Null),
            other => panic!("expected Task, got {other:?}"),
        }
    }

    #[test]
    fn unknown_incoming_type_fails_to_parse() {
        assert!(serde_json::from_str::<IncomingFrame>(r#"{"type":"nope"}"#).is_err());
    }

    #[test]
    fn auth_serializes_with_type_and_pairing_token() {
        let msg = OutgoingMessage::Auth {
            pairing_token: "tok".into(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "auth");
        assert_eq!(json["pairingToken"], "tok");
    }

    #[test]
    fn task_result_serializes_with_type_field() {
        let msg = OutgoingMessage::TaskResult {
            task_id: "t1".into(),
            result: serde_json::json!([1, 2, 3]),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "task_result");
        assert_eq!(json["taskId"], "t1");
        assert_eq!(json["result"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn task_error_serializes_structured_error() {
        let msg = OutgoingMessage::TaskError {
            task_id: "t1".into(),
            error: TaskErrorBody {
                code: TaskErrorCode::UnknownTool,
                message: "unknown tool: delete_everything".into(),
            },
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "task_error");
        assert_eq!(json["taskId"], "t1");
        assert_eq!(json["error"]["code"], "UNKNOWN_TOOL");
        assert_eq!(json["error"]["message"], "unknown tool: delete_everything");
    }

    #[test]
    fn ping_serializes_to_type_ping() {
        let json = serde_json::to_value(&OutgoingMessage::Ping).unwrap();
        assert_eq!(json["type"], "ping");
    }
}
