use std::future::Future;

use crate::{Error, Executor, FromRow, Result, ToSql};

/// Statically declared entity metadata. Prefer `#[derive(pgorm::Entity)]`.
///
/// The first release supports one primary-key column. Generated keys are omitted
/// on INSERT; INSERT and UPDATE both use RETURNING to read database defaults.
pub trait Entity: FromRow + Sync {
    type Id: ToSql + Sync;
    const TABLE: &'static str;
    const SCHEMA: Option<&'static str>;
    const COLUMNS: &'static [&'static str];
    const PRIMARY_KEY: &'static str;
    const INSERT_COLUMNS: &'static [&'static str];
    const UPDATE_COLUMNS: &'static [&'static str];
    fn id(&self) -> &Self::Id;
    fn insert_params(&self) -> Vec<&(dyn ToSql + Sync)>;
    fn update_params(&self) -> Vec<&(dyn ToSql + Sync)>;
}

/// Validated, one-based pagination. No implicit COUNT query is issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    page: u64,
    per_page: i64,
    offset: i64,
}

impl PageRequest {
    pub fn new(page: u64, per_page: u64) -> Result<Self> {
        if page == 0 || !(1..=1000).contains(&per_page) {
            return Err(Error::InvalidPage);
        }
        let offset = (page - 1)
            .checked_mul(per_page)
            .and_then(|n| i64::try_from(n).ok())
            .ok_or(Error::InvalidPage)?;
        Ok(Self {
            page,
            per_page: per_page as i64,
            offset,
        })
    }

    pub fn page(self) -> u64 {
        self.page
    }

    pub fn per_page(self) -> u64 {
        self.per_page as u64
    }
}

fn identifier(name: &str) -> Result<String> {
    if name.is_empty() || name.contains('\0') {
        return Err(Error::InvalidIdentifier);
    }
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}

fn table<E: Entity>() -> Result<String> {
    let name = identifier(E::TABLE)?;
    match E::SCHEMA {
        Some(schema) => Ok(format!("{}.{}", identifier(schema)?, name)),
        None => Ok(name),
    }
}

fn columns(names: &[&str]) -> Result<String> {
    names
        .iter()
        .map(|n| identifier(n))
        .collect::<Result<Vec<_>>>()
        .map(|n| n.join(", "))
}

/// Small, typed CRUD helpers. Custom queries remain ordinary PostgreSQL SQL.
pub trait Crud: Executor {
    fn insert<E: Entity + Send>(&self, entity: &E) -> impl Future<Output = Result<E>> + Send {
        async move {
            let params = entity.insert_params();
            let values = if E::INSERT_COLUMNS.is_empty() {
                "DEFAULT VALUES".to_owned()
            } else {
                let placeholders = (1..=E::INSERT_COLUMNS.len())
                    .map(|n| format!("${n}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({}) VALUES ({placeholders})", columns(E::INSERT_COLUMNS)?)
            };
            let sql = format!(
                "INSERT INTO {} {values} RETURNING {}",
                table::<E>()?,
                columns(E::COLUMNS)?
            );
            self.fetch_one(&sql, &params).await
        }
    }

    fn find_by_id<E: Entity + Send>(
        &self,
        id: &E::Id,
    ) -> impl Future<Output = Result<Option<E>>> + Send {
        async move {
            let sql = format!(
                "SELECT {} FROM {} WHERE {} = $1",
                columns(E::COLUMNS)?,
                table::<E>()?,
                identifier(E::PRIMARY_KEY)?
            );
            self.fetch_optional(&sql, &[id]).await
        }
    }

    /// Updates every mutable field, including `None` as SQL NULL.
    /// Use explicit SQL for a partial update (a missing field is not a NULL).
    fn update<E: Entity + Send>(
        &self,
        entity: &E,
    ) -> impl Future<Output = Result<Option<E>>> + Send {
        async move {
            if E::UPDATE_COLUMNS.is_empty() {
                return Err(Error::NoUpdateColumns);
            }
            let assignments = E::UPDATE_COLUMNS
                .iter()
                .enumerate()
                .map(|(i, col)| Ok(format!("{} = ${}", identifier(col)?, i + 1)))
                .collect::<Result<Vec<_>>>()?
                .join(", ");
            let mut params = entity.update_params();
            params.push(entity.id());
            let sql = format!(
                "UPDATE {} SET {assignments} WHERE {} = ${} RETURNING {}",
                table::<E>()?,
                identifier(E::PRIMARY_KEY)?,
                params.len(),
                columns(E::COLUMNS)?
            );
            self.fetch_optional(&sql, &params).await
        }
    }

    fn delete_by_id<E: Entity + Send>(
        &self,
        id: &E::Id,
    ) -> impl Future<Output = Result<u64>> + Send {
        async move {
            let sql = format!(
                "DELETE FROM {} WHERE {} = $1",
                table::<E>()?,
                identifier(E::PRIMARY_KEY)?
            );
            self.execute(&sql, &[id]).await
        }
    }

    /// Stable primary-key order; one bounded query, with no total-count promise.
    fn list<E: Entity + Send>(
        &self,
        page: PageRequest,
    ) -> impl Future<Output = Result<Vec<E>>> + Send {
        async move {
            let sql = format!(
                "SELECT {} FROM {} ORDER BY {} ASC LIMIT $1 OFFSET $2",
                columns(E::COLUMNS)?,
                table::<E>()?,
                identifier(E::PRIMARY_KEY)?
            );
            self.fetch_all(&sql, &[&page.per_page, &page.offset]).await
        }
    }
}

impl<T: Executor + ?Sized> Crud for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_rejects_invalid_or_overflowing_offsets() {
        for (p, n) in [(0, 10), (1, 0), (1, 1001), (u64::MAX, 1000)] {
            assert!(PageRequest::new(p, n).is_err());
        }
        let page = PageRequest::new(3, 20).unwrap();
        assert_eq!(page.offset, 40);
    }

    #[test]
    fn identifiers_are_delimited_without_interpreting_sql() {
        assert_eq!(identifier("schema.table").unwrap(), "\"schema.table\"");
        assert_eq!(
            identifier("a\"; DROP TABLE users; --").unwrap(),
            "\"a\"\"; DROP TABLE users; --\""
        );
        assert!(identifier("").is_err());
        assert!(identifier("x\0y").is_err());
    }
}
