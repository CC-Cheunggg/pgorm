# pgorm 第一阶段架构与选型记录

记录日期：2026-10-23。状态：0.1 开发初版，API 仍可随实现与验证调整。

## 目标与来源

pgorm 是一个仅支持 PostgreSQL、以 SQL 为中心的轻量 ORM crate。我们参考 [rbatis](https://github.com/rbatis/rbatis) 方便编写数据库访问代码的思路，独立设计和实现本项目，不复制 rbatis 的代码，不引入其驱动、连接池或其他底层组件。PostgreSQL 驱动选用 `tokio-postgres`。

本项目以独立公开仓库 [CC-Cheunggg/pgorm](https://github.com/CC-Cheunggg/pgorm) 开发，未复用 rbatis 的 Git 历史或源码。尚未发布到 crates.io，API 兼容性承诺尚未确定。

第一阶段以 `html_sql` / XML 动态 SQL 为核心。协议、认证、连接、类型编解码、连接池调度和日志基础设施交给成熟依赖；本项目负责 XML 模板编译、参数收集、实体映射和必要的 CRUD SQL 生成。

## 依赖选择

| 职责 | 选择 | 理由与边界 |
| --- | --- | --- |
| PostgreSQL 驱动 | `tokio-postgres 0.7.18` | rust-postgres 项目的原生异步 PostgreSQL 客户端。复用其参数绑定、`ToSql` / `FromSql`、事务和数据库错误类型，不自行实现协议与值编码。 |
| 连接池 | `deadpool-postgres 0.14.2` | 直接集成 `tokio-postgres`，提供连接管理、超时配置、池状态和预编译语句缓存。无需维护自有连接池。 |
| 结构化日志 | `tracing 0.1.44` | 使用 Tokio 生态的事件和 span，方便接入应用已有的过滤、输出与追踪设施。库只产生诊断信息。 |
| TLS | `native-tls` + `postgres-native-tls` | 使用成熟 TLS 实现和 rust-postgres 的适配器，不编写证书校验或加密逻辑。 |
| 宏与 XML | `syn` + `quote` + `roxmltree` | XML 解析交给 roxmltree，Rust 表达式交给 syn；生成条件、循环、参数绑定和映射代码，不创建运行时脚本解释器或动态值系统。 |

上述版本是本次选型记录，实际解析版本以 `Cargo.lock` 为准。声明的最低 Rust 版本为 1.85、edition 2024，与所选驱动对齐；本地验证使用 Rust 1.95。[驱动仓库](https://github.com/rust-postgres/rust-postgres)、[驱动 Cargo.toml](https://github.com/rust-postgres/rust-postgres/blob/master/tokio-postgres/Cargo.toml)、[deadpool-postgres 文档](https://docs.rs/deadpool-postgres/latest/deadpool_postgres/)、[tracing 文档](https://docs.rs/tracing/latest/tracing/)

`tokio-postgres` 是成熟的 Rust 社区驱动，**不是 PostgreSQL 官方维护的 Rust 驱动**。PostgreSQL 官方文档中的客户端库是 C 接口 `libpq`；官方 wiki 收录 rust-postgres，也不能据此称它为官方实现。本项目选择社区驱动，是为了采用现成的 Rust 异步生态。[libpq 官方文档](https://www.postgresql.org/docs/current/libpq.html)、[PostgreSQL 驱动列表](https://wiki.postgresql.org/wiki/List_of_drivers)

`deadpool` 与 `bb8` 都是可行的社区连接池。本次读取项目仓库时，deadpool 约 1.3k stars，bb8 约 955 stars；这只是关注度的粗略信号，不代表完整的使用量排名。最终选择 deadpool-postgres，主要因为它与驱动、事务及语句缓存的现成集成符合本项目范围。[deadpool 仓库](https://github.com/deadpool-rs/deadpool)、[bb8 仓库](https://github.com/djc/bb8)

## 分层与初版范围

调用路径为：应用 → pgorm 的 SQL / 实体操作 → deadpool-postgres 连接或事务 → tokio-postgres → PostgreSQL。

`pgorm` 提供运行时 API；`pgorm-macros` 提供可选的 XML 属性宏和派生宏，默认由 `derive` feature 启用。XML 在编译时解析，模板条件与参数引用作为 Rust 代码接受类型检查；运行时只构建 SQL 字符串并收集原生 `ToSql` 参数。实体映射与 SQL 生成不依赖具体池调度实现，连接与事务共用 `Executor`。保留原始 SQL 路径，使应用能直接使用 PostgreSQL 能力。

初版目标包括：

- **XML SQL**：外部文件、内联 XML、动态条件与循环、可复用片段、空集合和空更新检查、独立预览；具体规则见 [XML 指南](xml.md)。外部文件通过生成的 `include_str!` 进入 Cargo 依赖追踪，跨文件片段同样跟踪。数据库 SQL 语法、表结构和类型匹配仍在执行时由 PostgreSQL 检查。
- **SQL 参数绑定**：沿用 PostgreSQL `$1`、`$2` 等占位符与驱动的 `ToSql`，值通过绑定传入，不插值拼接。调用者编写的原始 SQL 仍由调用者负责；绑定参数不能代替表名、列名等 SQL 标识符。
- **`FromRow` 与 `Entity`**：将查询行映射到 Rust 结构体，并声明实体的表、列及单一主键。复用驱动的类型转换能力；不引入通用动态值系统。
- **基础 CRUD**：基于实体元数据生成简单的增、查、改、删语句。操作语义和字段要求以公开 API、示例与测试为准，不承诺完整兼容 rbatis。
- **分页**：提供有限的 `LIMIT` / `OFFSET` 分页，不自动执行 `COUNT(*)`。需要总数时由应用显式查询；内置 list 按主键升序，原始 SQL 分页由应用指定确定的排序。
- **事务**：复用驱动事务能力，支持显式提交和回滚，遵循驱动的生命周期与丢弃行为，不自行编写事务状态机。commit 开始后取消等待不保证回滚，提交结果不确定时由应用处理。
- **连接池、TLS 与诊断**：提供易于配置的默认入口，并保留接入应用已有连接池的入口。

## 连接与 TLS 默认策略

| 配置 | 初版默认值 |
| --- | --- |
| 最大连接数 | 16 |
| 等待池中连接超时 | 5 秒 |
| 新建连接超时 | 10 秒 |
| 回收检查超时 | 5 秒 |
| 回收方式 | `RecyclingMethod::Verified` |
| 默认连接入口的 TLS | 启用 native TLS，要求 TLS（`Require`） |

`Verified` 会检查连接是否关闭，并执行健康检查查询，比仅检查 `is_closed()` 的 `Fast` 多一次往返。它不等于清理会话状态；业务若修改 session 配置、使用临时对象或依赖其他会话状态，应自行管理会话，或通过自建池选择适合的清理策略。[回收方式说明](https://docs.rs/deadpool-postgres/latest/deadpool_postgres/enum.RecyclingMethod.html)

deadpool 的池创建本身不会验证数据库可达性，因此默认的 `connect` 流程会实际取连接并验证连通性，再返回成功。构建配置成功和数据库连接成功是不同状态。[连接池启动行为](https://docs.rs/deadpool-postgres/latest/deadpool_postgres/#faq)

默认连接入口使用 native TLS 及其证书校验，不通过“不安全接受证书”等开关绕过校验。明确需要本地或受控环境明文连接时，调用者可以显式选择 `Disable`。应用也可以使用 `from_pool` 注入自行构建的 deadpool-postgres 池；该路径的 TLS、连接验证、池大小与超时由调用者配置，不强行套用默认入口的策略。[tokio-postgres TLS 接口](https://docs.rs/tokio-postgres/latest/tokio_postgres/#ssltls-support)

池等待、创建和回收超时只约束相应池操作，**不构成 SQL 执行超时**。长查询控制应使用 PostgreSQL 的 `statement_timeout` 等适合应用的机制，避免给使用者造成错误的超时保证。

## 日志约定

库使用 `tracing` 记录操作类别、耗时、结果行数或影响行数、成功状态和必要的错误类别 / SQLSTATE。pgorm 自身不记录 SQL 文本、参数值、DSN、密码或错误详细文本。上游 tokio-postgres 的 debug/trace 日志可能包含 SQL 和参数，应用必须独立配置过滤；示例显式关闭 tokio_postgres 日志目标。

库不安装全局 subscriber，不自行创建文件日志输出或后台写入线程。应用通过 `tracing-subscriber` 等组件决定是否采集、如何过滤和向何处输出；示例可以演示配置。异步 span 使用适合 future 的方式关联，不让 `Span::enter()` 的 guard 跨越 `await`。[tracing 库使用约定](https://docs.rs/tracing/latest/tracing/#in-libraries)

## 明确不包含的能力

第一阶段不支持多数据库适配、`py_sql`、迁移管理、关联关系加载、复合主键和自动重试。也不承诺 rbatis API 或模板的兼容性。

自动重试尤其需要业务语义：连接失败、事务冲突和提交结果不确定并不能统一当作可重试操作。初版向调用者返回错误，由应用决定恢复方式。

后续按实际使用需求扩展。围绕 XML 编写 SQL 的体验持续完善诊断、编辑器支持和测试工具；其他 PostgreSQL 能力在真实业务需求驱动下加入。
