use super::schema::*;
use std::collections::HashMap;

/// Инвертированный индекс для полнотекстового поиска
pub struct InvertedIndex {
    /// Токен -> [(document_id, section_id, term_frequency)]
    index: HashMap<String, Vec<(String, String, f32)>>,
    /// Количество проиндексированных единиц (секции + методы + объекты).
    /// Именно оно, а не число документов-версий, служит N в формуле IDF.
    total_units: usize,
    /// Вес совпадения в заголовке и в ключевых словах — из `[search]` конфига.
    /// До 0.3.0 эти ключи объявлялись и не читались: индекс был зашит на 3.0/2.0.
    boost_title: f32,
    boost_keywords: f32,
}

impl InvertedIndex {
    pub fn with_boosts(boost_title: f32, boost_keywords: f32) -> Self {
        Self {
            index: HashMap::new(),
            total_units: 0,
            boost_title,
            boost_keywords,
        }
    }

    /// Индексация документа
    pub fn index_document(&mut self, doc: &Document) {
        let doc_id = doc.version.clone();

        // Индексация секций
        for section in &doc.sections {
            self.index_section(&doc_id, section);
        }

        // Индексация API методов
        for method in &doc.api_methods {
            self.index_api_method(&doc_id, method);
        }

        // Индексация API объектов
        for obj in &doc.api_objects {
            self.index_api_object(&doc_id, obj);
        }
    }

    fn index_section(&mut self, doc_id: &str, section: &Section) {
        let section_id = section.id.clone();
        self.total_units += 1;

        // Токенизация заголовка (с буством)
        let title_tokens = tokenize(&section.title);
        for token in title_tokens {
            self.add_token(doc_id, &section_id, &token, self.boost_title);
        }

        // Токенизация контента
        let content_tokens = tokenize(&section.content);
        for token in content_tokens {
            self.add_token(doc_id, &section_id, &token, 1.0);
        }

        // Токенизация ключевых слов (с буством)
        for keyword in &section.keywords {
            let keyword_tokens = tokenize(keyword);
            for token in keyword_tokens {
                self.add_token(doc_id, &section_id, &token, self.boost_keywords);
            }
        }

        // Рекурсивно обрабатываем подсекции
        for subsection in &section.subsections {
            self.index_section(doc_id, subsection);
        }
    }

    fn index_api_method(&mut self, doc_id: &str, method: &ApiMethod) {
        let section_id = format!("method_{}", method.name);
        self.total_units += 1;

        // Имя метода
        let name_tokens = tokenize(&method.name);
        for token in name_tokens {
            self.add_token(doc_id, &section_id, &token, self.boost_title);
        }

        // Описание
        let desc_tokens = tokenize(&method.description);
        for token in desc_tokens {
            self.add_token(doc_id, &section_id, &token, 1.0);
        }
    }

    fn index_api_object(&mut self, doc_id: &str, obj: &ApiObject) {
        let section_id = format!("object_{}", obj.name);
        self.total_units += 1;

        let name_tokens = tokenize(&obj.name);
        for token in name_tokens {
            self.add_token(doc_id, &section_id, &token, self.boost_title);
        }

        let desc_tokens = tokenize(&obj.description);
        for token in desc_tokens {
            self.add_token(doc_id, &section_id, &token, 1.0);
        }

        // Индексация свойств
        for prop in &obj.properties {
            let prop_tokens = tokenize(&prop.description);
            for token in prop_tokens {
                self.add_token(doc_id, &section_id, &token, 0.5);
            }
        }
    }

    fn add_token(&mut self, doc_id: &str, section_id: &str, token: &str, weight: f32) {
        let entry = self.index.entry(token.to_lowercase()).or_default();

        // Ищем существующую запись для этого документа/секции
        if let Some(pos) = entry
            .iter()
            .position(|(d, s, _)| d == doc_id && s == section_id)
        {
            entry[pos].2 += weight;
        } else {
            entry.push((doc_id.to_string(), section_id.to_string(), weight));
        }
    }

