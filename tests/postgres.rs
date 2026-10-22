//! Run against a disposable database with:
//! DATABASE_URL='postgres://postgres:postgres@localhost:5432/pgorm_test?sslmode=disable' \
//!     cargo test --workspace --all-features -- --include-ignored
//!
//! Every table is temporary and belongs to one checked-out connection. The
//! tests never create, modify, or drop a persistent application table.

use pgorm::{Connection, Crud, Database, Error, Executor, PageRequest};
use std::time::Duration;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Debug, PartialEq, pgorm::Entity)]
#[pgorm(table = "pgorm_test_users")]
struct User {
    #[pgorm(id, generated)]
    id: i64,
    #[pgorm(column = "display_name")]
    name: String,
    bio: Option<String>,
}

#[derive(Debug, PartialEq, pgorm::FromRow)]
struct TextValue {
    value: String,
}

#[derive(Debug, PartialEq, pgorm::FromRow)]
struct Number {
    value: i64,
}

fn database_url() -> String {
    std::env::var("DATABASE_URL").expect(
        "PostgreSQL integration tests require DATABASE_URL pointing to a disposable test database",
    )
}

async fn isolated_connection() -> Result<(Database, Connection), Box<dyn std::error::Error>> {
    let database = Database::connect(&database_url()).await?;
    let connection = database.acquire().await?;
    Ok((database, connection))
}

fn single_connection_database() -> Result<Database, Box<dyn std::error::Error>> {
    use pgorm::deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
    use pgorm::tokio_postgres::config::SslMode;

    let mut config: pgorm::tokio_postgres::Config = database_url().parse()?;
    if config.get_ssl_mode() != SslMode::Disable {
        config.ssl_mode(SslMode::Require);
    }
    let tls = pgorm::postgres_native_tls::MakeTlsConnector::new(native_tls::TlsConnector::new()?);
    let manager = Manager::from_config(
        config,
        tls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Verified,
        },
    );
    let pool = Pool::builder(manager)
        .runtime(Runtime::Tokio1)
        .max_size(1)
        .wait_timeout(Some(Duration::from_millis(50)))
        .create_timeout(Some(Duration::from_secs(5)))
        .recycle_timeout(Some(Duration::from_secs(5)))
        .build()?;
    Ok(Database::from_pool(pool))
}

async fn create_users(connection: &Connection) -> pgorm::Result<()> {
    connection
        .execute(
            "CREATE TEMP TABLE pgorm_test_users (\
             id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \
             display_name TEXT NOT NULL, bio TEXT)",
            &[],
        )
        .await?;
    Ok(())
}

