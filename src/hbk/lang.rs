//! lang.rs — язык справки: имена файлов книг и маркеры разбора страниц.
//!
//! Разбор опознаёт разделы страницы по их заголовкам, а заголовки в справке
//! написаны на её языке. Поэтому выбор языка — это не только суффикс имени
//! файла (`_ru` / `_root`), но и вся таблица маркеров: с русскими маркерами
//! английская справка разобралась бы в пустую базу.

use crate::knowledge::schema::SectionType;

/// Книга языка рядом с `shcntx`: шаблон имени, префикс идентификаторов,
/// заголовок корневой секции и тип её подсекций.
pub struct Book {
    pub stem: &'static str,
    pub prefix: &'static str,
    pub title: &'static str,
    pub kind: SectionType,
}

impl Book {
    /// Имя файла книги для выбранного языка: `shlang` + `_ru` → `shlang_ru.hbk`.
    pub fn file_name(&self, lang: &HelpLang) -> String {
        format!("{}_{}.hbk", self.stem, lang.suffix)
    }
}

/// Таблица маркеров одного языка справки.
pub struct HelpLang {
    /// Код языка, как его задаёт пользователь: `ru` / `en`.
    pub code: &'static str,
    /// Суффикс имени файла справки: `ru` / `root`.
    pub suffix: &'static str,

    // Заголовки глав страницы (`<p class="V8SH_chapter">`).
    pub ch_syntax_variant: &'static str,
    pub ch_syntax: &'static str,
    pub ch_parameters: &'static str,
    pub ch_returns: &'static str,
    pub ch_description: &'static str,
    pub ch_description_variant: &'static str,
    pub ch_availability: &'static str,
    pub ch_example: &'static str,
    pub ch_note: &'static str,
    pub ch_usage: &'static str,
    pub ch_properties: &'static str,
    pub ch_methods: &'static str,

    /// Слово перед типом в строке «Тип: Число.» — в регулярном выражении.
    pub type_word: &'static str,
    /// Начало пометки обязательности параметра.
    pub required: &'static str,
    /// Пометка режима использования свойства (в нижнем регистре).
    pub read_only: &'static str,
    /// Слово доступности, означающее клиента (в нижнем регистре).
    pub client: &'static str,
    /// Слова доступности, означающие сервер (в нижнем регистре).
    pub server: &'static [&'static str],
    /// Как называется вызов конструктора: `Новый Объект` / `New Object`.
    pub ctor_word: &'static str,

    // Подписи, попадающие в готовую базу знаний.
    pub note_syntax: &'static str,
    pub note_returns: &'static str,
    pub note_note: &'static str,
    /// Шаблон примечания о варианте синтаксиса: `{name}` и `{syntax}`.
    pub note_variant: &'static str,
    /// Шаблон содержимого корневой секции: `{title}` и `{book}`.
    pub section_blurb: &'static str,

    pub books: &'static [Book],
}

impl HelpLang {
    /// Язык по коду; `None` — код неизвестен.
    pub fn from_code(code: &str) -> Option<&'static HelpLang> {
        match code.trim().to_lowercase().as_str() {
            "ru" => Some(&RU),
            "en" => Some(&EN),
            _ => None,
        }
    }

    /// Имя точки входа: `shcntx_ru.hbk` / `shcntx_root.hbk`.
    pub fn context_book(&self) -> String {
        format!("shcntx_{}.hbk", self.suffix)
    }

    /// Примечание о варианте синтаксиса.
    pub fn variant_note(&self, name: &str, syntax: &str) -> String {
        self.note_variant
            .replace("{name}", name)
            .replace("{syntax}", syntax)
    }

    /// Содержимое корневой секции книги.
    pub fn blurb(&self, title: &str, book: &str) -> String {
        self.section_blurb
            .replace("{title}", title)
            .replace("{book}", book)
    }
}

/// Перечень поддерживаемых кодов — для справки и сообщений об ошибке.
pub const CODES: [&str; 2] = ["ru", "en"];

