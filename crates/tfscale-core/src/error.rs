pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("invalid value for {field}: {reason}")]
    InvalidValue { field: &'static str, reason: String },
}
