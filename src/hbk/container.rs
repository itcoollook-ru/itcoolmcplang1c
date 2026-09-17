//! container.rs — контейнер 1С (V8 container), формат `.cf`/`.epf`/`.hbk`.
//!
//! Раскладка:
//! ```text
//! файл: [16 байт заголовка][блок таблицы элементов][блоки...]
//! заголовок блока: "\r\n" + 8 hex (размер данных) + " " + 8 hex (размер страницы)
//!                  + " " + 8 hex (адрес продолжения) + " " + "\r\n"   = 31 байт
//! таблица элементов: тройки uint32 (адрес заголовка, адрес данных, 0x7fffffff)
//! заголовок элемента: 8 байт дата создания, 8 байт дата изменения, 4 нуля,
//!                     имя в UTF-16LE, 4 нуля
//! данные элемента: raw deflate либо как есть; могут сами быть контейнером
//! ```

use std::io::Read;

/// Пустой адрес: и признак «нет продолжения», и заполнитель третьего слова
/// тройки в таблице элементов.
pub const EMPTY: u32 = 0x7FFF_FFFF;

/// Длина заголовка блока.
const HDR: usize = 31;

/// Отказ разбора контейнера.
#[derive(Debug)]
pub struct ContainerError(String);

impl std::fmt::Display for ContainerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ContainerError {}

type Result<T> = std::result::Result<T, ContainerError>;

/// Срез в семантике Python: выход за границы усекается, а не паникует.
///
/// Разбор идёт по адресам из самого файла; на повреждённом контейнере они
/// указывают куда угодно, и паника по индексу превратила бы битый `.hbk` в
/// падение сервера.
fn slice(buf: &[u8], from: usize, to: usize) -> &[u8] {
    let from = from.min(buf.len());
    let to = to.clamp(from, buf.len());
    &buf[from..to]
}

/// 8 hex-цифр заголовка блока как число.
fn hex_field(head: &[u8], from: usize, to: usize) -> Result<u32> {
    let raw = slice(head, from, to);
    let text = std::str::from_utf8(raw)
        .map_err(|_| ContainerError("ru: Не hex в заголовке блока, en: Block header is not hex".to_string()))?;
    u32::from_str_radix(text.trim(), 16).map_err(|_| {
        ContainerError(format!(
            "ru: Не hex в заголовке блока: {:?}, en: Block header is not hex: {:?}",
            text, text
        ))
    })
}

