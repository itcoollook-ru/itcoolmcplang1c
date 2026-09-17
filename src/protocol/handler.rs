use super::capabilities;
use crate::hbk::Progress;
use crate::knowledge::schema::{ApiMethod, ExecutionContext};
use crate::knowledge::{KnowledgeBase, SearchQuery};
use crate::prompts::PromptsEngine;
use anyhow::Result;
use mcp_common::{
    Content, GetPromptParams, GetPromptResult, InitializeParams, InitializeResult, Prompt,
    PromptArgument, PromptMessage, Request, Response, RpcError, ServerInfo, ToolCallParams,
    ToolCallResult, ToolsListResult, MCP_PROTOCOL_VERSION,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Потолок выдачи поиска, который не переступает даже аргумент хоста.
///
/// Тот же, что у `list_*` и `search_in_modules` в conf1c: один потолок на каталог
/// — иначе «максимум» означает разное у соседних серверов.
const MAX_SEARCH_RESULTS: usize = 500;

/// Маркер «метод не поддерживается».
///
/// В JSON-RPC 2.0 у такого отказа свой код (−32601), и клиент читает его как
/// «этого метода тут нет, обойдусь без него». Общий −32603 (internal error)
/// клиент трактует как сбой сервера и уходит в ненужные перезапуски.
#[derive(Debug)]
struct MethodNotFound(String);

impl std::fmt::Display for MethodNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ru: Метод не поддерживается: {}, en: Method not found: {}",
            self.0, self.0
        )
    }
}

impl std::error::Error for MethodNotFound {}

/// Маркер «невалидные параметры» (−32602).
///
/// Отличие от общего −32603 практическое: −32602 клиент читает как «исправь свой
/// запрос», а −32603 — как сбой сервера и повод для перезапуска.
#[derive(Debug)]
struct InvalidParams(String);

impl std::fmt::Display for InvalidParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for InvalidParams {}

/// Обработчик MCP запросов
pub struct ProtocolHandler {
    knowledge_base: Arc<RwLock<KnowledgeBase>>,
    prompts_engine: Arc<RwLock<PromptsEngine>>,
    /// Ход самосборки базы знаний: пока индекс строится, инструменты отвечают
    /// не отказом контура, а текстом с процентом.
    progress: Arc<Progress>,
    max_results: usize,
    server_info: ServerInfo,
}

impl ProtocolHandler {
    /// Обработчик с потолком выдачи поиска из конфига (`[search] max_results`).
    /// До 0.3.0 ключ объявлялся и не читался — в коде было зашито 10.
    pub fn with_max_results(
        knowledge_base: Arc<RwLock<KnowledgeBase>>,
        prompts_engine: Arc<RwLock<PromptsEngine>>,
        progress: Arc<Progress>,
        max_results: usize,
    ) -> Self {
        Self {
            knowledge_base,
            prompts_engine,
            progress,
            max_results,
            server_info: ServerInfo {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: mcp_common::build_version!().to_string(),
                // Заполняется на рукопожатии: база грузится после создания
                // обработчика, и на момент конструктора она ещё пуста.
                data: None,
            },
        }
    }

    /// Обработка JSON-RPC запроса
    pub async fn handle_request(&self, request: Request) -> Response {
        let id = request.id.clone();

        match self.dispatch(&request).await {
            Ok(result) => Response::success(id, result),
            Err(e) if e.downcast_ref::<MethodNotFound>().is_some() => {
                tracing::warn!(
                    "ru: Неподдержанный метод: {}, en: Unsupported method: {}",
                    e,
                    e
                );
                Response::failure(id, RpcError::method_not_found(e.to_string()))
            }
            Err(e) if e.downcast_ref::<InvalidParams>().is_some() => {
                tracing::warn!("ru: Невалидные параметры: {}, en: Invalid params: {}", e, e);
                Response::failure(id, RpcError::invalid_params(e.to_string()))
            }
            Err(e) => {
                tracing::error!(
                    "ru: Ошибка обработки запроса: {}, en: Request error: {}",
                    e,
                    e
                );
                Response::failure(id, RpcError::internal_error(e.to_string()))
            }
        }
    }

