//! Инструмент `server_info` — одинаковый у трёх серверов.
//!
//! Версия и метка сборки уже есть в `serverInfo` при рукопожатии, но рукопожатие
//! видит только хост. Потребитель, работающий инструментами, назвать свою сборку
//! не мог: в `tools/list` версии не было ни у одного сервера. Инструмент закрывает
//! ровно это — «какая у тебя версия» становится вопросом, на который отвечает сам
//! сервер, а не поход на машину за `--version`.
//!
//! Объявление и формат ответа лежат здесь, а не в трёх серверах: имя инструмента
//! у них обязано совпадать слово в слово, иначе потребителю придётся помнить, у
//! кого он `server_info`, а у кого `about`.

use crate::protocol::Tool;
use serde_json::json;

/// Имя инструмента. Одно на три сервера.
pub const SERVER_INFO_TOOL: &str = "server_info";

/// Объявление для `tools/list`.
///
/// Без параметров: у вопроса «что за сборка» аргументов нет, а пустая схема
/// избавляет модель от догадок, чем его вызывать.
pub fn server_info_tool() -> Tool {
    Tool {
        name: SERVER_INFO_TOOL.to_string(),
        description: "Имя сервера, версия с меткой сборки и сводка о загруженных данных. \
Этим отвечают на вопрос «какая версия инструмента» — версию называет сам сервер, \
а не файл на диске."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
    }
}

/// Текст ответа: строка версии и, если серверу есть что сказать о данных, сводка.
///
/// Формат совпадает с тем, что уходит в `serverInfo` и в стартовую запись журнала:
/// одну и ту же сборку по трём каналам должно быть видно одинаково.
pub fn server_info_text(name: &str, version: &str, data: Option<&str>) -> String {
    match data {
        Some(data) => format!("{name} {version}\nданные: {data}"),
        None => format!("{name} {version}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_names_server_version_and_data() {
        // Arrange
        let version = "0.4.0 (2026-08-07, abc1234)";

        // Act
        let text = server_info_text("itcoolmcpconf1c", version, Some("3154 объекта"));

        // Assert
        assert!(text.contains("itcoolmcpconf1c"));
        assert!(text.contains(version));
        assert!(text.contains("3154 объекта"));
    }

    /// Сервер без собственных данных не должен выдумывать пустую строку сводки.
    #[test]
    fn text_without_data_is_single_line() {
        // Arrange & Act
        let text = server_info_text("itcoolmcprun1c", "0.4.0 (2026-08-07, abc1234)", None);

        // Assert
        assert_eq!(text.lines().count(), 1);
        assert!(!text.contains("данные:"));
    }

    /// Инструмент без аргументов: `required` в схеме означал бы, что вызвать его
    /// нельзя, не угадав параметр.
    #[test]
    fn tool_declares_no_arguments() {
        // Arrange & Act
        let tool = server_info_tool();

        // Assert
        assert_eq!(tool.name, SERVER_INFO_TOOL);
        assert_eq!(tool.input_schema["properties"], json!({}));
        assert!(tool.input_schema.get("required").is_none());
    }
}
