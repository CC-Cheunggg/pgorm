//! Compiles macro expansion against the real runtime with a keyword crate alias.
//!
//! Entity models must explicitly select their primary key.
//!
//! ```compile_fail
//! #[derive(pgorm_macros::Entity)]
//! #[pgorm(table = "records")]
//! struct Record { id: i64 }
//! ```
//!
//! Column names must be distinct after mappings are applied.
//!
//! ```compile_fail
//! #[derive(pgorm_macros::FromRow)]
//! struct Projection { id: i64, #[pgorm(column = "id")] other: i64 }
//! ```
//!
//! Generic structs are explicitly outside the initial derive contract.
//!
//! ```compile_fail
//! #[derive(pgorm_macros::FromRow)]
//! struct Projection<T> { value: T }
//! ```
//!
//! Only primary keys may use the generated flag in this first version.
//!
//! ```compile_fail
//! #[derive(pgorm_macros::Entity)]
//! #[pgorm(table = "records")]
//! struct Record {
//!     #[pgorm(id)] id: i64,
//!     #[pgorm(generated)] value: i64,
//! }
//! ```

mod xml;

#[derive(pgorm_macros::Entity)]
#[pgorm(table = "people", schema = "app space")]
pub struct Person {
    #[pgorm(id, generated, column = "person id")]
    pub id: i64,
    #[pgorm(column = "display\"name")]
    pub name: String,
    #[pgorm(skip_update)]
    pub created: i64,
    #[pgorm(skip_insert)]
    pub updated: Option<i64>,
    pub r#type: String,
}

#[derive(pgorm_macros::FromRow)]
pub struct Projection {
    pub r#type: String,
    #[pgorm(column = "total")]
    pub count: i64,
}

// Names used internally by expansion must not interfere with application types.
#[derive(pgorm_macros::Entity)]
#[pgorm(table = "only_id")]
pub struct Row {
    #[pgorm(id, generated)]
    pub row: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use r#type::{Entity, FromRow};

    fn assert_from_row<T: FromRow>() {}

    #[test]
    fn implements_real_traits_and_preserves_column_parameter_order() {
        assert_from_row::<Person>();
        assert_from_row::<Projection>();
        assert_from_row::<Row>();
        let person = Person {
            id: 7,
            name: "Ada".into(),
            created: 10,
            updated: None,
            r#type: "person".into(),
        };
        assert_eq!(*person.id(), 7);
        assert_eq!(Person::TABLE, "people");
        assert_eq!(Person::SCHEMA, Some("app space"));
        assert_eq!(Person::PRIMARY_KEY, "person id");
        assert_eq!(Person::INSERT_COLUMNS, ["display\"name", "created", "type"]);
        assert_eq!(Person::UPDATE_COLUMNS, ["display\"name", "updated", "type"]);
        // ToSql requires Debug, so inspection also verifies values follow the
        // exact same field order as the associated column constants.
        assert_eq!(
            format!("{:?}", person.insert_params()),
            "[\"Ada\", 10, \"person\"]"
        );
        assert_eq!(
            format!("{:?}", person.update_params()),
            "[\"Ada\", None, \"person\"]"
        );
        assert!(Row { row: 1 }.insert_params().is_empty());
        assert!(Row { row: 1 }.update_params().is_empty());
    }
}
