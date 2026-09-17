mod config;
mod data_manager;
mod hbk;
mod knowledge;
mod prompts;
mod protocol;
mod server;
mod utils;

use anyhow::Result;
use config::Config;
use data_manager::DataManager;
use mcp_common::cli::{self, Setting, Source};
use server::Server;
use std::path::{Path, PathBuf};
use utils::init_logging;

/// Значение аргумента и в раздельной форме, и через `=`.
///
/// Раздельная форма — контракт с core (`mcp_common::cli::arg`), и его трогать
/// нельзя. Но `index --platform-dir=...` набирает руками человек из README
/// публичного репозитория, и «молча не сработало» — худший из возможных ответов.
/// Поэтому обе формы принимают только флаги самосборки, добавленные здесь.
fn self_build_arg(name: &str) -> Option<String> {
    if let Some(value) = cli::arg(name) {
        return Some(value);
    }

    let prefix = format!("{}=", name);
    std::env::args().find_map(|a| a.strip_prefix(&prefix).map(str::to_string))
}

/// Корень каталога базы знаний: `--data-dir <путь>` > env `MCP_LANG_DATA_DIR`.
///
/// Одного этого аргумента хватает на весь запуск: внутри корня лежит **всё** —
/// конфиг сервера, каталог `data` с JSON базы знаний и `prompts.json`. Так хост
/// задаёт запуск одной строкой, а сервер не зависит от того, какой файл окажется
/// рядом с exe.
fn resolve_root() -> Option<Setting> {
    if let Some(value) = cli::arg("--data-dir") {
        return Some(Setting {
            value,
            source: Source::Arg("--data-dir".to_string()),
        });
    }

    match std::env::var("MCP_LANG_DATA_DIR") {
        Ok(value) if !value.is_empty() => Some(Setting {
            value,
            source: Source::Env("MCP_LANG_DATA_DIR".to_string()),
        }),
        _ => None,
    }
}

/// Путь к конфигу: `--config <path>` > env `MCP_LANG_CONFIG` > файл **в корне
/// каталога знаний** > файл рядом с exe.
///
/// Каталог знаний первым, потому что конфиг — его часть: он описывает именно эту
/// базу, и переносится вместе с ней. Каталог exe остаётся фолбэком для запуска
/// без аргумента; cwd не используется вовсе — рабочий каталог наследуется от
/// хоста и хосту не принадлежит.
///
/// Имя `config.toml` остаётся фолбэком: так назывался конфиг до 0.3.0.
fn resolve_config_path(root: Option<&Path>) -> String {
    if let Some(p) = cli::arg("--config") {
        return p;
    }
    if let Ok(p) = std::env::var("MCP_LANG_CONFIG") {
        if !p.is_empty() {
            return p;
        }
    }

    let dir = match root {
        Some(root) => Some(root.to_path_buf()),
        None => cli::beside_exe("itcoolmcplang1c.toml")
            .and_then(|p| p.parent().map(Path::to_path_buf)),
    };

    let Some(dir) = dir else {
        return "itcoolmcplang1c.toml".to_string();
    };

    for name in ["itcoolmcplang1c.toml", "config.toml"] {
        let candidate = dir.join(name);
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
    }

    dir.join("itcoolmcplang1c.toml").to_string_lossy().to_string()
}

/// Каталог JSON базы знаний внутри корня, когда конфига нет.
///
/// Вложенный `data` предпочтительнее самого корня: загрузчик обходит каталог
/// рекурсивно, и корень затянул бы соседние `backup`/`demo_backup`, а два
/// документа одной версии затирают друг друга. Плоскую раскладку (JSON прямо в
/// корне) при этом тоже надо поддержать — отсюда проверка.
fn knowledge_dir_in(root: &Path) -> PathBuf {
    let nested = root.join("data");
    if nested.is_dir() {
        nested
    } else {
        root.to_path_buf()
    }
}

