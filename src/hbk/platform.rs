//! platform.rs — установка платформы 1С как источник справки.

use super::lang::HelpLang;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

/// Каталог `bin` установки платформы и её версия.
#[derive(Debug, Clone)]
pub struct Platform {
    pub bin_dir: PathBuf,
    /// Полная версия установки: `8.3.27.1936`. Ею инвалидируется кэш.
    pub version: String,
}

impl Platform {
    /// Версия для базы знаний — три компонента: `8.3.27`.
    ///
    /// Четвёртый компонент — номер сборки платформы; справка между сборками
    /// одного релиза не меняется, а `list_versions` и параметр `version`
    /// инструментов говорят именно трёхкомпонентными номерами.
    pub fn document_version(&self) -> String {
        let mut parts = self.version.split('.');
        let head: Vec<&str> = parts.by_ref().take(3).collect();
        head.join(".")
    }

    /// Книги справки выбранного языка, которые реально лежат в каталоге.
    ///
    /// Список — вход инвалидации кэша: изменилась любая из них, индекс собирается
    /// заново.
    pub fn help_books(&self, lang: &HelpLang) -> Vec<PathBuf> {
        let mut books = vec![self.bin_dir.join(lang.context_book())];
        books.extend(lang.books.iter().map(|b| self.bin_dir.join(b.file_name(lang))));
        books.retain(|p| p.is_file());
        books
    }
}

/// Каталог `bin`, указанный пользователем.
///
/// Принимается и сам `bin`, и каталог версии над ним: пользователь показывает на
/// установку, а где внутри лежит справка — деталь раскладки.
pub fn from_dir(dir: &Path, lang: &HelpLang, version_hint: Option<&str>) -> Result<Platform> {
    let book = lang.context_book();

    let bin_dir = if dir.join(&book).is_file() {
        dir.to_path_buf()
    } else if dir.join("bin").join(&book).is_file() {
        dir.join("bin")
    } else {
        return Err(anyhow!(
            "ru: В каталоге {:?} нет файла справки {} (язык {}), en: No help file {} (language {}) in {:?}",
            dir, book, lang.code, book, lang.code, dir
        ));
    };

    let version = match version_hint {
        Some(v) if !v.is_empty() => v.to_string(),
        // `--version` занят выводом версии сервера, поэтому ключ отдельный.
        _ => detect_version(&bin_dir).ok_or_else(|| {
            anyhow!(
                "ru: Версию платформы не видно из пути {:?}: укажите --platform-version <версия>, en: Cannot detect platform version from {:?}: pass --platform-version <version>",
                bin_dir, bin_dir
            )
        })?,
    };

    Ok(Platform { bin_dir, version })
}

/// Старшая установка платформы из стандартных мест.
pub fn discover(lang: &HelpLang) -> Option<Platform> {
    let book = lang.context_book();

    let mut found: Vec<Platform> = Vec::new();
    for root in install_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let dir = entry.path();
            let bin_dir = dir.join("bin");
            if !bin_dir.join(&book).is_file() {
                continue;
            }
            let Some(version) = dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if version_key(version).is_empty() {
                continue;
            }
            found.push(Platform {
                bin_dir,
                version: version.to_string(),
            });
        }
    }

    found.sort_by(|a, b| version_key(&a.version).cmp(&version_key(&b.version)));
    found.pop()
}

/// Стандартные корни установки: внутри — каталоги версий.
fn install_roots() -> Vec<PathBuf> {
    if cfg!(windows) {
        // Через окружение, а не строкой: `Program Files` переезжает вместе с
        // системным диском и локализуется.
        let mut roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|base| PathBuf::from(base).join("1cv8"))
            .collect();
        roots.push(PathBuf::from(r"C:\Program Files\1cv8"));
        roots.sort();
        roots.dedup();
        roots
    } else {
        vec![
            PathBuf::from("/opt/1cv8/x86_64"),
            PathBuf::from("/opt/1cv8/i386"),
        ]
    }
}

