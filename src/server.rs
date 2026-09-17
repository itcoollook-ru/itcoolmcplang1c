use crate::config::{Config, KnowledgeConfig};
use crate::hbk::lang::HelpLang;
use crate::hbk::platform::Platform;
use crate::hbk::{cache, platform, Outcome, Progress};
use crate::knowledge::KnowledgeBase;
use crate::prompts::PromptsEngine;
use crate::protocol::ProtocolHandler;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Главный координатор сервера
pub struct Server {
    config: Config,
    knowledge_base: Arc<RwLock<KnowledgeBase>>,
    prompts_engine: Arc<RwLock<PromptsEngine>>,
    progress: Arc<Progress>,
    handler: Arc<ProtocolHandler>,
}

impl Server {
    /// Создание нового сервера
    pub fn new(config: Config) -> Self {
        // Настройки поиска доезжают до индекса и обработчика: до 0.3.0 секция
        // [search] объявлялась в конфиге и не читалась ни тем, ни другим.
        let knowledge_base = Arc::new(RwLock::new(KnowledgeBase::with_boosts(
            config.search.boost_title,
            config.search.boost_keywords,
        )));
        let prompts_engine = Arc::new(RwLock::new(PromptsEngine::new()));
        let progress = Arc::new(Progress::new());

        let handler = Arc::new(ProtocolHandler::with_max_results(
            knowledge_base.clone(),
            prompts_engine.clone(),
            progress.clone(),
            config.search.max_results.max(1),
        ));

        Self {
            config,
            knowledge_base,
            prompts_engine,
            progress,
            handler,
        }
    }

    /// Инициализация сервера
    pub async fn initialize(&self) -> Result<()> {
        tracing::info!("ru: Инициализация сервера, en: Initializing server");

        // Готовый индекс по настроенному пути — приоритетный режим: им живёт
        // прод. Платформа не трогается, пока база по этому пути есть.
        let loaded = {
            let mut kb = self.knowledge_base.write().await;
            kb.load_from_directory(&self.config.knowledge.data_dir)?;

            let stats = kb.statistics();
            tracing::info!(
                "ru: Загружено документов: {}, методов: {}, объектов: {}, en: Loaded {} documents, {} methods, {} objects",
                stats.total_documents,
                stats.total_api_methods,
                stats.total_api_objects,
                stats.total_documents,
                stats.total_api_methods,
                stats.total_api_objects
            );
            stats.total_documents
        };

        if loaded > 0 {
            self.progress.set_outcome(Outcome::Ready);
        } else {
            self.start_self_build().await;
        }

        // Загрузка промптов. Пустая коллекция старт не валит — core работает и
        // без промптов, — но должна быть видна: сервер, объявивший capability
        // `prompts` с пустым списком, невалиден, поэтому capability снимается
        // (см. `capabilities::get_server_capabilities`), а причина пишется в лог.
        {
            let mut pe = self.prompts_engine.write().await;
            pe.load_from_file(&self.config.prompts.template_file)?;

            if pe.list_prompts().is_empty() {
                tracing::warn!(
                    "ru: Промпты не загружены ({:?}) — capability prompts не объявляется, en: No prompts loaded ({:?}) — prompts capability is not advertised",
                    self.config.prompts.template_file,
                    self.config.prompts.template_file
                );
            }
        }

        tracing::info!("ru: Инициализация завершена, en: Initialization complete");

        Ok(())
    }