fn print_help() {
    println!("MCP сервер базы знаний языка 1С");
    println!("\nИСПОЛЬЗОВАНИЕ:\n    itcoolmcplang1c --data-dir <КАТАЛОГ ЗНАНИЙ>");
    println!("\nOPTIONS:");
    println!("    --data-dir <PATH>      Корень каталога базы знаний");
    println!("                           Обязателен, если не задан --platform-dir: при");
    println!("                           самосборке каталог берётся по умолчанию — там же,");
    println!("                           где кэш индекса");
    println!("                           Внутри: конфиг сервера, data\\ с JSON, prompts.json");
    println!("                           env MCP_LANG_DATA_DIR");
    println!("\n    --prompts <PATH>       Файл промптов [default: prompts.json в каталоге знаний]");
    println!("                           env MCP_LANG_PROMPTS, либо [prompts] template_file");
    println!("\n    --log-level <LEVEL>    Уровень логирования [default: info]");
    println!("                           env MCP_LANG_LOG_LEVEL, либо [logging] level");
    println!("\n    --config <PATH>        Путь к itcoolmcplang1c.toml (необязателен)");
    println!("                           (иначе файл в каталоге знаний, иначе рядом с exe)");
    println!("\n    --help                 Показать справку");
    println!("\n    --version              Показать версию");
    println!("\nСАМОСБОРКА БАЗЫ ЗНАНИЙ:");
    println!("    Готового индекса по --data-dir нет — сервер собирает базу сам, из справки");
    println!("    вашей установки платформы 1С. Справка при этом никуда не передаётся.");
    println!("    Разбор идёт секунды и выполняется в фоне: initialize отвечает сразу,");
    println!("    а инструменты до готовности отдают текст с прогрессом. Итог кэшируется.");
    println!("\n    itcoolmcplang1c index --platform-dir <BIN>   Собрать индекс и выйти");
    println!("\n    itcoolmcplang1c --check                      Проверить установку и выйти");
    println!("                           Печатает платформу, справку, индекс, числа и");
    println!("                           тестовый запрос; код возврата 1, если что-то не так");
    println!("\n    --platform-dir <PATH>  Каталог bin установки платформы 1С");
    println!("                           (иначе автопоиск; либо [knowledge] platform_dir)");
    println!("                           env MCP_LANG_PLATFORM_DIR");
    println!("\n    --help-lang <ru|en>    Язык справки [default: ru]");
    println!("                           ru -> sh*_ru.hbk, en -> sh*_root.hbk");
    println!("                           env MCP_LANG_HELP_LANG, либо [knowledge] help_language");
    println!("\n    --platform-version <V> Версия платформы, если её не видно из пути");
    println!("                           env MCP_LANG_PLATFORM_VERSION");
    println!("\n    --out <PATH>           Только для index: выгрузить JSON базы (отладка)");
    println!("\nКОНФИГУРАЦИЯ:");
    println!("    Аргументы в приоритете, файл — запасной вариант: аргумент > env > файл > дефолт,");
    println!("    по каждому ключу отдельно. С --data-dir сервер работает без файла вообще.");
    println!("    Файлу остаются только тонкие настройки: [search], [logging] format.");
}

/// Относительный путь разворачивается от каталога знаний (а без него — от
/// каталога config-файла), абсолютный — как есть. Так запуск не зависит от
/// текущего каталога (cwd) хоста.
fn resolve_relative(base_dir: &Path, p: &Path) -> PathBuf {
    // Нормализуется и абсолютный путь: он тоже печатается человеку — каталог
    // платформы в отчёте `--check` и в стартовой записи журнала приходит
    // абсолютным, прямо из аргумента, и до этого показывался в том стиле, в
    // каком его набрали. Отчёт, где один путь со слэшами, а другой с обратными,
    // читается хуже любого из двух стилей по отдельности.
    if p.is_absolute() {
        normalize(p)
    } else {
        normalize(&base_dir.join(p))
    }
}