    /// Поиск с TF-IDF ранжированием
    pub fn search(&self, query: &str, max_results: usize) -> Vec<(String, String, f32)> {
        let query_tokens = tokenize(query);
        let mut scores: HashMap<(String, String), f32> = HashMap::new();

        for token in &query_tokens {
            let token_lower = token.to_lowercase();
            if let Some(postings) = self.index.get(&token_lower) {
                let idf = self.calculate_idf(postings.len());

                for (doc_id, section_id, tf) in postings {
                    let score = tf * idf;
                    *scores
                        .entry((doc_id.clone(), section_id.clone()))
                        .or_insert(0.0) += score;
                }
            }
        }

        // Сортировка по убыванию score; total_cmp не паникует (NaN недостижим,
        // но unwrap на partial_cmp в прод-пути запрещён каноном).
        let mut results: Vec<_> = scores.into_iter().collect();
        results.sort_by(|a, b| b.1.total_cmp(&a.1));

        results
            .into_iter()
            .take(max_results)
            .map(|((doc_id, section_id), score)| (doc_id, section_id, score))
            .collect()
    }

    /// IDF в неотрицательной (BM25) форме от числа проиндексированных единиц.
    ///
    /// Раньше здесь стояло `ln(total_docs / doc_freq)`, где `total_docs` — число
    /// документов-версий. На одной версии это давало отрицательный IDF для любого
    /// токена, встречающегося чаще одного раза, а `score = tf * idf` переворачивало
    /// выдачу: чем сильнее совпадение, тем ниже позиция.
    fn calculate_idf(&self, doc_freq: usize) -> f32 {
        if doc_freq == 0 {
            return 0.0;
        }
        let n = self.total_units.max(doc_freq) as f32;
        let df = doc_freq as f32;
        (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
    }
}

/// Токенизация текста (русский + английский)
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty() && s.len() > 2) // игнорируем короткие слова
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize() {
        let tokens = tokenize("Синтез речи в 1С");
        assert!(tokens.contains(&"Синтез".to_string()));
        assert!(tokens.contains(&"речи".to_string()));
    }

    #[test]
    fn test_index_search() {
        let mut index = InvertedIndex::with_boosts(3.0, 2.0);

        let doc = Document {
            version: "8.3.25".to_string(),
            platform: "1С".to_string(),
            release_date: None,
            sections: vec![Section {
                id: "s1".to_string(),
                title: "Синтез речи".to_string(),
                section_type: SectionType::Text,
                content: "Работа со звуком".to_string(),
                keywords: vec!["звук".to_string()],
                subsections: vec![],
            }],
            api_methods: vec![],
            api_objects: vec![],
            relationships: vec![],
        };

        index.index_document(&doc);

        let results = index.search("синтез", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "8.3.25");
    }

    /// Регрессия: точное совпадение с именем должно быть первым, а не последним.
    /// При отрицательном IDF выдача переворачивалась — упоминание в описании
    /// свойства обходило сам метод.
    #[test]
    fn exact_name_match_outranks_passing_mention() {
        let mut index = InvertedIndex::with_boosts(3.0, 2.0);

        let doc = Document {
            version: "8.3.27".to_string(),
            platform: "1С".to_string(),
            release_date: None,
            sections: vec![],
            api_methods: vec![ApiMethod {
                name: "ЗначениеЗаполнено".to_string(),
                description: "Проверяет заполненность значения".to_string(),
                context: ExecutionContext::Both,
                parameters: vec![],
                return_type: None,
                examples: vec![],
                notes: vec![],
            }],
            api_objects: vec![ApiObject {
                name: "ПроверкаЗаполнения".to_string(),
                description: "Варианты проверки реквизитов".to_string(),
                properties: vec![Property {
                    name: "Выдавать".to_string(),
                    property_type: "Булево".to_string(),
                    description: "Учитывается функцией ЗначениеЗаполнено".to_string(),
                    readonly: false,
                }],
                methods: vec![],
            }],
            relationships: vec![],
        };

        index.index_document(&doc);

        let results = index.search("ЗначениеЗаполнено", 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].1, "method_ЗначениеЗаполнено");
        assert!(results[0].2 > results[1].2);
        assert!(results[1].2 > 0.0, "IDF не должен быть отрицательным");
    }
}
