#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("unauthorized")]
    Unauthorized,
}

pub type CoreResult<T> = Result<T, CoreError>;