/// Приводит путь к виду, пригодному для показа человеку: без сегментов `.` и с
/// разделителями одного вида.
///
/// Косметика, но не только: путь печатается и в стартовой записи журнала, и в
/// отчёте `--check`. Дефолт `data_dir = "./data"` из шаблона давал
/// `<корень>\.\data`, а `--data-dir F:/knowledge` — `F:/knowledge\data`, где
/// первую половину набрал пользователь, а вторую дописала ОС. Человек, который
/// сверяет строку отчёта со своим каталогом, спотыкается и о лишний сегмент, и
/// о смену разделителя на середине.
///
/// Семантика пути не меняется: `.` — это тот же каталог, а Windows одинаково
/// принимает оба разделителя. `..` не трогаем: без обращения к файловой системе
/// его сворачивать нельзя, симлинк сделает результат неверным.
fn normalize(p: &Path) -> PathBuf {
    let cleaned: PathBuf = p
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect();

    if cleaned.as_os_str().is_empty() {
        return PathBuf::from(".");
    }

    // Только для валидного UTF-8: `to_string_lossy` на пути с суррогатами вернул
    // бы искажённое имя, а путь — это адрес, а не текст.
    #[cfg(windows)]
    if let Some(text) = cleaned.to_str() {
        if text.contains('/') {
            return PathBuf::from(text.replace('/', "\\"));
        }
    }

    cleaned
}

/// Непустое значение переменной окружения.
fn non_empty(value: std::result::Result<String, std::env::VarError>) -> Option<String> {
    value.ok().filter(|v| !v.is_empty())
}

/// `index` — собрать базу знаний из справки платформы и выйти.
///
/// Прогрев до подключения MCP-клиента: разбор стоит секунд, и пользователь
/// вправе оплатить их осознанно, а не ждать первого ответа сервера.
async fn run_index(knowledge: &config::KnowledgeConfig) -> Result<()> {
    let help_lang = hbk::lang::HelpLang::from_code(&knowledge.help_language).ok_or_else(|| {
        anyhow::anyhow!(
            "ru: Неизвестный язык справки «{}», поддерживаются: {}, en: Unknown help language '{}', supported: {}",
            knowledge.help_language,
            hbk::lang::CODES.join(", "),
            knowledge.help_language,
            hbk::lang::CODES.join(", ")
        )
    })?;

    let platform = server::resolve_platform(knowledge, help_lang)
        .map_err(|text| anyhow::anyhow!("{}", text))?;

    println!(
        "Платформа: {} (версия {}), язык справки {}",
        platform.bin_dir.display(),
        platform.version,
        help_lang.code
    );

    let progress = std::sync::Arc::new(hbk::Progress::new());
    progress.set_outcome(hbk::Outcome::Building);

    let worker_progress = progress.clone();
    let worker_platform = platform.clone();
    let mut build = Box::pin(tokio::task::spawn_blocking(move || {
        hbk::build(&worker_platform, help_lang, &worker_progress)
    }));

    // Тикер на время разбора: обычно это секунды, но на медленном диске молчащий
    // процесс неотличим от зависшего.
    let (document, report) = loop {
        tokio::select! {
            finished = &mut build => break finished??,
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                if let Some(hint) = progress.hint() {
                    eprintln!("   {}", hint);
                }
            }
        }
    };

    println!("{}", report);
    for book in &report.books {
        println!(
            "   {:<18} {} разделов, пропущено {}",
            book.book, book.taken, book.skipped
        );
    }

    match hbk::cache::store(&platform, help_lang, &document) {
        Ok(path) => println!("Кэш индекса: {}", path.display()),
        Err(e) => eprintln!("Кэш не записан: {}", e),
    }

    if let Some(out) = self_build_arg("--out") {
        hbk::cache::write_json(&document, Path::new(&out))?;
        println!("JSON базы знаний: {}", out);
    }

    Ok(())
}

