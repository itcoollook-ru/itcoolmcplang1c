//! MCP wire-типы: то, что уходит на провод в теле JSON-RPC `result`/`params`.
//!
//! Сериализация — строго в camelCase, как требует спецификация Model Context Protocol.
//! Типы собраны как надмножество прежних копий обоих серверов:
//! `Content` несёт и `text`, и `image`; `ServerCapabilities` типизирован
//! (tools/resources/prompts), а не `Value`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── initialize ──────────────────────────────────────────────────────────────

/// Версия протокола MCP, которую заявляют серверы каталога в ответе на `initialize`.
///
/// В MCP это датированная строка ревизии спецификации, а не semver: хост,
/// получивший `"1.0"`, не может согласовать версию и рвёт соединение. Серверам
/// хардкодить своё значение нельзя — берут отсюда.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Параметры `initialize`, присылаемые клиентом.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: String,
    #[serde(default)]
    pub capabilities: Value,
    pub client_info: ClientInfo,
}

/// Информация о клиенте.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// Результат `initialize`, возвращаемый сервером.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
}

/// Информация о сервере.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,

    /// Сводка о данных, которые сервер реально видит: загруженные версии базы
    /// знаний, объём разобранной конфигурации.
    ///
    /// Сервер с пустой или чужой базой отвечает на `initialize` и `tools/list`
    /// неотличимо от рабочего — качество ответов проседает молча. Поле даёт
    /// хосту увидеть это при рукопожатии. Необязательное и текстовое: клиент,
    /// который его не знает, просто игнорирует.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

/// Возможности сервера. Незаявленные разделы опускаются (`None`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsCapability {
    #[serde(default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesCapability {
    #[serde(default)]
    pub subscribe: bool,
    #[serde(default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptsCapability {
    #[serde(default)]
    pub list_changed: bool,
}

// ── tools ─────────────────────────────────────────────────────────────────────

/// Описание инструмента для `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Результат `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsListResult {
    pub tools: Vec<Tool>,
}

/// Параметры `tools/call`. `arguments` оставлены нетипизированными (`Value`):
/// сервер сам достаёт нужные поля или десериализует в свою структуру.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

/// Результат `tools/call`.
///
/// `result_count` — необязательное машиночитаемое число найденного. Пустая выдача
/// у нас не отказ и не пустой `content`: там остаётся текст «не найдено», полезный
/// модели-потребителю. Хосту же текст читать нельзя — он не контракт и меняется,
/// поэтому «нашлось ноль» приезжает числом. Поле необязательное: клиент, который
/// его не знает, ведёт себя как раньше.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallResult {
    pub content: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_count: Option<usize>,
}

impl ToolCallResult {
    /// Удобный конструктор результата из одного текстового блока.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::text(text)],
            is_error: None,
            result_count: None,
        }
    }

    /// Успешная выдача перечисляющего инструмента: текст плюс число найденного.
    ///
    /// Ноль — штатный ответ «не найдено», а не отказ: `isError` остаётся `false`.
    pub fn found(text: impl Into<String>, count: usize) -> Self {
        Self {
            content: vec![Content::text(text)],
            is_error: Some(false),
            result_count: Some(count),
        }
    }
}

/// Блок содержимого ответа инструмента/промпта.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Content {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image", rename_all = "camelCase")]
    Image { data: String, mime_type: String },
}

impl Content {
    pub fn text(text: impl Into<String>) -> Self {
        Content::Text { text: text.into() }
    }
}

// ── resources ─────────────────────────────────────────────────────────────────

/// Описание ресурса для `resources/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Результат `resources/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesListResult {
    pub resources: Vec<Resource>,
}

/// Параметры `resources/read`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceReadParams {
    pub uri: String,
}

/// Содержимое одного ресурса.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContents {
    pub uri: String,
    pub mime_type: String,
    pub text: String,
}

/// Результат `resources/read`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceReadResult {
    pub contents: Vec<ResourceContents>,
}

