use thiserror::Error as ThisError;

#[derive(Debug, ThisError)]
pub enum Error {
    #[error("LLM provider error: {0}")]
    Provider(String),

    #[error("Tool execution error: {0}")]
    Tool(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Session error: {0}")]
    Session(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SQL error: {0}")]
    Sql(#[from] rusqlite::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Brokering error: {0}")]
    Brokering(#[from] crate::brokering::BrokeringError),

    #[error("Budget exceeded: {max_iterations} iterations")]
    BudgetExceeded { max_iterations: usize },
}

pub type Result<T> = std::result::Result<T, Error>;
