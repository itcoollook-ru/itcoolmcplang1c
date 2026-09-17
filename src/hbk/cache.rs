//! cache.rs — кэш собранного индекса между запусками.
//!
//! Разбор справки занимает секунды, и платить ими на каждом старте незачем:
//! MCP-хост порождает сервер дочерним процессом при каждом подключении.

use super::lang::HelpLang;
use super::platform::Platform;
use crate::knowledge::schema::Document;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Отпечаток одного файла справки.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceStamp {
    name: String,
    size: u64,
    /// Время изменения в секундах Unix; `None` — файловая система не сказала.
    mtime: Option<u64>,
}

/// Метаданные кэша: по ним решается, годен ли он.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Meta {
    platform_version: String,
    help_language: String,
    sources: Vec<SourceStamp>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    meta: Meta,
    document: Document,
}

/// Корень кэша: `%LOCALAPPDATA%` на Windows, `~/.cache` на остальных.
pub fn root() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    }?;

    Some(base.join("itcoolmcplang1c"))
}

/// Файл кэша для конкретной версии платформы и языка справки.
pub fn path_for(platform: &Platform, lang: &HelpLang) -> Option<PathBuf> {
    Some(root()?.join(format!("index-{}-{}.json", lang.code, platform.version)))
}

fn stamps(platform: &Platform, lang: &HelpLang) -> Vec<SourceStamp> {
    platform
        .help_books(lang)
        .iter()
        .map(|path| {
            let meta = std::fs::metadata(path).ok();
            SourceStamp {
                name: path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string(),
                size: meta.as_ref().map_or(0, |m| m.len()),
                mtime: meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
            }
        })
        .collect()
}

fn meta_for(platform: &Platform, lang: &HelpLang) -> Meta {
    Meta {
        platform_version: platform.version.clone(),
        help_language: lang.code.to_string(),
        sources: stamps(platform, lang),
    }
}

/// Годный кэш для этой платформы и языка; `None` — кэша нет или он устарел.
///
/// Любой отказ чтения — это просто «кэша нет»: пересборка всегда возможна, а
/// падать из-за повреждённого временного файла незачем.
pub fn load(platform: &Platform, lang: &HelpLang) -> Option<Document> {
    let path = path_for(platform, lang)?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let entry: Entry = serde_json::from_str(&raw).ok()?;

    if entry.meta != meta_for(platform, lang) {
        tracing::info!(
            "ru: Кэш индекса устарел, пересборка: {:?}, en: Index cache is stale, rebuilding: {:?}",
            path,
            path
        );
        return None;
    }

    tracing::info!(
        "ru: Индекс взят из кэша: {:?}, en: Index loaded from cache: {:?}",
        path,
        path
    );
    Some(entry.document)
}

/// Сохранение собранного индекса. Отказ записи — предупреждение, не сбой: база
/// уже в памяти, кэш лишь ускоряет следующий запуск.
pub fn store(platform: &Platform, lang: &HelpLang, document: &Document) -> Result<PathBuf> {
    let path = path_for(platform, lang)
        .context("ru: Не определён каталог кэша, en: Cache directory is unknown")?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context(format!(
            "ru: Не создан каталог кэша {:?}, en: Failed to create cache directory {:?}",
            parent, parent
        ))?;
    }

    let entry = Entry {
        meta: meta_for(platform, lang),
        document: document.clone(),
    };
    let json = serde_json::to_string(&entry)?;

    // Через временный файл: оборванная запись не должна оставить кэш, который
    // разберётся наполовину.
    let temp = path.with_extension("json.part");
    std::fs::write(&temp, json).context(format!(
        "ru: Не записан кэш {:?}, en: Failed to write cache {:?}",
        temp, temp
    ))?;
    std::fs::rename(&temp, &path).context(format!(
        "ru: Не переименован кэш в {:?}, en: Failed to rename cache to {:?}",
        path, path
    ))?;

    Ok(path)
}

/// Запись документа рядом с базой знаний — отладочная опция `index --out`.
pub fn write_json(document: &Document, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, serde_json::to_string_pretty(document)?).context(format!(
        "ru: Не записан файл {:?}, en: Failed to write file {:?}",
        path, path
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        Document {
            version: "8.3.27".to_string(),
            platform: "1С:Предприятие".to_string(),
            release_date: None,
            sections: vec![],
            api_methods: vec![],
            api_objects: vec![],
            relationships: vec![],
        }
    }

    fn platform_in(dir: &Path) -> Platform {
        std::fs::write(dir.join("shcntx_ru.hbk"), b"stub").unwrap();
        Platform {
            bin_dir: dir.to_path_buf(),
            version: "8.3.27.1936".to_string(),
        }
    }

    /// Кэш годен, пока исходные книги не менялись.
    #[test]
    fn fresh_cache_is_accepted() {
        let temp = tempfile::TempDir::new().unwrap();
        let platform = platform_in(temp.path());
        let lang = HelpLang::from_code("ru").unwrap();

        let entry = Entry {
            meta: meta_for(&platform, lang),
            document: doc(),
        };

        assert_eq!(entry.meta, meta_for(&platform, lang));
    }

    /// Книга изменилась — отпечаток другой, кэш пересобирается.
    #[test]
    fn changed_book_invalidates_the_stamp() {
        let temp = tempfile::TempDir::new().unwrap();
        let platform = platform_in(temp.path());
        let lang = HelpLang::from_code("ru").unwrap();
        let before = meta_for(&platform, lang);

        std::fs::write(temp.path().join("shcntx_ru.hbk"), b"stub with different size").unwrap();

        assert_ne!(before, meta_for(&platform, lang));
    }

    /// Другая версия платформы — другой отпечаток и другой файл кэша.
    #[test]
    fn platform_version_is_part_of_the_key() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut platform = platform_in(temp.path());
        let lang = HelpLang::from_code("ru").unwrap();
        let before = meta_for(&platform, lang);

        platform.version = "8.3.24.1368".to_string();

        assert_ne!(before, meta_for(&platform, lang));
        assert_ne!(path_for(&platform, lang), None);
    }

    /// Язык справки входит в имя файла кэша: две базы не затирают друг друга.
    #[test]
    fn language_separates_cache_files() {
        let temp = tempfile::TempDir::new().unwrap();
        let platform = platform_in(temp.path());

        let ru = path_for(&platform, HelpLang::from_code("ru").unwrap());
        let en = path_for(&platform, HelpLang::from_code("en").unwrap());

        assert_ne!(ru, en);
    }

    #[test]
    fn debug_json_is_written_with_directories() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("nested/data/1c.json");

        write_json(&doc(), &path).unwrap();

        assert!(path.is_file());
    }
}
