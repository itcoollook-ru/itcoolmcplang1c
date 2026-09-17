//! stdio-транспорт MCP: построчный JSON-RPC на stdin/stdout.
//!
//! Это единственная копия цикла чтения/записи для всех серверов каталога.
//! Диспетчеризация методов остаётся в сервере: `run_stdio` принимает асинхронный
//! обработчик `Request -> Response`, ничего не зная о конкретных методах.
//!
//! Один JSON-RPC объект на строку (line-delimited). Нотификации (запросы без `id`)
//! обрабатываются, но ответ на них не отправляется — как требует JSON-RPC 2.0.

use crate::jsonrpc::{Request, Response, RpcError};
use serde_json::Value;
use std::future::Future;
use tokio::io::{stdin, stdout, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// Прочитать stdin построчно и обслужить каждый JSON-RPC запрос обработчиком.
///
/// `handler` вызывается на каждый корректно разобранный запрос и возвращает
/// [`Response`]. Ответ пишется в stdout только для запросов с `id`
/// (для нотификаций — молча пропускается). Ошибочные строки получают ответ по
/// JSON-RPC 2.0: не-JSON и не-UTF-8 — `-32700`, валидный JSON неверной формы —
/// `-32600`. Завершается на EOF (`Ok(0)`).
pub async fn run_stdio<F, Fut>(mut handler: F) -> std::io::Result<()>
where
    F: FnMut(Request) -> Fut,
    Fut: Future<Output = Response>,
{
    let reader = BufReader::new(stdin());
    let writer = stdout();
    run_stdio_with(reader, writer, &mut handler).await
}

/// Как [`run_stdio`], но с явными потоками ввода/вывода — для тестов и нестандартных
/// хостов. `R` должен быть `AsyncBufRead`.
pub async fn run_stdio_with<R, W, F, Fut>(
    mut reader: R,
    mut writer: W,
    mut handler: F,
) -> std::io::Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWrite + Unpin,
    F: FnMut(Request) -> Fut,
    Fut: Future<Output = Response>,
{
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) => break, // EOF
            Ok(_) => {
                // Не UTF-8 — parse error, а не смерть всего цикла. Подменять битые
                // байты на U+FFFD нельзя: искажённый запрос мог бы молча исполниться.
                let Ok(text) = std::str::from_utf8(&line) else {
                    let response = Response::failure(
                        None,
                        RpcError::parse_error(
                            "ru: Строка не в кодировке UTF-8, en: Input line is not valid UTF-8",
                        ),
                    );
                    write_line(&mut writer, &response).await?;
                    continue;
                };

                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }

                // Двухэтапный разбор по JSON-RPC 2.0: синтаксически битый JSON —
                // -32700; валидный JSON, не являющийся Request-объектом, — -32600
                // (с эхом id, когда его удалось извлечь).
                let value: Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(e) => {
                        let response = Response::failure(
                            None,
                            RpcError::parse_error(format!(
                                "ru: Ошибка разбора JSON: {e}, en: Parse error: {e}"
                            )),
                        );
                        write_line(&mut writer, &response).await?;
                        continue;
                    }
                };

                let id = value.get("id").cloned().filter(|v| !v.is_null());
                match serde_json::from_value::<Request>(value) {
                    Ok(request) => {
                        let is_notification = request.is_notification();
                        let response = handler(request).await;
                        if !is_notification {
                            write_line(&mut writer, &response).await?;
                        }
                    }
                    Err(e) => {
                        let response = Response::failure(
                            id,
                            RpcError::invalid_request(format!(
                                "ru: Невалидный JSON-RPC запрос: {e}, en: Invalid Request: {e}"
                            )),
                        );
                        write_line(&mut writer, &response).await?;
                    }
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Сериализовать значение в одну строку и отправить с переводом строки и flush.
async fn write_line<W, T>(writer: &mut W, value: &T) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: serde::Serialize,
{
    let json = serde_json::to_string(value)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonrpc::Response;
    use serde_json::json;

    #[tokio::test]
    async fn answers_request_and_skips_notification() {
        // Вход: один запрос с id и одна нотификация без id.
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n"
        );
        let reader = BufReader::new(input.as_bytes());
        let mut out: Vec<u8> = Vec::new();

        run_stdio_with(reader, &mut out, |req| async move {
            Response::success(req.id, json!({"method": req.method}))
        })
        .await
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        // Ровно одна строка ответа — на запрос с id; нотификация без ответа.
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1, "нотификация не должна давать ответ");
        assert!(lines[0].contains(r#""id":7"#));
        assert!(lines[0].contains(r#""method":"ping""#));
    }

    #[tokio::test]
    async fn reports_parse_error_with_null_id() {
        let reader = BufReader::new(&b"not json\n"[..]);
        let mut out: Vec<u8> = Vec::new();

        run_stdio_with(reader, &mut out, |req| async move {
            Response::success(req.id, json!(null))
        })
        .await
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(r#""code":-32700"#));
        // JSON-RPC 2.0: id обязателен и равен null, когда его не удалось определить.
        assert!(text.contains(r#""id":null"#));
    }

    /// Битая кодировка — parse error и продолжение цикла, а не завершение сервера.
    #[tokio::test]
    async fn invalid_utf8_reports_parse_error_and_continues() {
        let mut input: Vec<u8> = b"\xff\xfe\xfd\n".to_vec();
        input.extend_from_slice(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
        input.push(b'\n');
        let reader = BufReader::new(&input[..]);
        let mut out: Vec<u8> = Vec::new();

        run_stdio_with(reader, &mut out, |req| async move {
            Response::success(req.id, json!({}))
        })
        .await
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "после битой строки цикл должен продолжиться");
        assert!(lines[0].contains(r#""code":-32700"#));
        assert!(lines[1].contains(r#""id":1"#));
    }

    /// Валидный JSON, не являющийся Request-объектом, — -32600 с эхом id.
    #[tokio::test]
    async fn invalid_request_shape_reports_invalid_request() {
        let input = concat!(
            r#"{"foo":1}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":42,"params":{}}"#,
            "\n"
        );
        let reader = BufReader::new(input.as_bytes());
        let mut out: Vec<u8> = Vec::new();

        run_stdio_with(reader, &mut out, |req| async move {
            Response::success(req.id, json!(null))
        })
        .await
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        // Без id — в ответе null.
        assert!(lines[0].contains(r#""code":-32600"#));
        assert!(lines[0].contains(r#""id":null"#));
        // id извлекается из невалидного запроса (нет обязательного method).
        assert!(lines[1].contains(r#""code":-32600"#));
        assert!(lines[1].contains(r#""id":42"#));
    }
}
