use serde::{Deserialize, Serialize};

/// Корневой документ с документацией для конкретной версии
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub version: String,
    pub platform: String,
    pub release_date: Option<String>,
    pub sections: Vec<Section>,
    #[serde(default)]
    pub api_methods: Vec<ApiMethod>,
    #[serde(default)]
    pub api_objects: Vec<ApiObject>,
    #[serde(default)]
    pub relationships: Vec<Relationship>,
}

/// Раздел документации
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub section_type: SectionType,
    pub content: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub subsections: Vec<Section>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SectionType {
    Category,
    ApiReference,
    Text,
    QueryLanguage,
}

/// Метод API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiMethod {
    pub name: String,
    pub description: String,
    pub context: ExecutionContext,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    pub return_type: Option<String>,
    #[serde(default)]
    pub examples: Vec<Example>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionContext {
    Client,
    Server,
    Both,
}

/// Параметр метода
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: String,
    pub required: bool,
    pub description: String,
}

/// Пример кода
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Example {
    pub title: Option<String>,
    pub code: String,
    pub description: Option<String>,
}

/// API объект
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiObject {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub properties: Vec<Property>,
    #[serde(default)]
    pub methods: Vec<String>,
}

/// Свойство объекта
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Property {
    pub name: String,
    #[serde(rename = "type")]
    pub property_type: String,
    pub description: String,
    pub readonly: bool,
}

/// Связь между сущностями
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub rel_type: RelationshipType,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipType {
    Uses,
    Returns,
    Contains,
    Extends,
}

/// Результат поиска
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub score: f32,
    pub source: SearchSource,
    pub highlights: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related: Option<Vec<RelatedItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SearchSource {
    Section {
        version: String,
        section_id: String,
        title: String,
        content: String,
    },
    ApiMethod {
        version: String,
        name: String,
        description: String,
        context: ExecutionContext,
    },
    ApiObject {
        version: String,
        name: String,
        description: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedItem {
    pub name: String,
    pub relationship: String,
    pub description: String,
}

impl Document {
    /// Валидация документа
    pub fn validate(&self) -> Result<(), String> {
        if self.version.is_empty() {
            return Err("ru: Версия не может быть пустой, en: Version cannot be empty".to_string());
        }

        if self.platform.is_empty() {
            return Err(
                "ru: Платформа не может быть пустой, en: Platform cannot be empty".to_string(),
            );
        }

        // Проверка уникальности ID секций
        let mut section_ids = std::collections::HashSet::new();
        for section in &self.sections {
            if !section_ids.insert(&section.id) {
                return Err(format!(
                    "ru: Дублирующийся ID секции: {}, en: Duplicate section ID: {}",
                    section.id, section.id
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_validation() {
        let doc = Document {
            version: "8.3.25".to_string(),
            platform: "1С:Предприятие".to_string(),
            release_date: None,
            sections: vec![],
            api_methods: vec![],
            api_objects: vec![],
            relationships: vec![],
        };

        assert!(doc.validate().is_ok());
    }

    #[test]
    fn test_empty_version() {
        let doc = Document {
            version: "".to_string(),
            platform: "1С:Предприятие".to_string(),
            release_date: None,
            sections: vec![],
            api_methods: vec![],
            api_objects: vec![],
            relationships: vec![],
        };

        assert!(doc.validate().is_err());
    }
}
