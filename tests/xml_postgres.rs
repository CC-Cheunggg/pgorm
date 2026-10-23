//! Each ignored test uses its own connection-local temporary table.
//! Run with DATABASE_URL pointing at a disposable PostgreSQL database and
//! `cargo test --all-features --test xml_postgres -- --include-ignored`.

#![cfg(all(feature = "derive", feature = "native-tls"))]

use pgorm::{Connection, Database, Error, Executor};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Debug, PartialEq, pgorm::FromRow)]
struct Person {
    id: i64,
    name: String,
    active: bool,
    bio: Option<String>,
}

#[derive(Debug, PartialEq, pgorm::FromRow)]
struct Number {
    value: i64,
}

async fn connection() -> Result<(Database, Connection), Box<dyn std::error::Error>> {
    let database = Database::connect(
        &std::env::var("DATABASE_URL")
            .expect("XML integration tests require a disposable PostgreSQL DATABASE_URL"),
    )
    .await?;
    let connection = database.acquire().await?;
    connection
        .execute(
            "CREATE TEMP TABLE pgorm_xml_people (id BIGINT PRIMARY KEY, name TEXT NOT NULL, active BOOLEAN NOT NULL, bio TEXT)",
            &[],
        )
        .await?;
    Ok((database, connection))
}

#[pgorm::html_sql("tests/xml/postgres.xml", id = "insert_person")]
async fn insert_person(
    db: &impl Executor,
    id: i64,
    name: &str,
    active: bool,
    bio: Option<&str>,
) -> pgorm::Result<Person> {
}

#[pgorm::html_sql("tests/xml/postgres.xml")]
async fn list_people(
    db: &impl Executor,
    active: Option<bool>,
    ids: Option<Vec<i64>>,
) -> pgorm::Result<Vec<Person>> {
}

#[pgorm::html_sql("tests/xml/postgres.xml", id = "get_person")]
async fn optional_person(db: &impl Executor, id: i64) -> pgorm::Result<Option<Person>> {}

#[pgorm::html_sql("tests/xml/postgres.xml", id = "get_person")]
async fn required_person(db: &impl Executor, id: i64) -> pgorm::Result<Person> {}

#[pgorm::html_sql("tests/xml/postgres.xml", id = "change_person")]
async fn change_person(
    db: &impl Executor,
    id: i64,
    name: Option<String>,
    active: Option<bool>,
) -> pgorm::Result<Option<Person>> {
}

#[pgorm::html_sql("tests/xml/postgres.xml")]
async fn remove_people(db: &impl Executor, ids: &[i64]) -> pgorm::Result<u64> {}

#[pgorm::html_sql(r#"<update>UPDATE pgorm_xml_people SET bio = #{bio} WHERE id = #{id}</update>"#)]
async fn set_bio(db: &impl Executor, id: i64, bio: &str) -> pgorm::Result<()> {}

