pub mod index;
pub mod loader;
pub mod schema;

use self::index::InvertedIndex;
use self::loader::Loader;
use self::schema::*;
use anyhow::Result;
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;
use std::path::Path;

/// База знаний с поддержкой поиска и графа связей
pub struct KnowledgeBase {
    documents: HashMap<String, Document>,
    search_index: InvertedIndex,
    relationship_graph: DiGraph<String, String>,
    node_map: HashMap<String, NodeIndex>,
}

impl KnowledgeBase {
    pub fn new() -> Self {
        Self::with_boosts(3.0, 2.0)
    }

    /// База с весами ранжирования из конфига (`[search] boost_*`).
    pub fn with_boosts(boost_title: f32, boost_keywords: f32) -> Self {
        Self {
            documents: HashMap::new(),
            search_index: InvertedIndex::with_boosts(boost_title, boost_keywords),
            relationship_graph: DiGraph::new(),
            node_map: HashMap::new(),
        }
    }

    /// Загрузка документов из директории
    pub fn load_from_directory(&mut self, data_dir: impl AsRef<Path>) -> Result<()> {
        let loader = Loader::new(data_dir);
        let documents = loader.load_all()?;

        for doc in documents {
            self.add_document(doc)?;
        }

        Ok(())
    }

    /// Добавление документа в базу
    pub fn add_document(&mut self, doc: Document) -> Result<()> {
        let version = doc.version.clone();

        // Индексация для поиска
        self.search_index.index_document(&doc);

        // Построение графа связей
        self.build_relationship_graph(&doc);

        self.documents.insert(version.clone(), doc);

        tracing::info!(
            "ru: Документ {} добавлен в базу знаний, en: Document {} added to knowledge base",
            version,
            version
        );

        Ok(())
    }

    /// Построение графа связей из документа
    fn build_relationship_graph(&mut self, doc: &Document) {
        for rel in &doc.relationships {
            let from_node = self.get_or_create_node(&rel.from);
            let to_node = self.get_or_create_node(&rel.to);
            self.relationship_graph
                .add_edge(from_node, to_node, format!("{:?}", rel.rel_type));
        }
    }

    fn get_or_create_node(&mut self, name: &str) -> NodeIndex {
        if let Some(&idx) = self.node_map.get(name) {
            idx
        } else {
            let idx = self.relationship_graph.add_node(name.to_string());
            self.node_map.insert(name.to_string(), idx);
            idx
        }
    }

    /// Загруженные версии, от старшей к младшей.
    pub fn versions(&self) -> Vec<String> {
        let mut versions: Vec<String> = self.documents.keys().cloned().collect();
        versions.sort_by_key(|version| std::cmp::Reverse(version_key(version)));
        versions
    }

    /// Есть ли версия в базе.
    pub fn has_version(&self, version: &str) -> bool {
        self.documents.contains_key(version)
    }

    /// Старшая загруженная версия — значение параметра `latest`.
    pub fn latest_version(&self) -> Option<String> {
        self.versions().into_iter().next()
    }

    /// Поиск по базе знаний
    ///
    /// Версия в запросе — конкретная: `latest` разрешает вызывающая сторона,
    /// иначе «старшая» и «любая» неотличимы в самом поиске.
    pub fn search(&self, query: &SearchQuery<'_>) -> Vec<SearchResult> {
        let search_results = self.search_index.search(query.query, query.max_results * 2);

        let mut results = Vec::new();

        for (doc_id, section_id, score) in search_results {
            // Фильтрация по версии
            if let Some(v) = query.version {
                if doc_id != v {
                    continue;
                }
            }

            if let Some(doc) = self.documents.get(&doc_id) {
                if let Some(mut result) = self.create_search_result(doc, &section_id, score) {
                    if !matches_context(&result.source, query.context.as_ref()) {
                        continue;
                    }
                    if query.include_related {
                        result.related = self.related_of(doc, &result.source);
                    }

                    results.push(result);
                    if results.len() >= query.max_results {
                        break;
                    }
                }
            }
        }

        results
    }