// ── prompts ───────────────────────────────────────────────────────────────────

/// Описание промпта для `prompts/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgument>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

/// Параметры `prompts/get`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPromptParams {
    pub name: String,
    #[serde(default)]
    pub arguments: std::collections::HashMap<String, String>,
}

/// Результат `prompts/get`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPromptResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub messages: Vec<PromptMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMessage {
    pub role: String,
    pub content: Content,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_text_is_tagged() {
        let s = serde_json::to_string(&Content::text("hi")).unwrap();
        assert_eq!(s, r#"{"type":"text","text":"hi"}"#);
    }

    #[test]
    fn content_image_uses_camel_case() {
        let s = serde_json::to_string(&Content::Image {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
        })
        .unwrap();
        assert!(s.contains(r#""type":"image""#));
        assert!(s.contains(r#""mimeType":"image/png""#));
    }

    /// Хост читает «нашлось ноль» числом, а не текстом: поле обязано доезжать
    /// до провода под camelCase-именем.
    #[test]
    fn tool_call_result_serializes_result_count() {
        // Arrange
        let result = ToolCallResult::found("Совпадений не найдено", 0);

        // Act
        let json = serde_json::to_string(&result).unwrap();

        // Assert
        assert!(json.contains(r#""resultCount":0"#), "{json}");
        assert!(json.contains(r#""isError":false"#), "{json}");
    }

    /// Инструменту, которому счёт неприменим (одна карточка, ответ моста),
    /// поле не навязывается: клиент не должен видеть его пустым.
    #[test]
    fn tool_call_result_omits_absent_result_count() {
        // Arrange
        let result = ToolCallResult::text("одна карточка");

        // Act
        let json = serde_json::to_string(&result).unwrap();

        // Assert
        assert!(!json.contains("resultCount"), "{json}");
    }

    #[test]
    fn tool_input_schema_is_camel_case() {
        let t = Tool {
            name: "n".into(),
            description: "d".into(),
            input_schema: serde_json::json!({"type": "object"}),
        };
        let s = serde_json::to_string(&t).unwrap();
        assert!(s.contains(r#""inputSchema""#));
    }

    /// Входящий путь: параметры `initialize` разбираются из camelCase-JSON клиента.
    #[test]
    fn initialize_params_deserialize_from_camel_case() {
        let p: InitializeParams = serde_json::from_str(
            r#"{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"c","version":"1"}}"#,
        )
        .unwrap();
        assert_eq!(p.protocol_version, "2024-11-05");
        assert_eq!(p.client_info.name, "c");

        // capabilities опциональны (default).
        let p: InitializeParams = serde_json::from_str(
            r#"{"protocolVersion":"2024-11-05","clientInfo":{"name":"c","version":"1"}}"#,
        )
        .unwrap();
        assert!(p.capabilities.is_null());
    }

    /// `tools/call` без arguments — валидный вызов, arguments по умолчанию Null.
    #[test]
    fn tool_call_params_default_arguments() {
        let p: ToolCallParams = serde_json::from_str(r#"{"name":"t"}"#).unwrap();
        assert_eq!(p.name, "t");
        assert!(p.arguments.is_null());
    }

    /// `prompts/get` без arguments — пустая map.
    #[test]
    fn get_prompt_params_default_arguments() {
        let p: GetPromptParams = serde_json::from_str(r#"{"name":"p"}"#).unwrap();
        assert_eq!(p.name, "p");
        assert!(p.arguments.is_empty());
    }

    #[test]
    fn initialize_result_round_trips() {
        let r = InitializeResult {
            protocol_version: "2024-11-05".into(),
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability::default()),
                ..Default::default()
            },
            server_info: ServerInfo {
                name: "srv".into(),
                version: "0.1.0".into(),
                data: None,
            },
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""protocolVersion""#));
        assert!(s.contains(r#""serverInfo""#));
        let _back: InitializeResult = serde_json::from_str(&s).unwrap();
    }
}
