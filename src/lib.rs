//! PostgreSQL-only, SQL-first ORM built on community infrastructure.
//!
//! With the default `derive` feature, `#[html_sql("path/to/mapper.xml")]`
//! compiles XML SQL templates into typed Rust functions. Use [`BoundSql`] to
//! inspect rendered SQL and parameters without a database connection.
//!
//! Import [`Executor`] for parameterized SQL and [`Crud`] for entity operations.
//! The application owns logging configuration; this library only emits tracing events.
//! Drivers, pools, transactions, type codecs and TLS remain upstream implementations.

#![forbid(unsafe_code)]

extern crate self as pgorm;

mod database;
mod entity;
mod error;
mod executor;
#[doc(hidden)]
pub mod template;

pub use database::{Connection, Database, Transaction};
pub use entity::{Crud, Entity, PageRequest};
pub use error::{Error, Result};
pub use executor::{Executor, FromRow};
#[doc(hidden)]
pub use template::SqlBuilder;
pub use template::{BoundSql, SqlIdentifier};
pub use tokio_postgres::{Row, types::ToSql};

// Public escape hatches preserve the upstream ecosystem and its configuration.
pub use deadpool_postgres;
#[cfg(feature = "native-tls")]
pub use postgres_native_tls;
pub use tokio_postgres;

#[cfg(feature = "derive")]
pub use pgorm_macros::{Entity, FromRow, html_sql};