    /// Связи сущности из того же документа: имя, вид связи и пояснение.
    fn related_of(&self, doc: &Document, source: &SearchSource) -> Option<Vec<RelatedItem>> {
        let name = match source {
            SearchSource::ApiMethod { name, .. } | SearchSource::ApiObject { name, .. } => name,
            // У раздела документации имени сущности нет — связывать нечего.
            SearchSource::Section { .. } => return None,
        };

        let related: Vec<RelatedItem> = doc
            .relationships
            .iter()
            .filter(|rel| rel.from == *name)
            .map(|rel| RelatedItem {
                name: rel.to.clone(),
                relationship: relationship_label(&rel.rel_type).to_string(),
                description: rel.description.clone().unwrap_or_default(),
            })
            .collect();

        (!related.is_empty()).then_some(related)
    }

    fn create_search_result(
        &self,
        doc: &Document,
        section_id: &str,
        score: f32,
    ) -> Option<SearchResult> {
        // Поиск в секциях
        if let Some(section) = self.find_section(&doc.sections, section_id) {
            return Some(SearchResult {
                score,
                source: SearchSource::Section {
                    version: doc.version.clone(),
                    section_id: section.id.clone(),
                    title: section.title.clone(),
                    content: section.content.clone(),
                },
                highlights: vec![],
                related: None,
            });
        }

        // Поиск в API методах
        if let Some(method_name) = section_id.strip_prefix("method_") {
            if let Some(method) = doc.api_methods.iter().find(|m| m.name == method_name) {
                return Some(SearchResult {
                    score,
                    source: SearchSource::ApiMethod {
                        version: doc.version.clone(),
                        name: method.name.clone(),
                        description: method.description.clone(),
                        context: method.context.clone(),
                    },
                    highlights: vec![],
                    related: None,
                });
            }
        }

        // Поиск в API объектах
        if let Some(obj_name) = section_id.strip_prefix("object_") {
            if let Some(obj) = doc.api_objects.iter().find(|o| o.name == obj_name) {
                return Some(SearchResult {
                    score,
                    source: SearchSource::ApiObject {
                        version: doc.version.clone(),
                        name: obj.name.clone(),
                        description: obj.description.clone(),
                    },
                    highlights: vec![],
                    related: None,
                });
            }
        }

        None
    }

    fn find_section<'a>(&self, sections: &'a [Section], id: &str) -> Option<&'a Section> {
        for section in sections {
            if section.id == id {
                return Some(section);
            }
            if let Some(found) = self.find_section(&section.subsections, id) {
                return Some(found);
            }
        }
        None
    }

    /// Метод API из документации конкретной версии.
    ///
    /// Без версии берётся старшая загруженная: обход `HashMap` документов давал
    /// бы при нескольких версиях недетерминированный ответ.
    pub fn get_method_details(
        &self,
        method_name: &str,
        version: Option<&str>,
    ) -> Option<&ApiMethod> {
        let version = match version {
            Some(v) => v.to_string(),
            None => self.latest_version()?,
        };

        self.documents
            .get(&version)?
            .api_methods
            .iter()
            .find(|m| m.name == method_name)
    }

    /// Примеры использования объекта или метода API.
    ///
    /// Сначала идут примеры самой сущности (`ТаблицаЗначений` — это и
    /// `ТаблицаЗначений.Добавить`, и `Новый ТаблицаЗначений`), затем — примеры
    /// других методов, в коде которых сущность упоминается.
    pub fn find_usage_examples(
        &self,
        entity_name: &str,
        version: Option<&str>,
        max_examples: usize,
    ) -> Vec<UsageExample> {
        let entity = entity_name.trim();
        let own_prefix = format!("{}.", entity);
        let ctor = format!("Новый {}", entity);
        let ctor_dash = format!("{} — ", ctor);

        let mut own = Vec::new();
        let mut mentions = Vec::new();

        for doc in self.documents.values() {
            if let Some(v) = version {
                if doc.version != v {
                    continue;
                }
            }

            for method in &doc.api_methods {
                let is_own = method.name == entity
                    || method.name.starts_with(&own_prefix)
                    || method.name == ctor
                    || method.name.starts_with(&ctor_dash);

                for example in &method.examples {
                    let bucket = if is_own {
                        &mut own
                    } else if example.code.contains(entity) {
                        &mut mentions
                    } else {
                        continue;
                    };

                    bucket.push(UsageExample {
                        entity: method.name.clone(),
                        version: doc.version.clone(),
                        code: example.code.clone(),
                        description: example.description.clone(),
                    });
                }
            }
        }

        own.append(&mut mentions);
        own.truncate(max_examples);
        own
    }

    /// Наполнение одной версии — для перечня версий в `list_versions`.
    pub fn version_statistics(&self, version: &str) -> VersionStats {
        match self.documents.get(version) {
            Some(doc) => VersionStats {
                api_methods: doc.api_methods.len(),
                api_objects: doc.api_objects.len(),
            },
            None => VersionStats::default(),
        }
    }

    /// Статистика базы знаний
    pub fn statistics(&self) -> KnowledgeStats {
        KnowledgeStats {
            total_documents: self.documents.len(),
            total_api_methods: self.documents.values().map(|d| d.api_methods.len()).sum(),
            total_api_objects: self.documents.values().map(|d| d.api_objects.len()).sum(),
        }
    }
}

