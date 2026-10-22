# 第一阶段验证记录

日期：2026-10-23（Asia/Shanghai）。本地环境：macOS Apple Silicon，Rust / Cargo 1.95.0，PostgreSQL 18.6。

## 已执行并通过

| 检查 | 结果 |
| --- | --- |
| `cargo check --workspace --all-features --all-targets` | 通过 |
| `cargo check --workspace --no-default-features --locked` | 通过，运行库可以关闭派生宏与 native TLS |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 通过，无警告 |
| `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked` | 通过 |
| `cargo test --workspace --all-features --locked -- --include-ignored` | 28 项测试通过，0 项跳过 |
| 宏独立 fixture | 1 项运行测试和 4 项 compile-fail doctest 通过 |
| `RUST_LOG=debug cargo run --example basic --locked` | 实际连接 PostgreSQL，CRUD / 事务 / 分页正常；输出 pgorm 元数据日志 |
| `cargo tree --all-features` 依赖检查 | 不含 rbatis / rbdc / rbs / fast_pool / fast_log |

合计 33 项测试：2 项运行库单元测试、11 项派生宏单元测试、15 项 PostgreSQL 集成测试、5 项宏编译/运行 fixture。

## 数据库场景

使用本次新建的临时 PostgreSQL 实例和会话临时表，不使用已有业务数据库。集成测试覆盖：

- 生成主键、数据库默认值、插入、查找、更新与删除。
- NULL、Unicode、引号参数及 UUID / JSON / timestamp 的原生类型往返。
- 查询零行/多行语义、缺列/类型映射错误、保留 SQLSTATE。
- 主键稳定排序分页、页码边界、无可更新列。
- 列名映射、schema、保留字标识符、skip_insert / skip_update。
- 事务提交、显式回滚、drop 回滚、失败事务恢复。
- 取消持有事务的任务后，重新使用同一池连接且没有未提交数据残留。
- 连接池耗尽时明确超时，释放 lease 后恢复。
- tracing 成功/失败事件含耗时、行数和 SQLSTATE，不含 SQL 文本、绑定值或唯一约束错误中的业务数据。

## 当前验证边界

- 数据库集成测试连接本机 PostgreSQL 18.6，明确使用 `sslmode=disable`。默认 TLS、证书链、私有 CA、直接 TLS 协商和跨平台 TLS 组合尚未做端到端测试；实现复用上游 native TLS 适配器，直接协商配置已通过编译及独立审查。
- 最低 Rust 1.85 是声明值；本机使用 1.95，未执行 1.85 工具链测试。
- 已提供 PostgreSQL 16 / 18 的 [GitHub Actions 模板](../ci/postgres.yml)，当前尚未启用自动工作流。本记录只列出本地实测结果，不把模板当作矩阵验证结果。
- 本次测试是功能验证，不是性能、压力或长时间稳定性评估。
- SQL 执行超时、会话清理策略和提交结果不确定时的业务恢复由应用负责。
