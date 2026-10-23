//! Typed SQL values and identifiers used by compile-time XML templates.

use crate::{Error, Result, ToSql};

const MAX_PARAMETERS: usize = u16::MAX as usize;

/// A generated SQL statement with native PostgreSQL bind values.
///
/// Values are deliberately not exposed through `Debug` or interpolated into SQL.
pub struct BoundSql<'a> {
    sql: String,
    params: Vec<Box<dyn ToSql + Sync + Send + 'a>>,
}

impl BoundSql<'_> {
    /// SQL containing numbered PostgreSQL placeholders.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// Borrow the values in placeholder order for an [`crate::Executor`] call.
    pub fn params(&self) -> Vec<&(dyn ToSql + Sync)> {
        self.params
            .iter()
            .map(|value| value.as_ref() as &(dyn ToSql + Sync))
            .collect()
    }

    pub fn parameter_count(&self) -> usize {
        self.params.len()
    }
}

/// A validated, quoted SQL identifier, optionally qualified with a schema name.
///
/// `new("app.users")` quotes a single name as `"app.users"`. Use
/// `qualified(["app", "users"])` to produce `"app"."users"`.
/// This type accepts identifiers only; it cannot represent a SQL expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlIdentifier(String);

impl SqlIdentifier {
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.is_empty() || name.contains('\0') {
            return Err(Error::InvalidIdentifier);
        }
        Ok(Self(format!("\"{}\"", name.replace('"', "\"\""))))
    }

    pub fn qualified<I, S>(parts: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut sql = String::new();
        for part in parts {
            let quoted = Self::new(part)?;
            if !sql.is_empty() {
                sql.push('.');
            }
            sql.push_str(quoted.as_sql());
        }
        if sql.is_empty() {
            return Err(Error::InvalidIdentifier);
        }
        Ok(Self(sql))
    }

    pub fn as_sql(&self) -> &str {
        &self.0
    }
}

/// Assembly machinery for generated template code.
///
/// Each child must be appended before the parent receives another parameter.
/// Children inherit the next placeholder number, and discarded children do not
/// consume parameter numbers in the parent.
#[doc(hidden)]
pub struct SqlBuilder<'a> {
    sql: String,
    params: Vec<Box<dyn ToSql + Sync + Send + 'a>>,
    base_offset: usize,
    separator_pending: bool,
}