    async fn dispatch(&self, request: &Request) -> Result<serde_json::Value> {
        match request.method.as_str() {
            "initialize" => self.handle_initialize(request).await,
            "tools/list" => self.handle_tools_list().await,
            "tools/call" => self.handle_tools_call(request).await,
            "prompts/list" => self.handle_prompts_list().await,
            "prompts/get" => self.handle_prompts_get(request).await,
            // Спецификация MCP 2024-11-05: на ping отвечают пустым result.
            "ping" => Ok(json!({})),
            // Нотификации клиента — штатные события: в stdio ответ подавляет
            // транспорт, в HTTP тело не отправляется (см. handle_mcp_request).
            m if m.starts_with("notifications/") && request.is_notification() => {
                tracing::debug!("MCP notification: {}", request.method);
                Ok(serde_json::Value::Null)
            }
            // `resources/*` сюда не попадает намеренно: capability не объявлена,
            // см. `capabilities::get_server_capabilities`.
            _ => Err(MethodNotFound(request.method.clone()).into()),
        }
    }

    async fn handle_initialize(&self, request: &Request) -> Result<serde_json::Value> {
        let _params: InitializeParams = serde_json::from_value(
            request.params.clone().unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| {
            InvalidParams(format!(
                "ru: Невалидные параметры initialize: {e}, en: Invalid initialize parameters: {e}"
            ))
        })?;

        let prompts_loaded = self.prompts_engine.read().await.list_prompts().len();

        let mut server_info = self.server_info.clone();
        server_info.data = Some(format!(
            "{}; промптов: {}",
            knowledge_summary(&*self.knowledge_base.read().await),
            prompts_loaded
        ));

        let result = InitializeResult {
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            capabilities: capabilities::get_server_capabilities(prompts_loaded > 0),
            server_info,
        };

        Ok(serde_json::to_value(result)?)
    }

    async fn handle_tools_list(&self) -> Result<serde_json::Value> {
        let tools = capabilities::get_tools();
        Ok(serde_json::to_value(ToolsListResult { tools })?)
    }