impl Default for KnowledgeBase {
    fn default() -> Self {
        Self::new()
    }
}

/// Запрос к базе знаний.
///
/// Собран структурой, а не пятью аргументами: `context` и `include_related`
/// объявлены в схеме инструмента и должны доезжать до поиска, а не теряться по
/// дороге, как это было до тикета 2026-08-06.
pub struct SearchQuery<'a> {
    pub query: &'a str,
    /// Конкретная версия документации; `None` — искать по всем загруженным.
    pub version: Option<&'a str>,
    /// Контекст исполнения: метод, недоступный в нём, в выдачу не идёт.
    pub context: Option<ExecutionContext>,
    pub include_related: bool,
    pub max_results: usize,
}

/// Ключ сортировки версии по числовым компонентам.
///
/// Строковое сравнение поставило бы `8.3.9` выше `8.3.27`, и `latest` показывал
/// бы документацию не той платформы.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// Доступен ли источник в запрошенном контексте исполнения.
///
/// Метод контекста `Both` доступен в любом. У разделов и объектов контекста
/// нет — их фильтр не касается.
fn matches_context(source: &SearchSource, requested: Option<&ExecutionContext>) -> bool {
    let Some(requested) = requested else {
        return true;
    };

    match source {
        SearchSource::ApiMethod { context, .. } => {
            context == requested || *context == ExecutionContext::Both
        }
        _ => true,
    }
}

/// Вид связи по-русски: `Debug`-имя варианта в выдачу не отдаём.
fn relationship_label(rel_type: &RelationshipType) -> &'static str {
    match rel_type {
        RelationshipType::Uses => "использует",
        RelationshipType::Returns => "возвращает",
        RelationshipType::Contains => "содержит",
        RelationshipType::Extends => "расширяет",
    }
}

/// Пример использования с указанием, чей он и из какой версии документации
#[derive(Debug, Clone)]
pub struct UsageExample {
    pub entity: String,
    pub version: String,
    pub code: String,
    pub description: Option<String>,
}

/// Наполнение одной версии документации.
#[derive(Debug, Default)]
pub struct VersionStats {
    pub api_methods: usize,
    pub api_objects: usize,
}

#[derive(Debug)]
pub struct KnowledgeStats {
    pub total_documents: usize,
    pub total_api_methods: usize,
    pub total_api_objects: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::schema::{ApiMethod, Document, Example, ExecutionContext};

    fn doc_with_method(version: &str, method_name: &str, code: &str) -> Document {
        Document {
            version: version.to_string(),
            platform: "1С".to_string(),
            release_date: None,
            sections: vec![],
            api_methods: vec![ApiMethod {
                name: method_name.to_string(),
                description: format!("Метод {}", method_name),
                context: ExecutionContext::Both,
                parameters: vec![],
                return_type: None,
                examples: vec![Example {
                    title: None,
                    code: code.to_string(),
                    description: None,
                }],
                notes: vec![],
            }],
            api_objects: vec![],
            relationships: vec![],
        }
    }

    /// Запрос без лишних параметров: в тесте проверяется одна вещь за раз.
    fn query<'a>(text: &'a str, version: Option<&'a str>) -> SearchQuery<'a> {
        SearchQuery {
            query: text,
            version,
            context: None,
            include_related: false,
            max_results: 10,
        }
    }

