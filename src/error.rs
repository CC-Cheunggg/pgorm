/// Errors retain their upstream source, including PostgreSQL SQLSTATE.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Database(#[from] tokio_postgres::Error),
    #[error(transparent)]
    Pool(#[from] deadpool_postgres::PoolError),
    #[error(transparent)]
    PoolBuild(#[from] deadpool_postgres::BuildError),
    #[cfg(feature = "native-tls")]
    #[error(transparent)]
    Tls(#[from] native_tls::Error),
    #[error("query returned no rows")]
    NotFound,
    #[error("page must be >= 1, page size must be 1..=1000, and offset must fit i64")]
    InvalidPage,
    #[error("entity has no columns that can be updated")]
    NoUpdateColumns,
    #[error("SQL identifiers must be nonempty and contain no NUL bytes")]
    InvalidIdentifier,
}

pub type Result<T> = std::result::Result<T, Error>;
