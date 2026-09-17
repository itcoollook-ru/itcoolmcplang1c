//! extract.rs — синтакс-помощник 1С (`.hbk`) в [`Document`] базы знаний.
//!
//! Область:
//! * API — объекты (со свойствами) и методы, из `shcntx_<lang>.hbk`;
//! * разделы языка — из `shlang`, `shclang` и `shquery`, в `sections`.
//!
//! Операторы языка (`ВызватьИсключение`, `Попытка`, `Если`) в контекстной
//! справке отсутствуют как класс — они описаны в отдельных книгах рядом с
//! `shcntx`, поэтому три книги подхватываются из его каталога.

use super::container;
use super::html::{clip, TextRx};
use super::lang::HelpLang;
use super::Progress;
use crate::knowledge::schema::{
    ApiMethod, ApiObject, Document, Example, ExecutionContext, Parameter, Property, Section,
};
use anyhow::{anyhow, Context, Result};
use indexmap::IndexMap;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::Path;

/// ZIP со страницами справки, распакованный из элемента `FileStorage`.
pub type Storage = zip::ZipArchive<Cursor<Vec<u8>>>;

/// Каталоги членов объекта внутри `objects/…`.
///
/// `events` в списке намеренно: страница события обязана опознаваться как член
/// объекта (иначе она попала бы в первый проход как самостоятельный объект), но
/// события вне области — во втором проходе они отбрасываются.
const MEMBER_DIRS: [&str; 4] = ["/methods/", "/properties/", "/ctors/", "/events/"];

/// Служебные страницы книги: оглавление, дерево разделов.
const SERVICE_PAGES: [&str; 3] = ["index", "__categories__", "root.html"];

/// Страница глобального контекста: её методы именуются без префикса объекта.
const GLOBAL_CONTEXT_PAGE: &str = "objects/Global context.html";

/// Что и почему не попало в базу.
#[derive(Debug, Default)]
pub struct Skipped {
    pub category: usize,
    pub dup_method: usize,
    pub no_owner: usize,
}

/// Сколько разделов взято и сколько пропущено в одной книге.
#[derive(Debug)]
pub struct BookStats {
    pub book: String,
    pub taken: usize,
    pub skipped: usize,
}

/// Разобранный член объекта.
struct Member {
    description: String,
    context: ExecutionContext,
    return_type: Option<String>,
    parameters: Vec<Parameter>,
    examples: Vec<Example>,
    notes: Vec<String>,
}

/// Объект в процессе сборки: свойства доезжают вторым проходом.
struct ObjectDraft {
    name: String,
    description: String,
    methods: Vec<String>,
    is_global: bool,
}

/// Извлекатель: язык справки плюс скомпилированные один раз выражения.
pub struct Extractor {
    lang: &'static HelpLang,
    text: TextRx,
    pagetitle: Regex,
    title: Regex,
    heading: Regex,
    chapter: Regex,
    type_line: Regex,
    name_en: Regex,
    param_head: Regex,
    h1: Regex,
    latin: Regex,
    hr: Regex,
}