    /// Самосборка базы знаний из справки установленной платформы.
    ///
    /// # CRITICAL_LOGIC (не трогать при рефакторинге)
    /// Разбор справки **не может** выполняться синхронно в обработчике
    /// `initialize`: MCP-клиент ждёт рукопожатия с таймаутом, а время разбора
    /// зависит от диска и версии платформы. Здесь
    /// синхронно делается только чтение готового кэша; сама сборка уходит в
    /// фоновую задачу, а инструменты до её конца отвечают мягким отказом с
    /// прогрессом.
    async fn start_self_build(&self) {
        let knowledge = &self.config.knowledge;

        let Some(help_lang) = HelpLang::from_code(&knowledge.help_language) else {
            let text = format!(
                "Неизвестный язык справки «{}». Поддерживаются: {}.",
                knowledge.help_language,
                crate::hbk::lang::CODES.join(", ")
            );
            tracing::error!("{}", text);
            self.progress.set_outcome(Outcome::NoPlatform(text));
            return;
        };

        let platform = match resolve_platform(knowledge, help_lang) {
            Ok(platform) => platform,
            Err(text) => {
                tracing::warn!("{}", text);
                self.progress.set_outcome(Outcome::NoPlatform(text));
                return;
            }
        };

        tracing::info!(
            "ru: Платформа для самосборки: {:?} (версия {}), en: Platform for self-build: {:?} (version {})",
            platform.bin_dir,
            platform.version,
            platform.bin_dir,
            platform.version
        );

        if let Some(document) = cache::load(&platform, help_lang) {
            let mut kb = self.knowledge_base.write().await;
            match kb.add_document(document) {
                Ok(()) => {
                    self.progress.set_outcome(Outcome::Ready);
                    return;
                }
                Err(e) => tracing::warn!(
                    "ru: Кэш индекса не принят ({}), пересборка, en: Cached index rejected ({}), rebuilding",
                    e,
                    e
                ),
            }
        }

        self.progress.set_outcome(Outcome::Building);

        let knowledge_base = self.knowledge_base.clone();
        let progress = self.progress.clone();
        tokio::spawn(async move {
            let worker_progress = progress.clone();
            let worker_platform = platform.clone();
            // Разбор — сплошной CPU и чтение файлов: в blocking-пуле, чтобы не
            // держать поток исполнителя, обслуживающий stdio.
            let built = tokio::task::spawn_blocking(move || {
                crate::hbk::build(&worker_platform, help_lang, &worker_progress)
            })
            .await;

            match built {
                Ok(Ok((document, report))) => {
                    tracing::info!("ru: База знаний собрана: {}, en: Knowledge base built: {}", report, report);
                    match cache::store(&platform, help_lang, &document) {
                        Ok(path) => tracing::info!(
                            "ru: Индекс сохранён в кэш: {:?}, en: Index cached at {:?}",
                            path, path
                        ),
                        Err(e) => tracing::warn!(
                            "ru: Кэш не записан ({}), следующий запуск снова соберёт индекс, en: Cache not written ({}), next start will rebuild",
                            e, e
                        ),
                    }

                    let mut kb = knowledge_base.write().await;
                    match kb.add_document(document) {
                        Ok(()) => progress.set_outcome(Outcome::Ready),
                        Err(e) => progress.set_outcome(Outcome::Failed(e.to_string())),
                    }
                }
                Ok(Err(e)) => {
                    tracing::error!("ru: Сборка базы знаний сорвалась: {}, en: Knowledge base build failed: {}", e, e);
                    progress.set_outcome(Outcome::Failed(e.to_string()));
                }
                Err(e) => {
                    tracing::error!("ru: Задача сборки не завершилась: {}, en: Build task did not finish: {}", e, e);
                    progress.set_outcome(Outcome::Failed(e.to_string()));
                }
            }
        });
    }

    /// Запуск сервера
    pub async fn run(&self) -> Result<()> {
        let mode = &self.config.server.mode;

        validate_mode(mode)?;

        tracing::info!(
            "ru: Запуск сервера в режиме: {}, en: Starting server in mode: {}",
            mode,
            mode
        );

        self.run_stdio().await
    }

    async fn run_stdio(&self) -> Result<()> {
        mcp_common::run_stdio(move |request| {
            let handler = self.handler.clone();
            async move { handler.handle_request(request).await }
        })
        .await?;

        Ok(())
    }
}

/// Установка платформы: заданная явно либо найденная автопоиском.
///
/// `Err` — готовый текст с инструкцией: он же уходит клиенту мягким отказом,
/// поэтому «платформа не найдена» обязано объяснять, что делать.
/// Единственный допустимый транспорт.
///
/// # CRITICAL_LOGIC (не трогать при рефакторинге)
/// Проверка живёт одной функцией и зовётся из двух мест — из `main` до загрузки
/// базы и из `Server::run`. Молча подставить stdio нельзя: конфиг, перенесённый
/// с 0.6.x, обязан сказать владельцу, что его настройка не действует, — иначе
/// сервер работает не так, как написано в файле, и об этом никто не узнает.
pub fn validate_mode(mode: &str) -> Result<()> {
    if mode == "stdio" {
        return Ok(());
    }

    Err(anyhow::anyhow!("{}", unsupported_mode(mode)))
}

/// Текст отказа для режима, которого больше нет.
fn unsupported_mode(mode: &str) -> String {
    format!(
        "ru: Режим [server] mode = \"{mode}\" не поддерживается: HTTP-транспорт удалён, \
сервер работает только по stdio. Поставьте mode = \"stdio\" или уберите ключ, \
en: Mode [server] mode = \"{mode}\" is not supported: the HTTP transport has been removed, \
the server speaks stdio only. Set mode = \"stdio\" or drop the key"
    )
}

pub fn resolve_platform(
    knowledge: &KnowledgeConfig,
    help_lang: &'static HelpLang,
) -> Result<Platform, String> {
    match &knowledge.platform_dir {
        Some(dir) => platform::from_dir(dir, help_lang, knowledge.platform_version.as_deref())
            .map_err(|e| e.to_string()),
        None => platform::discover(help_lang).ok_or_else(|| platform::not_found_hint(help_lang)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Конфиг с 0.6.x приносит "http" или "both". Это отказ, а не тихий stdio:
    /// сервер, работающий не так, как написано в файле пользователя, хуже
    /// сервера, который не запустился и сказал почему.
    #[test]
    fn removed_modes_are_refused() {
        assert!(validate_mode("stdio").is_ok(), "stdio — единственный рабочий режим");

        for mode in ["http", "both", "grpc"] {
            let error = validate_mode(mode)
                .expect_err(&format!("режим «{mode}» не поддерживается и обязан быть отказом"));
            let text = format!("{error}");

            assert!(text.contains(mode), "отказ должен называть режим: {text}");
            assert!(
                text.contains("stdio"),
                "отказ должен называть поддерживаемый режим: {text}"
            );
        }
    }
}
