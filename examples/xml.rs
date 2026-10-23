use pgorm::{BoundSql, Database, Executor, FromRow, Result, html_sql};

#[derive(Debug, FromRow)]
struct User {
    id: i64,
    name: String,
    email: Option<String>,
}

#[html_sql("examples/sql/users.xml")]
async fn find_users(
    db: &impl Executor,
    name: Option<&str>,
    ids: &[i64],
    by_name: bool,
) -> Result<Vec<User>> {
}

// The same XML can be rendered and inspected without a database.
#[html_sql("examples/sql/users.xml", id = "find_users")]
fn preview_users<'a>(name: Option<&'a str>, ids: &'a [i64], by_name: bool) -> Result<BoundSql<'a>> {
}

#[html_sql("examples/sql/users.xml")]
async fn insert_user(db: &impl Executor, name: &str, email: Option<&str>) -> Result<User> {}

#[html_sql("examples/sql/users.xml")]
async fn update_user(
    db: &impl Executor,
    id: i64,
    name: Option<&str>,
    email: Option<&str>,
) -> Result<u64> {
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let preview = preview_users(Some("Alice"), &[1, 2], false)?;
    println!("SQL template:\n{}", preview.sql());
    println!(
        "{} bound parameters (values omitted)",
        preview.parameter_count()
    );

    let db = Database::connect(&std::env::var("DATABASE_URL")?).await?;
    let mut connection = db.acquire().await?;
    connection.execute(
        "CREATE TEMP TABLE pgorm_xml_users (id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, name TEXT NOT NULL, email TEXT)",
        &[],
    ).await?;
    let tx = connection.transaction().await?;
    let alice = insert_user(&tx, "Alice", None).await?;
    assert_eq!(alice.name, "Alice");
    assert_eq!(alice.email, None);
    let affected = update_user(&tx, alice.id, None, Some("alice@example.invalid")).await?;
    assert_eq!(affected, 1);
    let users = find_users(&tx, Some("Alice"), &[alice.id], true).await?;
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].email.as_deref(), Some("alice@example.invalid"));
    tx.commit().await?;
    println!("XML insert, partial update, search and transaction completed.");
    Ok(())
}