impl Default for SqlBuilder<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> SqlBuilder<'a> {
    pub fn new() -> Self {
        Self {
            sql: String::new(),
            params: Vec::new(),
            base_offset: 0,
            separator_pending: false,
        }
    }

    pub fn child(&self) -> Self {
        Self {
            sql: String::new(),
            params: Vec::new(),
            base_offset: self.base_offset + self.params.len(),
            separator_pending: false,
        }
    }

    pub fn push_sql(&mut self, sql: &str) {
        if sql.is_empty() {
            return;
        }
        if self.separator_pending
            && self
                .sql
                .chars()
                .next_back()
                .zip(sql.chars().next())
                .is_some_and(|(left, right)| needs_separator(left, right))
        {
            self.sql.push(' ');
        }
        self.separator_pending = false;
        self.sql.push_str(sql);
    }

    /// Mark a dynamic XML node boundary, including an omitted branch.
    /// Prevent independently rendered tokens from becoming SQL comments or words.
    pub fn boundary(&mut self) {
        self.separator_pending = true;
    }

    pub fn bind<T>(&mut self, value: T)
    where
        T: ToSql + Sync + Send + 'a,
    {
        self.params.push(Box::new(value));
        self.push_sql(&format!("${}", self.base_offset + self.params.len()));
    }

    pub fn push_identifier(&mut self, identifier: &SqlIdentifier) {
        self.push_sql(identifier.as_sql());
    }

    pub fn append(&mut self, child: Self) {
        let has_sql = !child.sql.is_empty();
        self.push_sql(&child.sql);
        self.params.extend(child.params);
        if has_sql {
            self.separator_pending = child.separator_pending;
        }
    }

    /// Append a nonempty child, removing at most one override at each edge.
    ///
    /// Only override tokens are removed: whitespace, string literals and SQL
    /// comments retain their original bytes. In particular, a newline after a
    /// line comment must survive before a closing suffix is added.
    pub fn append_trimmed(
        &mut self,
        mut child: Self,
        prefix: &str,
        suffix: &str,
        prefix_overrides: &str,
        suffix_overrides: &str,
    ) -> bool {
        remove_prefix_override(&mut child.sql, prefix_overrides);
        remove_suffix_override(&mut child.sql, suffix_overrides);
        let edges = sql_edges(&child.sql);
        if edges.first.is_none() {
            return false;
        }
        // A dynamic clause must not become part of a preceding line comment.
        // The cheap marker check avoids scanning normal accumulated SQL here.
        if self.sql.contains("--") && sql_edges(&self.sql).ends_in_line_comment {
            self.sql.push('\n');
        }
        if !prefix.is_empty() {
            self.push_separated(prefix);
        }
        if edges.ends_in_line_comment {
            child.sql.push('\n');
        }
        self.separator_pending = true;
        self.append(child);
        if !suffix.is_empty() {
            self.push_separated(suffix);
        }
        self.separator_pending = true;
        true
    }

    fn push_separated(&mut self, sql: &str) {
        self.separator_pending = true;
        self.push_sql(sql);
    }

    pub fn is_empty(&self) -> bool {
        sql_edges(&self.sql).first.is_none()
    }

    pub fn finish(self) -> Result<BoundSql<'a>> {
        if self.is_empty() {
            return Err(Error::EmptySql);
        }
        if self.base_offset + self.params.len() > MAX_PARAMETERS {
            return Err(Error::TooManyParameters);
        }
        Ok(BoundSql {
            sql: self.sql,
            params: self.params,
        })
    }
}

fn needs_separator(left: char, right: char) -> bool {
    // Delimiters already separate SQL tokens. Keep wrappers and list separators
    // exact, while separating words, quotes, operators and comment openers.
    !left.is_whitespace()
        && !right.is_whitespace()
        && !matches!(left, '(' | '[' | '.' | ',')
        && !matches!(right, ')' | ']' | '.' | ',' | ';')
}

fn is_word_character(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '$')
}

fn remove_prefix_override(sql: &mut String, overrides: &str) {
    let Some(first) = sql_edges(sql).first.filter(|token| !token.quoted) else {
        return;
    };
    let start = first.start;
    for token in overrides
        .split('|')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let Some(candidate) = sql.get(start..start + token.len()) else {
            continue;
        };
        if !candidate.eq_ignore_ascii_case(token) {
            continue;
        }
        let remainder = &sql[start + token.len()..];
        if token.chars().next_back().is_some_and(is_word_character)
            && remainder.chars().next().is_some_and(is_word_character)
        {
            continue;
        }
        sql.replace_range(start..start + token.len(), "");
        break;
    }
}

fn remove_suffix_override(sql: &mut String, overrides: &str) {
    let Some(last) = sql_edges(sql).last.filter(|token| !token.quoted) else {
        return;
    };
    let end = last.end;
    for token in overrides
        .split('|')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let Some(start) = end.checked_sub(token.len()) else {
            continue;
        };
        let Some(candidate) = sql.get(start..end) else {
            continue;
        };
        if !candidate.eq_ignore_ascii_case(token) {
            continue;
        }
        if token.chars().next().is_some_and(is_word_character)
            && sql[..start]
                .chars()
                .next_back()
                .is_some_and(is_word_character)
        {
            continue;
        }
        sql.replace_range(start..end, "");
        break;
    }
}

#[derive(Clone, Copy)]
struct SqlToken {
    start: usize,
    end: usize,
    quoted: bool,
}

