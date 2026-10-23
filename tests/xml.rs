#![cfg(feature = "derive")]

use pgorm::{BoundSql, Error, SqlIdentifier};

fn normalized(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn values(sql: &BoundSql<'_>) -> String {
    format!("{:?}", sql.params())
}

#[pgorm::html_sql("tests/xml/queries.xml", id = "find_users")]
fn find_users(active: bool, name: Option<String>) -> pgorm::Result<BoundSql<'static>> {}

#[pgorm::html_sql("tests/xml/queries.xml")]
fn default_statement(value: i64) -> pgorm::Result<BoundSql<'static>> {}

#[pgorm::html_sql("tests/xml/queries.xml", id = "find_ids")]
fn find_ids<'a>(ids: &'a [i64]) -> pgorm::Result<BoundSql<'a>> {}

#[test]
fn external_mapper_includes_cross_file_fragments_and_tracks_conditional_parameters() {
    let query = find_users(true, Some("Ada".to_owned())).unwrap();
    assert_eq!(
        normalized(query.sql()),
        "SELECT id, name, active FROM users WHERE active = $1 AND name = $2 ORDER BY id"
    );
    assert_eq!(values(&query), "[true, \"Ada\"]");
    assert_eq!(query.parameter_count(), 2);

    let query = find_users(false, Some("Grace".to_owned())).unwrap();
    assert_eq!(
        normalized(query.sql()),
        "SELECT id, name, active FROM users WHERE name = $1 ORDER BY id"
    );
    assert_eq!(values(&query), "[\"Grace\"]");

    let query = find_users(false, None).unwrap();
    assert_eq!(
        normalized(query.sql()),
        "SELECT id, name, active FROM users ORDER BY id"
    );
    assert_eq!(query.parameter_count(), 0);
}

#[test]
fn function_name_selects_default_statement_id() {
    let query = default_statement(17).unwrap();
    assert_eq!(normalized(query.sql()), "SELECT $1::BIGINT AS value");
    assert_eq!(values(&query), "[17]");
}

#[pgorm::html_sql(r#"<select>SELECT #{name}, #{name}, #{suffix.to_uppercase()}</select>"#)]
fn repeated_owned_values(name: String, suffix: String) -> pgorm::Result<BoundSql<'static>> {}

#[test]
fn repeated_owned_strings_and_expression_temporaries_remain_alive() {
    let query = repeated_owned_values("雪".to_owned(), "ending".to_owned()).unwrap();
    assert_eq!(query.sql(), "SELECT $1, $2, $3");
    assert_eq!(values(&query), "[\"雪\", \"雪\", \"ENDING\"]");
}

#[pgorm::html_sql(r#"<select>SELECT <choose><when test="first">#{10_i64}</when><when test="second">#{20_i64}</when><otherwise>#{30_i64}</otherwise></choose></select>"#)]
fn chosen_value(first: bool, second: bool) -> pgorm::Result<BoundSql<'static>> {}

#[test]
fn choose_renders_only_the_first_matching_branch_or_otherwise() {
    for (first, second, expected) in [
        (true, true, "[10]"),
        (true, false, "[10]"),
        (false, true, "[20]"),
        (false, false, "[30]"),
    ] {
        let query = chosen_value(first, second).unwrap();
        assert_eq!(query.sql(), "SELECT $1");
        assert_eq!(values(&query), expected);
    }
}

#[test]
fn foreach_binds_each_element_and_rejects_empty_collections() {
    let query = find_ids(&[11, 22, 33]).unwrap();
    assert_eq!(
        normalized(query.sql()),
        "SELECT id FROM users WHERE id IN ($1,$2,$3)"
    );
    assert_eq!(values(&query), "[11, 22, 33]");
    assert!(matches!(find_ids(&[]), Err(Error::EmptyCollection(_))));
}

#[pgorm::html_sql(r#"<select>SELECT 1<foreach collection="ids" item="id" open=" WHERE id IN (" close=")" separator="," empty="skip">#{id}</foreach></select>"#)]
fn optional_ids<'a>(ids: &'a [i64]) -> pgorm::Result<BoundSql<'a>> {}

#[test]
fn foreach_empty_skip_is_explicit_and_emits_no_wrappers() {
    let query = optional_ids(&[]).unwrap();
    assert_eq!(query.sql(), "SELECT 1");
    assert_eq!(query.parameter_count(), 0);
    let query = optional_ids(&[7]).unwrap();
    assert_eq!(query.sql(), "SELECT 1 WHERE id IN ($1)");
    assert_eq!(values(&query), "[7]");
}