static RU: HelpLang = HelpLang {
    code: "ru",
    suffix: "ru",
    ch_syntax_variant: "Вариант синтаксиса",
    ch_syntax: "Синтаксис",
    ch_parameters: "Параметры",
    ch_returns: "Возвращаемое значение",
    ch_description: "Описание",
    ch_description_variant: "Описание варианта метода",
    ch_availability: "Доступность",
    ch_example: "Пример",
    ch_note: "Примечание",
    ch_usage: "Использование",
    ch_properties: "Свойства",
    ch_methods: "Методы",
    type_word: "Тип",
    required: "обязательный",
    read_only: "только чтение",
    client: "клиент",
    server: &["сервер", "внешнее соединение", "интеграция"],
    ctor_word: "Новый",
    note_syntax: "Синтаксис: ",
    note_returns: "Возвращает: ",
    note_note: "Примечание: ",
    note_variant: "Вариант синтаксиса «{name}»: {syntax}",
    section_blurb: "{title}: справка платформы ({book}).",
    books: &[
        Book {
            stem: "shlang",
            prefix: "lang",
            title: "Встроенный язык",
            kind: SectionType::Text,
        },
        Book {
            stem: "shclang",
            prefix: "clang",
            title: "Общие правила языка",
            kind: SectionType::Text,
        },
        Book {
            stem: "shquery",
            prefix: "query",
            title: "Язык запросов",
            kind: SectionType::QueryLanguage,
        },
    ],
};

static EN: HelpLang = HelpLang {
    code: "en",
    suffix: "root",
    ch_syntax_variant: "Syntax variant",
    ch_syntax: "Syntax",
    ch_parameters: "Parameters",
    ch_returns: "Returned value",
    ch_description: "Description",
    ch_description_variant: "Description of method variant",
    ch_availability: "Availability",
    ch_example: "Example",
    ch_note: "Note",
    ch_usage: "Usage",
    ch_properties: "Properties",
    ch_methods: "Methods",
    type_word: "Type",
    required: "required",
    read_only: "read only",
    client: "client",
    server: &["server", "external connection", "integration"],
    ctor_word: "New",
    note_syntax: "Syntax: ",
    note_returns: "Returns: ",
    note_note: "Note: ",
    note_variant: "Syntax variant \"{name}\": {syntax}",
    section_blurb: "{title}: platform help ({book}).",
    books: &[
        Book {
            stem: "shlang",
            prefix: "lang",
            title: "Built-in language",
            kind: SectionType::Text,
        },
        Book {
            stem: "shclang",
            prefix: "clang",
            title: "General language rules",
            kind: SectionType::Text,
        },
        Book {
            stem: "shquery",
            prefix: "query",
            title: "Query language",
            kind: SectionType::QueryLanguage,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_selects_file_suffix() {
        assert_eq!(HelpLang::from_code("ru").unwrap().context_book(), "shcntx_ru.hbk");
        assert_eq!(HelpLang::from_code("EN").unwrap().context_book(), "shcntx_root.hbk");
        assert!(HelpLang::from_code("de").is_none());
    }

    #[test]
    fn book_file_name_follows_language() {
        let ru = HelpLang::from_code("ru").unwrap();
        let en = HelpLang::from_code("en").unwrap();

        assert_eq!(ru.books[2].file_name(ru), "shquery_ru.hbk");
        assert_eq!(en.books[2].file_name(en), "shquery_root.hbk");
    }

    /// Русские подписи уходят в базу знаний дословно: по ним идёт сличение с
    /// эталонным выходом Python-скрипта.
    #[test]
    fn russian_labels_match_reference_extractor() {
        let ru = HelpLang::from_code("ru").unwrap();

        assert_eq!(ru.variant_note("По строке", "Х(Y)"), "Вариант синтаксиса «По строке»: Х(Y)");
        assert_eq!(
            ru.blurb("Язык запросов", "shquery_ru.hbk"),
            "Язык запросов: справка платформы (shquery_ru.hbk)."
        );
    }
}
