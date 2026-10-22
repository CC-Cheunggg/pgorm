//! Derives for `pgorm`; normally imported from the runtime crate's reexports.

use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Fields, Ident, LitStr, Type, ext::IdentExt,
    meta::ParseNestedMeta, parse_macro_input,
};

/// Maps a PostgreSQL row into an owned struct by column name.
///
/// Supports `#[pgorm(column = "database_column")]` on named fields.
/// Use `Entity` alone for models that also need entity metadata.
#[proc_macro_derive(FromRow, attributes(pgorm))]
pub fn derive_from_row(input: TokenStream) -> TokenStream {
    derive(
        parse_macro_input!(input as DeriveInput),
        DeriveKind::FromRow,
    )
}

/// Derives `Entity` and `FromRow` for a named-field struct.
///
/// Requires `#[pgorm(table = "table_name")]` and one `#[pgorm(id)]` field.
/// The `schema` struct attribute and `column`, `generated`, `skip_insert`,
/// and `skip_update` field attributes are optional. `generated` is permitted
/// only on the primary key. The primary key is always excluded from updates.
#[proc_macro_derive(Entity, attributes(pgorm))]
pub fn derive_entity(input: TokenStream) -> TokenStream {
    derive(parse_macro_input!(input as DeriveInput), DeriveKind::Entity)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeriveKind {
    FromRow,
    Entity,
}

fn derive(input: DeriveInput, kind: DeriveKind) -> TokenStream {
    let result = Model::parse(&input, kind)
        .and_then(|model| runtime_path().map(|runtime| model.expand(&runtime, kind)));
    result.unwrap_or_else(syn::Error::into_compile_error).into()
}

fn runtime_path() -> syn::Result<TokenStream2> {
    match crate_name("pgorm") {
        // The runtime declares `extern crate self as pgorm`; the absolute alias
        // also works in the runtime package's integration tests and examples.
        Ok(FoundCrate::Itself) => Ok(quote!(::pgorm)),
        Ok(FoundCrate::Name(name)) => {
            let name = runtime_ident(&name)?;
            Ok(quote!(::#name))
        }
        Err(error) => Err(syn::Error::new(
            Span::call_site(),
            format!("pgorm derives require a dependency on the `pgorm` package: {error}"),
        )),
    }
}

fn runtime_ident(name: &str) -> syn::Result<Ident> {
    syn::parse_str(name)
        .or_else(|_| syn::parse_str(&format!("r#{name}")))
        .map_err(|_| {
            syn::Error::new(
                Span::call_site(),
                format!(
                    "the pgorm dependency alias `{name}` is not a usable Rust crate identifier"
                ),
            )
        })
}

struct Model {
    name: Ident,
    table: Option<LitStr>,
    schema: Option<LitStr>,
    fields: Vec<ModelField>,
}

struct ModelField {
    name: Ident,
    ty: Type,
    column: LitStr,
    id: bool,
    generated: bool,
    skip_insert: bool,
    skip_update: bool,
}

impl Model {
    fn parse(input: &DeriveInput, kind: DeriveKind) -> syn::Result<Self> {
        if !input.generics.params.is_empty() || input.generics.where_clause.is_some() {
            return Err(syn::Error::new_spanned(
                &input.generics,
                "pgorm derives currently support only structs without generic parameters or where clauses",
            ));
        }
        let fields = match &input.data {
            Data::Struct(data) => match &data.fields {
                Fields::Named(fields) => &fields.named,
                _ => {
                    return Err(syn::Error::new_spanned(
                        &input.ident,
                        "pgorm derives require a struct with named fields",
                    ));
                }
            },
            _ => {
                return Err(syn::Error::new_spanned(
                    &input.ident,
                    "pgorm derives require a struct with named fields",
                ));
            }
        };

        let mut model = Self {
            name: input.ident.clone(),
            table: None,
            schema: None,
            fields: Vec::new(),
        };
        let mut seen = HashSet::new();
        parse_attributes(&input.attrs, |meta| {
            let key = attribute_key(&meta)?;
            require_unique(&mut seen, &key, &meta)?;
            if kind == DeriveKind::FromRow {
                return Err(meta.error("FromRow does not support struct-level pgorm attributes"));
            }
            match key.as_str() {
                "table" => model.table = Some(parse_identifier(&meta)?),
                "schema" => model.schema = Some(parse_identifier(&meta)?),
                _ => return Err(meta.error("expected `table` or `schema` on an Entity struct")),
            }
            Ok(())
        })?;
        if kind == DeriveKind::Entity && model.table.is_none() {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "Entity requires #[pgorm(table = \"table_name\")]",
            ));
        }

        let mut columns = HashSet::new();
        let mut has_id = false;
        for field in fields {
            // Named fields were checked above.
            let name = field.ident.clone().expect("named field has an identifier");
            let mut parsed = ModelField {
                column: LitStr::new(&name.unraw().to_string(), name.span()),
                name,
                ty: field.ty.clone(),
                id: false,
                generated: false,
                skip_insert: false,
                skip_update: false,
            };
            let mut seen = HashSet::new();
            parse_attributes(&field.attrs, |meta| {
                let key = attribute_key(&meta)?;
                require_unique(&mut seen, &key, &meta)?;
                if key == "column" {
                    parsed.column = parse_identifier(&meta)?;
                    return Ok(());
                }
                if kind == DeriveKind::FromRow {
                    return Err(meta.error("FromRow fields support only `column = \"name\"`"));
                }
                match key.as_str() {
                    "id" => parsed.id = true,
                    "generated" => parsed.generated = true,
                    "skip_insert" => parsed.skip_insert = true,
                    "skip_update" => parsed.skip_update = true,
                    _ => {
                        return Err(meta.error(
                            "expected `column`, `id`, `generated`, `skip_insert`, or `skip_update`",
                        ));
                    }
                }
                if meta.input.peek(syn::Token![=]) || meta.input.peek(syn::token::Paren) {
                    return Err(meta.error(format!("`{key}` is a flag and takes no value")));
                }
                Ok(())
            })?;
            if parsed.generated && !parsed.id {
                return Err(syn::Error::new_spanned(
                    &parsed.name,
                    "`generated` is supported only on the #[pgorm(id)] field",
                ));
            }
            if parsed.id {
                if has_id {
                    return Err(syn::Error::new_spanned(
                        &parsed.name,
                        "Entity requires exactly one #[pgorm(id)] field; composite keys are not supported",
                    ));
                }
                has_id = true;
            }
            if !columns.insert(parsed.column.value()) {
                return Err(syn::Error::new_spanned(
                    &parsed.column,
                    "duplicate database column name",
                ));
            }
            model.fields.push(parsed);
        }
        if kind == DeriveKind::Entity && !has_id {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "Entity requires exactly one #[pgorm(id)] field",
            ));
        }
        Ok(model)
    }

    fn expand(&self, runtime: &TokenStream2, kind: DeriveKind) -> TokenStream2 {
        let name = &self.name;
        let field_names: Vec<_> = self.fields.iter().map(|field| &field.name).collect();
        let columns: Vec<_> = self.fields.iter().map(|field| &field.column).collect();
        let from_row = quote! {
            impl #runtime::FromRow for #name {
                fn from_row(row: &#runtime::Row) -> #runtime::Result<Self> {
                    ::std::result::Result::Ok(Self {
                        #(#field_names: row.try_get(#columns)?,)*
                    })
                }
            }
        };
        if kind == DeriveKind::FromRow {
            return from_row;
        }

        // Parsing guarantees these are present for the Entity derive.
        let table = self.table.as_ref().expect("Entity has a table");
        let primary_key = self
            .fields
            .iter()
            .find(|field| field.id)
            .expect("Entity has an id");
        let id_name = &primary_key.name;
        let id_type = &primary_key.ty;
        let id_column = &primary_key.column;
        let schema = match &self.schema {
            Some(schema) => quote!(::std::option::Option::Some(#schema)),
            None => quote!(::std::option::Option::None),
        };
        let inserts: Vec<_> = self
            .fields
            .iter()
            .filter(|field| !field.generated && !field.skip_insert)
            .collect();
        let updates: Vec<_> = self
            .fields
            .iter()
            .filter(|field| !field.id && !field.skip_update)
            .collect();
        let insert_columns = inserts.iter().map(|field| &field.column);
        let insert_fields = inserts.iter().map(|field| &field.name);
        let update_columns = updates.iter().map(|field| &field.column);
        let update_fields = updates.iter().map(|field| &field.name);
        quote! {
            #from_row

            impl #runtime::Entity for #name {
                type Id = #id_type;

                const TABLE: &'static str = #table;
                const SCHEMA: ::std::option::Option<&'static str> = #schema;
                const COLUMNS: &'static [&'static str] = &[#(#columns),*];
                const PRIMARY_KEY: &'static str = #id_column;
                const INSERT_COLUMNS: &'static [&'static str] = &[#(#insert_columns),*];
                const UPDATE_COLUMNS: &'static [&'static str] = &[#(#update_columns),*];

                fn id(&self) -> &Self::Id {
                    &self.#id_name
                }

                fn insert_params(&self) -> ::std::vec::Vec<&(dyn #runtime::ToSql + ::std::marker::Sync)> {
                    ::std::vec![#(&self.#insert_fields as &(dyn #runtime::ToSql + ::std::marker::Sync)),*]
                }

                fn update_params(&self) -> ::std::vec::Vec<&(dyn #runtime::ToSql + ::std::marker::Sync)> {
                    ::std::vec![#(&self.#update_fields as &(dyn #runtime::ToSql + ::std::marker::Sync)),*]
                }
            }
        }
    }
}

fn parse_attributes(
    attributes: &[Attribute],
    mut parse: impl FnMut(ParseNestedMeta<'_>) -> syn::Result<()>,
) -> syn::Result<()> {
    for attribute in attributes
        .iter()
        .filter(|attr| attr.path().is_ident("pgorm"))
    {
        let mut populated = false;
        attribute.parse_nested_meta(|meta| {
            populated = true;
            parse(meta)
        })?;
        if !populated {
            return Err(syn::Error::new_spanned(attribute, "empty pgorm attribute"));
        }
    }
    Ok(())
}

fn attribute_key(meta: &ParseNestedMeta<'_>) -> syn::Result<String> {
    meta.path
        .get_ident()
        .map(ToString::to_string)
        .ok_or_else(|| meta.error("expected a pgorm attribute name"))
}

fn require_unique(
    seen: &mut HashSet<String>,
    key: &str,
    meta: &ParseNestedMeta<'_>,
) -> syn::Result<()> {
    if !seen.insert(key.to_owned()) {
        return Err(meta.error(format!("duplicate pgorm attribute `{key}`")));
    }
    Ok(())
}

fn parse_identifier(meta: &ParseNestedMeta<'_>) -> syn::Result<LitStr> {
    let value: LitStr = meta.value()?.parse()?;
    if value.value().is_empty() || value.value().contains('\0') {
        return Err(syn::Error::new_spanned(
            value,
            "database identifiers must be nonempty and cannot contain NUL",
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    fn assert_error(input: DeriveInput, kind: DeriveKind, expected: &str) {
        match Model::parse(&input, kind) {
            Ok(_) => panic!("expected error containing {expected:?}"),
            Err(error) => assert!(error.to_string().contains(expected), "{error}"),
        }
    }

    #[test]
    fn entity_expands_with_renamed_runtime_and_parameter_order() {
        let input = parse_quote! {
            #[pgorm(table = "people", schema = "app")]
            struct Person {
                #[pgorm(id, generated, column = "person_id")]
                id: i64,
                #[pgorm(column = "full name")]
                name: String,
                #[pgorm(skip_update)]
                created: i64,
                #[pgorm(skip_insert)]
                edited: i64,
            }
        };
        let model = Model::parse(&input, DeriveKind::Entity).unwrap();
        let expansion = model.expand(&quote!(::renamed_orm), DeriveKind::Entity);
        let parsed: syn::File = syn::parse2(expansion.clone()).unwrap();
        assert_eq!(parsed.items.len(), 2);
        let output = expansion.to_string();
        assert!(output.contains("impl :: renamed_orm :: Entity for Person"));
        assert!(output.contains(
            "INSERT_COLUMNS : & 'static [& 'static str] = & [\"full name\" , \"created\"]"
        ));
        assert!(output.contains(
            "UPDATE_COLUMNS : & 'static [& 'static str] = & [\"full name\" , \"edited\"]"
        ));
        assert!(output.contains("self . name as & (dyn :: renamed_orm :: ToSql"));
        assert!(!output.contains("self . id as & (dyn"));
    }

    #[test]
    fn raw_rust_identifiers_use_unprefixed_column_names() {
        let input = parse_quote! { struct Projection { r#type: String } };
        let model = Model::parse(&input, DeriveKind::FromRow).unwrap();
        assert_eq!(model.fields[0].column.value(), "type");
        let expanded = model.expand(&quote!(::pgorm), DeriveKind::FromRow);
        syn::parse2::<syn::File>(expanded.clone()).unwrap();
        assert!(
            expanded
                .to_string()
                .contains("r#type : row . try_get (\"type\") ?")
        );
    }

    #[test]
    fn runtime_aliases_accept_keywords_without_panicking() {
        assert_eq!(runtime_ident("query_orm").unwrap().to_string(), "query_orm");
        assert_eq!(runtime_ident("type").unwrap().to_string(), "r#type");
        assert!(runtime_ident("self").is_err());
        assert!(runtime_ident("has-dash").is_err());
    }

    #[test]
    fn quoted_identifiers_are_preserved_for_runtime_escaping() {
        let input = parse_quote! {
            #[pgorm(table = "a\"table", schema = "app space")]
            struct Record { #[pgorm(id, column = "odd\"id")] id: i64 }
        };
        let model = Model::parse(&input, DeriveKind::Entity).unwrap();
        assert_eq!(model.table.unwrap().value(), "a\"table");
        assert_eq!(model.schema.unwrap().value(), "app space");
        assert_eq!(model.fields[0].column.value(), "odd\"id");
    }

    #[test]
    fn malformed_struct_shapes_have_useful_errors() {
        for input in [
            parse_quote! { struct Tuple(i64); },
            parse_quote! { struct Unit; },
            parse_quote! { enum Choice { A } },
            parse_quote! { union Value { id: i64 } },
        ] {
            assert_error(input, DeriveKind::FromRow, "named fields");
        }
        assert_error(
            parse_quote! { struct Generic<T> { value: T } },
            DeriveKind::FromRow,
            "without generic parameters",
        );
    }

    #[test]
    fn entity_requires_table_and_one_primary_key() {
        assert_error(
            parse_quote! { struct NoTable { #[pgorm(id)] id: i64 } },
            DeriveKind::Entity,
            "requires #[pgorm(table",
        );
        assert_error(
            parse_quote! { #[pgorm(table = "t")] struct NoId { value: i64 } },
            DeriveKind::Entity,
            "exactly one #[pgorm(id)]",
        );
        assert_error(
            parse_quote! {
                #[pgorm(table = "t")]
                struct Composite { #[pgorm(id)] a: i64, #[pgorm(id)] b: i64 }
            },
            DeriveKind::Entity,
            "composite keys are not supported",
        );
    }

    #[test]
    fn generated_non_primary_key_is_rejected() {
        assert_error(
            parse_quote! {
                #[pgorm(table = "t")]
                struct Record { #[pgorm(id)] id: i64, #[pgorm(generated)] value: i64 }
            },
            DeriveKind::Entity,
            "`generated` is supported only",
        );
    }

    #[test]
    fn duplicate_keys_are_rejected_across_separate_attributes() {
        assert_error(
            parse_quote! {
                #[pgorm(table = "one")]
                #[pgorm(table = "two")]
                struct Record { #[pgorm(id)] id: i64 }
            },
            DeriveKind::Entity,
            "duplicate pgorm attribute `table`",
        );
        assert_error(
            parse_quote! {
                struct Record { #[pgorm(column = "a")] #[pgorm(column = "b")] value: i64 }
            },
            DeriveKind::FromRow,
            "duplicate pgorm attribute `column`",
        );
    }

    #[test]
    fn duplicate_columns_are_rejected_even_after_renaming() {
        assert_error(
            parse_quote! { struct Record { one: i64, #[pgorm(column = "one")] two: i64 } },
            DeriveKind::FromRow,
            "duplicate database column name",
        );
    }

    #[test]
    fn empty_and_nul_identifiers_are_rejected() {
        assert_error(
            parse_quote! { struct Record { #[pgorm(column = "")] value: i64 } },
            DeriveKind::FromRow,
            "nonempty and cannot contain NUL",
        );
        assert_error(
            parse_quote! {
                #[pgorm(table = "t", schema = "app\0secret")]
                struct Record { #[pgorm(id)] id: i64 }
            },
            DeriveKind::Entity,
            "nonempty and cannot contain NUL",
        );
    }

    #[test]
    fn unsupported_and_misplaced_attributes_are_rejected() {
        assert_error(
            parse_quote! { #[pgorm(table = "t")] struct Record { value: i64 } },
            DeriveKind::FromRow,
            "does not support struct-level",
        );
        assert_error(
            parse_quote! { struct Record { #[pgorm(skip_insert)] value: i64 } },
            DeriveKind::FromRow,
            "support only `column",
        );
        assert_error(
            parse_quote! { #[pgorm(table = "t", typo)] struct Record { value: i64 } },
            DeriveKind::Entity,
            "expected `table` or `schema`",
        );
        assert_error(
            parse_quote! {
                #[pgorm(table = "t")]
                struct Record { #[pgorm(id = true)] id: i64 }
            },
            DeriveKind::Entity,
            "is a flag and takes no value",
        );
        assert_error(
            parse_quote! { struct Record { #[pgorm()] id: i64 } },
            DeriveKind::FromRow,
            "empty pgorm attribute",
        );
    }
}
