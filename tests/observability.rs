#![cfg(feature = "native-tls")]

use std::{
    io,
    sync::{Arc, Mutex},
};

use pgorm::{Database, Executor, Row};
use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn query_events_report_metadata_without_sql_values_or_error_details() {
    let output = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter("pgorm=trace,tokio_postgres=off")
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();

    async {
        let dsn = std::env::var("DATABASE_URL").expect("DATABASE_URL is required");
        let db = Database::connect(&dsn).await.unwrap();
        let conn = db.acquire().await.unwrap();
        let row: Row = conn
            .fetch_one("SELECT $1::TEXT AS value", &[&"secret_bound_value"])
            .await
            .unwrap();
        assert_eq!(row.get::<_, String>(0), "secret_bound_value");
        let _: Row = conn
            .fetch_one("SELECT 'secret_sql_literal'::TEXT", &[])
            .await
            .unwrap();
        conn.execute(
            "CREATE TEMP TABLE secret_table_name (value TEXT UNIQUE)",
            &[],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO secret_table_name VALUES ($1)",
            &[&"secret_unique_value"],
        )
        .await
        .unwrap();
        assert!(
            conn.execute(
                "INSERT INTO secret_table_name VALUES ($1)",
                &[&"secret_unique_value"]
            )
            .await
            .is_err()
        );
    }
    .with_subscriber(subscriber)
    .await;

    let log = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("database operation completed"), "{log}");
    assert!(log.contains("database operation failed"), "{log}");
    assert!(log.contains("23505"), "SQLSTATE should be available: {log}");
    assert!(log.contains("elapsed_ms"), "{log}");
    assert!(log.contains("db.rows"), "{log}");
    for secret in [
        "secret_bound_value",
        "secret_sql_literal",
        "secret_unique_value",
        "secret_table_name",
        "SELECT",
        "INSERT",
        "postgresql://",
    ] {
        assert!(
            !log.contains(secret),
            "unexpected SQL/application data in pgorm events: {log}"
        );
    }
}