    async fn handle_tools_call(&self, request: &Request) -> Result<serde_json::Value> {
        let params: ToolCallParams = serde_json::from_value(request.params.clone().ok_or_else(
            || InvalidParams("ru: Отсутствуют параметры, en: Missing params".to_string()),
        )?)
        .map_err(|e| {
            InvalidParams(format!(
                "ru: Невалидные параметры tools/call: {e}, en: Invalid tools/call parameters: {e}"
            ))
        })?;

        match params.name.as_str() {
            "search_1c_docs" => {
                let query = params
                    .arguments
                    .get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        InvalidParams(
                            "ru: Отсутствует query, en: Missing query parameter".to_string(),
                        )
                    })?;

                let kb = self.knowledge_base.read().await;
                let version = match resolve_version(&kb, &params.arguments, &self.progress) {
                    Ok(version) => version,
                    Err(message) => return Ok(version_message(message)),
                };

                let context = params
                    .arguments
                    .get("context")
                    .and_then(|v| v.as_str())
                    .and_then(parse_context);

                let include_related = params
                    .arguments
                    .get("include_related")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                // Объём контекста определяет тот, кто платит за токены промпта, —
                // хост. Конфиг сервера остаётся дефолтом на случай, когда хост
                // ничего не просил; потолок 500 — общий с conf1c.
                let max_results = params
                    .arguments
                    .get("max_results")
                    .and_then(|v| v.as_u64())
                    .map(|v| (v as usize).clamp(1, MAX_SEARCH_RESULTS))
                    .unwrap_or(self.max_results);

                let results = kb.search(&SearchQuery {
                    query,
                    version: Some(&version),
                    context,
                    include_related,
                    max_results,
                });

                let content = if results.is_empty() {
                    vec![Content::Text {
                        text: format!(
                            "ru: Результаты не найдены для запроса '{}', en: No results found for query '{}'",
                            query, query
                        ),
                    }]
                } else {
                    let text = self.format_search_results(&results);
                    vec![Content::Text { text }]
                };

                Ok(serde_json::to_value(ToolCallResult {
                    content,
                    is_error: Some(false),
                    result_count: Some(results.len()),
                })?)
            }
            "get_method_details" => {
                let method_name = params
                    .arguments
                    .get("method_name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        InvalidParams(
                            "ru: Отсутствует method_name, en: Missing method_name parameter"
                                .to_string(),
                        )
                    })?;

                let kb = self.knowledge_base.read().await;
                let version = match resolve_version(&kb, &params.arguments, &self.progress) {
                    Ok(version) => version,
                    Err(message) => return Ok(version_message(message)),
                };

                let include_examples = params
                    .arguments
                    .get("include_examples")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                let details = kb.get_method_details(method_name, Some(&version));

                let text = match details {
                    Some(method) => format_method_details(method, &version, include_examples),
                    None => format!(
                        "ru: Метод '{}' не найден в версии {}, en: Method '{}' not found in version {}",
                        method_name, version, method_name, version
                    ),
                };

                Ok(serde_json::to_value(ToolCallResult {
                    content: vec![Content::Text { text }],
                    is_error: Some(details.is_none()),
                    // Карточка одна: нашлась — единица, нет — ноль.
                    result_count: Some(details.is_some() as usize),
                })?)
            }
            "find_usage_examples" => {
                let entity = params
                    .arguments
                    .get("entity_name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        InvalidParams(
                            "ru: Отсутствует entity_name, en: Missing entity_name parameter"
                                .to_string(),
                        )
                    })?;

                let max_examples = params
                    .arguments
                    .get("max_examples")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(5)
                    .clamp(1, 20) as usize;

                let kb = self.knowledge_base.read().await;
                let version = match resolve_version(&kb, &params.arguments, &self.progress) {
                    Ok(version) => version,
                    Err(message) => return Ok(version_message(message)),
                };

                let examples = kb.find_usage_examples(entity, Some(&version), max_examples);

                let text = if examples.is_empty() {
                    format!(
                        "ru: Примеры использования '{}' не найдены в версии {}, en: No usage examples found for '{}' in version {}",
                        entity, version, entity, version
                    )
                } else {
                    self.format_usage_examples(entity, &examples)
                };

                Ok(serde_json::to_value(ToolCallResult {
                    content: vec![Content::Text { text }],
                    is_error: Some(false),
                    result_count: Some(examples.len()),
                })?)
            }
            "list_versions" => {
                let kb = self.knowledge_base.read().await;
                let text = format_versions(&kb, &self.progress);
                let loaded = kb.versions().len();

                Ok(serde_json::to_value(ToolCallResult {
                    content: vec![Content::Text { text }],
                    is_error: Some(false),
                    result_count: Some(loaded),
                })?)
            }
            mcp_common::SERVER_INFO_TOOL => {
                // Сводка та же, что уходит в `serverInfo.data` на рукопожатии:
                // одну сборку по всем каналам должно быть видно одинаково.
                let kb = self.knowledge_base.read().await;
                let text = mcp_common::server_info_text(
                    &self.server_info.name,
                    &self.server_info.version,
                    Some(&knowledge_summary(&kb)),
                );

                Ok(serde_json::to_value(ToolCallResult {
                    content: vec![Content::Text { text }],
                    is_error: Some(false),
                    // Ответ о самом сервере — не выдача: счёт неприменим.
                    result_count: None,
                })?)
            }
            // По спеке MCP неизвестный инструмент — Invalid params (−32602),
            // а не внутренняя ошибка сервера.
            _ => Err(InvalidParams(format!(
                "ru: Неизвестный инструмент: {}, en: Unknown tool: {}",
                params.name, params.name
            ))
            .into()),
        }
    }

    async fn handle_prompts_list(&self) -> Result<serde_json::Value> {
        let engine = self.prompts_engine.read().await;
        let prompts = engine.list_prompts();

        let mcp_prompts: Vec<Prompt> = prompts
            .iter()
            .map(|p| Prompt {
                name: p.name.clone(),
                description: Some(p.description.clone()),
                arguments: Some(
                    p.arguments
                        .iter()
                        .map(|a| PromptArgument {
                            name: a.name.clone(),
                            description: Some(a.description.clone()),
                            required: a.required,
                        })
                        .collect(),
                ),
            })
            .collect();

        Ok(json!({ "prompts": mcp_prompts }))
    }

    async fn handle_prompts_get(&self, request: &Request) -> Result<serde_json::Value> {
        let params: GetPromptParams =
            serde_json::from_value(request.params.clone().ok_or_else(|| {
                InvalidParams("ru: Отсутствуют параметры, en: Missing params".to_string())
            })?)
            .map_err(|e| {
                InvalidParams(format!(
                    "ru: Невалидные параметры prompts/get: {e}, en: Invalid prompts/get parameters: {e}"
                ))
            })?;

        // Неизвестное имя промпта — ошибка параметров клиента, не сбой сервера.
        {
            let engine = self.prompts_engine.read().await;
            if engine.get_prompt(&params.name).is_none() {
                return Err(InvalidParams(format!(
                    "ru: Промпт '{}' не найден, en: Prompt '{}' not found",
                    params.name, params.name
                ))
                .into());
            }
        }

        let mut engine = self.prompts_engine.write().await;
        let rendered = engine.render_prompt(&params.name, &params.arguments)?;

        let result = GetPromptResult {
            description: Some(format!("Промпт: {}", params.name)),
            messages: vec![PromptMessage {
                role: "user".to_string(),
                content: Content::Text { text: rendered },
            }],
        };

        Ok(serde_json::to_value(result)?)
    }

    fn format_search_results(&self, results: &[crate::knowledge::schema::SearchResult]) -> String {
        let mut output = String::new();

        for (i, result) in results.iter().enumerate() {
            output.push_str(&format!(
                "\n## Результат {} (score: {:.2})\n\n",
                i + 1,
                result.score
            ));

            match &result.source {
                crate::knowledge::schema::SearchSource::Section {
                    version,
                    title,
                    content,
                    ..
                } => {
                    output.push_str(&format!("**Версия:** {}\n", version));
                    output.push_str(&format!("**Заголовок:** {}\n\n", title));
                    output.push_str(&format!("{}\n", content));
                }
                crate::knowledge::schema::SearchSource::ApiMethod {
                    version,
                    name,
                    description,
                    context,
                } => {
                    output.push_str(&format!("**Метод:** {}\n", name));
                    output.push_str(&format!("**Версия:** {}\n", version));
                    output.push_str(&format!("**Контекст:** {}\n", context_label(context)));
                    output.push_str(&format!("\n{}\n", description));
                }
                crate::knowledge::schema::SearchSource::ApiObject {
                    version,
                    name,
                    description,
                } => {
                    output.push_str(&format!("**Объект:** {}\n", name));
                    output.push_str(&format!("**Версия:** {}\n", version));
                    output.push_str(&format!("\n{}\n", description));
                }
            }

            if !result.highlights.is_empty() {
                output.push_str("\n**Фрагменты:**\n");
                for highlight in &result.highlights {
                    output.push_str(&format!("- {}\n", highlight));
                }
            }

            if let Some(related) = &result.related {
                output.push_str("\n**Связанные:**\n");
                for item in related {
                    output.push_str(&format!("- {} — {}", item.relationship, item.name));
                    if !item.description.is_empty() {
                        output.push_str(&format!(" ({})", item.description));
                    }
                    output.push('\n');
                }
            }

            output.push_str("\n---\n");
        }

        output
    }

    #[cfg(test)]
    fn for_tests() -> Self {
        Self::with_max_results(
            Arc::new(RwLock::new(KnowledgeBase::new())),
            Arc::new(RwLock::new(PromptsEngine::new())),
            Arc::new(Progress::new()),
            10,
        )
    }

    fn format_usage_examples(
        &self,
        entity: &str,
        examples: &[crate::knowledge::UsageExample],
    ) -> String {
        let mut output = format!("# Примеры использования: {}\n", entity);

        for (i, example) in examples.iter().enumerate() {
            output.push_str(&format!("\n## Пример {}\n\n", i + 1));
            output.push_str(&format!("**Источник:** {}\n", example.entity));
            output.push_str(&format!("**Версия:** {}\n", example.version));
            if let Some(description) = &example.description {
                output.push_str(&format!("\n{}\n", description));
            }
            output.push_str(&format!("\n```bsl\n{}\n```\n", example.code));
        }

        output
    }
}

