use super::loader::PromptTemplate;
use anyhow::{Context, Result};
use std::collections::HashMap;
use tera::{Context as TeraContext, Tera};

/// Движок рендеринга промптов
pub struct PromptsRenderer {
    tera: Tera,
}

impl PromptsRenderer {
    pub fn new() -> Self {
        Self {
            tera: Tera::default(),
        }
    }

    /// Рендеринг промпта с подстановкой аргументов
    pub fn render(
        &mut self,
        template: &PromptTemplate,
        arguments: &HashMap<String, String>,
        related_docs: Option<String>,
    ) -> Result<String> {
        // Валидация аргументов
        template.validate_arguments(arguments)?;

        // Создание контекста для рендеринга
        let mut context = TeraContext::new();

        // Добавление всех аргументов
        for (key, value) in arguments {
            context.insert(key, value);
        }

        // Добавление документации (если есть)
        if let Some(docs) = related_docs {
            context.insert("related_docs", &docs);
        }

        // Рендеринг шаблона
        self.tera
            .render_str(&template.template, &context)
            .context(format!(
                "ru: Ошибка рендеринга промпта '{}', en: Failed to render prompt '{}'",
                template.name, template.name
            ))
    }
}

impl Default for PromptsRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::loader::PromptArgument;
    use super::*;

    #[test]
    fn test_simple_render() {
        let mut renderer = PromptsRenderer::new();

        let template = PromptTemplate {
            name: "test".to_string(),
            description: "Test prompt".to_string(),
            arguments: vec![PromptArgument {
                name: "query".to_string(),
                description: "Query text".to_string(),
                required: true,
                choices: None,
            }],
            template: "Search for: {{ query }}".to_string(),
        };

        let mut args = HashMap::new();
        args.insert("query".to_string(), "синтез речи".to_string());

        let result = renderer.render(&template, &args, None).unwrap();
        assert_eq!(result, "Search for: синтез речи");
    }

    #[test]
    fn test_render_with_docs() {
        let mut renderer = PromptsRenderer::new();

        let template = PromptTemplate {
            name: "test".to_string(),
            description: "Test prompt".to_string(),
            arguments: vec![],
            template: "Query\n{% if related_docs %}Docs:\n{{ related_docs }}{% endif %}"
                .to_string(),
        };

        let args = HashMap::new();
        let docs = "Documentation content".to_string();

        let result = renderer.render(&template, &args, Some(docs)).unwrap();
        assert!(result.contains("Docs:"));
        assert!(result.contains("Documentation content"));
    }

    #[test]
    fn test_conditional_rendering() {
        let mut renderer = PromptsRenderer::new();

        let template = PromptTemplate {
            name: "test".to_string(),
            description: "Test".to_string(),
            arguments: vec![PromptArgument {
                name: "explain".to_string(),
                description: "Add explanations".to_string(),
                required: false,
                choices: None,
            }],
            template: "Result{% if explain %} with explanations{% endif %}".to_string(),
        };

        let mut args = HashMap::new();
        let result1 = renderer.render(&template, &args, None).unwrap();
        assert_eq!(result1, "Result");

        args.insert("explain".to_string(), "true".to_string());
        let result2 = renderer.render(&template, &args, None).unwrap();
        assert_eq!(result2, "Result with explanations");
    }
}
