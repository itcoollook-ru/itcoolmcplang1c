//! hbk — самосборка базы знаний из справки установленной платформы 1С.
//!
//! Справка не покидает машину пользователя: сервер читает `.hbk` из каталога
//! `bin` его собственной установки и строит [`Document`] в памяти. В поставке
//! нет и не будет ни одного файла справки.
//!
//! Слои: [`container`] — контейнер 1С, [`extract`] — страницы справки в схему,
//! [`html`] — разметка в текст, [`lang`] — маркеры разбора по языку справки,
//! [`platform`] — где лежит установка, [`cache`] — готовый индекс между запусками.

pub mod cache;
pub mod container;
pub mod extract;
pub mod html;
pub mod lang;
pub mod platform;

use crate::knowledge::schema::Document;
use anyhow::Result;
use lang::HelpLang;
use platform::Platform;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

/// Чем занят сборщик индекса.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Сборка не запускалась или уже не нужна: база знаний на месте.
    Idle,
    Building,
    /// Платформа не найдена — текст уже содержит инструкцию.
    NoPlatform(String),
    Failed(String),
    Ready,
}

/// Ход сборки, видимый обработчику запросов.
///
/// Нужен затем, что индексация не может выполняться в `initialize`: MCP-клиент
/// ждёт рукопожатия с таймаутом. Пока индекс строится,
/// инструменты отвечают мягким отказом с процентом, а не ошибкой контура.
pub struct Progress {
    percent: AtomicU8,
    stage: Mutex<&'static str>,
    outcome: Mutex<Outcome>,
}

impl Progress {
    pub fn new() -> Self {
        Self {
            percent: AtomicU8::new(0),
            stage: Mutex::new(""),
            outcome: Mutex::new(Outcome::Idle),
        }
    }

    pub fn outcome(&self) -> Outcome {
        match self.outcome.lock() {
            Ok(guard) => guard.clone(),
            // Отравленный мьютекс здесь не опасен: внутри обычное перечисление,
            // и наблюдать состояние важнее, чем ловить панику чужого потока.
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn set_outcome(&self, outcome: Outcome) {
        match self.outcome.lock() {
            Ok(mut guard) => *guard = outcome,
            Err(poisoned) => *poisoned.into_inner() = outcome,
        }
    }

    /// Доля внутри этапа, пересчитанная в общий процент.
    pub fn stage_percent(&self, stage: &'static str, done: usize, total: usize, from: usize, to: usize) {
        let percent = if total == 0 {
            to
        } else {
            from + (to - from) * done.min(total) / total
        };
        let percent = percent.min(100) as u8;

        if self.percent.swap(percent, Ordering::Relaxed) != percent {
            match self.stage.lock() {
                Ok(mut guard) => *guard = stage,
                Err(poisoned) => *poisoned.into_inner() = stage,
            }
        }
    }

    /// Текст для мягкого отказа инструмента; `None` — база готова, отвечать
    /// нужно по существу.
    pub fn hint(&self) -> Option<String> {
        match self.outcome() {
            Outcome::Idle | Outcome::Ready => None,
            Outcome::Building => {
                let stage = match self.stage.lock() {
                    Ok(guard) => *guard,
                    Err(poisoned) => *poisoned.into_inner(),
                };
                Some(format!(
                    "Идёт индексация справки платформы: {}% ({}). Повторите запрос через \
                     несколько секунд — разбор справки занимает секунды, его результат \
                     кэшируется, и следующий запуск сервера будет мгновенным.",
                    self.percent.load(Ordering::Relaxed),
                    if stage.is_empty() { "подготовка" } else { stage }
                ))
            }
            Outcome::NoPlatform(text) => Some(text),
            Outcome::Failed(text) => Some(format!(
                "Индексация справки платформы не удалась: {}. База знаний пуста.",
                text
            )),
        }
    }
}

impl Default for Progress {
    fn default() -> Self {
        Self::new()
    }
}

/// Итог сборки — для журнала и отчёта CLI.
#[derive(Debug)]
pub struct BuildReport {
    pub version: String,
    pub objects: usize,
    pub methods: usize,
    pub properties: usize,
    pub root_sections: usize,
    pub subsections: usize,
    pub skipped: extract::Skipped,
    pub books: Vec<extract::BookStats>,
}

impl std::fmt::Display for BuildReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "версия {}: объектов {}, методов {}, свойств {}, разделов {} корневых / {}; \
             пропущено категорий {}, дублей методов {}, без владельца {}",
            self.version,
            self.objects,
            self.methods,
            self.properties,
            self.root_sections,
            self.subsections,
            self.skipped.category,
            self.skipped.dup_method,
            self.skipped.no_owner
        )
    }
}

/// Сборка базы знаний из справки платформы.
///
/// Разделы языка живут в том же документе: второй файл с этой же версией молча
/// затёр бы первый — база знаний ключует документы версией.
pub fn build(
    platform: &Platform,
    help_lang: &'static HelpLang,
    progress: &Progress,
) -> Result<(Document, BuildReport)> {
    let version = platform.document_version();
    let extractor = extract::Extractor::new(help_lang)?;

    let context_book = platform.bin_dir.join(help_lang.context_book());
    tracing::info!(
        "ru: Сборка базы знаний из {:?} (язык {}), en: Building knowledge base from {:?} (language {})",
        context_book,
        help_lang.code,
        context_book,
        help_lang.code
    );

    let mut storage = extract::open_storage(&context_book)?;
    let (mut document, skipped) = extractor.extract(&mut storage, &version, progress)?;
    let (sections, books) = extractor.extract_sections(&platform.bin_dir, progress)?;
    document.sections = sections;

    progress.stage_percent("done", 1, 1, 100, 100);

    let report = BuildReport {
        version,
        objects: document.api_objects.len(),
        methods: document.api_methods.len(),
        properties: document.api_objects.iter().map(|o| o.properties.len()).sum(),
        root_sections: document.sections.len(),
        subsections: document.sections.iter().map(|s| s.subsections.len()).sum(),
        skipped,
        books,
    };

    Ok((document, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пока идёт сборка, инструмент обязан отвечать текстом с процентом, а не
    /// пустой выдачей: иначе модель решит, что метода нет в платформе.
    #[test]
    fn building_state_reports_percent() {
        let progress = Progress::new();
        progress.set_outcome(Outcome::Building);
        progress.stage_percent("members", 1, 2, 0, 100);

        let hint = progress.hint().unwrap();

        assert!(hint.contains("50%"), "{}", hint);
        assert!(hint.contains("members"), "{}", hint);
    }

    /// Готовая база подсказку не выдаёт — отвечаем по существу.
    #[test]
    fn ready_state_has_no_hint() {
        let progress = Progress::new();
        progress.set_outcome(Outcome::Ready);

        assert_eq!(progress.hint(), None);
    }

    /// Отсутствие платформы — инструкция, а не пустая выдача и не паника.
    #[test]
    fn missing_platform_keeps_its_instruction() {
        let progress = Progress::new();
        progress.set_outcome(Outcome::NoPlatform("укажите --platform-dir".to_string()));

        assert_eq!(progress.hint().as_deref(), Some("укажите --platform-dir"));
    }

    #[test]
    fn stage_percent_never_exceeds_the_range() {
        let progress = Progress::new();
        progress.set_outcome(Outcome::Building);
        progress.stage_percent("objects", 999, 10, 2, 35);

        assert!(progress.hint().unwrap().contains("35%"));
    }
}
