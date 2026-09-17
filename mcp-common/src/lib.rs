//! `mcp-common` — общий слой MCP-серверов каталога itcoollook.
//!
//! Крейт содержит то, что до его выделения дублировалось в каждом сервере и уже
//! начинало расходиться:
//!
//! - [`jsonrpc`] — конверт JSON-RPC 2.0 ([`Request`], [`Response`], [`RpcError`]);
//! - [`protocol`] — MCP wire-типы (`initialize`, `tools`, `resources`, `prompts`,
//!   [`Content`]), сериализуемые в camelCase;
//! - [`server_info`] — инструмент `server_info`, одинаковый у трёх серверов;
//! - [`stdio`] — единый цикл транспорта ([`run_stdio`]).
//!
//! Диспетчеризация методов остаётся за конкретным сервером: он передаёт в
//! [`run_stdio`] замыкание `Request -> Response`, реализуя ровно тот набор методов,
//! который поддерживает (conf: `tools`+`resources`; lang: `tools`+`prompts`;
//! run: `tools`).
//!
//! # Пример
//! ```no_run
//! use mcp_common::{run_stdio, Request, Response, RpcError};
//! use serde_json::json;
//!
//! # async fn run() -> std::io::Result<()> {
//! run_stdio(|req: Request| async move {
//!     match req.method.as_str() {
//!         "initialize" => Response::success(req.id, json!({ /* InitializeResult */ })),
//!         other => Response::failure(
//!             req.id,
//!             RpcError::method_not_found(format!(
//!                 "ru: Неизвестный метод: {other}, en: Unknown method: {other}"
//!             )),
//!         ),
//!     }
//! })
//! .await
//! # }
//! ```

pub mod cli;
pub mod jsonrpc;
pub mod protocol;
pub mod server_info;
pub mod stdio;

pub use jsonrpc::{Request, Response, RpcError};
pub use protocol::*;
pub use server_info::{server_info_text, server_info_tool, SERVER_INFO_TOOL};
pub use stdio::{run_stdio, run_stdio_with};

/// Версия сервера с меткой сборки: `0.2.0 (2026-08-06, e450f3d)`.
///
/// Одна строка на все точки вывода — `--version`, `serverInfo.version` и
/// стартовая запись журнала. Без метки шесть архивов подряд представлялись
/// одинаково, и «обновились» с «не обновились» снаружи не различались.
///
/// Раскрывается в крейте сервера: `BUILD_DATE` и `BUILD_COMMIT` задаёт его
/// собственный build-скрипт (`build.rs` → `../build_info.rs`).
#[macro_export]
macro_rules! build_version {
    () => {
        concat!(
            env!("CARGO_PKG_VERSION"),
            " (",
            env!("BUILD_DATE"),
            ", ",
            env!("BUILD_COMMIT"),
            ")"
        )
    };
}
