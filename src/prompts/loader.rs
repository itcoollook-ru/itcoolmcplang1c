use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Коллекция промптов
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptsCollection {
    pub prompts: Vec<PromptTemplate>,
}

/// Шаблон промпта
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub arguments: Vec<PromptArgument>,
    pub template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
}

impl PromptTemplate {
    /// Валидация аргументов
    pub fn validate_arguments(&self, args: &HashMap<String, String>) -> Result<()> {
        // Проверка обязательных аргументов
        for arg in &self.arguments {
            if arg.required && !args.contains_key(&arg.name) {
                return Err(anyhow::anyhow!(
                    "ru: Отсутствует обязательный аргумент '{}', en: Missing required argument '{}'",
                    arg.name, arg.name
                ));
            }

            // Проверка choices
            if let Some(value) = args.get(&arg.name) {
                if let Some(choices) = &arg.choices {
                    if !choices.contains(value) {
                        return Err(anyhow::anyhow!(
                            "ru: Недопустимое значение '{}' для аргумента '{}', допустимые: {:?}, en: Invalid value '{}' for argument '{}', allowed: {:?}",
                            value, arg.name, choices, value, arg.name, choices
                        ));
                    }
                }
            }
        }

        Ok(())
    }
}

/// Загрузчик промптов
pub struct PromptsLoader {
    template_file: std::path::PathBuf,
}

impl PromptsLoader {
    pub fn new(template_file: impl AsRef<Path>) -> Self {
        Self {
            template_file: template_file.as_ref().to_path_buf(),
        }
    }

    /// Загрузка всех промптов из файла
    pub fn load(&self) -> Result<PromptsCollection> {
        if !self.template_file.exists() {
            tracing::warn!(
                "ru: Файл промптов не найден: {:?}, используем пустую коллекцию, en: Prompts file not found: {:?}, using empty collection",
                self.template_file, self.template_file
            );
            return Ok(PromptsCollection {
                prompts: Vec::new(),
            });
        }

        let content = fs::read_to_string(&self.template_file).context(format!(
            "ru: Не удалось прочитать файл промптов {:?}, en: Failed to read prompts file {:?}",
            self.template_file, self.template_file
        ))?;

        let collection: PromptsCollection = serde_json::from_str(&content).context(format!(
            "ru: Ошибка парсинга промптов из {:?}, en: Failed to parse prompts from {:?}",
            self.template_file, self.template_file
        ))?;

        tracing::info!(
            "ru: Загружено промптов: {}, en: Loaded {} prompts",
            collection.prompts.len(),
            collection.prompts.len()
        );

        Ok(collection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_arguments() {
        let template = PromptTemplate {
            name: "test".to_string(),
            description: "Test".to_string(),
            arguments: vec![
                PromptArgument {
                    name: "required_arg".to_string(),
                    description: "Required".to_string(),
                    required: true,
                    choices: None,
                },
                PromptArgument {
                    name: "optional_arg".to_string(),
                    description: "Optional".to_string(),
                    required: false,
                    choices: Some(vec!["a".to_string(), "b".to_string()]),
                },
            ],
            template: "Test".to_string(),
        };

        let mut args = HashMap::new();
        assert!(template.validate_arguments(&args).is_err()); // missing required

        args.insert("required_arg".to_string(), "value".to_string());
        assert!(template.validate_arguments(&args).is_ok());

        args.insert("optional_arg".to_string(), "c".to_string());
        assert!(template.validate_arguments(&args).is_err()); // invalid choice

        args.insert("optional_arg".to_string(), "a".to_string());
        assert!(template.validate_arguments(&args).is_ok());
    }
}
