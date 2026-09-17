use mcp_common::*;
use serde_json::json;

/// Получение capabilities сервера.
///
/// Сервер сознательно tools-only по ресурсам: `resources` не объявляется, потому
/// что чтения ресурсов (`resources/read`) он не реализует. Объявленная, но не
/// обслуженная capability хуже отсутствующей — клиент честно берёт
/// `resources/list`, а на каждом чтении получает отказ, и знания не доезжают до
/// модели вообще. Знания доступны через `tools/call`.
///
/// `prompts` объявляется только когда промпты действительно загружены:
/// `prompts.json` лежит в каталоге знаний и может не доехать вместе с базой.
/// Сервер с объявленной capability и пустым `prompts/list` — невалидный MCP;
/// старт при этом не валится, потому что core работает и без промптов.
pub fn get_server_capabilities(has_prompts: bool) -> ServerCapabilities {
    ServerCapabilities {
        // list_changed не объявляется: список инструментов статичен, и нотификацию
        // notifications/tools/list_changed сервер не отправляет.
        tools: Some(ToolsCapability {
            list_changed: false,
        }),
        resources: None,
        prompts: has_prompts.then_some(PromptsCapability {
            list_changed: false,
        }),
    }
}

/// Список доступных инструментов
pub fn get_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "search_1c_docs".to_string(),
            description: "Поиск по документации 1С:Предприятие с учётом версии, контекста выполнения и связей между объектами".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Поисковый запрос (русский/английский)"
                    },
                    "version": {
                        "type": "string",
                        "description": "Версия платформы (например, '8.3.25'); 'latest' — старшая загруженная. Состав базы знаний показывает list_versions",
                        "default": "latest"
                    },
                    "context": {
                        "type": "string",
                        "enum": ["client", "server", "both"],
                        "description": "Контекст выполнения кода"
                    },
                    "include_related": {
                        "type": "boolean",
                        "description": "Включить связанные объекты в граф",
                        "default": true
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Максимальное количество результатов; по умолчанию — значение [search] max_results из конфига сервера",
                        "minimum": 1,
                        "maximum": 500
                    }
                },
                "required": ["query"]
            }),
        },
        Tool {
            name: "get_method_details".to_string(),
            description: "Получение подробной информации о методе API 1С, включая параметры, возвращаемые значения и примеры использования".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "method_name": {
                        "type": "string",
                        "description": "Имя метода API"
                    },
                    "version": {
                        "type": "string",
                        "description": "Версия платформы; 'latest' — старшая загруженная. Состав базы знаний показывает list_versions",
                        "default": "latest"
                    },
                    "include_examples": {
                        "type": "boolean",
                        "description": "Включить примеры использования",
                        "default": true
                    }
                },
                "required": ["method_name"]
            }),
        },
        Tool {
            name: "find_usage_examples".to_string(),
            description: "Поиск примеров использования конкретного API объекта или метода в документации".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "entity_name": {
                        "type": "string",
                        "description": "Имя API объекта или метода"
                    },
                    "version": {
                        "type": "string",
                        "description": "Версия платформы; 'latest' — старшая загруженная. Состав базы знаний показывает list_versions",
                        "default": "latest"
                    },
                    "max_examples": {
                        "type": "integer",
                        "description": "Максимальное количество примеров",
                        "default": 5,
                        "minimum": 1,
                        "maximum": 20
                    }
                },
                "required": ["entity_name"]
            }),
        },
        Tool {
            name: "list_versions".to_string(),
            description: "Версии документации 1С, загруженные в базу знаний, с наполнением каждой".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        // Версия сервера, а не платформы 1С: `list_versions` отвечает про базу
        // знаний, и спутать их — значит назвать владельцу 8.3.27 вместо сборки.
        mcp_common::server_info_tool(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capabilities() {
        let caps = get_server_capabilities(true);
        assert!(caps.tools.is_some());
        assert!(caps.prompts.is_some());
    }

    /// Регрессия: capability `resources` объявлялась при нереализованном
    /// `resources/read` — клиент получал список ресурсов и отказ на каждом чтении.
    #[test]
    fn resources_capability_is_not_advertised() {
        assert!(get_server_capabilities(true).resources.is_none());
    }

    /// Без промптов capability не объявляется: пустой `prompts/list` при
    /// объявленной capability — невалидный MCP.
    #[test]
    fn prompts_capability_follows_loaded_prompts() {
        assert!(get_server_capabilities(false).prompts.is_none());
        assert!(get_server_capabilities(false).tools.is_some());
    }

    #[test]
    fn test_tools_list() {
        let tools = get_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();

        assert_eq!(
            names,
            vec![
                "search_1c_docs",
                "get_method_details",
                "find_usage_examples",
                "list_versions",
                "server_info",
            ]
        );
    }
}