fn user(name: &str) -> User {
    User {
        id: 0,
        name: name.to_owned(),
        bio: None,
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn crud_returns_generated_ids_and_database_rows() -> TestResult {
    let (_database, connection) = isolated_connection().await?;
    create_users(&connection).await?;

    let inserted = connection.insert(&user("Ada")).await?;
    assert!(inserted.id > 0);
    assert_eq!(inserted.name, "Ada");
    assert_eq!(inserted.bio, None);
    assert_eq!(
        connection.find_by_id::<User>(&inserted.id).await?,
        Some(User {
            id: inserted.id,
            name: "Ada".to_owned(),
            bio: None,
        })
    );

    let changed = User {
        id: inserted.id,
        name: "Grace".to_owned(),
        bio: Some("compiler pioneer".to_owned()),
    };
    assert_eq!(connection.update(&changed).await?, Some(changed));
    assert_eq!(connection.delete_by_id::<User>(&inserted.id).await?, 1);
    assert_eq!(connection.find_by_id::<User>(&inserted.id).await?, None);
    assert_eq!(connection.delete_by_id::<User>(&inserted.id).await?, 0);
    assert_eq!(
        connection
            .update(&User {
                id: inserted.id,
                name: "missing".to_owned(),
                bio: None,
            })
            .await?,
        None
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn bound_parameters_preserve_quotes_unicode_and_nulls() -> TestResult {
    let (_database, connection) = isolated_connection().await?;
    create_users(&connection).await?;
    let name = "O'Reilly; \"quoted\" — 中文";
    let inserted = connection.insert(&user(name)).await?;
    assert_eq!(inserted.name, name);
    assert_eq!(inserted.bio, None);

    let fetched = connection
        .fetch_one::<User>(
            "SELECT id, display_name, bio FROM pgorm_test_users WHERE display_name = $1",
            &[&name],
        )
        .await?;
    assert_eq!(fetched, inserted);

    let count = connection
        .fetch_one::<Number>("SELECT count(*) AS value FROM pgorm_test_users", &[])
        .await?;
    assert_eq!(count.value, 1);
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn result_cardinality_and_mapping_failures_are_reported() -> TestResult {
    let (_database, connection) = isolated_connection().await?;
    assert_eq!(
        connection
            .fetch_optional::<TextValue>("SELECT 'value'::TEXT AS value WHERE FALSE", &[])
            .await?,
        None
    );
    let missing = connection
        .fetch_one::<TextValue>("SELECT 'value'::TEXT AS value WHERE FALSE", &[])
        .await
        .unwrap_err();
    assert!(matches!(missing, Error::NotFound));

    assert!(
        connection
            .fetch_optional::<TextValue>(
                "SELECT value FROM (VALUES ('one'), ('two')) AS v(value)",
                &[]
            )
            .await
            .is_err()
    );
    assert!(
        connection
            .fetch_one::<TextValue>("SELECT 42::BIGINT AS value", &[])
            .await
            .is_err()
    );
    assert!(
        connection
            .fetch_one::<TextValue>("SELECT 'value'::TEXT AS another_column", &[])
            .await
            .is_err()
    );

    let values = connection
        .fetch_all::<Number>(
            "SELECT value::BIGINT AS value FROM generate_series(1, 3) AS value ORDER BY value",
            &[],
        )
        .await?;
    assert_eq!(
        values,
        vec![
            Number { value: 1 },
            Number { value: 2 },
            Number { value: 3 }
        ]
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn pagination_is_stable_and_bounded() -> TestResult {
    let (_database, connection) = isolated_connection().await?;
    create_users(&connection).await?;
    for name in ["one", "two", "three", "four", "five"] {
        connection.insert(&user(name)).await?;
    }

    let page = connection.list::<User>(PageRequest::new(2, 2)?).await?;
    assert_eq!(
        page.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        vec!["three", "four"]
    );
    let final_page = connection.list::<User>(PageRequest::new(3, 2)?).await?;
    assert_eq!(final_page.len(), 1);
    assert_eq!(final_page[0].name, "five");
    assert!(
        connection
            .list::<User>(PageRequest::new(4, 2)?)
            .await?
            .is_empty()
    );
    assert!(PageRequest::new(0, 10).is_err());
    assert!(PageRequest::new(1, 0).is_err());
    assert!(PageRequest::new(1, 1001).is_err());
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn explicit_transactions_commit_and_rollback() -> TestResult {
    let (_database, mut connection) = isolated_connection().await?;
    create_users(&connection).await?;

    let transaction = connection.transaction().await?;
    let committed = transaction.insert(&user("committed")).await?;
    transaction.commit().await?;
    assert_eq!(
        connection.find_by_id::<User>(&committed.id).await?,
        Some(committed)
    );

    let transaction = connection.transaction().await?;
    let rolled_back = transaction.insert(&user("rolled back")).await?;
    assert!(
        transaction
            .find_by_id::<User>(&rolled_back.id)
            .await?
            .is_some()
    );
    transaction.rollback().await?;
    assert_eq!(connection.find_by_id::<User>(&rolled_back.id).await?, None);
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn dropping_a_transaction_rolls_back_before_the_next_query() -> TestResult {
    let (_database, mut connection) = isolated_connection().await?;
    create_users(&connection).await?;
    let dropped_id = {
        let transaction = connection.transaction().await?;
        transaction.insert(&user("dropped")).await?.id
    };
    assert_eq!(connection.find_by_id::<User>(&dropped_id).await?, None);

    // The same connection remains usable after the driver's queued rollback.
    let inserted = connection.insert(&user("after rollback")).await?;
    assert_eq!(
        connection.find_by_id::<User>(&inserted.id).await?,
        Some(inserted)
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn postgres_errors_preserve_sqlstate_and_failed_transactions_can_rollback() -> TestResult {
    let (_database, mut connection) = isolated_connection().await?;
    connection
        .execute(
            "CREATE TEMP TABLE pgorm_test_unique (id BIGINT PRIMARY KEY)",
            &[],
        )
        .await?;
    let transaction = connection.transaction().await?;
    transaction
        .execute("INSERT INTO pgorm_test_unique (id) VALUES ($1)", &[&1_i64])
        .await?;
    let error = transaction
        .execute("INSERT INTO pgorm_test_unique (id) VALUES ($1)", &[&1_i64])
        .await
        .unwrap_err();
    match error {
        Error::Database(error) => assert_eq!(
            error.code(),
            Some(&pgorm::tokio_postgres::error::SqlState::UNIQUE_VIOLATION)
        ),
        other => panic!("expected the upstream PostgreSQL error, received {other:?}"),
    }
    transaction.rollback().await?;
    let count = connection
        .fetch_one::<Number>("SELECT count(*) AS value FROM pgorm_test_unique", &[])
        .await?;
    assert_eq!(count.value, 0);
    Ok(())
}

#[derive(Debug, PartialEq, pgorm::Entity)]
#[pgorm(table = "pgorm_test_policies")]
struct PolicyRecord {
    #[pgorm(id, generated)]
    id: i64,
    name: String,
    #[pgorm(skip_update)]
    immutable: String,
    #[pgorm(skip_insert, skip_update)]
    server_value: String,
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn insert_and_update_honor_server_and_immutable_columns() -> TestResult {
    let (_database, connection) = isolated_connection().await?;
    connection
        .execute(
            "CREATE TEMP TABLE pgorm_test_policies (\
             id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \
             name TEXT NOT NULL, immutable TEXT NOT NULL, \
             server_value TEXT NOT NULL DEFAULT 'from database')",
            &[],
        )
        .await?;
    let inserted = connection
        .insert(&PolicyRecord {
            id: 0,
            name: "before".to_owned(),
            immutable: "initial".to_owned(),
            server_value: "must not be inserted".to_owned(),
        })
        .await?;
    assert_eq!(inserted.server_value, "from database");

    let updated = connection
        .update(&PolicyRecord {
            id: inserted.id,
            name: "after".to_owned(),
            immutable: "must not be updated".to_owned(),
            server_value: "must not be updated".to_owned(),
        })
        .await?
        .expect("inserted row exists");
    assert_eq!(updated.name, "after");
    assert_eq!(updated.immutable, "initial");
    assert_eq!(updated.server_value, "from database");
    Ok(())
}

#[derive(Debug, PartialEq, pgorm::Entity)]
#[pgorm(table = "order", schema = "pg_temp")]
struct QualifiedRecord {
    #[pgorm(id, generated)]
    id: i64,
    #[pgorm(column = "select")]
    value: String,
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn quoted_identifiers_and_explicit_schema_work_together() -> TestResult {
    let (_database, connection) = isolated_connection().await?;
    connection
        .execute(
            "CREATE TEMP TABLE \"order\" (id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \"select\" TEXT NOT NULL)",
            &[],
        )
        .await?;
    let record = connection
        .insert(&QualifiedRecord {
            id: 0,
            value: "reserved names".to_owned(),
        })
        .await?;
    assert_eq!(
        connection.find_by_id::<QualifiedRecord>(&record.id).await?,
        Some(record)
    );
    Ok(())
}

#[derive(Debug, PartialEq, pgorm::Entity)]
#[pgorm(table = "pgorm_test_id_only")]
struct IdOnly {
    #[pgorm(id, generated)]
    id: i64,
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn no_mutable_columns_returns_an_explicit_error() -> TestResult {
    let (_database, connection) = isolated_connection().await?;
    connection
        .execute(
            "CREATE TEMP TABLE pgorm_test_id_only (id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY)",
            &[],
        )
        .await?;
    let inserted = connection.insert(&IdOnly { id: 0 }).await?;
    assert!(inserted.id > 0);
    let error = connection.update(&inserted).await.unwrap_err();
    assert!(matches!(error, Error::NoUpdateColumns));
    Ok(())
}

#[derive(Debug, PartialEq, pgorm::Entity)]
#[pgorm(table = "pgorm_test_native_types")]
struct NativeTypes {
    #[pgorm(id)]
    id: uuid::Uuid,
    created_at: chrono::DateTime<chrono::Utc>,
    payload: serde_json::Value,
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn native_uuid_json_and_timestamp_codecs_round_trip() -> TestResult {
    let (_database, connection) = isolated_connection().await?;
    connection
        .execute(
            "CREATE TEMP TABLE pgorm_test_native_types (\
             id UUID PRIMARY KEY, created_at TIMESTAMPTZ NOT NULL, payload JSONB NOT NULL)",
            &[],
        )
        .await?;
    let expected = NativeTypes {
        id: uuid::Uuid::new_v4(),
        // PostgreSQL timestamps have microsecond precision.
        created_at: chrono::DateTime::from_timestamp(1_700_000_000, 123_456_000).unwrap(),
        payload: serde_json::json!({
            "nested": { "enabled": true },
            "array": [1, 2, null],
            "text": "雪"
        }),
    };
    assert_eq!(connection.insert(&expected).await?, expected);
    assert_eq!(
        connection.find_by_id::<NativeTypes>(&expected.id).await?,
        Some(expected)
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn database_executes_queries_and_accepts_an_existing_pool() -> TestResult {
    let (database, _connection) = isolated_connection().await?;
    let from_pool = Database::from_pool(database.pool().clone());
    let number = 7_i64;
    assert_eq!(
        from_pool
            .fetch_one::<Number>("SELECT $1::BIGINT AS value", &[&number])
            .await?,
        Number { value: number }
    );
    assert_eq!(
        database
            .fetch_optional::<Number>("SELECT 1::BIGINT AS value WHERE FALSE", &[])
            .await?,
        None
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn exhausted_pool_times_out_and_recovers_when_its_lease_is_returned() -> TestResult {
    use pgorm::deadpool_postgres::{PoolError, TimeoutType};

    let database = single_connection_database()?;
    let held_connection = database.acquire().await?;
    let result = tokio::time::timeout(Duration::from_secs(2), database.acquire())
        .await
        .expect("the configured 50ms checkout timeout must finish before the test deadline");
    let error = result
        .err()
        .expect("the only connection is still checked out");
    assert!(matches!(
        error,
        Error::Pool(PoolError::Timeout(TimeoutType::Wait))
    ));

    drop(held_connection);
    let recovered = database.acquire().await?;
    assert_eq!(
        recovered
            .fetch_one::<Number>("SELECT 7::BIGINT AS value", &[])
            .await?,
        Number { value: 7 }
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL; run with --include-ignored"]
async fn cancelling_a_transaction_rolls_back_before_its_pool_connection_is_reused() -> TestResult {
    let database = single_connection_database()?;
    let mut connection = database.acquire().await?;
    create_users(&connection).await?;
    let original_backend = connection
        .fetch_one::<Number>("SELECT pg_backend_pid()::BIGINT AS value", &[])
        .await?;

    let (inserted_sender, inserted_receiver) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let transaction = connection.transaction().await.expect("transaction starts");
        let inserted = transaction
            .insert(&user("cancelled transaction"))
            .await
            .expect("uncommitted insert succeeds");
        inserted_sender
            .send(inserted.id)
            .expect("the test is waiting for the insert");
        std::future::pending::<()>().await;
        drop(transaction);
    });

    // Cancel only after PostgreSQL has accepted the uncommitted write.
    let inserted_id = tokio::time::timeout(Duration::from_secs(5), inserted_receiver).await??;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());

    let reused_connection = database.acquire().await?;
    let reused_backend = reused_connection
        .fetch_one::<Number>("SELECT pg_backend_pid()::BIGINT AS value", &[])
        .await?;
    assert_eq!(reused_backend, original_backend);
    assert_eq!(
        reused_connection.find_by_id::<User>(&inserted_id).await?,
        None
    );
    assert_eq!(
        reused_connection
            .fetch_one::<Number>("SELECT count(*) AS value FROM pgorm_test_users", &[])
            .await?,
        Number { value: 0 }
    );
    let inserted = reused_connection
        .insert(&user("after cancellation"))
        .await?;
    assert_eq!(
        reused_connection.find_by_id::<User>(&inserted.id).await?,
        Some(inserted)
    );
    Ok(())
}