/// Версия из пути: имя каталога над `bin`, если оно похоже на номер версии.
fn detect_version(bin_dir: &Path) -> Option<String> {
    let mut dir = bin_dir;
    if dir.file_name().and_then(|n| n.to_str()) == Some("bin") {
        dir = dir.parent()?;
    }
    let name = dir.file_name()?.to_str()?;

    let looks_like_version = name.contains('.')
        && name
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));

    looks_like_version.then(|| name.to_string())
}

/// Ключ сравнения версий по числовым компонентам: `8.3.9` младше `8.3.27`.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<u64>>>()
        .unwrap_or_default()
}

/// Подсказка, куда смотреть, когда платформа не найдена.
pub fn not_found_hint(lang: &HelpLang) -> String {
    let roots = install_roots()
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "Установка платформы 1С не найдена. Искали {} внутри: {}. \
         Укажите каталог bin явно: --platform-dir <путь> (или [knowledge] platform_dir в конфиге). \
         Язык справки задаётся --help-lang ru|en.",
        lang.context_book(),
        roots
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ru() -> &'static HelpLang {
        HelpLang::from_code("ru").unwrap()
    }

    #[test]
    fn document_version_drops_the_build_number() {
        let platform = Platform {
            bin_dir: PathBuf::from("/x"),
            version: "8.3.27.1936".to_string(),
        };

        assert_eq!(platform.document_version(), "8.3.27");
    }

    /// Короткая версия не должна превращаться в мусор.
    #[test]
    fn document_version_keeps_short_version_as_is() {
        let platform = Platform {
            bin_dir: PathBuf::from("/x"),
            version: "8.3".to_string(),
        };

        assert_eq!(platform.document_version(), "8.3");
    }

    #[test]
    fn version_is_detected_from_the_directory_above_bin() {
        assert_eq!(
            detect_version(Path::new("/opt/1cv8/x86_64/8.3.27.1936/bin")),
            Some("8.3.27.1936".to_string())
        );
        assert_eq!(detect_version(Path::new("/home/user/help")), None);
    }

    /// Строковое сравнение поставило бы `8.3.9` выше `8.3.27`.
    #[test]
    fn versions_compare_numerically() {
        assert!(version_key("8.3.27.1936") > version_key("8.3.9.1"));
        assert!(version_key("8.3.24.1368") < version_key("8.3.27.1936"));
    }

    /// Каталог без справки — внятный отказ, а не «пустая база».
    #[test]
    fn directory_without_help_is_rejected() {
        let temp = tempfile::TempDir::new().unwrap();

        let err = from_dir(temp.path(), ru(), None).unwrap_err().to_string();

        assert!(err.contains("shcntx_ru.hbk"), "{}", err);
    }

    /// Показали каталог версии — `bin` под ним находится сам.
    #[test]
    fn version_directory_resolves_to_its_bin() {
        let temp = tempfile::TempDir::new().unwrap();
        let version_dir = temp.path().join("8.3.27.1936");
        let bin = version_dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("shcntx_ru.hbk"), b"stub").unwrap();

        let platform = from_dir(&version_dir, ru(), None).unwrap();

        assert_eq!(platform.bin_dir, bin);
        assert_eq!(platform.version, "8.3.27.1936");
    }

    /// Нераспознаваемая версия перекрывается явным указанием.
    #[test]
    fn version_hint_overrides_detection() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("shcntx_ru.hbk"), b"stub").unwrap();

        let platform = from_dir(temp.path(), ru(), Some("8.3.27.1936")).unwrap();

        assert_eq!(platform.document_version(), "8.3.27");
    }

    #[test]
    fn help_books_list_only_existing_files() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("shcntx_ru.hbk"), b"stub").unwrap();
        std::fs::write(temp.path().join("shquery_ru.hbk"), b"stub").unwrap();

        let platform = from_dir(temp.path(), ru(), Some("8.3.27.1936")).unwrap();
        let books = platform.help_books(ru());

        assert_eq!(books.len(), 2);
        assert!(books.iter().any(|p| p.ends_with("shquery_ru.hbk")));
        assert!(!books.iter().any(|p| p.ends_with("shlang_ru.hbk")));
    }
}