#[pgorm::html_sql(r#"<select>VALUES <foreach collection="rows" item="row" separator=","><foreach collection="row" item="value" open="(" close=")" separator=",">#{value}</foreach></foreach></select>"#)]
fn nested_rows(rows: Vec<Vec<i64>>) -> pgorm::Result<BoundSql<'static>> {}

#[test]
fn nested_foreach_preserves_global_parameter_numbering() {
    let query = nested_rows(vec![vec![1, 2], vec![3, 4]]).unwrap();
    assert_eq!(query.sql(), "VALUES ($1,$2),($3,$4)");
    assert_eq!(values(&query), "[1, 2, 3, 4]");
    assert_eq!(query.parameter_count(), 4);
}

#[pgorm::html_sql(r#"<select>SELECT <foreach collection="ids" item="id" index="position" open="(" close=")" separator="," empty="skip"><if test="*id &gt; 0">#{position as i64} + #{id}</if></foreach></select>"#)]
fn filtered_ids<'a>(ids: &'a [i64]) -> pgorm::Result<BoundSql<'a>> {}

#[test]
fn foreach_separators_apply_only_to_rendered_bodies_and_keep_source_indices() {
    let query = filtered_ids(&[0, 8, 0, 9, 0]).unwrap();
    assert_eq!(query.sql(), "SELECT ($1 + $2,$3 + $4)");
    assert_eq!(values(&query), "[1, 8, 3, 9]");
    let query = filtered_ids(&[0, 0]).unwrap();
    assert_eq!(query.sql(), "SELECT ");
    assert_eq!(query.parameter_count(), 0);
}

#[pgorm::html_sql(r#"<select><bind name="pattern" value="format!(&quot;%{}%&quot;, name.trim())"/>SELECT #{pattern}, #{pattern}<if test="!name.is_empty() &amp;&amp; enabled">, #{enabled}</if></select>"#)]
fn computed_binding(name: String, enabled: bool) -> pgorm::Result<BoundSql<'static>> {}

#[test]
fn bind_expressions_own_computed_values_and_decode_xml_entities() {
    let query = computed_binding(" Ada ".to_owned(), true).unwrap();
    assert_eq!(query.sql(), "SELECT $1, $2, $3");
    assert_eq!(values(&query), "[\"%Ada%\", \"%Ada%\", true]");
}

#[pgorm::html_sql(r#"<update>UPDATE users <set><if test="name.is_some()">name = #{name.as_ref().unwrap()},</if><if test="active.is_some()">active = #{active.unwrap()},</if></set> WHERE id = #{id}</update>"#)]
fn update_fields(
    id: i64,
    name: Option<String>,
    active: Option<bool>,
) -> pgorm::Result<BoundSql<'static>> {
}

#[test]
fn set_trims_final_comma_and_refuses_an_empty_update() {
    let query = update_fields(8, Some("Ada".to_owned()), Some(true)).unwrap();
    assert_eq!(
        normalized(query.sql()),
        "UPDATE users SET name = $1,active = $2 WHERE id = $3"
    );
    assert_eq!(values(&query), "[\"Ada\", true, 8]");
    let query = update_fields(9, None, Some(false)).unwrap();
    assert_eq!(
        normalized(query.sql()),
        "UPDATE users SET active = $1 WHERE id = $2"
    );
    assert!(matches!(update_fields(1, None, None), Err(Error::EmptySet)));
}

#[pgorm::html_sql(r#"<select>SELECT 1 <trim prefix="WHERE (" suffix=")" prefixOverrides="AND|OR" suffixOverrides=","> AND id = #{id}, </trim></select>"#)]
fn trimmed(id: i64) -> pgorm::Result<BoundSql<'static>> {}

#[pgorm::html_sql(r#"<select>SELECT 1 <where>ORDERED = #{ordered}</where></select>"#)]
fn keyword_prefix(ordered: bool) -> pgorm::Result<BoundSql<'static>> {}

#[pgorm::html_sql(r#"<select>SELECT 1 <where>or enabled = #{enabled}</where></select>"#)]
fn lowercase_where(enabled: bool) -> pgorm::Result<BoundSql<'static>> {}

