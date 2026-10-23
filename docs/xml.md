# XML SQL 指南

pgorm 参考 [rbatis 的 html_sql / XML SQL 设计](https://github.com/rbatis/rbatis)，独立实现面向 PostgreSQL 的模板编译器。XML 是核心查询入口，CRUD 是方便简单单表操作的补充。XML 解析使用 `roxmltree`，Rust 表达式解析使用 `syn`，数据库值沿用 `tokio-postgres::types::ToSql`。

## 声明与执行

```rust,ignore
use pgorm::{html_sql, Executor, FromRow, Result};

#[derive(FromRow)]
struct User { id: i64, name: String }

#[html_sql("sql/users.xml")]
async fn find_users(db: &impl Executor, name: Option<&str>) -> Result<Vec<User>> {}
```

```xml
<mapper>
  <select id="find_users">
    SELECT id, name FROM users
    <where><if test="name.is_some()">AND name = #{name}</if></where>
    ORDER BY id
  </select>
</mapper>
```

- 文件路径相对调用者 crate 的 `Cargo.toml` 所在目录。`id` 默认是函数名，可用 `#[html_sql("sql/users.xml", id = "find_users")]` 显式选择。
- 支持 `<select>`、`<insert>`、`<update>`、`<delete>`。内联形式如 `#[html_sql(r#"<select>SELECT #{id} AS id</select>"#)]` 可以省略根 `<mapper>` 与 `id`。
- 异步函数第一个参数是执行器，例如 `&Database`、`&Connection`、`&Transaction` 或 `&impl Executor`。传事务即可让同一模板加入现有事务。
- 函数体必须为空或仅有 `todo!()`；宏生成整个函数体。自定义业务逻辑放在调用层。
- 默认 `derive` feature 同时导出 `html_sql`、`FromRow` 和 `Entity` 宏。

| 返回值 | 执行行为 |
| --- | --- |
| `Result<Vec<T>>` | 查询并映射所有行，T 实现 `FromRow` |
| `Result<Option<T>>` | 查询零行或一行，多于一行报错 |
| `Result<T>` | 查询一行，零行或多行报错 |
| `Result<u64>` | 执行变更并返回影响行数 |
| `Result<()>` | 执行变更，丢弃影响行数 |
| 同步 `Result<BoundSql<'a>>` | 只渲染 SQL 与参数，不连接数据库 |

`INSERT / UPDATE / DELETE ... RETURNING` 可返回 `T`、`Option<T>` 或 `Vec<T>`。`<select>` 必须返回行数据；单列结果也用带一个字段的 `FromRow` 结构体。返回类型分派按上述类型名称识别，不解析自定义别名。

## 参数与表达式

`#{expr}` 将 Rust 表达式克隆后作为原生参数保存，按实际渲染顺序生成 `$1`、`$2`。支持 `String`、借用字符串、数字、`Option<T>` 以及启用对应驱动 feature 的 UUID / JSON / chrono 等类型。表达式结果需支持 `Clone`，存储的值需实现 `ToSql + Sync + Send`。重复引用同一变量会产生独立参数；不会把值转成 JSON 再编码。

```xml
<select id="search">
  <bind name="pattern" value="format!(&quot;%{}%&quot;, name.trim())"/>
  SELECT id, name FROM users WHERE name ILIKE #{pattern}
</select>
```

`test`、`collection`、`value` 和占位符内都是 **Rust 表达式**。变量名称、字段、方法和类型由 Rust 编译器检查，不是 rbatis 的动态值表达式语法。未知变量会成为编译错误；模板不是可运行不受信任脚本的沙箱。

XML 属性内的 `&&` 写成 `&amp;&amp;`，`<` 写成 `&lt;`，双引号写成 `&quot;`。SQL 正文可以用 CDATA：

```xml
<select id="older">SELECT id FROM users <![CDATA[WHERE age < #{limit}]]></select>
```

SQL 字符串、引用标识符、美元引号字符串和注释里的 `#{...}` / `${...}` 保持原样。模板中的动态标签不能置于 SQL 字符串或注释内部。动态节点前后是 SQL token 边界，必要时自动添加分隔空格，避免两个独立的 `-` 拼成 `--` 注释；空分支和仅计算变量的 `bind` 也保留此边界。不要跨动态节点拼接单个标识符、运算符或字符串。

静态 `include` 会先展开，再与相邻正文一同进行词法分析。XML 正文保留 SQL 字面量的空格与换行，不会全局压缩空白；XML 的标准换行及实体解码规则仍适用。不要在 XML 中手写 `$1` 等参数编号，以免与动态分支编号冲突；直接使用 `Executor` 写原生 SQL 时仍使用 `$n`。

## 条件、修剪与更新

| 标签 | 行为 |
| --- | --- |
| `<if test="condition">` | 条件为 true 时渲染子节点 |
| `<choose><when test="...">…</when><otherwise>…</otherwise></choose>` | 只渲染第一个命中的 when；否则选择可选 otherwise |
| `<where>` | 子节点非空时添加 WHERE，移除开头的 AND / OR |
| `<set>` | 子节点非空时添加 SET，移除结尾逗号；无可更新项返回 `Error::EmptySet` |
| `<trim prefix="..." suffix="..." prefixOverrides="AND\|OR" suffixOverrides=",">` | 非空时包裹前后缀，并按 `\|` 分隔列表修剪边缘 token |
| `<bind name="pattern" value="..."/>` | 在当前 Rust 作用域定义局部变量供后续节点使用 |

修剪忽略边缘空白和注释，按 token 边界处理关键词：`ORDERED` 不会被误删为 `DERED`。注释与字符串内容保持原样。`<choose>` 的分支局部变量只在该分支内有效。

```xml
<update id="update_user">
  UPDATE users
  <set>
    <if test="name.is_some()">name = #{name},</if>
    <if test="email.is_some()">email = #{email},</if>
  </set>
  WHERE id = #{id}
</update>
```

这里 `None` 表示不更新该字段。需要区分“不修改”与“写入 NULL”，可以使用 `Option<Option<T>>` 并绑定内部的 `Option<T>`。`<where>` 内容为空会省略整个 WHERE；它不会推断更新/删除是否允许影响整张表，应用必须明确控制必要条件。

## foreach 与空集合

```xml
<foreach collection="ids" item="id" index="position"
         open="(" close=")" separator=",">
  #{id}
</foreach>
```

`collection` 需要提供 `.iter()`，例如切片、数组或 `Vec<T>`。循环借用集合，`item` 是迭代项，`index` 是从 0 开始的原始迭代序号；过滤掉某项不会重排序号。可嵌套循环，同一个集合可以在模板后文继续使用。必要时先将迭代器收集为 Vec。

只有实际渲染出 SQL 的迭代才添加分隔符。空集合或所有迭代都被条件过滤时，默认返回 `Error::EmptyCollection`，避免默默生成 `IN ()` 或丢失过滤条件。可显式指定 `empty="skip"` 省略整个循环，包含 open / close；这种选择应符合业务语义。也可在外层 `<if>` 决定空集合的处理，例如选择静态 `FALSE` 条件。

`open`、`close`、`separator` 以及 trim 的前后缀是开发者提供的静态 SQL 结构，不展开占位符；参数、动态标识符应放在正文中。这些属性通常填写括号、逗号、关键词，不能用来包裹未闭合的 SQL 字符串或注释，也不要在其中手写 `$n`。

## 片段复用

```xml
<mapper>
  <sql id="columns">id, name</sql>
  <select id="users">
    SELECT <include refid="columns"/> FROM users
    <include file="filters.xml" refid="active_filter"/>
  </select>
</mapper>
```

`refid` 引用 `<sql id="...">`；`file` 相对当前 XML 文件目录，内联 XML 的外部 include 相对 crate 根目录。片段可以继续引用其他片段。宏检查重复 id、缺失片段和 include 循环；所有读到的 XML 文件都会加入编译依赖，修改主文件或片段文件都能触发重编译。`mapper` 的可选 `namespace` 仅作说明，不参与查找；跨文件引用必须明确写 `file`。

## 动态表名与列名

`${expr}` 只接受 `SqlIdentifier`，不接受普通 `String` 或 SQL 片段：

```rust,ignore
let table = pgorm::SqlIdentifier::qualified(["public", "users"])?;
let column = pgorm::SqlIdentifier::new("display name")?;
```

```xml
<select id="find">SELECT ${column} FROM ${table} WHERE id = #{id}</select>
```

输出的标识符分别为 `"public"."users"`、`"display name"`。`new("public.users")` 会引用成单个名字 `"public.users"`；schema 与表应使用 `qualified`。内部双引号会转义，空值和 NUL 被拒绝。该类型负责正确引用，业务仍应限制允许访问的表、列。ASC / DESC、运算符或任意 SQL 片段应通过静态 `<choose>` 分支表达。

## 预览、编辑器与诊断

```rust,ignore
#[pgorm::html_sql("sql/users.xml", id = "find_users")]
fn preview<'a>(name: Option<&'a str>) -> pgorm::Result<pgorm::BoundSql<'a>> {}

let query = preview(Some("Alice"))?;
assert_eq!(query.parameter_count(), 1);
assert!(query.sql().contains("name = $1"));
// 如需手动执行，可将 query.sql() 和 &query.params() 传给 Executor。
```

`BoundSql` 保存渲染后的 SQL 及参数，不会自动记录参数。借用值的生命周期保留在返回类型中；全部拥有值的模板可以返回 `BoundSql<'static>`。参数数量超过 PostgreSQL 协议上限 65,535 时返回错误。

模板解析、结构、include 和表达式语法错误带源文件及行列信息；未知变量、类型不匹配等由生成代码的 Rust 编译诊断报告。拒绝 DTD，片段引用只读取明确指定的本地文件。XML 实体解码后的复杂多行文本可能使表达式内的精细列偏移与原始文本不同。数据库表结构、SQL 类型和查询计划仍需在实际 PostgreSQL 中验证。

仓库提供 [pgorm-mapper.xsd](pgorm-mapper.xsd)，可复制到应用中并为 mapper 指定相对路径：

```xml
<mapper xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
        xsi:noNamespaceSchemaLocation="../pgorm-mapper.xsd">
  <select id="one">SELECT 1 AS value</select>
</mapper>
```

支持 XSD 的 XML 编辑器可据此补全标签、属性并检查结构。XSD 不检查 Rust 表达式或数据库 SQL 语义；宏也不会访问网络下载 schema。完整用法见 [运行示例](../examples/xml.rs) 和 [对应 XML](../examples/sql/users.xml)。

## 当前边界

这是独立设计，未承诺与 rbatis XML / API 兼容。当前不提供运行时热加载、任意字符串 SQL 插值、`py_sql`、模板 resultMap、SQL schema 的编译期验证或自研动态值语言。XML 编译期处理消除了运行时 XML 解析，但 SQL 字符串构建、参数克隆与装箱仍有开销；尚未做与其他 ORM 的性能对比。
