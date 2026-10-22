use std::future::Future;

use crate::{Error, Result, Row, ToSql};

/// Map a PostgreSQL row by column name, retaining the driver's typed decoding.
pub trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self>;
}

impl FromRow for Row {
    fn from_row(row: &Row) -> Result<Self> {
        Ok(row.clone())
    }
}

/// SQL execution shared by a pool, checked-out connection and transaction.
///
/// Use PostgreSQL's `$1`, `$2`, ... placeholders and bind all data values.
/// SQL identifiers cannot be bound; use static SQL or the entity derive.
/// No SQL rewriting, cross-database abstraction or custom value codec is involved.
pub trait Executor: Sync {
    fn execute(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<u64>> + Send;

    fn query_raw(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Vec<Row>>> + Send;

    /// Returns an error if more than one row matches.
    fn query_optional_raw(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Option<Row>>> + Send;

    fn fetch_all<T: FromRow + Send>(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Vec<T>>> + Send {
        async move {
            self.query_raw(sql, params)
                .await?
                .iter()
                .map(T::from_row)
                .collect()
        }
    }

    fn fetch_optional<T: FromRow + Send>(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Option<T>>> + Send {
        async move {
            self.query_optional_raw(sql, params)
                .await?
                .as_ref()
                .map(T::from_row)
                .transpose()
        }
    }

    fn fetch_one<T: FromRow + Send>(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<T>> + Send {
        async move {
            self.fetch_optional(sql, params)
                .await?
                .ok_or(Error::NotFound)
        }
    }
}