impl Extractor {
    pub fn new(lang: &'static HelpLang) -> Result<Self> {
        Ok(Self {
            lang,
            text: TextRx::new()?,
            pagetitle: Regex::new(r#"(?s)V8SH_pagetitle">(.*?)</h1>"#)?,
            title: Regex::new(r#"(?s)V8SH_title">(.*?)</p>"#)?,
            heading: Regex::new(r#"(?s)V8SH_heading">(.*?)</p>"#)?,
            chapter: Regex::new(r#"(?s)<p class="V8SH_chapter">\s*(.*?)\s*</p>"#)?,
            type_line: Regex::new(&format!(
                r"(?si){}:\s*(.*?)\.\s*(?:<br>|<br/>|$)",
                regex::escape(lang.type_word)
            ))?,
            name_en: Regex::new(r"^(.*?)\s*\((.*?)\)\s*$")?,
            // Вестигиальные `<`, `&lt;`, `>` в шаблоне — из исходного скрипта:
            // на вход подаётся строка, из которой угловые скобки уже удалены.
            param_head: Regex::new(r"^<?&?l?t?;?\s*(.*?)\s*>?\s*\((.*?)\)\s*$")?,
            // Заголовок берётся по ЛЮБОМУ <h1>, не по классу: в shcntx это
            // class="V8SH_pagetitle", в shlang тот же класс без кавычек, а в
            // shquery/shclang класс пуст. Разбор по конкретному классу терял бы
            // 40 страниц из 62.
            h1: Regex::new(r"(?is)<h1[^>]*>(.*?)</h1>")?,
            latin: Regex::new(r"^[A-Za-z][A-Za-z0-9]*$")?,
            hr: Regex::new(r"(?i)<hr\s*/?>")?,
        })
    }

    // ── разбор страницы ─────────────────────────────────────────────────────

    /// Подвал «Методическая информация» отсекается по первому `<hr>`.
    fn before_footer<'a>(&self, body: &'a str) -> &'a str {
        match self.hr.find(body) {
            Some(m) => &body[..m.start()],
            None => body,
        }
    }

    /// `[(заголовок главы, её html)]` в порядке следования.
    fn chapters<'a>(&self, body: &'a str) -> Vec<(String, &'a str)> {
        let body = self.before_footer(body);

        let marks: Vec<(usize, usize, String)> = self
            .chapter
            .captures_iter(body)
            .filter_map(|c| {
                let all = c.get(0)?;
                let inner = c.get(1)?;
                Some((all.start(), all.end(), self.text.text(inner.as_str(), false)))
            })
            .collect();

        marks
            .iter()
            .enumerate()
            .map(|(i, (_, end, title))| {
                let stop = marks.get(i + 1).map_or(body.len(), |next| next.0);
                (
                    title.trim_end_matches(':').trim().to_string(),
                    &body[*end..stop],
                )
            })
            .collect()
    }

    /// Главы страницы как словарь; при повторе заголовка выигрывает последний —
    /// как `dict()` из списка пар в исходном скрипте.
    fn chapter_map<'a>(&self, body: &'a str) -> HashMap<String, &'a str> {
        self.chapters(body).into_iter().collect()
    }

    /// `СхемаЗапроса (QuerySchema)` → `("СхемаЗапроса", "QuerySchema")`.
    fn split_names(&self, raw: &str) -> (String, String) {
        let plain = self.text.text(raw, false);
        match self.name_en.captures(&plain) {
            Some(c) => match (c.get(1), c.get(2)) {
                (Some(a), Some(b)) => (a.as_str().trim().to_string(), b.as_str().trim().to_string()),
                _ => (plain, String::new()),
            },
            None => (plain, String::new()),
        }
    }

    /// Разбивка блока параметров на рубрики.
    ///
    /// # CRITICAL_LOGIC (не трогать при рефакторинге)
    /// Исходное выражение опиралось на lookahead `(?=<div class="V8SH_rubric">|$)`,
    /// которого в `regex` нет. Нарезка идёт по позициям тега: рубрика — до
    /// первого `</div>`, хвост — до следующего открывающего тега или конца
    /// блока; следующая итерация начинается ровно с этого тега.
    fn rubrics<'a>(&self, block: &'a str) -> Vec<(&'a str, &'a str)> {
        const OPEN: &str = r#"<div class="V8SH_rubric">"#;
        const CLOSE: &str = "</div>";

        let mut out = Vec::new();
        let mut pos = 0;

        while let Some(rel) = block[pos..].find(OPEN) {
            let open = pos + rel;
            let content_start = open + OPEN.len();
            // Без закрывающего тега исходное выражение не совпало бы ни здесь,
            // ни дальше: все последующие рубрики тоже остались бы незакрытыми.
            let Some(close_rel) = block[content_start..].find(CLOSE) else {
                break;
            };
            let close = content_start + close_rel;
            let tail_start = close + CLOSE.len();
            let tail_end = block[tail_start..]
                .find(OPEN)
                .map_or(block.len(), |r| tail_start + r);

            out.push((&block[content_start..close], &block[tail_start..tail_end]));
            pos = tail_end;
        }

        out
    }

    fn parse_params(&self, block: &str) -> Vec<Parameter> {
        let mut params = Vec::new();

        for (rubric, tail) in self.rubrics(block) {
            let head = self
                .text
                .text(rubric, false)
                .replace(['<', '>'], "");
            let Some(c) = self.param_head.captures(&head) else {
                continue;
            };
            let (Some(name), Some(flag)) = (c.get(1), c.get(2)) else {
                continue;
            };

            let (param_type, description) = match self.type_line.captures(tail) {
                Some(t) => {
                    let value = t.get(1).map_or(String::new(), |g| self.text.text(g.as_str(), false));
                    let rest = t.get(0).map_or(tail, |g| &tail[g.end()..]);
                    (value, self.text.text(rest, false))
                }
                None => (String::new(), self.text.text(tail, false)),
            };

            params.push(Parameter {
                name: name.as_str().trim().to_string(),
                param_type,
                required: flag
                    .as_str()
                    .trim()
                    .to_lowercase()
                    .starts_with(self.lang.required),
                description: clip(&description, 400),
            });
        }

        params
    }

    /// Контекст исполнения из раздела «Доступность».
    fn availability(&self, raw: &str) -> ExecutionContext {
        let s = raw.to_lowercase();
        let client = s.contains(self.lang.client);
        let server = self.lang.server.iter().any(|word| s.contains(word));

        match (client, server) {
            (true, true) => ExecutionContext::Both,
            (false, true) => ExecutionContext::Server,
            (true, false) => ExecutionContext::Client,
            (false, false) => ExecutionContext::Both,
        }
    }

    /// Метод/функция: описание, контекст, параметры, возврат, примеры, заметки.
    fn parse_member(&self, body: &str) -> Member {
        let l = self.lang;

        let mut description = String::new();
        let mut context = ExecutionContext::Both;
        let mut return_type = None;
        let mut parameters: Vec<Parameter> = Vec::new();
        let mut examples: Vec<Example> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        let mut variants: Vec<(String, String, Vec<Parameter>)> = Vec::new();

        for (title, block) in self.chapters(body) {
            if title.starts_with(l.ch_syntax_variant) {
                // «Вариант синтаксиса: По строке» → «По строке»; без двоеточия
                // именем остаётся весь заголовок.
                let name = match title.split_once(':') {
                    Some((_, rest)) => rest.trim().to_string(),
                    None => title.trim().to_string(),
                };
                variants.push((name, String::new(), Vec::new()));
            } else if title == l.ch_syntax {
                let signature = self.text.text(block, false);
                if let Some(last) = variants.last_mut() {
                    last.1 = signature;
                } else if !signature.is_empty() {
                    notes.insert(0, format!("{}{}", l.note_syntax, signature));
                }
            } else if title == l.ch_parameters {
                let parsed = self.parse_params(block);
                if let Some(last) = variants.last_mut() {
                    last.2 = parsed;
                } else if parameters.is_empty() {
                    parameters = parsed;
                }
            } else if title == l.ch_returns {
                if let Some(t) = self.type_line.captures(block) {
                    return_type = t.get(1).map(|g| self.text.text(g.as_str(), false));
                    let rest = t.get(0).map_or(block, |g| &block[g.end()..]);
                    let tail = self.text.text(rest, false);
                    if !tail.is_empty() {
                        notes.push(format!("{}{}", l.note_returns, clip(&tail, 300)));
                    }
                }
            } else if title == l.ch_description || title == l.ch_description_variant {
                let piece = self.text.text(block, false);
                description = if description.is_empty() {
                    piece
                } else {
                    format!("{} {}", description, piece)
                };
            } else if title == l.ch_availability {
                context = self.availability(&self.text.text(block, false));
            } else if title == l.ch_example && examples.len() < 2 {
                let code = self.text.text(block, true);
                if !code.is_empty() {
                    examples.push(Example {
                        title: None,
                        code: clip(&code, 800),
                        description: None,
                    });
                }
            } else if title == l.ch_note {
                notes.push(format!(
                    "{}{}",
                    l.note_note,
                    clip(&self.text.text(block, false), 300)
                ));
            }
        }

        if !variants.is_empty() {
            if parameters.is_empty() {
                parameters = variants
                    .iter()
                    .find(|v| !v.2.is_empty())
                    .map_or_else(Vec::new, |v| v.2.clone());
            }
            // Вставка в начало по очереди: последний вариант оказывается первым.
            for (name, signature, _) in &variants {
                notes.insert(0, l.variant_note(name, signature));
            }
        }

        Member {
            description,
            context,
            return_type,
            parameters,
            examples,
            notes,
        }
    }

    /// Свойство: тип, описание, режим использования.
    fn parse_property(&self, body: &str) -> (String, String, bool) {
        let mut property_type = String::new();
        let mut description = String::new();
        let mut readonly = false;

        for (title, block) in self.chapters(body) {
            if title == self.lang.ch_usage {
                readonly = self
                    .text
                    .text(block, false)
                    .to_lowercase()
                    .contains(self.lang.read_only);
            } else if title == self.lang.ch_description {
                match self.type_line.captures(block) {
                    Some(t) => {
                        property_type = t
                            .get(1)
                            .map_or(String::new(), |g| self.text.text(g.as_str(), false));
                        let rest = t.get(0).map_or(block, |g| &block[g.end()..]);
                        description = self.text.text(rest, false);
                    }
                    None => description = self.text.text(block, false),
                }
            }
        }

        (property_type, description, readonly)
    }

    /// Имена, по которым раздел должен находиться (буст ×2 в индексе).
    ///
    /// # CRITICAL_LOGIC (не трогать при рефакторинге)
    /// Английское имя берётся только если оно действительно латиница: у языка
    /// запросов в скобках стоит русское пояснение («Секция ВЫБРАТЬ (Описание
    /// запроса)»), ключевым словом оно не является. Имя страницы (`ISNULL`,
    /// `LEFTJOIN`, `SUBSTRING`) добавляется по тому же признаку.
    fn page_keywords(&self, title_ru: &str, title_en: &str, page: &str) -> Vec<String> {
        let mut words = Vec::new();
        if !title_ru.is_empty() {
            words.push(title_ru.to_string());
        }
        if !title_en.is_empty() && self.latin.is_match(&title_en.replace(' ', "")) {
            words.push(title_en.to_string());
        }

        let stem = page.strip_suffix(".html").unwrap_or(page);
        if self.latin.is_match(stem) && !words.iter().any(|w| w == stem) {
            words.push(stem.to_string());
        }

        words
    }

    // ── обход контейнера ────────────────────────────────────────────────────

    /// Объекты и методы контекстной справки.
    pub fn extract(
        &self,
        storage: &mut Storage,
        version: &str,
        progress: &Progress,
    ) -> Result<(Document, Skipped)> {
        let pages = html_pages(storage);
        let mut skipped = Skipped::default();

        let mut objects: IndexMap<String, ObjectDraft> = IndexMap::new();
        let mut methods: IndexMap<String, ApiMethod> = IndexMap::new();
        let mut props: HashMap<String, Vec<Property>> = HashMap::new();
        let mut owner_names: HashMap<String, String> = HashMap::new();

        // Первый проход: объекты — чтобы во втором знать имена владельцев.
        for (n, (index, path)) in pages.iter().enumerate() {
            progress.stage_percent("objects", n, pages.len(), 2, 35);
            if owner_of(path).is_some() {
                continue;
            }

            let body = read_lossy(storage, *index)?;
            let Some(raw_title) = self.pagetitle.captures(&body).and_then(|c| c.get(1)) else {
                continue;
            };
            let (ru, en) = self.split_names(raw_title.as_str());
            let chapters = self.chapter_map(&body);
            let is_global = path == GLOBAL_CONTEXT_PAGE;

            // Категории каталога: без английского имени и без разделов API.
            if !is_global
                && en.is_empty()
                && !chapters.contains_key(self.lang.ch_properties)
                && !chapters.contains_key(self.lang.ch_methods)
            {
                skipped.category += 1;
                continue;
            }

            let mut description = self
                .text
                .text(chapters.get(self.lang.ch_description).copied().unwrap_or(""), false);
            if !en.is_empty() {
                description = append_english(&description, &en);
            }

            let base = path.trim_end_matches(".html").to_string();
            owner_names.insert(base.clone(), ru.clone());
            objects.insert(
                base,
                ObjectDraft {
                    name: ru,
                    description: clip(&description, 700),
                    methods: Vec::new(),
                    is_global,
                },
            );
        }

        // Второй проход: члены объектов.
        for (n, (index, path)) in pages.iter().enumerate() {
            progress.stage_percent("members", n, pages.len(), 35, 85);
            let Some(owner) = owner_of(path) else {
                continue;
            };
            if !objects.contains_key(owner) {
                skipped.no_owner += 1;
                continue;
            }

            let body = read_lossy(storage, *index)?;
            let heading = self.heading.captures(&body).and_then(|c| c.get(1));
            let (Some(_), Some(heading)) = (self.pagetitle.find(&body), heading) else {
                continue;
            };
            let (member_ru, member_en) = self.split_names(heading.as_str());
            let owner_ru = owner_names.get(owner).cloned().unwrap_or_default();

            if path.contains("/properties/") {
                let (property_type, mut description, readonly) = self.parse_property(&body);
                if !member_en.is_empty() {
                    description = append_english(&description, &member_en);
                }
                props.entry(owner.to_string()).or_default().push(Property {
                    name: member_ru,
                    property_type,
                    description: clip(&description, 400),
                    readonly,
                });
                continue;
            }

            // События вне области: страница опознана как член объекта, но не
            // обрабатывается.
            if !path.contains("/methods/") && !path.contains("/ctors/") {
                continue;
            }

            let is_global = objects.get(owner).is_some_and(|o| o.is_global);
            let mut full = if is_global {
                member_ru.clone()
            } else {
                format!("{}.{}", owner_ru, member_ru)
            };
            if path.contains("/ctors/") {
                // Конструктор вызывается как «Новый Объект», а не «Объект.По умолчанию».
                full = format!("{} {}", self.lang.ctor_word, owner_ru);
                if methods.contains_key(&full) {
                    // У объекта несколько вариантов конструктора.
                    full = format!("{} — {}", full, member_ru);
                }
            }
            if methods.contains_key(&full) {
                skipped.dup_method += 1;
                continue;
            }

            let member = self.parse_member(&body);

            let page_en = self
                .title
                .captures(&body)
                .and_then(|c| c.get(1))
                .map(|g| self.split_names(g.as_str()).1);
            let en_full = match (page_en, member_en.is_empty()) {
                (Some(owner_en), false) => format!("{}.{}", owner_en, member_en),
                _ => member_en.clone(),
            };

            let mut description = member.description;
            if !en_full.is_empty() {
                description = append_english(&description, &en_full);
            }

            let mut notes = member.notes;
            notes.truncate(4);

            methods.insert(
                full.clone(),
                ApiMethod {
                    name: full,
                    description: clip(&description, 700),
                    context: member.context,
                    parameters: member.parameters,
                    return_type: member.return_type,
                    examples: member.examples,
                    notes,
                },
            );
            if let Some(object) = objects.get_mut(owner) {
                object.methods.push(member_ru);
            }
        }

        // Объект без свойств, методов и описания в выход не попадает;
        // одноимённые объекты из разных каталогов — «первый выигрывает».
        let mut api_objects = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (base, draft) in objects {
            let properties = props.remove(&base).unwrap_or_default();
            if properties.is_empty() && draft.methods.is_empty() && draft.description.is_empty() {
                continue;
            }
            if !seen.insert(draft.name.clone()) {
                continue;
            }
            api_objects.push(ApiObject {
                name: draft.name,
                description: draft.description,
                properties,
                methods: draft.methods,
            });
        }

        let document = Document {
            version: version.to_string(),
            platform: "1С:Предприятие".to_string(),
            release_date: None,
            sections: Vec::new(),
            api_methods: methods.into_values().collect(),
            api_objects,
            relationships: Vec::new(),
        };

        Ok((document, skipped))
    }

    /// Разделы языка из книг рядом с `shcntx`.
    ///
    /// Отсутствие книги — не отказ: предупреждение и пропуск раздела. База
    /// собирается из того, что есть.
    pub fn extract_sections(
        &self,
        bin_dir: &Path,
        progress: &Progress,
    ) -> Result<(Vec<Section>, Vec<BookStats>)> {
        let mut sections = Vec::new();
        let mut stats = Vec::new();

        for (book_index, book) in self.lang.books.iter().enumerate() {
            let file_name = book.file_name(self.lang);
            let path = bin_dir.join(&file_name);
            if !path.exists() {
                tracing::warn!(
                    "ru: Нет {:?} — раздел «{}» пропущен, en: No {:?} — section '{}' skipped",
                    path,
                    book.title,
                    path,
                    book.title
                );
                continue;
            }

            let mut storage = open_storage(&path)?;
            let pages = all_pages(&mut storage);
            let mut subsections = Vec::new();
            let mut skipped = 0usize;

            for (n, (index, page)) in pages.iter().enumerate() {
                let base = 85 + book_index * 5;
                progress.stage_percent("sections", n, pages.len(), base, base + 5);

                // `.st` — вёрстка синтаксической врезки в формате 1С-структуры;
                // она дублирует раздел «Синтаксис» описательной страницы.
                if page.ends_with(".st")
                    || page.ends_with(".gif")
                    || SERVICE_PAGES.contains(&page.as_str())
                    || page.starts_with("_CONTENTS_NODE_")
                {
                    skipped += 1;
                    continue;
                }

                let Some(body) = read_strict(&mut storage, *index)? else {
                    skipped += 1;
                    continue;
                };

                let Some(heading) = self.h1.captures(&body) else {
                    skipped += 1;
                    continue;
                };
                let (Some(whole), Some(inner)) = (heading.get(0), heading.get(1)) else {
                    skipped += 1;
                    continue;
                };

                let page_title = self.text.text(inner.as_str(), false);
                let (ru, en) = self.split_names(&page_title);
                // Подвал «Методическая информация» отсекаем так же, как у методов.
                let content = self
                    .text
                    .text(self.before_footer(&body[whole.end()..]), true);
                if content.is_empty() {
                    skipped += 1;
                    continue;
                }

                subsections.push(Section {
                    id: format!("{}_{}", book.prefix, page.strip_suffix(".html").unwrap_or(page)),
                    title: page_title,
                    section_type: book.kind.clone(),
                    content: clip(&content, 4000),
                    keywords: self.page_keywords(&ru, &en, page),
                    subsections: Vec::new(),
                });
            }

            stats.push(BookStats {
                book: file_name.clone(),
                taken: subsections.len(),
                skipped,
            });

            if !subsections.is_empty() {
                sections.push(Section {
                    id: book.prefix.to_string(),
                    title: book.title.to_string(),
                    section_type: crate::knowledge::schema::SectionType::Category,
                    content: self.lang.blurb(book.title, &file_name),
                    keywords: Vec::new(),
                    subsections,
                });
            }
        }

        Ok((sections, stats))
    }
}

