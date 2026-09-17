use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Главная конфигурация сервера.
///
/// Здесь только читаемые кодом ручки: имя/версия сервера берутся из
/// `CARGO_PKG_NAME`/`CARGO_PKG_VERSION`, а не из конфига. Неизвестные ключи
/// в конфиге игнорируются.
///
/// **Каждая секция необязательна.** Файл — запасной вариант к аргументам
/// запуска, и частичный конфиг (одна секция `[search]` рядом с базой знаний) —
/// штатный случай. Пока секции были обязательными, такой файл не
/// десериализовался целиком и молча уходил в дефолты вместе со своими ключами.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub knowledge: KnowledgeConfig,
    #[serde(default)]
    pub prompts: PromptsConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

/// Транспорт. Значение одно — `"stdio"`: сервер работает с локально
/// установленной платформой 1С и удалённым быть не может, все клиенты поднимают
/// его дочерним процессом. HTTP-транспорт вместе с браузерным UI удалён.
///
/// Ключ оставлен именно затем, чтобы перенесённый конфиг с `"http"` или
/// `"both"` получил внятный отказ старта (`server::unsupported_mode`), а не
/// тихую подмену на stdio.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_mode() -> String {
    "stdio".to_string()
}

/// База знаний: готовый индекс по пути либо самосборка из справки платформы.
///
/// Приоритет за готовым индексом (`data_dir`): им живёт прод, и появление
/// второго режима не должно его трогать. Платформа читается только тогда, когда
/// по настроенному пути базы нет.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KnowledgeConfig {
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    /// Каталог `bin` установки платформы 1С. Пусто — автопоиск по стандартным
    /// местам установки.
    #[serde(default)]
    pub platform_dir: Option<PathBuf>,
    /// Язык справки: `ru` (`sh*_ru.hbk`) или `en` (`sh*_root.hbk`).
    #[serde(default = "default_help_language")]
    pub help_language: String,
    /// Версия платформы, когда её не вывести из пути к каталогу.
    #[serde(default)]
    pub platform_version: Option<String>,
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("./data")
}

fn default_help_language() -> String {
    "ru".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PromptsConfig {
    #[serde(default = "default_template_file")]
    pub template_file: PathBuf,
}

fn default_template_file() -> PathBuf {
    PathBuf::from("./prompts.json")
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchConfig {
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default = "default_boost_title")]
    pub boost_title: f32,
    #[serde(default = "default_boost_keywords")]
    pub boost_keywords: f32,
}

fn default_max_results() -> usize {
    10
}

fn default_boost_title() -> f32 {
    3.0
}

fn default_boost_keywords() -> f32 {
    2.0
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_format")]
    pub format: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_format() -> String {
    "pretty".to_string()
}

impl Config {
    /// Загрузка конфигурации из файла.
    ///
    /// Источник ровно один — файл. Прежний `config::Environment` с префиксом
    /// `ITCOOL1CLANG` был четвёртым, необъявленным каналом настройки: сервер
    /// наследует окружение хоста целиком, и посторонняя переменная молча
    /// переопределяла конфиг мимо контракта `аргумент > env > файл > дефолт`.
    pub fn load(path: &str) -> Result<Self> {
        let config_builder = config::Config::builder()
            .add_source(config::File::from(std::path::Path::new(path)))
            .build()
            .context(format!(
                "ru: Не удалось загрузить конфигурацию из {}, en: Failed to load config from {}",
                path, path
            ))?;

        config_builder
            .try_deserialize()
            .context("ru: Ошибка десериализации конфигурации, en: Failed to deserialize config")
    }

    /// Конфиг из файла: отсутствие файла — штатный случай, ошибка разбора — нет.
    ///
    /// # CRITICAL_LOGIC (не трогать при рефакторинге)
    /// Прежний тихий откат на дефолты стоил дороже опечатки, ради которой
    /// затевался. Конфиг читается раньше инициализации логирования, и
    /// `warn!` о неудачном разборе не попадал в журнал вовсе: файл не
    /// разбирался, а сервер поднимался на чужих значениях — своим каталогом
    /// данных, своими весами ранжирования, своим языком справки — без единого
    /// следа. Файл, который положили, обязан быть прочитан или назван сломанным.
    pub fn load_or_default(path: &str) -> Result<Self> {
        if !std::path::Path::new(path).exists() {
            return Ok(Self::default());
        }

        Self::load(path)
    }
}

// Дефолты живут рядом с полями (`#[serde(default = ...)]`), чтобы частичный
// конфиг и полное его отсутствие давали одно и то же. Пока дефолты дублировались
// в `impl Default for Config`, две копии расходились молча.
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            mode: default_mode(),
        }
    }
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            platform_dir: None,
            help_language: default_help_language(),
            platform_version: None,
        }
    }
}

impl Default for PromptsConfig {
    fn default() -> Self {
        Self {
            template_file: default_template_file(),
        }
    }
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_results: default_max_results(),
            boost_title: default_boost_title(),
            boost_keywords: default_boost_keywords(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_format(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server.mode, "stdio");
        assert_eq!(config.search.max_results, 10);
    }

    /// Регрессия: секции были обязательными, и конфиг с одной `[search]` не
    /// десериализовался целиком — свои же ключи молча уходили в дефолты.
    #[test]
    fn partial_config_keeps_its_own_keys() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("itcoolmcplang1c.toml");
        std::fs::write(&path, "[search]\nmax_results = 42\n").unwrap();

        let config = Config::load(path.to_str().unwrap()).unwrap();

        assert_eq!(config.search.max_results, 42);
        assert_eq!(config.server.mode, "stdio");
        assert_eq!(config.logging.level, "info");
    }

    /// Нет файла — штатный случай: дефолты и никаких жалоб.
    #[test]
    fn missing_config_is_not_an_error() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("нет-такого.toml");

        let config = Config::load_or_default(path.to_str().unwrap()).unwrap();

        assert_eq!(config.server.mode, "stdio");
    }

    /// Регрессия: сломанный файл откатывался на дефолты, а `warn!` о нём
    /// терялся — конфиг читается раньше инициализации логирования. Человек
    /// получал чужие настройки вместо своих и ни одного сигнала об этом.
    #[test]
    fn broken_config_is_a_startup_error() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("itcoolmcplang1c.toml");
        std::fs::write(&path, "[server\nmode = \"http\"\n").unwrap();

        let error = Config::load_or_default(path.to_str().unwrap())
            .expect_err("сломанный конфиг не должен подменяться дефолтами");

        assert!(
            format!("{error}").contains(path.to_str().unwrap()),
            "ошибка должна называть файл: {error}"
        );
    }
}
