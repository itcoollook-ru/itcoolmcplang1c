//! Разрешение параметров запуска: аргумент → env → файл → дефолт.
//!
//! Семантика живёт здесь, а не в трёх серверах, по одной причине: разойтись в
//! ней нельзя. Потребитель настраивает все три сервера одинаковой записью в
//! `.mcp.json`, и «у conf1c аргумент перекрывает файл, а у run1c отменяет его
//! целиком» — отказ, который никто не воспроизведёт.
//!
//! **Разрешение идёт по ключу, а не по источнику.** Наличие файла не отменяет
//! аргумент, наличие аргумента не отменяет остальные ключи файла.

use std::path::PathBuf;

/// Откуда взято значение — для стартовой записи.
///
/// Это не диагностика ради диагностики: когда сервер молча читает не тот файл,
/// именно источник каждого ключа отвечает на вопрос «почему пусто».
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Arg(String),
    Env(String),
    File(PathBuf),
    Default,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Arg(name) => write!(f, "{}", name),
            Source::Env(name) => write!(f, "env {}", name),
            Source::File(path) => write!(f, "файл {}", path.display()),
            Source::Default => write!(f, "дефолт"),
        }
    }
}

/// Значение параметра вместе с его происхождением.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub value: String,
    pub source: Source,
}

impl Setting {
    /// Строка стартовой записи: `conf-dir = /conf (--conf-dir)`.
    pub fn report(&self, key: &str) -> String {
        format!("{} = {} ({})", key, self.value, self.source)
    }

    /// То же, но значение под маской — для паролей.
    pub fn report_masked(&self, key: &str) -> String {
        format!("{} = {} ({})", key, mask(&self.value), self.source)
    }
}

/// Не задан обязательный параметр. Текст перечисляет **все** места, где его
/// можно задать: отказ «нет base_url» без этого списка отправляет читать код.
#[derive(Debug, Clone)]
pub struct Missing {
    pub key: String,
    pub arg: String,
    pub env: String,
    pub file_hint: String,
}

impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "не задан обязательный параметр {}: укажите {} <значение>, переменную окружения {} или {} в файле конфигурации",
            self.key, self.arg, self.env, self.file_hint
        )
    }
}

impl std::error::Error for Missing {}

/// Значение аргумента `--name <значение>` из командной строки.
///
/// Форма только раздельная (`--config <путь>`): именно её передаёт core, и
/// плодить второй вариант — значит однажды поддержать не тот.
pub fn arg(name: &str) -> Option<String> {
    arg_in(std::env::args().collect::<Vec<_>>().as_slice(), name)
}