    #[test]
    fn search_filters_by_exact_version() {
        let mut kb = KnowledgeBase::new();
        kb.add_document(doc_with_method("8.3.25", "СтрНайти", "СтрНайти(х, у)"))
            .unwrap();
        kb.add_document(doc_with_method("8.3.26", "СтрНайти", "СтрНайти(х, у)"))
            .unwrap();

        let results = kb.search(&query("СтрНайти", Some("8.3.25")));

        assert!(!results.is_empty());
        for r in &results {
            match &r.source {
                schema::SearchSource::ApiMethod { version, .. } => assert_eq!(version, "8.3.25"),
                other => panic!("неожиданный источник: {:?}", other),
            }
        }
    }

    /// Регрессия: версии сравнивались строкой, и старшей вышла бы `8.3.9` —
    /// `latest` показывал бы документацию не той платформы.
    #[test]
    fn latest_version_compares_numerically() {
        let mut kb = KnowledgeBase::new();
        kb.add_document(doc_with_method("8.3.9", "СтрНайти", "СтрНайти(х, у)"))
            .unwrap();
        kb.add_document(doc_with_method("8.3.27", "СтрНайти", "СтрНайти(х, у)"))
            .unwrap();

        assert_eq!(kb.latest_version(), Some("8.3.27".to_string()));
        assert_eq!(kb.versions(), vec!["8.3.27", "8.3.9"]);
    }

    /// Контекст исполнения объявлен в схеме инструмента и обязан фильтровать:
    /// серверный метод в клиентской выдаче — ложное разрешение конструкции.
    #[test]
    fn search_filters_by_execution_context() {
        let mut kb = KnowledgeBase::new();
        let mut doc = doc_with_method("8.3.27", "УстановитьПривилегированныйРежим", "код");
        doc.api_methods[0].context = ExecutionContext::Server;
        kb.add_document(doc).unwrap();

        let mut client = query("УстановитьПривилегированныйРежим", Some("8.3.27"));
        client.context = Some(ExecutionContext::Client);
        let mut server = query("УстановитьПривилегированныйРежим", Some("8.3.27"));
        server.context = Some(ExecutionContext::Server);

        assert!(
            kb.search(&client).is_empty(),
            "серверный метод попал в клиентскую выдачу"
        );
        assert!(!kb.search(&server).is_empty());
    }

    /// Регрессия: версия у `get_method_details` игнорировалась, и при двух
    /// загруженных версиях ответ зависел от порядка обхода `HashMap`.
    #[test]
    fn method_details_come_from_requested_version() {
        let mut kb = KnowledgeBase::new();
        let mut old = doc_with_method("8.3.25", "СтрНайти", "код");
        old.api_methods[0].description = "старое описание".to_string();
        kb.add_document(old).unwrap();
        kb.add_document(doc_with_method("8.3.27", "СтрНайти", "код"))
            .unwrap();

        let method = kb.get_method_details("СтрНайти", Some("8.3.25")).unwrap();

        assert_eq!(method.description, "старое описание");
    }

    /// Задокументированный порядок: сначала примеры самой сущности, затем упоминания.
    #[test]
    fn own_examples_come_before_mentions() {
        let mut kb = KnowledgeBase::new();
        let mut doc = doc_with_method("8.3.25", "ЧужойМетод", "х = ТаблицаЗначений.Скопировать()");
        doc.api_methods.push(ApiMethod {
            name: "ТаблицаЗначений.Добавить".to_string(),
            description: "Собственный метод".to_string(),
            context: ExecutionContext::Both,
            parameters: vec![],
            return_type: None,
            examples: vec![Example {
                title: None,
                code: "ТЗ.Добавить()".to_string(),
                description: None,
            }],
            notes: vec![],
        });
        kb.add_document(doc).unwrap();

        let examples = kb.find_usage_examples("ТаблицаЗначений", None, 10);
        assert_eq!(examples.len(), 2);
        assert_eq!(examples[0].entity, "ТаблицаЗначений.Добавить");
        assert_eq!(examples[1].entity, "ЧужойМетод");
    }
}