/// `--check` — самопроверка установки.
///
/// Пришла на смену браузерному UI: тот показывал только уже поднявшийся сервер,
/// а отказы случаются раньше — платформа не найдена, версия не та, кэш не
/// пишется. Каждый пункт печатается фактом; неуспешный — с причиной и с тем,
/// что сделать, а код возврата отличает «работает» от «не работает».
///
/// Вердикт называет проверенный источник базы: у сервера их два — готовый
/// индекс по `--data-dir` и самосборка из платформы, — и «работоспособен» без
/// указания, что именно проверено, оставляет открытым главный вопрос.
fn run_check(knowledge: &config::KnowledgeConfig, data_dir_defaulted: bool) -> bool {
    println!("версия сервера: itcoolmcplang1c {}", mcp_common::build_version!());

    fn fail(item: &str, reason: &str, hint: &str) -> bool {
        println!("{item}: ОТКАЗ — {reason}");
        println!("   что делать: {hint}");
        println!("вердикт: сервер неработоспособен");
        false
    }

    // Готовый индекс по настроенному пути — приоритетный режим сервера: пока он
    // есть, платформа не нужна вовсе. Проверка обязана мерить то же, что делает
    // сервер, иначе прод-установка без платформы получила бы ложный отказ.
    let ready: Vec<knowledge::schema::Document> =
        match knowledge::loader::Loader::new(&knowledge.data_dir).load_all() {
            Ok(documents) => documents,
            Err(e) => {
                println!(
                    "каталог знаний: {} — не прочитан ({e})",
                    knowledge.data_dir.display()
                );
                Vec::new()
            }
        };

    let help_lang = match hbk::lang::HelpLang::from_code(&knowledge.help_language) {
        Some(lang) => lang,
        None => {
            return fail(
                "язык справки",
                &format!("неизвестный код «{}»", knowledge.help_language),
                &format!("укажите --help-lang {}", hbk::lang::CODES.join(" или ")),
            )
        }
    };

    let platform = server::resolve_platform(knowledge, help_lang);
    match (&platform, ready.is_empty()) {
        (Ok(p), _) => println!(
            "каталог платформы: найден — {}, версия платформы {}",
            p.bin_dir.display(),
            p.version
        ),
        (Err(text), false) => {
            println!("каталог платформы: не найден — и не требуется, работает готовый индекс");
            println!("   подробность: {text}");
        }
        // Ни того, ни другого: брать базу неоткуда, и подсказка обязана назвать
        // оба пути — человек, у которого нет платформы, чинит это не тем же
        // способом, что человек, забывший указать каталог с готовым индексом.
        (Err(text), true) => {
            return fail(
                "источник базы знаний",
                &format!(
                    "нет ни готового индекса в {}, ни установленной платформы ({text})",
                    knowledge.data_dir.display()
                ),
                "дайте одно из двух: каталог с готовым индексом — --data-dir <путь>, \
                 либо каталог bin установки платформы — --platform-dir \"C:\\Program Files\\1cv8\\<версия>\\bin\"",
            )
        }
    }

    let started = std::time::Instant::now();

    let (document, checked) = if let Some(document) = ready.into_iter().next() {
        println!(
            "индекс: готовый, из каталога знаний {} (версия базы {})",
            knowledge.data_dir.display(),
            document.version
        );
        println!(
            "справка: не читалась — платформа не нужна, пока есть готовый индекс; корневых разделов {}",
            document.sections.len()
        );
        let checked = format!("готовый индекс из {}", knowledge.data_dir.display());
        (document, checked)
    } else {
        let platform = platform.expect("отсутствие платформы разобрано выше");
        let cache_path = hbk::cache::path_for(&platform, help_lang);

        // Первый запуск идёт без `--data-dir`, и человек вправе знать, куда
        // сервер положит собранную базу, — иначе каталог придётся искать.
        if data_dir_defaulted {
            println!(
                "каталог знаний: взят по умолчанию — {} (--data-dir не задан)",
                knowledge.data_dir.display()
            );
        }

        match hbk::cache::load(&platform, help_lang) {
            Some(document) => {
                println!(
                    "справка: не перечитывалась — разобранный результат взят из кэша; корневых разделов {}",
                    document.sections.len()
                );
                println!(
                    "индекс: из кэша за {:.1} с — {}",
                    started.elapsed().as_secs_f32(),
                    cache_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "путь кэша не определён".to_string())
                );
                let checked = format!(
                    "самосборка из платформы {} (индекс взят из кэша)",
                    platform.bin_dir.display()
                );
                (document, checked)
            }
            None => {
                let progress = hbk::Progress::new();
                let (document, report) = match hbk::build(&platform, help_lang, &progress) {
                    Ok(pair) => pair,
                    Err(e) => {
                        return fail(
                            "справка",
                            &format!("разбор не удался: {e}"),
                            "проверьте, что каталог bin принадлежит установленной платформе и файлы sh*.hbk на месте",
                        )
                    }
                };

                let books = report
                    .books
                    .iter()
                    .map(|b| format!("{} — {} разделов", b.book, b.taken))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "справка: {} — объекты и методы; {}",
                    help_lang.context_book(),
                    if books.is_empty() {
                        "разделов языка не найдено".to_string()
                    } else {
                        books
                    }
                );

                match hbk::cache::store(&platform, help_lang, &document) {
                    Ok(path) => println!(
                        "индекс: собран заново за {:.1} с, кэш записан — {}",
                        started.elapsed().as_secs_f32(),
                        path.display()
                    ),
                    Err(e) => {
                        println!(
                            "индекс: собран заново за {:.1} с, но кэш НЕ записан — {e}",
                            started.elapsed().as_secs_f32()
                        );
                        println!(
                            "   что делать: проверьте права на каталог кэша{}",
                            cache_path
                                .as_ref()
                                .map(|p| format!(" ({})", p.display()))
                                .unwrap_or_default()
                        );
                        println!(
                            "   следствие: сервер работоспособен, но собирать базу будет при каждом запуске"
                        );
                    }
                }

                let checked = format!("самосборка из платформы {}", platform.bin_dir.display());
                (document, checked)
            }
        }
    };

    let properties: usize = document.api_objects.iter().map(|o| o.properties.len()).sum();
    println!(
        "числа: объектов {}, методов {}, свойств {}",
        document.api_objects.len(),
        document.api_methods.len(),
        properties
    );

    // Тестовый запрос идёт на языке справки: в базе из sh*_root.hbk русского
    // имени метода нет, и «ЗначениеЗаполнено» не нашлось бы по причине, к
    // работоспособности отношения не имеющей.
    let probe = if help_lang.code == "ru" {
        "ЗначениеЗаполнено"
    } else {
        "ValueIsFilled"
    };

    let mut base = knowledge::KnowledgeBase::new();
    if let Err(e) = base.add_document(document) {
        return fail(
            "индекс",
            &format!("база знаний не приняла документ: {e}"),
            "индекс собран, но не проходит валидацию — сообщите владельцу",
        );
    }

    let found = base.search(&knowledge::SearchQuery {
        query: probe,
        version: None,
        context: None,
        include_related: false,
        max_results: 1,
    });

    match found.first() {
        Some(result) => println!(
            "тестовый запрос «{probe}»: найдено, первый результат — {}",
            result_name(&result.source)
        ),
        None => {
            return fail(
                "тестовый запрос",
                &format!("«{probe}» в собранной базе не найден"),
                "проверьте язык справки (--help-lang) и версию платформы",
            )
        }
    }

    println!("вердикт: Сервер работоспособен — проверено: {checked}");
    true
}