#[test]
fn trim_supports_prefix_suffix_overrides_without_eating_identifier_prefixes() {
    let query = trimmed(4).unwrap();
    assert_eq!(normalized(query.sql()), "SELECT 1 WHERE ( id = $1 )");
    assert_eq!(values(&query), "[4]");
    let query = keyword_prefix(true).unwrap();
    assert_eq!(normalized(query.sql()), "SELECT 1 WHERE ORDERED = $1");
    let query = lowercase_where(true).unwrap();
    assert_eq!(normalized(query.sql()), "SELECT 1 WHERE enabled = $1");
}

#[pgorm::html_sql(r#"<select><![CDATA[SELECT 'a  b', '#{not_a_parameter}', "${not_an_identifier}", $q$#{not_a_parameter} ${not_an_identifier}$q$
-- #{also_not_a_parameter}
/* ${not_an_identifier} */
, #{value} < 10 AS under_limit]]></select>"#)]
fn sql_lexical_context(value: i64) -> pgorm::Result<BoundSql<'static>> {}

#[test]
fn literals_comments_dollar_quotes_and_cdata_keep_sql_text_intact() {
    let query = sql_lexical_context(3).unwrap();
    assert_eq!(
        query.sql(),
        "SELECT 'a  b', '#{not_a_parameter}', \"${not_an_identifier}\", $q$#{not_a_parameter} ${not_an_identifier}$q$\n\
-- #{also_not_a_parameter}\n\
/* ${not_an_identifier} */\n\
, $1 < 10 AS under_limit"
    );
    assert_eq!(values(&query), "[3]");
}

#[pgorm::html_sql(r#"<select>SELECT ${column} FROM ${table} WHERE ${column} = #{value}</select>"#)]
fn identifiers(
    column: SqlIdentifier,
    table: SqlIdentifier,
    value: String,
) -> pgorm::Result<BoundSql<'static>> {
}

#[test]
fn identifier_substitution_quotes_each_component_and_does_not_bind_it_as_data() {
    let query = identifiers(
        SqlIdentifier::new("display\"name").unwrap(),
        SqlIdentifier::qualified(["app space", "order"]).unwrap(),
        "Ada".to_owned(),
    )
    .unwrap();
    assert_eq!(
        query.sql(),
        "SELECT \"display\"\"name\" FROM \"app space\".\"order\" WHERE \"display\"\"name\" = $1"
    );
    assert_eq!(values(&query), "[\"Ada\"]");
}

#[pgorm::html_sql(r#"<select>SELECT #{value}</select>"#)]
fn generic_value<T>(value: T) -> pgorm::Result<BoundSql<'static>>
where
    T: pgorm::ToSql + Sync + Send + Clone + 'static,
{
}

#[test]
fn render_functions_preserve_generic_bounds() {
    let query = generic_value(42_i64).unwrap();
    assert_eq!(query.sql(), "SELECT $1");
    assert_eq!(values(&query), "[42]");
}

#[pgorm::html_sql(r#"<select>SELECT #{if enabled { value } else { 0_i64 }}, #{format!("{}:{{}}", value)}</select>"#)]
fn nested_rust_expressions(enabled: bool, value: i64) -> pgorm::Result<BoundSql<'static>> {}

#[test]
fn parameter_expressions_accept_rust_blocks_and_braces_inside_string_literals() {
    let query = nested_rust_expressions(true, 8).unwrap();
    assert_eq!(query.sql(), "SELECT $1, $2");
    assert_eq!(values(&query), "[8, \"8:{}\"]");
    let query = nested_rust_expressions(false, 9).unwrap();
    assert_eq!(values(&query), "[0, \"9:{}\"]");
}

#[pgorm::html_sql(r#"<select>SELECT <foreach collection="ids" item="id" separator=",">#{id}</foreach> UNION ALL SELECT <foreach collection="ids" item="id" separator=",">#{id}</foreach><if test="ids.len() == 2">, #{ids.len() as i64}</if></select>"#)]
fn reused_collection(ids: Vec<i64>) -> pgorm::Result<BoundSql<'static>> {}

#[test]
fn foreach_borrows_owned_collections_so_later_loops_and_expressions_can_reuse_them() {
    let query = reused_collection(vec![10, 20]).unwrap();
    assert_eq!(query.sql(), "SELECT $1,$2 UNION ALL SELECT $3,$4, $5");
    assert_eq!(values(&query), "[10, 20, 10, 20, 2]");
}

#[pgorm::html_sql("tests/xml/queries.xml")]
fn fragment_comment(value: i64) -> pgorm::Result<BoundSql<'static>> {}

