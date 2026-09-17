use super::schema::Document;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

/// Загрузчик JSON документов
pub struct Loader {
    data_dir: std::path::PathBuf,
}

impl Loader {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    /// Загрузка всех JSON файлов из директории
    pub fn load_all(&self) -> Result<Vec<Document>> {
        let mut documents = Vec::new();

        if !self.data_dir.exists() {
            tracing::warn!(
                "ru: Директория данных не существует: {:?}, en: Data directory does not exist: {:?}",
                self.data_dir, self.data_dir
            );
            return Ok(documents);
        }

        for entry in WalkDir::new(&self.data_dir)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                match self.load_document(path) {
                    Ok(doc) => {
                        tracing::info!(
                            "ru: Загружен документ версии {}, en: Loaded document version {}",
                            doc.version,
                            doc.version
                        );
                        documents.push(doc);
                    }
                    Err(_) if is_index_cache(path) => {
                        tracing::debug!(
                            "ru: Пропущен файл кэша индекса: {:?}, en: Skipped index cache file: {:?}",
                            path,
                            path
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            "ru: Ошибка загрузки {:?}: {}, en: Failed to load {:?}: {}",
                            path,
                            e,
                            path,
                            e
                        );
                    }
                }
            }
        }

        tracing::info!(
            "ru: Загружено документов: {}, en: Loaded {} documents",
            documents.len(),
            documents.len()
        );

        Ok(documents)
    }
}

/// Файл кэша самосборки, а не документ базы знаний.
///
/// # CRITICAL_LOGIC (не трогать при рефакторинге)
/// При самосборке без `--data-dir` каталогом знаний становится тот же
/// пользовательский каталог, где лежит кэш индекса (`index-<язык>-<версия>.json`
/// с парой `meta`/`document`). Загрузчику это не документы, и без проверки
/// штатный первый запуск писал бы в журнал ошибку на каждый файл кэша —
/// сообщение о поломке там, где ничего не сломано.
fn is_index_cache(path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };

    value.get("meta").is_some() && value.get("document").is_some()
}

impl Loader {
    /// Загрузка одного документа
    pub fn load_document(&self, path: &Path) -> Result<Document> {
        let content = fs::read_to_string(path).context(format!(
            "ru: Не удалось прочитать файл {:?}, en: Failed to read file {:?}",
            path, path
        ))?;

        let doc: Document = serde_json::from_str(&content).context(format!(
            "ru: Ошибка парсинга JSON в {:?}, en: Failed to parse JSON in {:?}",
            path, path
        ))?;

        doc.validate().map_err(|e| anyhow::anyhow!(
            "ru: Валидация документа не прошла для {:?}: {}, en: Document validation failed for {:?}: {}",
            path, e, path, e
        ))?;

        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Каталог знаний по умолчанию — тот же, где лежит кэш самосборки. Записи
    /// кэша загрузчику не документы, и на них он раньше писал ошибку: штатный
    /// первый запуск выглядел поломкой.
    #[test]
    fn index_cache_files_are_skipped_quietly() {
        let temp_dir = TempDir::new().unwrap();

        fs::write(
            temp_dir.path().join("index-ru-8.3.27.1936.json"),
            r#"{"meta": {"platform_version": "8.3.27.1936"}, "document": {"version": "8.3.27"}}"#,
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("base.json"),
            r#"{
                "version": "8.3.27",
                "platform": "1С:Предприятие",
                "sections": [],
                "api_methods": [],
                "api_objects": [],
                "relationships": []
            }"#,
        )
        .unwrap();

        let documents = Loader::new(temp_dir.path()).load_all().unwrap();

        assert_eq!(documents.len(), 1, "кэш не должен попадать в базу знаний");
        assert_eq!(documents[0].version, "8.3.27");
        assert!(is_index_cache(&temp_dir.path().join("index-ru-8.3.27.1936.json")));
        assert!(!is_index_cache(&temp_dir.path().join("base.json")));
    }

    #[test]
    fn test_load_valid_document() {
        let temp_dir = TempDir::new().unwrap();
        let json_path = temp_dir.path().join("test.json");

        let test_doc = r#"{
            "version": "8.3.25",
            "platform": "1С:Предприятие",
            "sections": [],
            "api_methods": [],
            "api_objects": [],
            "relationships": []
        }"#;

        fs::write(&json_path, test_doc).unwrap();

        let loader = Loader::new(temp_dir.path());
        let docs = loader.load_all().unwrap();

        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].version, "8.3.25");
    }
}