/// Разрешение параметра `version` в конкретную загруженную версию.
///
/// `Err` — готовый текст для клиента. Отсутствующая версия обязана отличаться от
/// пустой выдачи: иначе модель делает вывод «такого метода в платформе нет»,
/// хотя сервер просто не знает этой версии.
fn resolve_version(
    kb: &KnowledgeBase,
    arguments: &serde_json::Value,
    progress: &Progress,
) -> Result<String, String> {
    let versions = kb.versions();

    let Some(latest) = versions.first() else {
        return Err(empty_base_message(progress));
    };

    match arguments.get("version").and_then(|v| v.as_str()) {
        None | Some("latest") => Ok(latest.clone()),
        Some(version) if kb.has_version(version) => Ok(version.to_string()),
        Some(version) => Err(format!(
            "Версия {} в базе знаний отсутствует. Загружены: {}.",
            version,
            versions.join(", ")
        )),
    }
}

/// Почему база пуста — с прогрессом самосборки, если она идёт.
///
/// Мягкий отказ, а не ошибка контура: «идёт индексация, 40%» модель понимает как
/// «повтори позже», а `-32603` клиент читает как сбой сервера и уходит в
/// перезапуски. Отсутствие платформы точно так же объясняется текстом с
/// инструкцией, а не пустой выдачей.
fn empty_base_message(progress: &Progress) -> String {
    match progress.hint() {
        Some(hint) => hint,
        None => "База знаний пуста: не загружено ни одной версии документации.".to_string(),
    }
}

/// Сообщение о версии — не отказ инструмента: `isError` остаётся `false`,
/// клиенту важен текст, а не код возврата.
fn version_message(text: String) -> serde_json::Value {
    json!(ToolCallResult {
        content: vec![Content::Text { text }],
        is_error: Some(false),
        // Запрошенной версии в базе нет — контекста для хоста ноль, что бы там
        // ни было написано в тексте для модели.
        result_count: Some(0),
    })
}