#[pgorm::html_sql(r#"<select>SELECT #{value}::BIGINT AS value</select>"#)]
async fn generic_number<E>(db: &E, value: i64) -> pgorm::Result<Number>
where
    E: Executor,
{
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL"]
async fn xml_select_matches_direct_sql_for_conditional_and_foreach_bindings() -> TestResult {
    let (_database, connection) = connection().await?;
    for (id, name, active) in [(1, "Ada", true), (2, "Grace", false), (3, "雪", true)] {
        insert_person(&connection, id, name, active, None).await?;
    }

    let actual = list_people(&connection, Some(true), Some(vec![1, 3])).await?;
    let expected = connection
        .fetch_all::<Person>(
            "SELECT id, name, active, bio FROM pgorm_xml_people WHERE active = $1 AND id IN ($2, $3) ORDER BY id",
            &[&true, &1_i64, &3_i64],
        )
        .await?;
    assert_eq!(actual, expected);
    assert_eq!(list_people(&connection, None, None).await?.len(), 3);
    assert_eq!(list_people(&connection, Some(false), None).await?[0].id, 2);
    assert_eq!(
        list_people(&connection, None, Some(vec![3])).await?[0].name,
        "雪"
    );

    assert!(matches!(
        list_people(&connection, None, Some(vec![])).await,
        Err(Error::EmptyCollection(_))
    ));
    assert_eq!(generic_number(&connection, 42).await?, Number { value: 42 });
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL"]
async fn xml_dispatches_returning_rows_optional_rows_counts_and_unit() -> TestResult {
    let (_database, connection) = connection().await?;
    let inserted = insert_person(&connection, 7, "O'Reilly — 中文", true, None).await?;
    assert_eq!(inserted.name, "O'Reilly — 中文");
    assert_eq!(inserted.bio, None);
    assert_eq!(optional_person(&connection, 7).await?, Some(inserted));
    assert_eq!(optional_person(&connection, 999).await?, None);
    assert!(matches!(
        required_person(&connection, 999).await,
        Err(Error::NotFound)
    ));

    let changed = change_person(&connection, 7, Some("updated".to_owned()), Some(false))
        .await?
        .expect("updated row is returned");
    assert_eq!(changed.name, "updated");
    assert!(!changed.active);
    assert!(matches!(
        change_person(&connection, 7, None, None).await,
        Err(Error::EmptySet)
    ));
    assert_eq!(
        change_person(&connection, 999, None, Some(false)).await?,
        None
    );

    set_bio(&connection, 7, "text with  two spaces").await?;
    assert_eq!(
        required_person(&connection, 7).await?.bio.as_deref(),
        Some("text with  two spaces")
    );
    assert_eq!(remove_people(&connection, &[7, 999]).await?, 1);
    assert_eq!(remove_people(&connection, &[7, 999]).await?, 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL"]
async fn xml_queries_use_the_callers_transaction_for_commit_and_rollback() -> TestResult {
    let (_database, mut connection) = connection().await?;

    let transaction = connection.transaction().await?;
    insert_person(&transaction, 1, "committed", true, Some("first")).await?;
    assert_eq!(list_people(&transaction, None, None).await?.len(), 1);
    transaction.commit().await?;
    assert_eq!(required_person(&connection, 1).await?.name, "committed");

    let transaction = connection.transaction().await?;
    insert_person(&transaction, 2, "rolled back", true, None).await?;
    change_person(&transaction, 1, Some("also rolled back".to_owned()), None).await?;
    transaction.rollback().await?;
    assert_eq!(optional_person(&connection, 2).await?, None);
    assert_eq!(required_person(&connection, 1).await?.name, "committed");
    Ok(())
}

#[pgorm::html_sql(r#"<select>SELECT #{text}::TEXT::BIGINT AS value</select>"#)]
async fn cast_number(db: &impl Executor, text: &str) -> pgorm::Result<Number> {}

#[pgorm::html_sql(r#"<select>SELECT 'not an integer'::TEXT AS value</select>"#)]
async fn wrong_row_type(db: &impl Executor) -> pgorm::Result<Number> {}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL"]
async fn xml_execution_preserves_driver_sqlstate_and_native_row_type_errors() -> TestResult {
    let (_database, mut connection) = connection().await?;
    assert_eq!(
        cast_number(&connection, "123").await?,
        Number { value: 123 }
    );
    let direct_error = connection
        .fetch_one::<Number>("SELECT $1::TEXT::BIGINT AS value", &[&"not an integer"])
        .await
        .unwrap_err();
    let xml_error = cast_number(&connection, "not an integer")
        .await
        .unwrap_err();
    match (direct_error, xml_error) {
        (Error::Database(direct), Error::Database(xml)) => {
            assert_eq!(direct.code(), xml.code());
            assert_eq!(
                xml.code(),
                Some(&pgorm::tokio_postgres::error::SqlState::INVALID_TEXT_REPRESENTATION)
            );
        }
        other => panic!("both paths must preserve upstream database errors: {other:?}"),
    }
    assert!(matches!(
        wrong_row_type(&connection).await,
        Err(Error::Database(_))
    ));

    let transaction = connection.transaction().await?;
    insert_person(&transaction, 1, "first", true, None).await?;
    let error = insert_person(&transaction, 1, "duplicate", false, None)
        .await
        .unwrap_err();
    match error {
        Error::Database(error) => assert_eq!(
            error.code(),
            Some(&pgorm::tokio_postgres::error::SqlState::UNIQUE_VIOLATION)
        ),
        other => panic!("expected PostgreSQL unique violation: {other:?}"),
    }
    transaction.rollback().await?;
    assert_eq!(optional_person(&connection, 1).await?, None);
    Ok(())
}

#[pgorm::html_sql(
    r#"<select>SELECT 10-<if test="negative">-</if>#{value}::BIGINT AS value</select>"#
)]
async fn dynamic_arithmetic(
    db: &impl Executor,
    negative: bool,
    value: i64,
) -> pgorm::Result<Number> {
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and DATABASE_URL"]
async fn xml_dynamic_node_boundaries_preserve_postgres_operator_semantics() -> TestResult {
    let (_database, connection) = connection().await?;
    assert_eq!(
        dynamic_arithmetic(&connection, true, 2).await?,
        Number { value: 12 }
    );
    assert_eq!(
        dynamic_arithmetic(&connection, false, 2).await?,
        Number { value: 8 }
    );
    Ok(())
}
