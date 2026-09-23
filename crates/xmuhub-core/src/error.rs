#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("未找到：{0}")]
    NotFound(&'static str),
    #[error("权限不足")]
    Forbidden,
    #[error("请先登录")]
    Unauthorized,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    TooMany(String),
    #[error("存储服务错误：{0}")]
    Upstream(String),
    #[error("内部错误：{0}")]
    Internal(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub fn bad(msg: impl Into<String>) -> Error {
    Error::BadRequest(msg.into())
}

macro_rules! internal_from {
    ($($t:ty),* $(,)?) => {$(
        impl From<$t> for Error {
            fn from(e: $t) -> Self { Error::Internal(e.to_string()) }
        }
    )*};
}

internal_from!(
    std::io::Error,
    redb::Error,
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError,
    postcard::Error,
    tantivy::TantivyError,
);

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Upstream(e.to_string())
    }
}