/// Имя найденного — для строки отчёта `--check`.
fn result_name(source: &knowledge::schema::SearchSource) -> &str {
    use knowledge::schema::SearchSource;

    match source {
        SearchSource::Section { title, .. } => title,
        SearchSource::ApiMethod { name, .. } => name,
        SearchSource::ApiObject { name, .. } => name,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    if cli::flag("--help") {
        print_help();
        return Ok(());
    }
    if cli::flag("--version") {
        println!("itcoolmcplang1c v{}", mcp_common::build_version!());
        return Ok(());
    }

    // Прогрев индекса и самопроверка — команды, а не запуск сервера: каталог
    // знаний им не нужен.
    let index_mode = std::env::args().nth(1).as_deref() == Some("index");
    let check_mode = cli::flag("--check");

    let root = resolve_root();
    let root_path = root.as_ref().map(|s| PathBuf::from(&s.value));
    let config_path = resolve_config_path(root_path.as_deref());
    let config_exists = Path::new(&config_path).exists();
    let mut config = Config::load_or_default(&config_path)?;

    // Самосборка задана — каталог знаний перестаёт быть обязательным: базу
    // соберёт сам сервер, и класть её некуда, кроме пользовательского каталога,
    // где уже лежит кэш индекса. Так сервер запускается по README одним
    // аргументом `--platform-dir`, как его и поднимает MCP-клиент на первом
    // запуске.
    let self_build_requested = self_build_arg("--platform-dir").is_some()
        || non_empty(std::env::var("MCP_LANG_PLATFORM_DIR")).is_some()
        || config.knowledge.platform_dir.is_some();

    // `None` — каталога пользователя нет (нет LOCALAPPDATA/HOME): выбрать дефолт
    // не из чего, и `--data-dir` снова обязателен.
    let default_root = if self_build_requested && root_path.is_none() && !config_exists {
        hbk::cache::root()
    } else {
        None
    };

    // База отсчёта относительных путей — каталог знаний: переданный, а при
    // самосборке без него — тот, что выбран по умолчанию. Иначе каталог конфига.
    // Ни в одном случае не cwd.
    let base_dir = root_path
        .clone()
        .or_else(|| default_root.clone())
        .unwrap_or_else(|| {
            Path::new(&config_path)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default()
        });

    // Каталог знаний и промпты. Без корня и без конфига стартовать нечем: сервер
    // поднялся бы с пустой базой и отвечал на `initialize` неотличимо от рабочего.
    if root_path.is_none()
        && !config_exists
        && !index_mode
        && !check_mode
        && default_root.is_none()
    {
        return Err(anyhow::anyhow!(
            "{}",
            cli::Missing {
                key: "каталог базы знаний".to_string(),
                arg: "--data-dir".to_string(),
                env: "MCP_LANG_DATA_DIR".to_string(),
                file_hint: "[knowledge] data_dir".to_string(),
            }
        ));
    }

    let data_dir = match (&root_path, config_exists, &default_root) {
        // Корень задан, конфига в нём нет — раскладка по умолчанию.
        (Some(root), false, _) => knowledge_dir_in(root),
        // Явный `--data-dir` главнее дефолта (так его передаёт core), поэтому
        // эта ветка — только для случая, когда корень не задан вовсе.
        (None, false, Some(default_root)) => default_root.clone(),
        // Конфиг есть: он описывает свою базу, его путь разворачивается от корня.
        _ => resolve_relative(&base_dir, &config.knowledge.data_dir),
    };

    // Разрешение по каждому ключу: аргумент > env > файл > дефолт. Файл участвует
    // только теми ключами, которые в нём есть, и не отменяет аргументы.
    let resolver = cli::Resolver::new(config_exists.then(|| PathBuf::from(&config_path)));

    let prompts = resolver.get_or(
        "--prompts",
        "MCP_LANG_PROMPTS",
        config_exists.then(|| config.prompts.template_file.to_string_lossy().to_string()),
        "prompts.json",
    );
    let log_level = resolver.get_or(
        "--log-level",
        "MCP_LANG_LOG_LEVEL",
        Some(config.logging.level.clone()),
        "info",
    );

    // Ключи самосборки: аргумент > env > файл. Путь к платформе разворачивается
    // от каталога знаний так же, как остальные относительные пути.
    if let Some(dir) = self_build_arg("--platform-dir")
        .or_else(|| non_empty(std::env::var("MCP_LANG_PLATFORM_DIR")))
    {
        config.knowledge.platform_dir = Some(PathBuf::from(dir));
    }
    config.knowledge.platform_dir = config
        .knowledge
        .platform_dir
        .as_deref()
        .map(|dir| resolve_relative(&base_dir, dir));

    if let Some(code) =
        self_build_arg("--help-lang").or_else(|| non_empty(std::env::var("MCP_LANG_HELP_LANG")))
    {
        config.knowledge.help_language = code;
    }
    if let Some(version) = self_build_arg("--platform-version")
        .or_else(|| non_empty(std::env::var("MCP_LANG_PLATFORM_VERSION")))
    {
        config.knowledge.platform_version = Some(version);
    }

    config.knowledge.data_dir = normalize(&data_dir);
    config.prompts.template_file = resolve_relative(&base_dir, Path::new(&prompts.value));
    config.logging.level = log_level.value.clone();

    // Инициализация логирования
    init_logging(&config.logging);

    if index_mode {
        return run_index(&config.knowledge).await;
    }

    if check_mode {
        let ok = run_check(&config.knowledge, default_root.is_some());
        std::process::exit(if ok { 0 } else { 1 });
    }

    // Транспорт один. Конфиг, перенесённый с 0.6.x, обязан получить отказ здесь,
    // до загрузки базы: тихая подмена на stdio означала бы, что сервер работает
    // не так, как написано в файле.
    server::validate_mode(&config.server.mode)?;

    tracing::info!(
        "ru: Запуск MCP сервера itcoolmcplang1c v{}, en: Starting itcoolmcplang1c MCP server v{}",
        mcp_common::build_version!(),
        mcp_common::build_version!()
    );

    // Что разрешилось и откуда: когда сервер читает не тот каталог, именно
    // источник каждого ключа отвечает на вопрос «почему пусто».
    tracing::info!("ru: Параметры запуска, en: Startup settings:");
    let data_dir_source = match (&root, config_exists, &default_root) {
        (Some(setting), false, _) => setting.source.to_string(),
        (_, true, _) => Source::File(PathBuf::from(&config_path)).to_string(),
        (None, false, Some(_)) => "по умолчанию: каталог кэша индекса".to_string(),
        (None, false, None) => Source::Default.to_string(),
    };
    for line in [
        match (&root, &default_root) {
            (Some(setting), _) => setting.report("knowledge-root"),
            (None, Some(path)) => format!(
                "knowledge-root = {} (не задан, взят каталог кэша индекса)",
                path.display()
            ),
            (None, None) => "knowledge-root = (не задан, взят каталог конфига)".to_string(),
        },
        format!(
            "config = {} ({})",
            config_path,
            if config_exists {
                "найден"
            } else {
                "не найден, тонкие настройки на дефолтах"
            }
        ),
        format!(
            "data-dir = {} ({})",
            config.knowledge.data_dir.display(),
            data_dir_source
        ),
        format!(
            "prompts = {} ({})",
            config.prompts.template_file.display(),
            prompts.source
        ),
        match &config.knowledge.platform_dir {
            Some(dir) => format!("platform-dir = {}", dir.display()),
            None => "platform-dir = (не задан; автопоиск, если база пуста)".to_string(),
        },
        format!("help-lang = {}", config.knowledge.help_language),
        format!("mode = {}", config.server.mode),
        log_level.report("log-level"),
    ] {
        tracing::info!("    {}", line);
    }

    // DataManager чисто диагностический (лог пути и занятого места), поэтому его
    // отказ не валит запуск: Loader тот же отсутствующий каталог толерирует
    // пустой базой.
    match DataManager::new(config.knowledge.data_dir.clone()) {
        Ok(data_manager) => {
            tracing::info!(
                "ru: DataManager инициализирован, путь к данным: {:?}, en: DataManager initialized, data path: {:?}",
                data_manager.data_path(),
                data_manager.data_path()
            );

            if let Ok(usage) = data_manager.get_disk_usage() {
                tracing::info!(
                    "ru: Использование диска - всего: {}, языки: {}, кэш: {}, логи: {}, en: Disk usage - total: {}, languages: {}, cache: {}, logs: {}",
                    data_manager::DiskUsage::format_size(usage.total),
                    data_manager::DiskUsage::format_size(usage.languages),
                    data_manager::DiskUsage::format_size(usage.cache),
                    data_manager::DiskUsage::format_size(usage.logs),
                    data_manager::DiskUsage::format_size(usage.total),
                    data_manager::DiskUsage::format_size(usage.languages),
                    data_manager::DiskUsage::format_size(usage.cache),
                    data_manager::DiskUsage::format_size(usage.logs),
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                "ru: DataManager не инициализирован ({}), сервер продолжает запуск, en: DataManager not initialized ({}), server startup continues",
                e,
                e
            );
        }
    }

    // Создание и инициализация сервера
    let server = Server::new(config);
    server.initialize().await?;

    // Запуск сервера
    server.run().await?;

    tracing::info!("ru: Сервер остановлен, en: Server stopped");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Конфиг ищется внутри каталога знаний — один аргумент подтягивает всё.
    #[test]
    fn config_is_found_inside_knowledge_root() {
        let temp = tempfile::TempDir::new().unwrap();
        let expected = temp.path().join("itcoolmcplang1c.toml");
        std::fs::write(&expected, "[search]\nmax_results = 5\n").unwrap();

        let found = resolve_config_path(Some(temp.path()));

        assert_eq!(PathBuf::from(found), expected);
    }

    /// Имя `config.toml` остаётся фолбэком: так назывался конфиг до 0.3.0, и
    /// боевые каталоги знаний до сих пор содержат именно его.
    #[test]
    fn legacy_config_name_is_still_found_in_root() {
        let temp = tempfile::TempDir::new().unwrap();
        let expected = temp.path().join("config.toml");
        std::fs::write(&expected, "[search]\nmax_results = 5\n").unwrap();

        let found = resolve_config_path(Some(temp.path()));

        assert_eq!(PathBuf::from(found), expected);
    }

    /// Конфига в каталоге знаний нет — путь всё равно указывает внутрь него, а
    /// не рядом с exe: иначе сервер подхватил бы чужой файл.
    #[test]
    fn missing_config_still_points_inside_root() {
        let temp = tempfile::TempDir::new().unwrap();

        let found = resolve_config_path(Some(temp.path()));

        assert_eq!(
            PathBuf::from(found),
            temp.path().join("itcoolmcplang1c.toml")
        );
    }

    /// Вложенный `data` предпочтительнее корня: загрузчик рекурсивный, и корень
    /// затянул бы соседний `backup` — документы одной версии затирают друг друга.
    #[test]
    fn nested_data_dir_wins_over_root() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(temp.path().join("data")).unwrap();
        std::fs::create_dir(temp.path().join("backup")).unwrap();

        assert_eq!(knowledge_dir_in(temp.path()), temp.path().join("data"));
    }

    /// Плоская раскладка (JSON прямо в корне) тоже должна подниматься.
    #[test]
    fn flat_layout_falls_back_to_root() {
        let temp = tempfile::TempDir::new().unwrap();

        assert_eq!(knowledge_dir_in(temp.path()), temp.path());
    }

    #[test]
    fn absolute_path_ignores_base_dir() {
        let base = Path::new("X:/knowledge");
        let absolute = Path::new("Y:/elsewhere/data");

        let resolved = resolve_relative(base, absolute);

        assert_eq!(resolved, normalize(absolute), "база отсчёта не при делах");
        assert!(
            !resolved.starts_with(base),
            "абсолютный путь не разворачивается от базы: {resolved:?}"
        );
    }

    #[test]
    fn relative_path_unfolds_from_knowledge_root() {
        let base = Path::new("X:/knowledge");

        assert_eq!(resolve_relative(base, Path::new("./data")), base.join("data"));
    }

    /// `./data` из шаблона конфига печатался в журнале и в отчёте `--check`
    /// как `<корень>\.\data`. Сегмент безвреден для файловой системы и мешает
    /// человеку, который сверяет строку отчёта со своим каталогом.
    #[test]
    fn current_dir_segments_do_not_reach_the_report() {
        let base = Path::new("X:/knowledge");

        for input in ["./data", "data/./", "././data"] {
            let resolved = resolve_relative(base, Path::new(input));

            assert!(
                !resolved
                    .components()
                    .any(|c| matches!(c, std::path::Component::CurDir)),
                "сегмент «.» остался в {resolved:?} (вход {input})"
            );
        }

        // Пустой результат — всё ещё путь: `.` лучше пустой строки в отчёте.
        assert_eq!(normalize(Path::new("./")), Path::new("."));
    }

    /// `--data-dir F:/knowledge` печатался как `F:/knowledge\data`: половину
    /// набрал человек, половину дописала ОС. Для чтения это хуже любого из
    /// двух стилей по отдельности.
    #[cfg(windows)]
    #[test]
    fn separators_are_one_kind_on_windows() {
        let resolved = normalize(&Path::new("F:/knowledge").join("data"));

        assert_eq!(resolved, Path::new(r"F:\knowledge\data"));
        assert!(
            !resolved.to_str().unwrap().contains('/'),
            "в пути остался чужой разделитель: {resolved:?}"
        );
    }
}
