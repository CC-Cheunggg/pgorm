use std::{future::Future, time::Instant};

use deadpool_postgres::{GenericClient, Pool};
use tracing::Instrument;

use crate::{Executor, Result, Row, ToSql};

/// Cheaply cloneable database handle backed by deadpool-postgres.
#[derive(Clone)]
pub struct Database {
    pool: Pool,
}

impl Database {
    /// Wrap an upstream pool, retaining its TLS, timeout and recycling settings.
    /// This constructor is lazy; use `acquire().await` to verify connectivity.
    pub fn from_pool(pool: Pool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Connect using native TLS, system certificate roots and bounded pool waits.
    ///
    /// TLS is required unless the DSN explicitly uses `sslmode=disable` (e.g. a
    /// local test server). Even `sslmode=prefer` is upgraded to Require: there is
    /// no silent plaintext fallback. Use `from_pool` for other TLS connectors,
    /// custom roots, pool sizes and advanced driver configuration.
    #[cfg(feature = "native-tls")]
    pub async fn connect(dsn: &str) -> Result<Self> {
        use std::time::Duration;

        use deadpool_postgres::{Manager, ManagerConfig, RecyclingMethod, Runtime};
        use tokio_postgres::config::{SslMode, SslNegotiation};

        let mut config: tokio_postgres::Config = dsn.parse()?;
        if config.get_ssl_mode() != SslMode::Disable {
            config.ssl_mode(SslMode::Require);
        }
        let mut tls_builder = native_tls::TlsConnector::builder();
        if config.get_ssl_negotiation() == SslNegotiation::Direct {
            postgres_native_tls::set_postgresql_alpn(&mut tls_builder);
        }
        let tls = postgres_native_tls::MakeTlsConnector::new(tls_builder.build()?);
        let manager = Manager::from_config(
            config,
            tls,
            ManagerConfig {
                recycling_method: RecyclingMethod::Verified,
            },
        );
        let pool = Pool::builder(manager)
            .runtime(Runtime::Tokio1)
            .max_size(16)
            .wait_timeout(Some(Duration::from_secs(5)))
            .create_timeout(Some(Duration::from_secs(10)))
            .recycle_timeout(Some(Duration::from_secs(5)))
            .build()?;
        let db = Self::from_pool(pool);
        // Building a pool alone does not establish a connection.
        db.acquire().await?.inner.simple_query("SELECT 1").await?;
        Ok(db)
    }

    pub async fn acquire(&self) -> Result<Connection> {
        async {
            let start = Instant::now();
            let result = self.pool.get().await;
            match &result {
                Ok(_) => tracing::debug!(
                    elapsed_ms = start.elapsed().as_secs_f64() * 1000.0,
                    "connection acquired"
                ),
                Err(_) => tracing::warn!(
                    elapsed_ms = start.elapsed().as_secs_f64() * 1000.0,
                    error.kind = "pool",
                    "connection acquisition failed"
                ),
            }
            Ok(Connection { inner: result? })
        }
        .instrument(tracing::debug_span!(
            "pgorm.acquire",
            db.system = "postgresql"
        ))
        .await
    }
}

/// A connection lease. Drop returns it to the upstream pool.
pub struct Connection {
    inner: deadpool_postgres::Client,
}

impl Connection {
    /// Direct driver access for COPY, LISTEN, custom transaction builders, etc.
    /// Operations through this escape hatch bypass pgorm's query tracing.
    pub fn raw(&self) -> &deadpool_postgres::Client {
        &self.inner
    }

    pub fn raw_mut(&mut self) -> &mut deadpool_postgres::Client {
        &mut self.inner
    }

    pub async fn transaction(&mut self) -> Result<Transaction<'_>> {
        let inner = observed("begin", self.inner.transaction(), |_| 0).await?;
        Ok(Transaction { inner })
    }
}

/// Borrowed driver transaction. Drop enqueues rollback through tokio-postgres.
/// Keep all transactional work on this value, rather than on `Database`.
pub struct Transaction<'a> {
    inner: deadpool_postgres::Transaction<'a>,
}

impl<'a> Transaction<'a> {
    pub fn raw(&self) -> &deadpool_postgres::Transaction<'a> {
        &self.inner
    }

    pub async fn commit(self) -> Result<()> {
        observed("commit", self.inner.commit(), |_| 0).await
    }

    pub async fn rollback(self) -> Result<()> {
        observed("rollback", self.inner.rollback(), |_| 0).await
    }
}

// Only static operation labels, counts and SQLSTATE are emitted. SQL text,
// parameter values, DSNs and database error details may contain application data.
async fn observed<T, F, C>(operation: &'static str, future: F, count: C) -> Result<T>
where
    T: Send,
    F: Future<Output = std::result::Result<T, tokio_postgres::Error>> + Send,
    C: Fn(&T) -> u64 + Send,
{
    async move {
        let start = Instant::now();
        let result = future.await;
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        match &result {
            Ok(value) => tracing::debug!(
                elapsed_ms,
                db.rows = count(value),
                "database operation completed"
            ),
            Err(error) => tracing::warn!(
                elapsed_ms,
                error.kind = "database",
                db.response.status_code = error.code().map_or("", |code| code.code()),
                "database operation failed"
            ),
        }
        result.map_err(Into::into)
    }
    .instrument(tracing::debug_span!(
        "pgorm.query",
        db.system = "postgresql",
        db.operation.name = operation
    ))
    .await
}

async fn execute<C: GenericClient>(
    client: &C,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<u64> {
    observed(
        "execute",
        async {
            let statement = client.prepare_cached(sql).await?;
            client.execute(&statement, params).await
        },
        |n| *n,
    )
    .await
}

async fn query<C: GenericClient>(
    client: &C,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Vec<Row>> {
    observed(
        "query",
        async {
            let statement = client.prepare_cached(sql).await?;
            client.query(&statement, params).await
        },
        |rows| rows.len() as u64,
    )
    .await
}

async fn query_optional<C: GenericClient>(
    client: &C,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Option<Row>> {
    observed(
        "query_optional",
        async {
            let statement = client.prepare_cached(sql).await?;
            client.query_opt(&statement, params).await
        },
        |row| u64::from(row.is_some()),
    )
    .await
}

impl Executor for Database {
    async fn execute(&self, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<u64> {
        self.acquire().await?.execute(sql, params).await
    }

    async fn query_raw(&self, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<Vec<Row>> {
        self.acquire().await?.query_raw(sql, params).await
    }

    async fn query_optional_raw(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Option<Row>> {
        self.acquire().await?.query_optional_raw(sql, params).await
    }
}

macro_rules! impl_executor {
    ($ty:ty) => {
        impl Executor for $ty {
            async fn execute(&self, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<u64> {
                execute(&self.inner, sql, params).await
            }

            async fn query_raw(
                &self,
                sql: &str,
                params: &[&(dyn ToSql + Sync)],
            ) -> Result<Vec<Row>> {
                query(&self.inner, sql, params).await
            }

            async fn query_optional_raw(
                &self,
                sql: &str,
                params: &[&(dyn ToSql + Sync)],
            ) -> Result<Option<Row>> {
                query_optional(&self.inner, sql, params).await
            }
        }
    };
}

impl_executor!(Connection);
impl_executor!(Transaction<'_>);
