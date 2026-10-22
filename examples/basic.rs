use pgorm::{Crud, Database, Entity, Executor, PageRequest};

#[derive(Debug, Entity)]
#[pgorm(table = "pgorm_example_users", schema = "pg_temp")]
struct User {
    #[pgorm(id, generated)]
    id: i64,
    name: String,
    email: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Driver debug/trace logs may contain SQL and parameter values. This example
    // enables our metadata-only events and explicitly disables driver logging.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,pgorm=debug"))
        .add_directive("tokio_postgres=off".parse()?);
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let db = Database::connect(&std::env::var("DATABASE_URL")?).await?;
    let mut conn = db.acquire().await?;
    // Session-local example: no persistent table or data is created.
    conn.execute("CREATE TEMP TABLE pgorm_example_users (id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, name TEXT NOT NULL, email TEXT)", &[]).await?;

    let tx = conn.transaction().await?;
    let mut user = tx
        .insert(&User {
            id: 0,
            name: "Alice".into(),
            email: None,
        })
        .await?;
    user.email = Some("alice@example.invalid".into());
    let saved = tx.update(&user).await?.expect("inserted user exists");
    let users: Vec<User> = tx
        .fetch_all(
            "SELECT id, name, email FROM pgorm_example_users WHERE name = $1",
            &[&"Alice"],
        )
        .await?;
    println!("saved: {saved:?}; matched: {}", users.len());
    tx.commit().await?;

    let page = conn.list::<User>(PageRequest::new(1, 20)?).await?;
    println!("page contains {} user(s)", page.len());
    conn.delete_by_id::<User>(&user.id).await?;
    Ok(())
}
