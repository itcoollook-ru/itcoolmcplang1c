//! JSON-RPC 2.0 — общий конверт запроса/ответа/ошибки.
//!
//! Серверы каталога до выделения этого крейта держали собственные, слегка
//! разошедшиеся копии этих типов. Здесь — единый источник.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 запрос.
///
/// `id` отсутствует у нотификаций. `params` опциональны.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Request {
    /// Это нотификация (запрос без `id`, ответ на который не отправляется).
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// JSON-RPC 2.0 ответ. Ровно одно из полей `result`/`error` заполнено.
///
/// `id` сериализуется всегда: по JSON-RPC 2.0 член `id` в ответе обязателен,
/// а когда id запроса определить не удалось (parse error / invalid request),
/// он обязан быть `null` — поэтому `None` уходит на провод как `"id":null`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    /// Успешный ответ с результатом.
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Ответ с ошибкой.
    pub fn failure(id: Option<Value>, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// JSON-RPC 2.0 объект ошибки.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// Произвольная ошибка с кодом и сообщением.
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// -32700 Parse error.
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(-32700, message)
    }

    /// -32600 Invalid Request.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(-32600, message)
    }

    /// -32601 Method not found.
    pub fn method_not_found(message: impl Into<String>) -> Self {
        Self::new(-32601, message)
    }

    /// -32602 Invalid params.
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message)
    }

    /// -32603 Internal error.
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new(-32603, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_request() {
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
                .unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "initialize");
        assert!(!req.is_notification());
    }

    #[test]
    fn detects_notification() {
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .unwrap();
        assert!(req.is_notification());
    }

    #[test]
    fn serializes_success_without_error_field() {
        let resp = Response::success(Some(json!(1)), json!({"ok": true}));
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains(r#""result""#));
        assert!(!s.contains(r#""error""#));
        assert!(s.contains(r#""jsonrpc":"2.0""#));
    }

    /// По JSON-RPC 2.0 `id` в ответе обязателен: при неопределимом id — `null`.
    #[test]
    fn serializes_failure_without_result_field() {
        let resp = Response::failure(None, RpcError::method_not_found("nope"));
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains(r#""error""#));
        assert!(!s.contains(r#""result""#));
        assert!(s.contains(r#""id":null"#));
    }
}
