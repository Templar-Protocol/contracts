#[derive(Debug, thiserror::Error)]
pub enum CoreError {}

pub type CoreResult<T> = Result<T, CoreError>;