/// Тестируемая часть [`arg`]: разбор готового списка аргументов.
fn arg_in(args: &[String], name: &str) -> Option<String> {
    let mut i = 1;
    while i < args.len() {
        if args[i] == name {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}

/// Есть ли флаг без значения (`--help`).
pub fn flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

/// Разрешение параметров относительно конкретного файла конфигурации.
pub struct Resolver {
    /// Путь прочитанного файла; `None` — файла не было.
    file: Option<PathBuf>,
}

impl Resolver {
    pub fn new(file: Option<PathBuf>) -> Self {
        Self { file }
    }

    /// Значение по приоритету: аргумент → env → файл → `None`.
    ///
    /// `from_file` — уже прочитанное из файла значение этого ключа; файл читает
    /// сам сервер, форматы у них разные (ini, toml).
    pub fn get(&self, arg_name: &str, env_name: &str, from_file: Option<String>) -> Option<Setting> {
        if let Some(value) = arg(arg_name) {
            return Some(Setting {
                value,
                source: Source::Arg(arg_name.to_string()),
            });
        }

        if let Ok(value) = std::env::var(env_name) {
            if !value.is_empty() {
                return Some(Setting {
                    value,
                    source: Source::Env(env_name.to_string()),
                });
            }
        }

        from_file.map(|value| Setting {
            value,
            source: Source::File(self.file.clone().unwrap_or_default()),
        })
    }

    /// То же с дефолтом: параметр не обязателен.
    pub fn get_or(
        &self,
        arg_name: &str,
        env_name: &str,
        from_file: Option<String>,
        default: impl Into<String>,
    ) -> Setting {
        self.get(arg_name, env_name, from_file)
            .unwrap_or_else(|| Setting {
                value: default.into(),
                source: Source::Default,
            })
    }

    /// То же для обязательного параметра.
    pub fn require(
        &self,
        key: &str,
        arg_name: &str,
        env_name: &str,
        from_file: Option<String>,
        file_hint: &str,
    ) -> Result<Setting, Missing> {
        self.get(arg_name, env_name, from_file).ok_or_else(|| Missing {
            key: key.to_string(),
            arg: arg_name.to_string(),
            env: env_name.to_string(),
            file_hint: file_hint.to_string(),
        })
    }
}

/// Маска секрета для журнала: `itcool` → `itc***`.
///
/// Пароль не должен попадать ни в стартовую запись, ни в диагностику отказа, ни
/// в текст ошибки клиенту; при этом «креды заданы» видеть нужно.
pub fn mask(secret: &str) -> String {
    let visible: String = secret.chars().take(3).collect();
    if secret.chars().count() < 4 {
        return "***".to_string();
    }
    format!("{}***", visible)
}

/// Файл конфигурации рядом с исполняемым файлом.
///
/// Каталог exe, а не текущий каталог: cwd наследуется от хоста и хосту не
/// принадлежит — сервер, ищущий конфиг в cwd, находит разный файл при запуске из
/// разных мест и молча поднимается на дефолтах.
pub fn beside_exe(file_name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(file_name))
}

/// Текст без BOM.
///
/// BOM приклеивается к первой строке, и `[paths]` перестаёт быть секцией: разбор
/// уходит в «ключ не найден», а причина — невидимый символ. Возвращаем признак,
/// чтобы сервер сказал о нём прямо.
pub fn strip_bom(content: &str) -> (&str, bool) {
    match content.strip_prefix('\u{feff}') {
        Some(stripped) => (stripped, true),
        None => (content, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arg_reads_separate_form() {
        let argv = args(&["exe", "--config", "C:/x.ini", "--log-level", "debug"]);

        assert_eq!(arg_in(&argv, "--config"), Some("C:/x.ini".to_string()));
        assert_eq!(arg_in(&argv, "--log-level"), Some("debug".to_string()));
        assert_eq!(arg_in(&argv, "--missing"), None);
    }

    /// Форма `--config=<путь>` контрактом не объявлена: core передаёт раздельную.
    #[test]
    fn joined_form_is_not_recognized() {
        let argv = args(&["exe", "--config=C:/x.ini"]);

        assert_eq!(arg_in(&argv, "--config"), None);
    }

    /// Ключ без значения в конце строки не должен паниковать.
    #[test]
    fn dangling_key_yields_none() {
        let argv = args(&["exe", "--config"]);

        assert_eq!(arg_in(&argv, "--config"), None);
    }

    #[test]
    fn file_value_is_used_when_nothing_else_given() {
        let resolver = Resolver::new(Some(PathBuf::from("C:/cfg.ini")));

        let setting = resolver
            .get("--nothing-like-this", "MCP_TEST_ABSENT_KEY", Some("из файла".into()))
            .unwrap();

        assert_eq!(setting.value, "из файла");
        assert_eq!(setting.source, Source::File(PathBuf::from("C:/cfg.ini")));
    }

    #[test]
    fn default_is_last() {
        let resolver = Resolver::new(None);

        let setting = resolver.get_or("--nothing-like-this", "MCP_TEST_ABSENT_KEY", None, "300");

        assert_eq!(setting.value, "300");
        assert_eq!(setting.source, Source::Default);
    }

    #[test]
    fn missing_required_names_every_source() {
        let resolver = Resolver::new(None);

        let err = resolver
            .require(
                "base_url",
                "--base-url",
                "ITCOOLMCPRUN1C_BASE_URL",
                None,
                "[service] base_url",
            )
            .unwrap_err();

        let text = err.to_string();
        assert!(text.contains("--base-url"), "{}", text);
        assert!(text.contains("ITCOOLMCPRUN1C_BASE_URL"), "{}", text);
        assert!(text.contains("[service] base_url"), "{}", text);
    }

    #[test]
    fn secret_is_masked() {
        assert_eq!(mask("itcool"), "itc***");
        assert_eq!(mask("abc"), "***");
        assert_eq!(mask(""), "***");
    }

    #[test]
    fn bom_is_detected_and_stripped() {
        let (text, had_bom) = strip_bom("\u{feff}[paths]");
        assert_eq!(text, "[paths]");
        assert!(had_bom);

        let (text, had_bom) = strip_bom("[paths]");
        assert_eq!(text, "[paths]");
        assert!(!had_bom);
    }

    #[test]
    fn report_line_names_key_value_and_source() {
        let setting = Setting {
            value: "/conf".to_string(),
            source: Source::Arg("--conf-dir".to_string()),
        };

        assert_eq!(setting.report("conf-dir"), "conf-dir = /conf (--conf-dir)");
    }

    #[test]
    fn masked_report_hides_secret() {
        let setting = Setting {
            value: "itcool".to_string(),
            source: Source::Env("ITCOOLMCPRUN1C_PASSWORD".to_string()),
        };

        let line = setting.report_masked("password");
        assert!(!line.contains("itcool"), "пароль в журнале: {}", line);
        assert!(line.contains("itc***"), "{}", line);
    }
}
