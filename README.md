# pgorm

仅支持 PostgreSQL、以 SQL 为中心的 Rust ORM 初版。协议、认证、类型编码、连接池和日志基础设施直接复用社区组件。

**参考项目：[rbatis/rbatis](https://github.com/rbatis/rbatis)。** 本项目参考 rbatis 的 SQL 优先、实体映射和便捷 CRUD 设计思路，感谢原项目提供的启发。pgorm 的代码独立实现，未复制 rbatis 源码，也不依赖 rbatis / rbdc / rbs / fast_pool / fast_log。

项目仓库：[CC-Cheunggg/pgorm](https://github.com/CC-Cheunggg/pgorm)。这是独立实现的公开项目，目前处于 0.1 开发阶段，暂不承诺 API 稳定性或 rbatis 兼容性。尚未发布到 crates.io，两个 package 均保留 `publish = false`。

## 已实现

- `tokio-postgres` 驱动、原生 `$1` 参数绑定及 `ToSql` / `FromSql`。
- `deadpool-postgres` 连接池、健康检查、超时和预编译语句缓存。
- `FromRow` / `Entity` 派生宏，按列名读取，不经过 JSON 或动态值中转。
- INSERT / UPDATE RETURNING、单主键 CRUD、稳定主键排序的分页。
- 借用式事务，显式 commit / rollback，驱动管理 drop 回滚。
- TLS、结构化 tracing 日志，以及原生连接池/驱动接口。

第一阶段不包含多数据库、py_sql / html_sql、迁移、关联、复合主键、泛型实体、批量写入优化和自动重试。复杂查询与部分更新直接写 SQL。[架构与选型](docs/architecture.md)说明了边界和后续方向。

## 快速开始

Rust 2024 edition，声明的最低版本为 1.85；本地验证使用 1.95。可以从 GitHub 引入开发版本：

```toml
[dependencies]
pgorm = { git = "https://github.com/CC-Cheunggg/pgorm", branch = "main", features = ["with-uuid", "with-chrono", "with-json"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

本地开发也可改用 `path = "../pgorm"`；需要固定版本时使用 Git commit 的 `rev`，避免依赖持续变化的 main 分支。

```rust,no_run
use pgorm::{Crud, Database, Entity, Executor, PageRequest};

#[derive(Debug, Entity)]
#[pgorm(table = "users", schema = "public")]
struct User {
    #[pgorm(id, generated)]
    id: i64,
    name: String,
    email: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::connect(&std::env::var("DATABASE_URL")?).await?;
    // 表由应用迁移创建：id BIGINT identity, name TEXT, email TEXT。
    let inserted = db.insert(&User {
        id: 0, // generated 主键不会发送到 INSERT。
        name: "Alice".into(),
        email: None,
    }).await?;
    let _found = db.find_by_id::<User>(&inserted.id).await?;
    let _matched: Vec<User> = db.fetch_all(
        "SELECT id, name, email FROM public.users WHERE name = $1",
        &[&"Alice"],
    ).await?;
    let _page = db.list::<User>(PageRequest::new(1, 20)?).await?;
    Ok(())
}
```

可直接运行 [examples/basic.rs](examples/basic.rs)，它只使用会话临时表：

```sh
DATABASE_URL='postgresql://user:password@localhost/db?sslmode=disable' \
  cargo run --example basic
```

`sslmode=disable` 只用于明确允许明文的连接。

## 实体和查询语义

| 声明/方法 | 行为 |
| --- | --- |
| `#[derive(Entity)]` | 同时实现 `Entity` 和 `FromRow`，无需重复 derive |
| `#[derive(FromRow)]` | 用于查询投影，按字段名取列 |
| `#[pgorm(column = "db_name")]` | 字段映射到指定数据库列 |
| `#[pgorm(table = "users", schema = "public")]` | 表名和 schema 分别引用；不会把表名里的点解析成 schema |
| `#[pgorm(id)]` | 必须且只能有一个主键，更新时不修改主键 |
| `#[pgorm(id, generated)]` | 插入省略主键，RETURNING 读取数据库生成值 |
| `#[pgorm(skip_insert)]` | 插入省略该列，返回值仍读取它 |
| `#[pgorm(skip_update)]` | 更新时保留数据库现有值 |
| `insert(&entity)` | 返回完整实体，包含数据库默认值 |
| `update(&entity)` | 更新所有可更新字段；`None` 写入 NULL，返回 `Option<Entity>` |
| `find_by_id` / `fetch_optional` | 零行返回 `None`，多于一行返回错误 |
| `fetch_one` | 零行返回 `Error::NotFound`，多于一行返回错误 |
| `delete_by_id` / `execute` | 返回影响行数 |
| `list(PageRequest)` | 页码从 1 开始，每页 1..=1000，主键升序；不隐式查询 COUNT |

表、schema、列名均按 PostgreSQL 规则引用和转义；空或含 NUL 的标识符被拒绝。数据使用参数绑定。原始 SQL 的结构由调用者负责，不能用参数代替标识符。

## 事务

```rust,ignore
let mut conn = db.acquire().await?;
let tx = conn.transaction().await?;
let saved = tx.insert(&new_user).await?;
tx.commit().await?;
```

事务内的操作都使用 `tx`；使用 `db` 会另取连接。未提交事务 drop 后由驱动回滚。**一旦开始 commit，取消等待或连接中断不保证没有提交**；库不自动重试写入。

## 连接池与 TLS

`Database::connect` 默认最大 16 个连接，等待 5 秒、创建 10 秒、回收 5 秒，使用 `RecyclingMethod::Verified`，并实际连接探活后返回。

默认 TLS 为 `Require`：省略 `sslmode` 或传 `prefer` 都不回退明文，明确传 `disable` 才关闭 TLS。采用系统证书根和主机名验证，直接协商模式会设置 PostgreSQL ALPN。DSN 语法沿用 tokio-postgres，不额外模拟 libpq 配置。

高级配置直接构建 `deadpool_postgres::Pool`，再传入 `Database::from_pool(pool)`；此构造不主动连接、不覆盖调用者配置，可通过 `acquire().await` 验证。私有 CA 或其他 TLS 实现同样通过上游 pool/connector 注入。

`Database::pool()` 暴露原生池；`Connection::raw()` / `raw_mut()`、`Transaction::raw()` 提供 COPY、LISTEN、事务配置等原生能力。直接调用这些接口不会经过 pgorm 的查询日志。

`Verified` 检查健康状态，不清理会话配置或临时表。池超时不是 SQL 超时；应用可配置 PostgreSQL `statement_timeout`。当前普通查询收集全部结果，大结果集可使用原生驱动流接口。

## 日志

pgorm 自身只记录操作、耗时、行数及 SQLSTATE，不记录 SQL 文本、参数、DSN 或数据库错误详情。库不安装全局 subscriber，也不创建后台日志线程。

**上游驱动 debug/trace 日志可能包含 SQL 和参数。** 示例会显式关闭 `tokio_postgres` 目标，即使设置全局 `RUST_LOG=debug` 也保留该过滤。应用应分别配置目标，例如：

```text
info,pgorm=debug,tokio_postgres=off
```

## 验证

```sh
cargo fmt --all -- --check
cargo check --workspace --no-default-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked

# 显式执行数据库测试必须提供 DATABASE_URL；不会静默跳过。
DATABASE_URL='postgresql://user:password@localhost/test_db?sslmode=disable' \
  cargo test --workspace --all-features --locked -- --include-ignored

# 依赖重命名及编译失败 fixture
cargo test --manifest-path pgorm-macros/tests/fixtures/renamed/Cargo.toml \
  --target-dir target --locked
```

所有数据库测试使用会话临时表。仓库保留 PostgreSQL 16 / 18 的 [CI 模板](ci/postgres.yml)，自动工作流尚未启用，启用方式见 [CI 说明](ci/README.md)。实际本地结果见 [验证记录](docs/validation.md)。TLS 证书与跨平台组合测试尚未纳入 CI。

## 依赖与许可

驱动采用 rust-postgres 社区维护的 [tokio-postgres](https://github.com/rust-postgres/rust-postgres)，不是 PostgreSQL 官方 Rust 驱动；连接池采用 [deadpool-postgres](https://docs.rs/deadpool-postgres/)，日志采用 [tracing](https://docs.rs/tracing/)。选择依据是维护活跃和现成集成，不宣称社区热度第一。

本项目独立实现，MIT OR Apache-2.0 许可；依赖保留各自许可。
