//! Invalid templates fail during compilation, before any database connection.
//!
//! Unknown elements are rejected, including misspelled dynamic elements.
//!
//! ```compile_fail
//! #[pgorm_macros::html_sql(r#"<select><iff test="true">SELECT 1</iff></select>"#)]
//! fn unknown_tag() -> r#type::Result<r#type::BoundSql<'static>> {}
//! ```
//!
//! Missing fragments are reported while compiling the caller.
//!
//! ```compile_fail
//! #[pgorm_macros::html_sql(r#"<select><include refid="missing"/></select>"#)]
//! fn missing_fragment() -> r#type::Result<r#type::BoundSql<'static>> {}
//! ```
//!
//! Dynamic identifiers require SqlIdentifier, so raw string substitution fails.
//!
//! ```compile_fail
//! #[pgorm_macros::html_sql(r#"<select>SELECT * FROM ${table}</select>"#)]
//! fn raw_identifier(table: String) -> r#type::Result<r#type::BoundSql<'static>> {}
//! ```
//!
//! Parameter expressions reference actual Rust variables.
//!
//! ```compile_fail
//! #[pgorm_macros::html_sql(r#"<select>SELECT #{missing}</select>"#)]
//! fn unknown_parameter() -> r#type::Result<r#type::BoundSql<'static>> {}
//! ```
//!
//! Malformed expressions fail during template compilation.
//!
//! ```compile_fail
//! #[pgorm_macros::html_sql(r#"<select><if test="value +">SELECT 1</if></select>"#)]
//! fn invalid_expression(value: i64) -> r#type::Result<r#type::BoundSql<'static>> {}
//! ```
//!
//! Duplicate statement identifiers cannot silently replace each other.
//!
//! ```compile_fail
//! #[pgorm_macros::html_sql(r#"<mapper><select id="find">SELECT 1</select><select id="find">SELECT 2</select></mapper>"#, id = "find")]
//! fn duplicate_ids() -> r#type::Result<r#type::BoundSql<'static>> {}
//! ```
//!
//! Include cycles are diagnosed, rather than recursively expanding forever.
//!
//! ```compile_fail
//! #[pgorm_macros::html_sql(r#"<mapper><sql id="a"><include refid="b"/></sql><sql id="b"><include refid="a"/></sql><select id="find"><include refid="a"/></select></mapper>"#, id = "find")]
//! fn include_cycle() -> r#type::Result<r#type::BoundSql<'static>> {}
//! ```
//!
//! XML attributes are validated, so typoed condition attributes are caught.
//!
//! ```compile_fail
//! #[pgorm_macros::html_sql(r#"<select><if tset="true">SELECT 1</if></select>"#)]
//! fn typoed_attribute() -> r#type::Result<r#type::BoundSql<'static>> {}
//! ```
//!
//! Manually numbered parameters cannot collide with generated placeholders.
//!
//! ```compile_fail
//! #[pgorm_macros::html_sql(r#"<select>SELECT $1, #{value}</select>"#)]
//! fn manual_parameter(value: i64) -> r#type::Result<r#type::BoundSql<'static>> {}
//! ```

#[cfg(test)]
mod tests {
    #[pgorm_macros::html_sql(r#"<select>SELECT ${column}, #{value}, #{value}</select>"#)]
    fn renamed_runtime(
        column: r#type::SqlIdentifier,
        value: String,
    ) -> r#type::Result<r#type::BoundSql<'static>> {
    }

    #[test]
    fn html_sql_uses_the_real_runtime_even_with_a_keyword_crate_alias() {
        let query = renamed_runtime(
            r#type::SqlIdentifier::new("display name").unwrap(),
            "Ada".to_owned(),
        )
        .unwrap();
        assert_eq!(query.sql(), "SELECT \"display name\", $1, $2");
        assert_eq!(format!("{:?}", query.params()), "[\"Ada\", \"Ada\"]");
    }
}