#[derive(Default)]
struct SqlEdges {
    first: Option<SqlToken>,
    last: Option<SqlToken>,
    ends_in_line_comment: bool,
}

/// Find the meaningful edges without interpreting SQL. Quoted strings and
/// identifiers remain opaque; comments and whitespace cannot become overrides.
/// PostgreSQL itself remains responsible for SQL syntax validation.
fn sql_edges(sql: &str) -> SqlEdges {
    let mut result = SqlEdges::default();
    let mut pos = 0;
    while pos < sql.len() {
        let rest = &sql[pos..];
        let current = rest.chars().next().unwrap();
        if current.is_whitespace() {
            pos += current.len_utf8();
            continue;
        }
        if let Some(comment) = rest.strip_prefix("--") {
            if let Some(end) = comment.find(['\r', '\n']) {
                pos += 2 + end + 1;
                continue;
            }
            result.ends_in_line_comment = true;
            break;
        }
        if rest.starts_with("/*") {
            pos = skip_block_comment(sql, pos);
            continue;
        }

        let start = pos;
        let quoted;
        if matches!(current, '\'' | '"') {
            pos = skip_quoted(sql, pos, current as u8, false);
            quoted = true;
        } else if matches!(current, 'e' | 'E') && rest.as_bytes().get(1) == Some(&b'\'') {
            pos = skip_quoted(sql, pos + 1, b'\'', true);
            quoted = true;
        } else if let Some(delimiter) = dollar_delimiter(rest) {
            let content = pos + delimiter.len();
            pos = sql[content..]
                .find(delimiter)
                .map_or(sql.len(), |end| content + end + delimiter.len());
            quoted = true;
        } else {
            quoted = false;
            pos += current.len_utf8();
            if is_word_character(current) {
                for next in sql[pos..].chars() {
                    if !is_word_character(next) {
                        break;
                    }
                    pos += next.len_utf8();
                }
            }
        }
        let token = SqlToken {
            start,
            end: pos,
            quoted,
        };
        result.first.get_or_insert(token);
        result.last = Some(token);
    }
    result
}

fn skip_block_comment(sql: &str, start: usize) -> usize {
    let bytes = sql.as_bytes();
    let mut pos = start + 2;
    let mut depth = 1;
    while pos < bytes.len() {
        if bytes.get(pos..pos + 2) == Some(b"/*") {
            depth += 1;
            pos += 2;
        } else if bytes.get(pos..pos + 2) == Some(b"*/") {
            depth -= 1;
            pos += 2;
            if depth == 0 {
                return pos;
            }
        } else {
            pos += 1;
        }
    }
    pos
}

fn skip_quoted(sql: &str, start: usize, quote: u8, escapes: bool) -> usize {
    let bytes = sql.as_bytes();
    let mut pos = start + 1;
    while pos < bytes.len() {
        if bytes[pos] == quote {
            pos += 1;
            if bytes.get(pos) == Some(&quote) {
                pos += 1;
            } else {
                return pos;
            }
        } else if escapes && bytes[pos] == b'\\' {
            pos = (pos + 2).min(bytes.len());
        } else {
            pos += 1;
        }
    }
    pos
}