/// `objects/a/b/Obj/methods/X123.html` → `objects/a/b/Obj`.
fn owner_of(path: &str) -> Option<&str> {
    MEMBER_DIRS
        .iter()
        .find_map(|seg| path.find(seg).map(|i| &path[..i]))
}

/// Английское имя в хвост описания: `описание (EnglishName)`.
fn append_english(description: &str, english: &str) -> String {
    if description.is_empty() {
        format!("({})", english)
    } else {
        format!("{} ({})", description, english)
    }
}

/// ZIP со страницами справки внутри контейнера `.hbk`.
pub fn open_storage(hbk_path: &Path) -> Result<Storage> {
    let raw = std::fs::read(hbk_path).context(format!(
        "ru: Не удалось прочитать {:?}, en: Failed to read {:?}",
        hbk_path, hbk_path
    ))?;

    let items = container::parse(&raw).map_err(|e| {
        anyhow!(
            "ru: Не разобран контейнер {:?}: {}, en: Failed to parse container {:?}: {}",
            hbk_path,
            e,
            hbk_path,
            e
        )
    })?;

    let storage = items
        .into_iter()
        .find(|(name, _)| name == "FileStorage")
        .map(|(_, data)| data)
        .ok_or_else(|| {
            anyhow!(
                "ru: В {:?} нет элемента FileStorage, en: No FileStorage element in {:?}",
                hbk_path,
                hbk_path
            )
        })?;

    zip::ZipArchive::new(Cursor::new(storage)).context(format!(
        "ru: FileStorage в {:?} не ZIP, en: FileStorage in {:?} is not a ZIP",
        hbk_path, hbk_path
    ))
}

