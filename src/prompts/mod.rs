pub mod loader;
pub mod renderer;

use self::loader::{PromptTemplate, PromptsCollection, PromptsLoader};
use self::renderer::PromptsRenderer;
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

/// Движок промптов
pub struct PromptsEngine {
    collection: PromptsCollection,
    renderer: PromptsRenderer,
}

impl PromptsEngine {
    pub fn new() -> Self {
        Self {
            collection: PromptsCollection {
                prompts: Vec::new(),
            },
            renderer: PromptsRenderer::new(),
        }
    }

    /// Загрузка промптов из файла
    pub fn load_from_file(&mut self, template_file: impl AsRef<Path>) -> Result<()> {
        let loader = PromptsLoader::new(template_file);
        self.collection = loader.load()?;

        tracing::info!(
            "ru: Промпты загружены: {} шаблонов, en: Prompts loaded: {} templates",
            self.collection.prompts.len(),
            self.collection.prompts.len()
        );

        Ok(())
    }

    /// Список всех промптов
    pub fn list_prompts(&self) -> &[PromptTemplate] {
        &self.collection.prompts
    }

    /// Получение промпта по имени
    pub fn get_prompt(&self, name: &str) -> Option<&PromptTemplate> {
        self.collection.prompts.iter().find(|p| p.name == name)
    }

    /// Рендеринг промпта с аргументами
    pub fn render_prompt(
        &mut self,
        name: &str,
        arguments: &HashMap<String, String>,
    ) -> Result<String> {
        // Клонируем template для избежания проблем с borrowing
        let template = self
            .get_prompt(name)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "ru: Промпт '{}' не найден, en: Prompt '{}' not found",
                    name,
                    name
                )
            })?
            .clone();

        // Инъекция related_docs из базы знаний не реализована — рендер без документации.
        self.renderer.render(&template, arguments, None)
    }
}

impl Default for PromptsEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_engine() {
        let engine = PromptsEngine::new();
        assert_eq!(engine.list_prompts().len(), 0);
    }
}