fn u32_le(buf: &[u8], off: usize) -> u32 {
    let b = slice(buf, off, off + 4);
    if b.len() < 4 {
        return 0;
    }
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Данные блока, склеенные из страниц продолжения.
pub fn read_block(buf: &[u8], addr: usize) -> Result<Vec<u8>> {
    let head = slice(buf, addr, addr + HDR);
    if head.len() < HDR || &head[0..2] != b"\r\n" {
        return Err(ContainerError(format!(
            "ru: Не заголовок блока по адресу {}, en: Not a block header at address {}",
            addr, addr
        )));
    }

    let data_size = hex_field(head, 2, 10)? as usize;
    let mut page_size = hex_field(head, 11, 19)? as usize;
    let mut next_addr = hex_field(head, 20, 28)? as usize;

    // Ёмкость по размеру файла, а не по объявленному размеру данных: у битого
    // блока там может стоять 0xFFFFFFFF, и резерв обрушил бы процесс.
    let mut out: Vec<u8> = Vec::with_capacity(data_size.min(buf.len()));
    let pos = addr + HDR;
    let take = page_size.min(data_size);
    out.extend_from_slice(slice(buf, pos, pos + take));

    while next_addr != EMPTY as usize && out.len() < data_size {
        let head = slice(buf, next_addr, next_addr + HDR);
        if head.len() < HDR {
            return Err(ContainerError(format!(
                "ru: Обрыв цепочки страниц на адресе {}, en: Page chain truncated at address {}",
                next_addr, next_addr
            )));
        }
        page_size = hex_field(head, 11, 19)? as usize;
        let nxt = hex_field(head, 20, 28)? as usize;
        let pos = next_addr + HDR;
        let take = page_size.min(data_size - out.len());
        let chunk = slice(buf, pos, pos + take);
        // Нулевая страница означала бы бесконечный цикл: в Python он и был,
        // здесь разбор просто останавливается на собранном.
        if chunk.is_empty() {
            break;
        }
        out.extend_from_slice(chunk);
        next_addr = nxt;
    }

    out.truncate(data_size);
    Ok(out)
}

/// Похожи ли байты на контейнер: пустой адрес в первом слове и `\r\n` на 16-м байте.
pub fn is_container(buf: &[u8]) -> bool {
    if buf.len() < 16 + HDR {
        return false;
    }
    if u32_le(buf, 0) != EMPTY {
        return false;
    }
    &buf[16..18] == b"\r\n"
}

/// Имя элемента из его заголовка: UTF-16LE между датами и четырьмя нулями.
fn element_name(head: &[u8]) -> String {
    let tail = slice(head, 20, head.len());
    let end = tail
        .windows(4)
        .position(|w| w == [0, 0, 0, 0])
        .unwrap_or(tail.len());
    let mut name = tail[..end].to_vec();
    if name.len() % 2 == 1 {
        name.push(0);
    }
    let units: Vec<u16> = name
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
        .trim_end_matches('\0')
        .to_string()
}

/// Список `(имя, данные)` элементов контейнера. Данные отдаются как есть, без
/// распаковки: `FileStorage` справки уже лежит готовым ZIP.
pub fn parse(buf: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let toc = read_block(buf, 16)?;
    let mut items = Vec::new();

    let mut off = 0;
    while off + 12 <= toc.len() {
        let h_addr = u32_le(&toc, off);
        let d_addr = u32_le(&toc, off + 4);
        off += 12;

        // Пустой адрес данных — элемент без содержимого (встречается в
        // shquery/shclang): на нём разбор падал целиком.
        if h_addr == EMPTY || d_addr == EMPTY {
            continue;
        }

        let head = read_block(buf, h_addr as usize)?;
        let data = read_block(buf, d_addr as usize)?;
        items.push((element_name(&head), data));
    }

    Ok(items)
}

/// Raw deflate; при неудаче — исходные байты.
///
/// Отдать исходное, а не упасть: часть элементов контейнера лежит без сжатия, и
/// отличить их от сжатых можно только попыткой распаковки.
pub fn inflate(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut decoder = flate2::read::DeflateDecoder::new(data);
    match decoder.read_to_end(&mut out) {
        Ok(_) => out,
        Err(_) => data.to_vec(),
    }
}

/// Рекурсивный обход: `(путь, данные)` листовых элементов.
///
/// Извлечению справки не нужен — `FileStorage` лежит на верхнем уровне, — но
/// остаётся полным API контейнера: им проверяется круговой разбор синтетического
/// вложенного контейнера в тестах.
#[allow(dead_code)]
pub fn walk(buf: &[u8], prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    for (name, raw) in parse(buf)? {
        let data = inflate(&raw);
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{}/{}", prefix, name)
        };
        if is_container(&data) {
            out.extend(walk(&data, &path)?);
        } else {
            out.push((path, data));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Блок с одной страницей: заголовок + данные.
    fn block(data: &[u8], next: u32) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(format!("{:08x} {:08x} {:08x} ", data.len(), data.len(), next).as_bytes());
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(data);
        out
    }

    /// Заголовок элемента: 8 байт даты, 8 байт даты, 4 нуля, имя UTF-16LE, 4 нуля.
    fn element_header(name: &str) -> Vec<u8> {
        let mut out = vec![0u8; 20];
        for unit in name.encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0, 0, 0]);
        out
    }

    /// Контейнер из перечисленных `(имя, данные)`; данные кладутся как есть.
    fn container(items: &[(&str, &[u8])]) -> Vec<u8> {
        let mut heads = Vec::new();
        let mut bodies = Vec::new();
        for (name, data) in items {
            heads.push(block(&element_header(name), EMPTY));
            bodies.push(block(data, EMPTY));
        }

        let toc_len = items.len() * 12;
        let toc_block_len = HDR + toc_len;
        let mut addr = 16 + toc_block_len;

        let mut toc = Vec::new();
        for i in 0..items.len() {
            let h_addr = addr as u32;
            addr += heads[i].len();
            let d_addr = addr as u32;
            addr += bodies[i].len();
            toc.extend_from_slice(&h_addr.to_le_bytes());
            toc.extend_from_slice(&d_addr.to_le_bytes());
            toc.extend_from_slice(&EMPTY.to_le_bytes());
        }

        let mut out = Vec::new();
        out.extend_from_slice(&EMPTY.to_le_bytes());
        out.extend_from_slice(&[0u8; 12]);
        out.extend_from_slice(&block(&toc, EMPTY));
        for i in 0..items.len() {
            out.extend_from_slice(&heads[i]);
            out.extend_from_slice(&bodies[i]);
        }
        out
    }

    #[test]
    fn parses_names_and_data() {
        let buf = container(&[("FileStorage", b"PK\x03\x04data"), ("MainData", b"xyz")]);

        let items = parse(&buf).unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "FileStorage");
        assert_eq!(items[0].1, b"PK\x03\x04data");
        assert_eq!(items[1].0, "MainData");
    }

    /// Синтетический контейнер обязан опознаваться как контейнер — иначе
    /// `walk` не отличит вложенный контейнер от листа.
    #[test]
    fn synthetic_container_is_recognized() {
        let buf = container(&[("A", b"1")]);

        assert!(is_container(&buf));
        assert!(!is_container(b"PK\x03\x04"));
    }

    /// Вложенный контейнер разворачивается в путь через `/`.
    #[test]
    fn walk_descends_into_nested_container() {
        let inner = container(&[("leaf", b"payload")]);
        let outer = container(&[("branch", inner.as_slice())]);

        let items = walk(&outer, "").unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "branch/leaf");
        assert_eq!(items[0].1, b"payload");
    }

    /// Пустые адреса в таблице (shquery/shclang) пропускаются, а не валят разбор.
    #[test]
    fn empty_addresses_are_skipped() {
        let mut buf = container(&[("A", b"1"), ("B", b"2")]);
        // Обнуляем адрес заголовка первой тройки таблицы элементов.
        let toc_at = 16 + HDR;
        buf[toc_at..toc_at + 4].copy_from_slice(&EMPTY.to_le_bytes());

        let items = parse(&buf).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "B");
    }

    /// Данные длиннее страницы собираются из цепочки продолжений.
    #[test]
    fn read_block_joins_continuation_pages() {
        // Блок объявляет 6 байт данных при странице в 3 байта и продолжении.
        let mut buf = vec![0u8; 0];
        let first_at = 0;
        buf.extend_from_slice(b"\r\n");
        buf.extend_from_slice(format!("{:08x} {:08x} {:08x} ", 6, 3, 40).as_bytes());
        buf.extend_from_slice(b"\r\n");
        buf.extend_from_slice(b"abc");
        buf.resize(40, 0);
        buf.extend_from_slice(b"\r\n");
        buf.extend_from_slice(format!("{:08x} {:08x} {:08x} ", 3, 3, EMPTY).as_bytes());
        buf.extend_from_slice(b"\r\n");
        buf.extend_from_slice(b"def");

        assert_eq!(read_block(&buf, first_at).unwrap(), b"abcdef");
    }

    /// Повреждённый адрес не должен паниковать по индексу.
    #[test]
    fn broken_address_is_an_error_not_a_panic() {
        let buf = container(&[("A", b"1")]);

        assert!(read_block(&buf, buf.len() + 1000).is_err());
    }

    #[test]
    fn inflate_returns_source_on_failure() {
        assert_eq!(inflate(b"not deflate at all"), b"not deflate at all");
    }
}
