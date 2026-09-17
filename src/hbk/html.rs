//! html.rs — разметка страницы справки в текст.
//!
//! Парсер HTML не подключается сознательно: страницы справки 1С генерируются
//! одним шаблонизатором, и регулярных выражений на них достаточно.

use regex::Regex;

/// Регулярные выражения текстового слоя, скомпилированные один раз.
pub struct TextRx {
    tag: Regex,
    ws: Regex,
    line_break: Regex,
    blank_lines: Regex,
    space_before_punct: Regex,
}

impl TextRx {
    pub fn new() -> Result<Self, regex::Error> {
        Ok(Self {
            tag: Regex::new(r"<[^>]+>")?,
            // ⚠️ Третий символ класса — неразрывный пробел U+00A0, а не обычный:
            // в исходнике он визуально неотличим, и при копировании теряется молча.
            ws: Regex::new("[ \t\u{00A0}]+")?,
            line_break: Regex::new(r"(?i)<br\s*/?>|</tr>|</p>")?,
            blank_lines: Regex::new(r"\n{3,}")?,
            space_before_punct: Regex::new(r"\s+([.,;:])")?,
        })
    }

    /// Фрагмент разметки в текст.
    ///
    /// # CRITICAL_LOGIC (не трогать при рефакторинге)
    /// - `keep_lines = true` — режим примера кода: теги удаляются **без замены**
    ///   на пробел, иначе `Новый Запрос(Текст);` превращается в
    ///   `Новый Запрос ( Текст ) ;`.
    /// - `keep_lines = false` — режим описания: тег заменяется **пробелом**,
    ///   потому что в исходнике абзацы стыкуются без разделителя и выходит
    ///   `типа.Не работает`.
    /// - Порядок «снять теги → раскрыть сущности» обязателен: экранированный
    ///   `&lt;b&gt;` не должен исчезнуть как тег.
    /// - Замена U+00A0 на пробел идёт до схлопывания пробелов.
    pub fn text(&self, fragment: &str, keep_lines: bool) -> String {
        let stripped = if keep_lines {
            let broken = self.line_break.replace_all(fragment, "\n");
            unescape(&self.tag.replace_all(&broken, ""))
        } else {
            unescape(&self.tag.replace_all(fragment, " "))
        };

        let s = stripped.replace('\u{00A0}', " ");

        if keep_lines {
            let joined = s
                .split('\n')
                .map(|line| self.ws.replace_all(line, " ").trim().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            return self.blank_lines.replace_all(&joined, "\n\n").trim().to_string();
        }

        let collapsed = self.ws.replace_all(&s.replace('\n', " "), " ").trim().to_string();
        self.space_before_punct
            .replace_all(&collapsed, "$1")
            .into_owned()
    }
}

/// Раскрытие HTML-сущностей, включая именованные (`&laquo;`, `&nbsp;`), а не
/// только `&amp;`/`&lt;`.
fn unescape(s: &str) -> String {
    html_escape::decode_html_entities(s).into_owned()
}

/// Обрезка до `limit` **символов** с попыткой закончить на границе предложения.
///
/// # CRITICAL_LOGIC (не трогать при рефакторинге)
/// - Счёт и резка идут по символам, не по байтам: срез по байтам паникует на
///   границе UTF-8, и на русском тексте это произойдёт гарантированно.
/// - Точка-разделитель ищется с конца; она принимается только дальше середины
///   лимита, иначе обрезка съедала бы больше половины текста.
/// - Хвост всегда ` …`, после `rstrip` остатка.
pub fn clip(s: &str, limit: usize) -> String {
    let total = s.chars().count();
    if total <= limit {
        return s.to_string();
    }

    let cut: String = s.chars().take(limit).collect();
    let head = match char_rfind(&cut, ". ") {
        Some(dot) if dot > limit / 2 => cut.chars().take(dot + 1).collect::<String>(),
        _ => cut,
    };

    format!("{} …", head.trim_end())
}

/// Позиция последнего вхождения `needle` **в символах**, как `str.rfind` в Python.
fn char_rfind(haystack: &str, needle: &str) -> Option<usize> {
    let byte_pos = haystack.rfind(needle)?;
    Some(haystack[..byte_pos].chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rx() -> TextRx {
        TextRx::new().unwrap()
    }

    /// Режим описания: тег становится пробелом, иначе абзацы слипаются.
    #[test]
    fn text_replaces_tags_with_space() {
        let rx = rx();

        let out = rx.text("<p>значение типа.</p><p>Не работает</p>", false);

        assert_eq!(out, "значение типа. Не работает");
    }

    /// Режим примера кода: теги снимаются без замены, перевод строки сохраняется.
    #[test]
    fn text_keeps_lines_without_spacing_out_code() {
        let rx = rx();

        let out = rx.text("Запрос = Новый Запрос(Текст);<br>Запрос.Выполнить();", true);

        assert_eq!(out, "Запрос = Новый Запрос(Текст);\nЗапрос.Выполнить();");
    }

    /// Неразрывный пробел обязан схлопываться наравне с обычным: иначе текст
    /// «поедет» без единой ошибки компиляции.
    #[test]
    fn non_breaking_space_collapses() {
        let rx = rx();

        let out = rx.text("Тип:\u{00A0}\u{00A0}Число", false);

        assert_eq!(out, "Тип: Число");
    }

    /// Именованные сущности раскрываются, а не только `&amp;`/`&lt;`.
    #[test]
    fn named_entities_are_unescaped() {
        let rx = rx();

        assert_eq!(rx.text("&laquo;Имя&raquo; &amp; &lt;b&gt;", false), "«Имя» & <b>");
    }

    /// Пробел перед знаком препинания появляется от замены тега — убирается.
    #[test]
    fn space_before_punctuation_is_removed() {
        let rx = rx();

        assert_eq!(rx.text("текст<b></b> , ещё<i></i> .", false), "текст, ещё.");
    }

    /// Три и более переводов строки в примере сжимаются до пустой строки.
    #[test]
    fn blank_lines_are_squeezed() {
        let rx = rx();

        let out = rx.text("а<br><br><br><br>б", true);

        assert_eq!(out, "а\n\nб");
    }

    /// Граница многобайтового символа: срез по байтам здесь паникует.
    #[test]
    fn clip_cuts_on_char_boundary() {
        let s = "Ещё один русский текст без точек внутри совсем";

        let out = clip(s, 10);

        assert_eq!(out.chars().count(), 10 + 2);
        assert!(out.ends_with(" …"));
        assert_eq!(out, "Ещё один р …");
    }

    /// Точка-разделитель дальше середины лимита — режем по ней.
    #[test]
    fn clip_prefers_sentence_end() {
        let s = "Первое предложение. Второе предложение продолжается дальше";

        let out = clip(s, 30);

        assert_eq!(out, "Первое предложение. …");
    }

    /// Точка в первой половине лимита не годится: обрезка съела бы текст.
    #[test]
    fn clip_ignores_early_sentence_end() {
        let s = "Да. Дальше идёт длинное продолжение без единой точки внутри";

        let out = clip(s, 40);

        assert!(!out.starts_with("Да. …"));
        assert!(out.ends_with(" …"));
    }

    #[test]
    fn clip_keeps_short_text_as_is() {
        assert_eq!(clip("Коротко", 100), "Коротко");
    }
}