/// Все записи архива в порядке их следования.
///
/// # CRITICAL_LOGIC (не трогать при рефакторинге)
/// Порядок обхода — по индексу записей, а не по отсортированному или
/// произвольному списку имён: правило «первый выигрывает» делает состав базы
/// зависимым от порядка, и сортировка изменила бы её содержимое.
fn all_pages(storage: &mut Storage) -> Vec<(usize, String)> {
    (0..storage.len())
        .filter_map(|i| storage.by_index(i).ok().map(|f| (i, f.name().to_string())))
        .collect()
}

fn html_pages(storage: &mut Storage) -> Vec<(usize, String)> {
    all_pages(storage)
        .into_iter()
        .filter(|(_, name)| name.ends_with(".html"))
        .collect()
}

fn read_bytes(storage: &mut Storage, index: usize) -> Result<Vec<u8>> {
    let mut file = storage.by_index(index)?;
    let mut buf = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Страница с заменой битых байт — как `decode("utf-8", "replace")`.
fn read_lossy(storage: &mut Storage, index: usize) -> Result<String> {
    Ok(String::from_utf8_lossy(&read_bytes(storage, index)?).into_owned())
}

/// Страница строгим декодированием: не UTF-8 — `None`, страница пропускается.
fn read_strict(storage: &mut Storage, index: usize) -> Result<Option<String>> {
    Ok(String::from_utf8(read_bytes(storage, index)?).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hbk::lang::HelpLang;

    fn ru() -> Extractor {
        Extractor::new(HelpLang::from_code("ru").unwrap()).unwrap()
    }

    #[test]
    fn owner_is_taken_before_the_first_member_dir() {
        assert_eq!(
            owner_of("objects/a/b/Obj/methods/X123.html"),
            Some("objects/a/b/Obj")
        );
        assert_eq!(owner_of("objects/Global context.html"), None);
    }

    /// Русское пояснение в скобках ключевым словом не является.
    #[test]
    fn page_keywords_reject_russian_parenthetical() {
        let e = ru();

        let words = e.page_keywords("Секция ВЫБРАТЬ", "Описание запроса", "SELECT.html");

        assert_eq!(words, vec!["Секция ВЫБРАТЬ", "SELECT"]);
    }

    /// Английское имя оператора — латиница, берётся.
    #[test]
    fn page_keywords_take_latin_english_name() {
        let e = ru();

        let words = e.page_keywords("ЕСТЬNULL", "ISNULL", "ISNULL.html");

        assert_eq!(words, vec!["ЕСТЬNULL", "ISNULL"]);
    }

    /// Все четыре комбинации доступности.
    #[test]
    fn availability_covers_all_combinations() {
        let e = ru();

        assert_eq!(
            e.availability("Тонкий клиент, сервер, толстый клиент."),
            ExecutionContext::Both
        );
        assert_eq!(e.availability("Сервер."), ExecutionContext::Server);
        assert_eq!(e.availability("Тонкий клиент, веб-клиент."), ExecutionContext::Client);
        // Ничего не распознано — метод не прячем: доступен везде.
        assert_eq!(e.availability("Мобильная платформа."), ExecutionContext::Both);
    }

    /// «Внешнее соединение» — серверный контекст, отдельного слова «сервер» нет.
    #[test]
    fn availability_treats_external_connection_as_server() {
        assert_eq!(ru().availability("Внешнее соединение."), ExecutionContext::Server);
    }

    /// Английская справка разбирается своими маркерами.
    #[test]
    fn english_availability_uses_english_markers() {
        let e = Extractor::new(HelpLang::from_code("en").unwrap()).unwrap();

        assert_eq!(
            e.availability("Server, thick client, external connection."),
            ExecutionContext::Both
        );
        assert_eq!(e.availability("Server."), ExecutionContext::Server);
    }

    /// Нарезка рубрик без lookahead: хвост тянется до следующей рубрики.
    #[test]
    fn rubrics_split_without_lookahead() {
        let e = ru();
        let block = concat!(
            r#"<div class="V8SH_rubric">&lt;Имя&gt; (обязательный)</div>Тип: Строка.<br>Имя."#,
            r#"<div class="V8SH_rubric">&lt;Флаг&gt; (необязательный)</div>Тип: Булево.<br>Флаг."#
        );

        let parts = e.rubrics(block);

        assert_eq!(parts.len(), 2);
        assert!(parts[0].1.contains("Строка"));
        assert!(!parts[0].1.contains("Булево"));
        assert!(parts[1].1.contains("Булево"));
    }

    #[test]
    fn params_take_name_type_and_required_flag() {
        let e = ru();
        let block = concat!(
            r#"<div class="V8SH_rubric">&lt;Имя&gt; (обязательный)</div>Тип: Строка.<br>Имя файла."#,
            r#"<div class="V8SH_rubric">&lt;Флаг&gt; (необязательный)</div>Тип: Булево.<br>Признак."#
        );

        let params = e.parse_params(block);

        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "Имя");
        assert_eq!(params[0].param_type, "Строка");
        assert!(params[0].required);
        assert_eq!(params[0].description, "Имя файла.");
        assert!(!params[1].required);
    }

    /// Подвал «Методическая информация» отсекается по первому `<hr>`.
    #[test]
    fn chapters_cut_the_footer() {
        let e = ru();
        let body = concat!(
            r#"<p class="V8SH_chapter">Описание:</p>Полезное.<hr>"#,
            r#"<p class="V8SH_chapter">Методическая информация</p>Мусор."#
        );

        let chapters = e.chapters(body);

        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].0, "Описание");
    }

    /// Заголовок раздела берётся по любому `<h1>`, класс не важен.
    #[test]
    fn h1_matches_regardless_of_class() {
        let e = ru();

        for markup in [
            r#"<h1 class="V8SH_pagetitle">Если</h1>"#,
            r#"<h1 class=V8SH_pagetitle>Если</h1>"#,
            r#"<h1 class="">Если</h1>"#,
            "<h1>Если</h1>",
        ] {
            let found = e.h1.captures(markup).and_then(|c| c.get(1));
            assert_eq!(found.map(|g| g.as_str()), Some("Если"), "{}", markup);
        }
    }

    #[test]
    fn split_names_separates_russian_and_english() {
        let e = ru();

        assert_eq!(
            e.split_names("СхемаЗапроса (QuerySchema)"),
            ("СхемаЗапроса".to_string(), "QuerySchema".to_string())
        );
        assert_eq!(
            e.split_names("ТаблицаЗначений"),
            ("ТаблицаЗначений".to_string(), String::new())
        );
    }

    /// Свойство «только чтение» распознаётся по разделу «Использование».
    #[test]
    fn property_readonly_comes_from_usage_chapter() {
        let e = ru();
        let body = concat!(
            r#"<p class="V8SH_chapter">Использование:</p>Только чтение."#,
            r#"<p class="V8SH_chapter">Описание:</p>Тип: Число.<br>Количество."#
        );

        let (property_type, description, readonly) = e.parse_property(body);

        assert_eq!(property_type, "Число");
        assert_eq!(description, "Количество.");
        assert!(readonly);
    }

    /// Варианты синтаксиса: параметры берутся от первого непустого варианта,
    /// а примечания встают в начало в обратном порядке.
    #[test]
    fn syntax_variants_fill_params_and_notes() {
        let e = ru();
        let body = concat!(
            r#"<p class="V8SH_chapter">Вариант синтаксиса: По строке</p>"#,
            r#"<p class="V8SH_chapter">Синтаксис:</p>Новый Запрос(Текст)"#,
            r#"<p class="V8SH_chapter">Параметры:</p>"#,
            r#"<div class="V8SH_rubric">&lt;Текст&gt; (обязательный)</div>Тип: Строка.<br>Текст."#,
            r#"<p class="V8SH_chapter">Вариант синтаксиса: По умолчанию</p>"#,
            r#"<p class="V8SH_chapter">Синтаксис:</p>Новый Запрос()"#
        );

        let member = e.parse_member(body);

        assert_eq!(member.parameters.len(), 1);
        assert_eq!(member.parameters[0].name, "Текст");
        assert_eq!(
            member.notes,
            vec![
                "Вариант синтаксиса «По умолчанию»: Новый Запрос()",
                "Вариант синтаксиса «По строке»: Новый Запрос(Текст)",
            ]
        );
    }

    /// Примеров не больше двух, примечания и «Возвращает» — с обрезкой.
    #[test]
    fn examples_are_capped_at_two() {
        let e = ru();
        let body = concat!(
            r#"<p class="V8SH_chapter">Пример:</p>А = 1;"#,
            r#"<p class="V8SH_chapter">Пример:</p>Б = 2;"#,
            r#"<p class="V8SH_chapter">Пример:</p>В = 3;"#
        );

        let member = e.parse_member(body);

        assert_eq!(member.examples.len(), 2);
        assert_eq!(member.examples[0].code, "А = 1;");
        assert_eq!(member.examples[1].code, "Б = 2;");
    }

    #[test]
    fn english_description_is_appended_in_parentheses() {
        assert_eq!(append_english("Описание", "Name"), "Описание (Name)");
        assert_eq!(append_english("", "Name"), "(Name)");
    }
}