#[pgorm::html_sql("tests/xml/queries.xml")]
fn fragment_block_comment(value: i64) -> pgorm::Result<BoundSql<'static>> {}

#[pgorm::html_sql("tests/xml/queries.xml")]
fn fragment_dollar_quote(value: i64) -> pgorm::Result<BoundSql<'static>> {}

#[pgorm::html_sql("tests/xml/queries.xml")]
fn fragment_escape_string(value: i64) -> pgorm::Result<BoundSql<'static>> {}

#[test]
fn included_fragments_can_form_sql_lexical_tokens_without_creating_extra_parameters() {
    for (query, expected_sql) in [
        (fragment_comment(9).unwrap(), "SELECT 1 -- #{value}\n, $1"),
        (
            fragment_block_comment(9).unwrap(),
            "SELECT 1 /* #{value} */, $1",
        ),
        (
            fragment_dollar_quote(9).unwrap(),
            "SELECT $q$#{value}$q$, $1",
        ),
        (
            fragment_escape_string(9).unwrap(),
            r"SELECT E'it\'s #{value}', $1",
        ),
    ] {
        assert_eq!(query.sql(), expected_sql);
        assert_eq!(values(&query), "[9]");
        assert_eq!(query.parameter_count(), 1);
    }
}

#[pgorm::html_sql(
    r#"<select>SELECT 1 -<!-- XML comments do not change SQL -->- #{value}
, /<![CDATA[* #{value} */]]> #{value}, $q<![CDATA[$#{value}$q$]]></select>"#
)]
fn fragmented_text(value: i64) -> pgorm::Result<BoundSql<'static>> {}

#[test]
fn xml_comments_and_cdata_boundaries_do_not_change_sql_lexing() {
    let query = fragmented_text(11).unwrap();
    assert_eq!(
        query.sql(),
        "SELECT 1 -- #{value}\n, /* #{value} */ $1, $q$#{value}$q$"
    );
    assert_eq!(values(&query), "[11]");
}

#[pgorm::html_sql(r#"<select>SELECT 10-<if test="negative">-</if>#{value} AS value</select>"#)]
fn dynamic_minus(negative: bool, value: i64) -> pgorm::Result<BoundSql<'static>> {}

#[pgorm::html_sql(r#"<select>SELECT 10-<choose><when test="negative">-</when><otherwise>+</otherwise></choose>#{value} AS value</select>"#)]
fn chosen_operator(negative: bool, value: i64) -> pgorm::Result<BoundSql<'static>> {}

#[pgorm::html_sql(
    r#"<select>SELECT 10-<bind name="offset" value="value"/>-#{offset} AS value</select>"#
)]
fn binding_between_operators(value: i64) -> pgorm::Result<BoundSql<'static>> {}

#[pgorm::html_sql(r#"<select>SELECT 10-<if test="include_plus">+</if>-#{value} AS value</select>"#)]
fn optional_operator(include_plus: bool, value: i64) -> pgorm::Result<BoundSql<'static>> {}

#[pgorm::html_sql(r#"<select>SELECT 10<foreach collection="ids" item="_id">-</foreach>#{value} AS value</select>"#)]
fn repeated_operators(ids: Vec<i64>, value: i64) -> pgorm::Result<BoundSql<'static>> {}

#[test]
fn dynamic_xml_nodes_keep_adjacent_operators_from_forming_sql_comments() {
    for (query, expected_tokens) in [
        (dynamic_minus(true, 2).unwrap(), "SELECT10--$1ASvalue"),
        (dynamic_minus(false, 2).unwrap(), "SELECT10-$1ASvalue"),
        (chosen_operator(true, 2).unwrap(), "SELECT10--$1ASvalue"),
        (chosen_operator(false, 2).unwrap(), "SELECT10-+$1ASvalue"),
        (binding_between_operators(2).unwrap(), "SELECT10--$1ASvalue"),
        (optional_operator(false, 2).unwrap(), "SELECT10--$1ASvalue"),
        (
            repeated_operators(vec![0, 0], 2).unwrap(),
            "SELECT10--$1ASvalue",
        ),
    ] {
        assert!(!query.sql().contains("--"), "{}", query.sql());
        assert_eq!(
            query.sql().split_whitespace().collect::<String>(),
            expected_tokens
        );
        assert_eq!(query.parameter_count(), 1);
        assert_eq!(values(&query), "[2]");
    }
}
