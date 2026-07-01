//! Wire format exchanged over the `/agent/desktop` WebSocket (§4-5 of the design spec).
//! Zero new protocol: `{ taskId, tool, params }` in, `{ taskId, result }` or
//! `{ taskId, error }` out, plus an app-level `{"type":"ping"}` heartbeat.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct IncomingTask {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub tool: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutgoingMessage {
    TaskResult { task_id: String, result: Value },
    TaskError { task_id: String, error: String },
    Ping,
}

impl OutgoingMessage {
    pub fn to_json_string(&self) -> serde_json::Result<String> {
        let value = match self {
            OutgoingMessage::TaskResult { task_id, result } => {
                serde_json::json!({ "taskId": task_id, "result": result })
            }
            OutgoingMessage::TaskError { task_id, error } => {
                serde_json::json!({ "taskId": task_id, "error": error })
            }
            OutgoingMessage::Ping => serde_json::json!({ "type": "ping" }),
        };
        serde_json::to_string(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incoming_task_parses_camel_case_task_id() {
        let raw = r#"{"taskId":"t1","tool":"wiki_search","params":{"query":"x"}}"#;
        let task: IncomingTask = serde_json::from_str(raw).unwrap();
        assert_eq!(task.task_id, "t1");
        assert_eq!(task.tool, "wiki_search");
        assert_eq!(task.params["query"], "x");
    }

    #[test]
    fn incoming_task_defaults_params_when_absent() {
        let raw = r#"{"taskId":"t1","tool":"wiki_get_ontology"}"#;
        let task: IncomingTask = serde_json::from_str(raw).unwrap();
        assert_eq!(task.params, Value::Null);
    }

    #[test]
    fn task_result_serializes_task_id_and_result() {
        let msg = OutgoingMessage::TaskResult {
            task_id: "t1".into(),
            result: serde_json::json!([1, 2, 3]),
        };
        let json: Value = serde_json::from_str(&msg.to_json_string().unwrap()).unwrap();
        assert_eq!(json["taskId"], "t1");
        assert_eq!(json["result"], serde_json::json!([1, 2, 3]));
        assert!(json.get("error").is_none());
    }

    #[test]
    fn task_error_serializes_task_id_and_error() {
        let msg = OutgoingMessage::TaskError {
            task_id: "t1".into(),
            error: "boom".into(),
        };
        let json: Value = serde_json::from_str(&msg.to_json_string().unwrap()).unwrap();
        assert_eq!(json["taskId"], "t1");
        assert_eq!(json["error"], "boom");
        assert!(json.get("result").is_none());
    }

    #[test]
    fn ping_serializes_to_type_ping() {
        let json: Value = serde_json::from_str(&OutgoingMessage::Ping.to_json_string().unwrap()).unwrap();
        assert_eq!(json["type"], "ping");
    }
}
