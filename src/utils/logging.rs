use crate::config::LoggingConfig;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Инициализация системы логирования
pub fn init_logging(config: &LoggingConfig) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.level));

    // stdio-транспорт держит JSON-RPC в stdout — диагностика обязана идти в stderr,
    // иначе строки логов ломают протокольный поток под core как MCP-host.
    match config.format.as_str() {
        "json" => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .with_writer(std::io::stderr),
                )
                .init();
        }
        "compact" => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .compact()
                        .with_writer(std::io::stderr),
                )
                .init();
        }
        _ => {
            // "pretty" by default
            tracing_subscriber::registry()
                .with(env_filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .pretty()
                        .with_writer(std::io::stderr),
                )
                .init();
        }
    }

    tracing::info!(
        "ru: Логирование инициализировано (уровень: {}, формат: {}), en: Logging initialized (level: {}, format: {})",
        config.level,
        config.format,
        config.level,
        config.format
    );
}