fn dollar_delimiter(sql: &str) -> Option<&str> {
    let tail = sql.strip_prefix('$')?;
    let end = tail.find('$')?;
    let tag = &tail[..end];
    let mut chars = tag.chars();
    if let Some(first) = chars.next() {
        if first.is_ascii() && !(first.is_ascii_alphabetic() || first == '_') {
            return None;
        }
    }
    if !chars.all(|c| !c.is_ascii() || c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(&sql[..end + 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trimmed(sql: &str, prefix: &str, prefix_overrides: &str, suffix_overrides: &str) -> String {
        let mut builder = SqlBuilder::new();
        let mut body = builder.child();
        body.push_sql(sql);
        builder.append_trimmed(body, prefix, "", prefix_overrides, suffix_overrides);
        builder.finish().unwrap().sql
    }

    #[test]
    fn keyword_overrides_respect_boundaries_and_case() {
        assert_eq!(
            trimmed("  aNd id = 1", "WHERE", "AND|OR", ""),
            "WHERE   id = 1"
        );
        assert_eq!(
            trimmed("ORDER BY id", "WHERE", "AND|OR", ""),
            "WHERE ORDER BY id"
        );
        assert_eq!(
            trimmed("ANDROID = 1", "WHERE", "AND|OR", ""),
            "WHERE ANDROID = 1"
        );
        assert_eq!(
            trimmed("AND_name = 1", "WHERE", "AND|OR", ""),
            "WHERE AND_name = 1"
        );
        assert_eq!(
            trimmed("AND$alias = 1", "WHERE", "AND|OR", ""),
            "WHERE AND$alias = 1"
        );
        assert_eq!(trimmed("ANDé = 1", "WHERE", "AND|OR", ""), "WHERE ANDé = 1");
        assert_eq!(
            trimmed("AND(id = 1)", "WHERE", "AND|OR", ""),
            "WHERE (id = 1)"
        );
        assert_eq!(trimmed("name, \n", "SET", "", ","), "SET name \n");
        assert_eq!(trimmed("brand", "", "", "AND"), "brand");
    }

    #[test]
    fn nested_parameters_and_discarded_subtrees_keep_numbering() {
        let mut builder = SqlBuilder::new();
        builder.push_sql("SELECT ");
        builder.bind(10_i32);
        let mut discarded = builder.child();
        discarded.bind(20_i32);
        assert!(!builder.append_trimmed(discarded, "WHERE", "", "$2", ""));
        let mut child = builder.child();
        child.push_sql(", ");
        child.bind(30_i32);
        let mut nested = child.child();
        nested.push_sql(", ");
        nested.bind(40_i32);
        child.append(nested);
        builder.append(child);
        let bound = builder.finish().unwrap();
        assert_eq!(bound.sql(), "SELECT $1, $2, $3");
        assert_eq!(bound.parameter_count(), 3);
        assert_eq!(bound.params().len(), 3);
    }

    #[test]
    fn empty_trim_emits_no_keywords_or_suffixes() {
        let mut builder = SqlBuilder::new();
        let mut body = builder.child();
        body.push_sql(" \n AND \t");
        assert!(!builder.append_trimmed(body, "WHERE", ")", "AND|OR", ""));
        assert!(builder.is_empty());
        assert!(matches!(builder.finish(), Err(Error::EmptySql)));
    }

    #[test]
    fn literals_and_line_comment_newlines_are_preserved() {
        let mut builder = SqlBuilder::new();
        builder.push_sql("SELECT * FROM users");
        let mut body = builder.child();
        body.push_sql("\n AND note = 'two  spaces\nand a newline' -- keep this newline\n");
        builder.append_trimmed(body, "WHERE (", ")", "AND|OR", "");
        builder.push_sql("ORDER BY id");
        assert_eq!(
            builder.finish().unwrap().sql(),
            "SELECT * FROM users WHERE (\n  note = 'two  spaces\nand a newline' -- keep this newline\n) ORDER BY id"
        );
    }

    #[test]
    fn trim_looks_past_comments_and_preserves_them() {
        assert_eq!(
            trimmed(
                " /* note /* nested */ kept */ AND id = 1",
                "WHERE",
                "AND|OR",
                ""
            ),
            "WHERE /* note /* nested */ kept */  id = 1"
        );
        assert_eq!(
            trimmed("-- explanation\n OR id = 1", "WHERE", "AND|OR", ""),
            "WHERE -- explanation\n  id = 1"
        );
        assert_eq!(
            trimmed("name = $1, -- note\n", "SET", "", ","),
            "SET name = $1 -- note\n"
        );
        assert_eq!(
            trimmed(
                "name = 'literal,', /* outer /* inner */ tail, */",
                "SET",
                "",
                ","
            ),
            "SET name = 'literal,' /* outer /* inner */ tail, */"
        );
    }

    #[test]
    fn trailing_line_comments_cannot_swallow_trim_suffix() {
        let mut builder = SqlBuilder::new();
        let mut child = builder.child();
        child.push_sql("id = 1 -- no newline");
        assert!(builder.append_trimmed(child, "(", ")", "", ""));
        builder.push_sql("AND active");
        assert_eq!(
            builder.finish().unwrap().sql(),
            "(id = 1 -- no newline\n) AND active"
        );
    }

    #[test]
    fn adjacent_clauses_receive_separators_without_changing_body_text() {
        let mut builder = SqlBuilder::new();
        builder.push_sql("UPDATE users");
        let mut child = builder.child();
        child.push_sql("name=");
        child.bind("Ada");
        child.push_sql(",");
        builder.append_trimmed(child, "SET", "", "", ",");
        builder.push_sql("");
        builder.append(builder.child());
        builder.push_sql("WHERE id=");
        builder.bind(7_i32);
        assert_eq!(
            builder.finish().unwrap().sql(),
            "UPDATE users SET name=$1 WHERE id=$2"
        );

        let mut builder = SqlBuilder::new();
        builder.push_sql("SELECT");
        let mut child = builder.child();
        let mut nested = child.child();
        nested.push_sql("1");
        child.append_trimmed(nested, "", "", "", "");
        builder.append_trimmed(child, "", "", "", "");
        builder.push_sql("AS one");
        assert_eq!(builder.finish().unwrap().sql(), "SELECT 1 AS one");
    }

    #[test]
    fn nested_wrappers_keep_punctuation_and_finish_without_added_spaces() {
        let mut builder = SqlBuilder::new();
        builder.push_sql("VALUES ");
        let mut values = builder.child();
        for (index, pair) in [[1_i32, 2_i32], [3_i32, 4_i32]].iter().enumerate() {
            if index > 0 {
                values.push_sql(",");
            }
            let mut row = values.child();
            row.bind(pair[0]);
            row.push_sql(",");
            row.bind(pair[1]);
            values.append_trimmed(row, "(", ")", "", "");
        }
        builder.append_trimmed(values, "", "", "", "");
        assert_eq!(builder.finish().unwrap().sql(), "VALUES ($1,$2),($3,$4)");

        let mut builder = SqlBuilder::new();
        builder.push_sql("SELECT 1-");
        let mut child = builder.child();
        child.push_sql("-2");
        builder.append_trimmed(child, "", "", "", "");
        assert_eq!(builder.finish().unwrap().sql(), "SELECT 1- -2");
    }

    #[test]
    fn line_comments_cannot_swallow_adjacent_clauses() {
        let mut builder = SqlBuilder::new();
        builder.push_sql("UPDATE users -- table note");
        let mut child = builder.child();
        child.push_sql("name=$1, -- assignment note");
        builder.append_trimmed(child, "SET", "", "", ",");
        builder.push_sql("WHERE id=$2");
        assert_eq!(
            builder.finish().unwrap().sql(),
            "UPDATE users -- table note\nSET name=$1 -- assignment note\nWHERE id=$2"
        );
        let mut builder = SqlBuilder::new();
        builder.push_sql("SELECT '--' AS note");
        let mut child = builder.child();
        child.push_sql("true");
        builder.append_trimmed(child, "WHERE", "", "", "");
        assert_eq!(
            builder.finish().unwrap().sql(),
            "SELECT '--' AS note WHERE true"
        );
    }

    #[test]
    fn comments_only_children_discard_their_parameters() {
        let mut builder = SqlBuilder::new();
        builder.push_sql("SELECT ");
        let mut child = builder.child();
        child.push_sql("/* a comment with a binding ");
        child.bind(1_i32);
        child.push_sql(" /* nested */ */ -- final comment");
        assert!(child.is_empty());
        assert!(!builder.append_trimmed(child, "WHERE", "", "AND|OR", ""));
        builder.bind(2_i32);
        let bound = builder.finish().unwrap();
        assert_eq!(bound.sql(), "SELECT $1");
        assert_eq!(bound.parameter_count(), 1);
        let mut comments = SqlBuilder::new();
        comments.push_sql(" /* only a comment */ -- and another\n");
        assert!(matches!(comments.finish(), Err(Error::EmptySql)));
    }

    #[test]
    fn quoted_comment_markers_and_commas_remain_opaque() {
        for literal in [
            "'--'",
            "'/* nested /* comment */ */'",
            "'single '' quote, --'",
            "\"identifier --,\"",
            "$$-- dollar quote /*, */$$",
            "$tag$comma, and '--' $tag$",
            "$💠$-- comma, and /* quote */$💠$",
            "E'escaped \\' -- still literal,'",
            "'ending comma,'",
        ] {
            let mut builder = SqlBuilder::new();
            let mut child = builder.child();
            child.push_sql(literal);
            assert!(!child.is_empty(), "{literal}");
            builder.append_trimmed(child, "(", ")", "AND|OR", ",");
            assert_eq!(builder.finish().unwrap().sql(), format!("({literal})"));
        }
        assert_eq!(
            trimmed(
                "name=$tag$-- content, /* literal */$tag$, -- note\n",
                "SET",
                "",
                ","
            ),
            "SET name=$tag$-- content, /* literal */$tag$ -- note\n"
        );
        assert_eq!(
            trimmed("name=$💠$-- text$💠$,", "SET", "", ","),
            "SET name=$💠$-- text$💠$"
        );
    }

    #[test]
    fn identifiers_are_quoted_without_becoming_sql() {
        assert_eq!(
            SqlIdentifier::new("app.users").unwrap().as_sql(),
            "\"app.users\""
        );
        assert_eq!(
            SqlIdentifier::qualified(["app", "odd\"name"])
                .unwrap()
                .as_sql(),
            "\"app\".\"odd\"\"name\""
        );
        assert_eq!(
            SqlIdentifier::new("a; DROP TABLE users").unwrap().as_sql(),
            "\"a; DROP TABLE users\""
        );
        assert!(SqlIdentifier::new("").is_err());
        assert!(SqlIdentifier::new("bad\0name").is_err());
        assert!(SqlIdentifier::qualified(Vec::<String>::new()).is_err());
        assert!(SqlIdentifier::qualified(["app", ""]).is_err());
        let mut builder = SqlBuilder::new();
        builder.push_sql("SELECT * FROM ");
        builder.push_identifier(&SqlIdentifier::qualified(["app", "users"]).unwrap());
        assert_eq!(
            builder.finish().unwrap().sql(),
            "SELECT * FROM \"app\".\"users\""
        );
    }

    #[test]
    fn parameter_limit_matches_postgresql_protocol() {
        let mut builder = SqlBuilder::new();
        for _ in 0..MAX_PARAMETERS {
            builder.bind(1_i32);
        }
        assert_eq!(builder.finish().unwrap().parameter_count(), MAX_PARAMETERS);
        let mut builder = SqlBuilder::new();
        for _ in 0..=MAX_PARAMETERS {
            builder.bind(1_i32);
        }
        assert!(matches!(builder.finish(), Err(Error::TooManyParameters)));
    }

    #[test]
    fn bound_values_can_borrow_and_remain_send() {
        fn assert_send<T: Send>(_: &T) {}
        let name = String::from("Ada");
        let mut builder = SqlBuilder::new();
        builder.push_sql("SELECT ");
        builder.bind(name.as_str());
        let bound = builder.finish().unwrap();
        assert_send(&bound);
        assert_eq!(bound.sql(), "SELECT $1");
        assert_eq!(bound.params().len(), 1);
    }
}
