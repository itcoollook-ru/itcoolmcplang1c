//! data_manager.rs - Каталог данных сервера: структура подпапок и статистика диска.
//!
//! Приоритет пути (env `MCP_LANG_DATA_PATH` > конфиг) разрешает `main` —
//! сюда приходит уже готовый путь.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

/// Менеджер каталога данных: гарантирует структуру подпапок и считает занятое место.
#[derive(Debug, Clone)]
pub struct DataManager {
    /// Корневой путь к папке данных
    data_path: PathBuf,
    /// Путь к папке с языковыми файлами
    languages_path: PathBuf,
    /// Путь к папке кэша
    cache_path: PathBuf,
    /// Путь к папке логов
    logs_path: PathBuf,
}

impl DataManager {
    /// Создать менеджер: каталог обязан существовать, подпапки создаются при отсутствии.
    pub fn new(data_path: PathBuf) -> Result<Self> {
        info!("Initializing DataManager with path: {:?}", data_path);

        if !data_path.exists() {
            return Err(anyhow::anyhow!(
                "ru: Папка данных не существует: {:?}, en: Data directory does not exist: {:?}",
                data_path,
                data_path
            ));
        }

        let languages_path = data_path.join("languages");
        let cache_path = data_path.join("cache");
        let logs_path = data_path.join("logs");

        Self::ensure_directory(&languages_path)?;
        Self::ensure_directory(&cache_path)?;
        Self::ensure_directory(&logs_path)?;

        info!("DataManager initialized successfully");

        Ok(Self {
            data_path,
            languages_path,
            cache_path,
            logs_path,
        })
    }

    /// Убедиться что директория существует, создать если нет.
    fn ensure_directory(path: &Path) -> Result<()> {
        if !path.exists() {
            fs::create_dir_all(path).context(format!(
                "ru: Не удалось создать директорию {:?}, en: Failed to create directory {:?}",
                path, path
            ))?;
            info!("Created directory: {:?}", path);
        }
        Ok(())
    }

    /// Получить корневой путь к данным.
    pub fn data_path(&self) -> &Path {
        &self.data_path
    }

    /// Получить статистику использования дискового пространства.
    pub fn get_disk_usage(&self) -> Result<DiskUsage> {
        let languages_size = Self::get_directory_size(&self.languages_path)?;
        let cache_size = Self::get_directory_size(&self.cache_path)?;
        let logs_size = Self::get_directory_size(&self.logs_path)?;

        Ok(DiskUsage {
            total: languages_size + cache_size + logs_size,
            languages: languages_size,
            cache: cache_size,
            logs: logs_size,
        })
    }

    /// Вычислить размер директории рекурсивно.
    fn get_directory_size(path: &Path) -> Result<u64> {
        if !path.exists() {
            return Ok(0);
        }

        let mut total = 0u64;

        let entries = fs::read_dir(path).context(format!(
            "ru: Не удалось прочитать директорию {:?}, en: Failed to read directory {:?}",
            path, path
        ))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Ok(metadata) = fs::metadata(&path) {
                    total += metadata.len();
                }
            } else if path.is_dir() {
                total += Self::get_directory_size(&path)?;
            }
        }

        Ok(total)
    }
}

/// Статистика использования дискового пространства
#[derive(Debug, Clone)]
pub struct DiskUsage {
    pub total: u64,
    pub languages: u64,
    pub cache: u64,
    pub logs: u64,
}

impl DiskUsage {
    /// Форматировать размер в человекочитаемый вид
    pub fn format_size(bytes: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if bytes >= GB {
            format!("{:.2} GB", bytes as f64 / GB as f64)
        } else if bytes >= MB {
            format!("{:.2} MB", bytes as f64 / MB as f64)
        } else if bytes >= KB {
            format!("{:.2} KB", bytes as f64 / KB as f64)
        } else {
            format!("{} bytes", bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disk_usage_format() {
        assert_eq!(DiskUsage::format_size(500), "500 bytes");
        assert_eq!(DiskUsage::format_size(1024), "1.00 KB");
        assert_eq!(DiskUsage::format_size(1024 * 1024), "1.00 MB");
        assert_eq!(DiskUsage::format_size(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_data_manager_creation() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let root = temp_dir.path().to_path_buf();

        let dm = DataManager::new(root.clone()).unwrap();

        assert_eq!(dm.data_path(), root.as_path());
        assert!(root.join("languages").exists());
        assert!(root.join("cache").exists());
        assert!(root.join("logs").exists());
    }

    #[test]
    fn missing_data_dir_is_an_error() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let gone = temp_dir.path().join("no-such-dir");
        assert!(DataManager::new(gone).is_err());
    }
}