/// Контекст исполнения из параметра инструмента.
fn parse_context(value: &str) -> Option<ExecutionContext> {
    match value {
        "client" => Some(ExecutionContext::Client),
        "server" => Some(ExecutionContext::Server),
        "both" => Some(ExecutionContext::Both),
        _ => None,
    }
}

/// Контекст исполнения по-русски.
fn context_label(context: &ExecutionContext) -> &'static str {
    match context {
        ExecutionContext::Client => "клиент",
        ExecutionContext::Server => "сервер",
        ExecutionContext::Both => "клиент и сервер",
    }
}

/// Карточка метода целиком: параметры, возвращаемое значение, примеры и
/// примечания.
///
/// Раньше сюда уходил `Debug` внутренних типов (`Parameter { name: … }`), а
/// возвращаемое значение, примеры и примечания не показывались вовсе — при том
/// что в базе знаний они есть.
fn format_method_details(method: &ApiMethod, version: &str, include_examples: bool) -> String {
    let mut output = format!("# Метод: {}\n\n{}\n\n", method.name, method.description);

    output.push_str(&format!("**Версия:** {}\n", version));
    output.push_str(&format!(
        "**Контекст:** {}\n",
        context_label(&method.context)
    ));
    if let Some(return_type) = &method.return_type {
        output.push_str(&format!("**Возвращает:** `{}`\n", return_type));
    }

    // Пустые разделы не печатаем: они читаются как «данных нет в природе».
    if !method.parameters.is_empty() {
        output.push_str("\n## Параметры\n\n");
        for parameter in &method.parameters {
            output.push_str(&format!(
                "- **{}** (`{}`, {}) — {}\n",
                parameter.name,
                parameter.param_type,
                if parameter.required {
                    "обязательный"
                } else {
                    "необязательный"
                },
                parameter.description
            ));
        }
    }

    if include_examples && !method.examples.is_empty() {
        output.push_str("\n## Примеры\n");
        for example in &method.examples {
            if let Some(title) = &example.title {
                output.push_str(&format!("\n**{}**\n", title));
            }
            output.push_str(&format!("\n```bsl\n{}\n```\n", example.code));
            if let Some(description) = &example.description {
                output.push_str(&format!("\n{}\n", description));
            }
        }
    }

    if !method.notes.is_empty() {
        output.push_str("\n## Примечания\n\n");
        for note in &method.notes {
            output.push_str(&format!("- {}\n", note));
        }
    }

    output
}

/// Однострочная сводка о базе — для `serverInfo.data` и стартовой записи.
///
/// Пустая база должна быть видна хосту на рукопожатии, а не выводиться из
/// качества ответов модели.
pub fn knowledge_summary(kb: &KnowledgeBase) -> String {
    let versions = kb.versions();
    if versions.is_empty() {
        return "база знаний пуста".to_string();
    }

    versions
        .iter()
        .map(|version| {
            let stats = kb.version_statistics(version);
            format!(
                "{}: {} методов, {} объектов",
                version, stats.api_methods, stats.api_objects
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Состав базы знаний: без него клиент узнаёт о загруженных версиях только
/// методом проб — по пустой выдаче.
fn format_versions(kb: &KnowledgeBase, progress: &Progress) -> String {
    let versions = kb.versions();

    if versions.is_empty() {
        return empty_base_message(progress);
    }

    let mut output = format!("# Загруженные версии ({})\n\n", versions.len());
    for (i, version) in versions.iter().enumerate() {
        let stats = kb.version_statistics(version);
        output.push_str(&format!(
            "- **{}**{} — методов: {}, объектов: {}\n",
            version,
            if i == 0 { " (latest)" } else { "" },
            stats.api_methods,
            stats.api_objects
        ));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::schema::{Document, Example, Parameter};

    /// Документация одной версии с одним методом.
    fn doc(version: &str, method: ApiMethod) -> Document {
        Document {
            version: version.to_string(),
            platform: "1С:Предприятие".to_string(),
            release_date: None,
            sections: vec![],
            api_methods: vec![method],
            api_objects: vec![],
            relationships: vec![],
        }
    }

    fn method_with_everything() -> ApiMethod {
        ApiMethod {
            name: "ЗначениеЗаполнено".to_string(),
            description: "Проверяет заполненность значения.".to_string(),
            context: ExecutionContext::Both,
            parameters: vec![Parameter {
                name: "Значение".to_string(),
                param_type: "Произвольный".to_string(),
                required: true,
                description: "Значение для проверки.".to_string(),
            }],
            return_type: Some("Булево".to_string()),
            examples: vec![Example {
                title: None,
                code: "Если ЗначениеЗаполнено(Товар) Тогда".to_string(),
                description: None,
            }],
            notes: vec!["Для неопределённых типов возвращает Ложь.".to_string()],
        }
    }

    fn bare_method() -> ApiMethod {
        ApiMethod {
            name: "ТекущаяДата".to_string(),
            description: "Возвращает дату сеанса.".to_string(),
            context: ExecutionContext::Server,
            parameters: vec![],
            return_type: None,
            examples: vec![],
            notes: vec![],
        }
    }

    /// Обработчик с наполненной базой знаний.
    fn handler_with(docs: Vec<Document>) -> ProtocolHandler {
        let mut kb = KnowledgeBase::new();
        for doc in docs {
            kb.add_document(doc).unwrap();
        }

        ProtocolHandler::with_max_results(
            Arc::new(RwLock::new(kb)),
            Arc::new(RwLock::new(PromptsEngine::new())),
            Arc::new(Progress::new()),
            10,
        )
    }

    /// Текст ответа инструмента.
    async fn call(handler: &ProtocolHandler, name: &str, arguments: serde_json::Value) -> String {
        let request = Request {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({ "name": name, "arguments": arguments })),
        };

        let response = handler.handle_request(request).await;
        let result = response.result.expect("инструмент должен отвечать успехом");
        result["content"][0]["text"]
            .as_str()
            .expect("текстовый ответ")
            .to_string()
    }

    /// Регрессия: отсутствующая версия давала пустую выдачу, неотличимую от
    /// «такого метода нет». Для конфигурации в режиме совместимости это
    /// ложноотрицательный ответ по языку платформы.
    #[tokio::test]
    async fn missing_version_is_reported_not_silently_empty() {
        let handler = handler_with(vec![doc("8.3.27", method_with_everything())]);

        let text = call(
            &handler,
            "search_1c_docs",
            json!({ "query": "ЗначениеЗаполнено", "version": "8.3.14" }),
        )
        .await;

        assert!(text.contains("8.3.14"), "нет запрошенной версии: {}", text);
        assert!(text.contains("8.3.27"), "нет состава базы: {}", text);
        assert!(
            !text.contains("не найдены") && !text.contains("Результаты не найдены"),
            "отсутствующая версия выдана за отсутствие результата: {}",
            text
        );
    }

    #[tokio::test]
    async fn list_versions_reports_loaded_versions() {
        let handler = handler_with(vec![
            doc("8.3.9", bare_method()),
            doc("8.3.27", method_with_everything()),
        ]);

        let text = call(&handler, "list_versions", json!({})).await;

        assert!(text.contains("8.3.27"), "{}", text);
        assert!(text.contains("8.3.9"), "{}", text);
        assert!(
            text.contains("методов: 1"),
            "нет наполнения версии: {}",
            text
        );
        // Старшая версия помечена как latest — иначе смысл значения по умолчанию
        // клиенту неоткуда узнать.
        let latest_line = text.lines().find(|l| l.contains("(latest)")).unwrap_or("");
        assert!(latest_line.contains("8.3.27"), "latest не та: {}", text);
    }

    /// Полный `result` инструмента: текста мало, когда проверяем `resultCount`.
    async fn call_raw(
        handler: &ProtocolHandler,
        name: &str,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        let request = Request {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({ "name": name, "arguments": arguments })),
        };

        handler
            .handle_request(request)
            .await
            .result
            .expect("инструмент должен отвечать успехом")
    }

    /// База знаний из нескольких методов — для проверки потолков выдачи.
    fn handler_with_methods(count: usize) -> ProtocolHandler {
        let mut kb = KnowledgeBase::new();
        kb.add_document(Document {
            version: "8.3.27".to_string(),
            platform: "1С:Предприятие".to_string(),
            release_date: None,
            sections: vec![],
            api_methods: (0..count)
                .map(|i| ApiMethod {
                    name: format!("Сообщить{i}"),
                    description: "Выводит сообщение пользователю.".to_string(),
                    context: ExecutionContext::Both,
                    parameters: vec![],
                    return_type: None,
                    examples: vec![],
                    notes: vec![],
                })
                .collect(),
            api_objects: vec![],
            relationships: vec![],
        })
        .unwrap();

        ProtocolHandler::with_max_results(
            Arc::new(RwLock::new(kb)),
            Arc::new(RwLock::new(PromptsEngine::new())),
            Arc::new(Progress::new()),
            10,
        )
    }

    /// Объём контекста определяет тот, кто платит за токены промпта, — хост.
    /// Конфиг сервера остаётся дефолтом, аргумент его перекрывает.
    #[tokio::test]
    async fn search_max_results_argument_overrides_config() {
        // Arrange
        let handler = handler_with_methods(5);

        // Act
        let unbounded = call_raw(&handler, "search_1c_docs", json!({ "query": "сообщение" })).await;
        let bounded = call_raw(
            &handler,
            "search_1c_docs",
            json!({ "query": "сообщение", "max_results": 2 }),
        )
        .await;

        // Assert
        assert!(
            unbounded["resultCount"].as_u64().unwrap() >= 3,
            "потолок конфига (10) урезал выдачу: {unbounded}"
        );
        assert_eq!(bounded["resultCount"], 2, "{bounded}");
    }

    /// Пустая выдача — не отказ, но хост обязан отличать её числом, а не
    /// разбором текста «не найдено».
    #[tokio::test]
    async fn empty_search_reports_zero_result_count() {
        // Arrange
        let handler = handler_with_methods(3);

        // Act
        let result = call_raw(
            &handler,
            "search_1c_docs",
            json!({ "query": "ЗаведомоНетТакогоМетода12345" }),
        )
        .await;

        // Assert
        assert_eq!(result["isError"], false, "{result}");
        assert_eq!(result["resultCount"], 0, "{result}");
    }

    /// Версия сборки должна доезжать до потребителя инструментами: в рукопожатие
    /// он не заглядывает, а `--version` ему негде выполнить.
    #[tokio::test]
    async fn server_info_reports_build_stamp_and_knowledge() {
        // Arrange
        let handler = handler_with(vec![doc("8.3.27", method_with_everything())]);

        // Act
        let text = call(&handler, "server_info", json!({})).await;

        // Assert
        assert!(text.contains(env!("CARGO_PKG_NAME")), "нет имени: {}", text);
        assert!(
            text.contains(env!("CARGO_PKG_VERSION")),
            "нет версии каталога: {}",
            text
        );
        assert!(
            text.contains("8.3.27"),
            "нет сводки о базе знаний: {}",
            text
        );
    }

    /// Регрессия: наружу уходило `Debug`-представление внутренних типов —
    /// `Parameter { name: "Значение", … }` и `Both`.
    #[tokio::test]
    async fn method_details_are_rendered_as_markdown() {
        let handler = handler_with(vec![doc("8.3.27", method_with_everything())]);

        let text = call(
            &handler,
            "get_method_details",
            json!({ "method_name": "ЗначениеЗаполнено" }),
        )
        .await;

        assert!(!text.contains("Parameter {"), "Debug в выдаче: {}", text);
        assert!(!text.contains("Both"), "Debug контекста в выдаче: {}", text);
        assert!(
            text.contains("**Значение** (`Произвольный`, обязательный)"),
            "{}",
            text
        );
        assert!(text.contains("клиент и сервер"), "{}", text);
        // Возвращаемое значение, примеры и примечания раньше терялись целиком.
        assert!(
            text.contains("Булево"),
            "нет возвращаемого значения: {}",
            text
        );
        assert!(text.contains("```bsl"), "нет примеров: {}", text);
        assert!(text.contains("Примечания"), "нет примечаний: {}", text);
    }

    #[tokio::test]
    async fn method_details_omit_empty_sections() {
        let handler = handler_with(vec![doc("8.3.27", bare_method())]);

        let text = call(
            &handler,
            "get_method_details",
            json!({ "method_name": "ТекущаяДата" }),
        )
        .await;

        assert!(!text.contains("## Параметры"), "пустой раздел: {}", text);
        assert!(!text.contains("## Примеры"), "пустой раздел: {}", text);
        assert!(!text.contains("## Примечания"), "пустой раздел: {}", text);
    }

    /// Параметр объявлен в схеме — значит, обязан работать.
    #[tokio::test]
    async fn include_examples_false_drops_examples() {
        let handler = handler_with(vec![doc("8.3.27", method_with_everything())]);

        let text = call(
            &handler,
            "get_method_details",
            json!({ "method_name": "ЗначениеЗаполнено", "include_examples": false }),
        )
        .await;

        assert!(!text.contains("## Примеры"), "{}", text);
        assert!(text.contains("## Параметры"), "карточка обрезана: {}", text);
    }

    /// Регрессия: `find_usage_examples` объявлялся в tools/list, но в диспетчере
    /// отсутствовал — вызов падал с «Неизвестный tool». Тест ловит любой такой
    /// рассинхрон между декларацией и реализацией.
    #[tokio::test]
    async fn every_declared_tool_is_dispatched() {
        let handler = ProtocolHandler::for_tests();

        for tool in capabilities::get_tools() {
            // Аргументы намеренно пустые: интересует не результат, а то,
            // что метод вообще известен диспетчеру.
            let request = Request {
                jsonrpc: "2.0".to_string(),
                id: Some(json!(1)),
                method: "tools/call".to_string(),
                params: Some(json!({ "name": tool.name, "arguments": {} })),
            };

            let response = handler.handle_request(request).await;
            if let Some(error) = response.error {
                assert!(
                    !error.message.contains("Неизвестный инструмент"),
                    "инструмент {} объявлен, но не реализован",
                    tool.name
                );
            }
        }
    }

    /// Спецификация MCP: ping обязан получать пустой result, а не -32601.
    #[tokio::test]
    async fn ping_returns_empty_result() {
        let handler = ProtocolHandler::for_tests();

        let request = Request {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "ping".to_string(),
            params: None,
        };

        let response = handler.handle_request(request).await;
        assert!(response.error.is_none());
        assert_eq!(response.result.unwrap(), json!({}));
    }

    /// Нотификация клиента — не «неизвестный метод» и не повод для warn-ошибки.
    #[tokio::test]
    async fn notification_is_not_an_error() {
        let handler = ProtocolHandler::for_tests();

        let request = Request {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: "notifications/initialized".to_string(),
            params: None,
        };

        let response = handler.handle_request(request).await;
        assert!(response.error.is_none());
    }

    /// Регрессия: ошибки параметров уходили как -32603 (internal error), и клиент
    /// перезапускал «сбойный» сервер вместо исправления собственного запроса.
    #[tokio::test]
    async fn missing_tool_argument_is_invalid_params() {
        let handler = ProtocolHandler::for_tests();

        let request = Request {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({ "name": "search_1c_docs", "arguments": {} })),
        };

        let response = handler.handle_request(request).await;
        assert_eq!(response.error.expect("ожидалась ошибка").code, -32602);
    }

    #[tokio::test]
    async fn unknown_tool_is_invalid_params() {
        let handler = ProtocolHandler::for_tests();

        let request = Request {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({ "name": "no_such_tool", "arguments": {} })),
        };

        let response = handler.handle_request(request).await;
        assert_eq!(response.error.expect("ожидалась ошибка").code, -32602);
    }

    #[tokio::test]
    async fn unknown_prompt_is_invalid_params() {
        let handler = ProtocolHandler::for_tests();

        let request = Request {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "prompts/get".to_string(),
            params: Some(json!({ "name": "no_such_prompt" })),
        };

        let response = handler.handle_request(request).await;
        assert_eq!(response.error.expect("ожидалась ошибка").code, -32602);
    }

    /// Регрессия: неизвестный метод уходил как −32603 (internal error), и клиент
    /// читал это как сбой сервера — повод для перезапуска вместо «метода нет».
    #[tokio::test]
    async fn unknown_method_reports_method_not_found() {
        let handler = ProtocolHandler::for_tests();

        let request = Request {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "totally/unknown".to_string(),
            params: None,
        };

        let response = handler.handle_request(request).await;
        assert_eq!(response.error.expect("ожидалась ошибка").code, -32601);
    }

    /// Сервер tools-only: capability `resources` не объявлена, значит и метод
    /// не обслуживается — отказ должен быть «метода нет», а не сбоем сервера.
    #[tokio::test]
    async fn resources_list_is_not_served() {
        let handler = ProtocolHandler::for_tests();

        let request = Request {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "resources/list".to_string(),
            params: None,
        };

        let response = handler.handle_request(request).await;
        assert_eq!(response.error.expect("ожидалась ошибка").code, -32601);
    }

    #[tokio::test]
    async fn usage_examples_report_missing_entity() {
        // База непустая: иначе ответ был бы о составе базы, а проверяем мы
        // отсутствие самой сущности.
        let handler = handler_with(vec![doc("8.3.27", bare_method())]);

        let text = call(
            &handler,
            "find_usage_examples",
            json!({ "entity_name": "ТаблицаЗначений" }),
        )
        .await;

        assert!(text.contains("не найдены"), "неожиданный ответ: {}", text);
    }

    /// Пустая база — тоже объяснимое состояние, а не «ничего не нашлось»:
    /// в поставке лежат демо-данные, и промах по версии здесь особенно вероятен.
    #[tokio::test]
    async fn empty_knowledge_base_is_reported() {
        let handler = ProtocolHandler::for_tests();

        let text = call(&handler, "search_1c_docs", json!({ "query": "СтрНайти" })).await;

        assert!(text.contains("пуста"), "неожиданный ответ: {}", text);
    }
}
