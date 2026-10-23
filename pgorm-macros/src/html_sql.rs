//! Independently implemented compile-time XML SQL compiler.
//! XML becomes typed Rust control flow; no runtime value interpreter is emitted.
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};
use syn::{
    Expr, FnArg, GenericArgument, Ident, ItemFn, LitStr, Pat, PathArguments, ReturnType, Stmt,
    Token, Type,
    parse::{Parse, ParseStream},
    spanned::Spanned,
};

struct Arguments {
    source: LitStr,
    id: Option<LitStr>,
}
impl Parse for Arguments {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let source = input.parse()?;
        let mut id = None;
        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let key: Ident = input.parse()?;
            if key != "id" {
                return Err(syn::Error::new(
                    key.span(),
                    "expected `id = \"statement_id\"`",
                ));
            }
            if id.is_some() {
                return Err(syn::Error::new(key.span(), "duplicate `id` argument"));
            }
            input.parse::<Token![=]>()?;
            id = Some(input.parse()?);
        }
        Ok(Self { source, id })
    }
}
#[derive(Clone)]
struct Location {
    source: String,
    line: u32,
    column: u32,
    span: Span,
}
impl Location {
    fn error(&self, message: impl std::fmt::Display) -> syn::Error {
        syn::Error::new(
            self.span,
            format!("{}:{}:{}: {message}", self.source, self.line, self.column),
        )
    }
    fn offset(&self, text: &str, offset: usize) -> Self {
        let before = &text[..offset];
        let lines = before.bytes().filter(|byte| *byte == b'\n').count() as u32;
        let column = match before.rsplit_once('\n') {
            Some((_, tail)) => tail.chars().count() as u32 + 1,
            None => self.column + before.chars().count() as u32,
        };
        Self {
            line: self.line + lines,
            column,
            ..self.clone()
        }
    }
}
#[derive(Clone)]
struct Element {
    tag: String,
    attributes: BTreeMap<String, String>,
    children: Vec<Content>,
    location: Location,
}
#[derive(Clone)]
enum Content {
    Text(String, Location),
    Element(Element),
}
impl Element {
    fn attribute(&self, key: &str) -> syn::Result<&str> {
        self.attributes
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                self.location.error(format!(
                    "<{}> requires a nonempty `{key}` attribute",
                    self.tag
                ))
            })
    }
    fn optional(&self, key: &str) -> &str {
        self.attributes.get(key).map(String::as_str).unwrap_or("")
    }
    fn allowed_attributes(&self, allowed: &[&str]) -> syn::Result<()> {
        for name in self.attributes.keys() {
            if !allowed.contains(&name.as_str()) {
                return Err(self.location.error(format!(
                    "unknown attribute `{name}` on <{}>; allowed attributes: {}",
                    self.tag,
                    allowed.join(", ")
                )));
            }
        }
        Ok(())
    }
    fn empty(&self) -> syn::Result<()> {
        if self
            .children
            .iter()
            .any(|child| !matches!(child, Content::Text(text, _) if text.trim().is_empty()))
        {
            return Err(self
                .location
                .error(format!("<{}> must not have content", self.tag)));
        }
        Ok(())
    }
    fn expression(&self, attribute: &str) -> syn::Result<Expr> {
        syn::parse_str(self.attribute(attribute)?).map_err(|error| {
            self.location
                .error(format!("invalid Rust expression in `{attribute}`: {error}"))
        })
    }
    fn identifier(&self, attribute: &str) -> syn::Result<Ident> {
        let value = self.attribute(attribute)?;
        let ident = syn::parse_str::<Ident>(value).map_err(|_| {
            self.location
                .error(format!("`{attribute}` must be a Rust identifier"))
        })?;
        if value.trim_start_matches("r#").starts_with("__pgorm_") {
            return Err(self
                .location
                .error("names starting with `__pgorm_` are reserved for generated code"));
        }
        Ok(ident)
    }
}
struct Source {
    path: Option<PathBuf>,
    statements: HashMap<String, Element>,
    fragments: HashMap<String, Element>,
    direct: Option<Element>,
}
impl Source {
    fn parse(text: &str, path: Option<PathBuf>, display: &str, span: Span) -> syn::Result<Self> {
        // Default roxmltree parsing rejects DTDs and does not load external entities.
        let document = roxmltree::Document::parse(text).map_err(|error| {
            let position = error.pos();
            syn::Error::new(
                span,
                format!(
                    "{display}:{}:{}: invalid XML: {error}",
                    position.row, position.col
                ),
            )
        })?;
        let root = read_element(document.root_element(), &document, display, span)?;
        let mut source = Self {
            path,
            statements: HashMap::new(),
            fragments: HashMap::new(),
            direct: None,
        };
        if root.tag == "mapper" {
            root.allowed_attributes(&["namespace", "xsi:noNamespaceSchemaLocation"])?;
            let mut ids = HashSet::new();
            for child in &root.children {
                let node = match child {
                    Content::Text(text, _) if text.trim().is_empty() => continue,
                    Content::Text(_, location) => {
                        return Err(
                            location.error("<mapper> accepts only statement and <sql> elements")
                        );
                    }
                    Content::Element(node) => node,
                };
                if !is_statement(&node.tag) && node.tag != "sql" {
                    return Err(node.location.error(format!("unknown <mapper> child <{}>; expected select, insert, update, delete, or sql", node.tag)));
                }
                node.allowed_attributes(&["id"])?;
                let id = node.attribute("id")?.to_owned();
                if !ids.insert(id.clone()) {
                    return Err(node.location.error(format!("duplicate XML id `{id}`")));
                }
                validate_children(node)?;
                if node.tag == "sql" {
                    source.fragments.insert(id, node.clone());
                } else {
                    source.statements.insert(id, node.clone());
                }
            }
        } else if is_statement(&root.tag) {
            root.allowed_attributes(&["id"])?;
            validate_children(&root)?;
            if root.attributes.contains_key("id") {
                source
                    .statements
                    .insert(root.attribute("id")?.to_owned(), root.clone());
            }
            source.direct = Some(root);
        } else {
            return Err(root
                .location
                .error("XML root must be <mapper>, <select>, <insert>, <update>, or <delete>"));
        }
        Ok(source)
    }
}
fn read_element(
    node: roxmltree::Node<'_, '_>,
    document: &roxmltree::Document<'_>,
    display: &str,
    span: Span,
) -> syn::Result<Element> {
    let position = document.text_pos_at(node.range().start);
    let location = Location {
        source: display.to_owned(),
        line: position.row,
        column: position.col,
        span,
    };
    if node.tag_name().namespace().is_some() {
        return Err(location.error("SQL elements must not use an XML namespace"));
    }
    let mut attributes = BTreeMap::new();
    for attr in node.attributes() {
        let name = match attr.namespace() {
            None => attr.name().to_owned(),
            Some("http://www.w3.org/2001/XMLSchema-instance")
                if node.tag_name().name() == "mapper"
                    && attr.name() == "noNamespaceSchemaLocation" =>
            {
                "xsi:noNamespaceSchemaLocation".to_owned()
            }
            Some(_) => {
                return Err(location.error(format!(
                    "unsupported namespaced attribute `{}`",
                    attr.name()
                )));
            }
        };
        attributes.insert(name, attr.value().to_owned());
    }
    let mut children = Vec::new();
    for child in node.children() {
        if child.is_element() {
            children.push(Content::Element(read_element(
                child, document, display, span,
            )?));
        } else if child.is_text() {
            let position = document.text_pos_at(child.range().start);
            children.push(Content::Text(
                child.text().unwrap_or_default().to_owned(),
                Location {
                    source: display.to_owned(),
                    line: position.row,
                    column: position.col,
                    span,
                },
            ));
        } else if child.is_pi() {
            return Err(
                location.error("XML processing instructions are unsupported inside SQL templates")
            );
        }
    }
    Ok(Element {
        tag: node.tag_name().name().to_owned(),
        attributes,
        children,
        location,
    })
}
fn is_statement(tag: &str) -> bool {
    matches!(tag, "select" | "insert" | "update" | "delete")
}
fn validate_children(parent: &Element) -> syn::Result<()> {
    for child in &parent.children {
        let Content::Element(node) = child else {
            continue;
        };
        match node.tag.as_str() {
            "if" | "when" => {
                node.allowed_attributes(&["test"])?;
                node.expression("test")?;
                if node.tag == "when" && parent.tag != "choose" {
                    return Err(node
                        .location
                        .error("<when> must be an immediate child of <choose>"));
                }
            }
            "otherwise" => {
                node.allowed_attributes(&[])?;
                if parent.tag != "choose" {
                    return Err(node
                        .location
                        .error("<otherwise> must be an immediate child of <choose>"));
                }
            }
            "where" | "set" => node.allowed_attributes(&[])?,
            "trim" => node.allowed_attributes(&[
                "prefix",
                "suffix",
                "prefixOverrides",
                "suffixOverrides",
            ])?,
            "foreach" => {
                node.allowed_attributes(&[
                    "collection",
                    "item",
                    "index",
                    "open",
                    "close",
                    "separator",
                    "empty",
                ])?;
                node.expression("collection")?;
                let item = node.identifier("item")?;
                if node.attributes.contains_key("index") && node.identifier("index")? == item {
                    return Err(node
                        .location
                        .error("foreach `item` and `index` must have different names"));
                }
                if !matches!(node.optional("empty"), "" | "error" | "skip") {
                    return Err(node
                        .location
                        .error("foreach `empty` must be `error` or `skip`"));
                }
            }
            "bind" => {
                node.allowed_attributes(&["name", "value"])?;
                node.identifier("name")?;
                node.expression("value")?;
                node.empty()?;
            }
            "include" => {
                node.allowed_attributes(&["refid", "file"])?;
                node.attribute("refid")?;
                if node.attributes.contains_key("file") {
                    node.attribute("file")?;
                }
                node.empty()?;
            }
            "choose" => {
                node.allowed_attributes(&[])?;
                let mut when_count = 0;
                let mut otherwise = false;
                for child in &node.children {
                    match child {
                        Content::Text(text, _) if text.trim().is_empty() => {}
                        Content::Element(branch) if branch.tag == "when" => {
                            if otherwise {
                                return Err(branch
                                    .location
                                    .error("<when> cannot follow <otherwise>"));
                            }
                            when_count += 1;
                        }
                        Content::Element(branch) if branch.tag == "otherwise" => {
                            if otherwise {
                                return Err(branch
                                    .location
                                    .error("<choose> permits only one <otherwise>"));
                            }
                            otherwise = true;
                        }
                        Content::Text(_, location) => {
                            return Err(location
                                .error("<choose> accepts only <when> and <otherwise> branches"));
                        }
                        Content::Element(branch) => {
                            return Err(branch
                                .location
                                .error("<choose> accepts only <when> and <otherwise> branches"));
                        }
                    }
                }
                if when_count == 0 {
                    return Err(node.location.error("<choose> requires at least one <when>"));
                }
            }
            _ => {
                return Err(node
                    .location
                    .error(format!("unknown SQL element <{}>", node.tag)));
            }
        }
        validate_children(node)?;
    }
    Ok(())
}
struct Compiler {
    runtime: TokenStream,
    sources: HashMap<String, Source>,
    dependencies: Vec<PathBuf>,
    span: Span,
    next_name: usize,
}
impl Compiler {
    fn fresh(&mut self, stem: &str) -> Ident {
        self.next_name += 1;
        format_ident!("__pgorm_{stem}_{}", self.next_name)
    }
    fn load(&mut self, path: &Path, location: &Location) -> syn::Result<String> {
        let path = path.canonicalize().map_err(|error| {
            location.error(format!(
                "cannot locate XML file `{}`: {error}",
                path.display()
            ))
        })?;
        let key = path.to_string_lossy().into_owned();
        if !self.sources.contains_key(&key) {
            let text = std::fs::read_to_string(&path).map_err(|error| {
                location.error(format!(
                    "cannot read XML file `{}`: {error}",
                    path.display()
                ))
            })?;
            let source = Source::parse(&text, Some(path.clone()), &key, self.span)?;
            self.dependencies.push(path);
            self.sources.insert(key.clone(), source);
        }
        Ok(key)
    }
    fn include(&mut self, source: &str, node: &Element) -> syn::Result<(String, String, Element)> {
        let key = if node.attributes.contains_key("file") {
            let path = node.attribute("file")?;
            let base = match self.sources[source].path.as_ref().and_then(|p| p.parent()) {
                Some(base) => base.to_path_buf(),
                None => PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").map_err(|_| {
                    node.location
                        .error("CARGO_MANIFEST_DIR is unavailable for relative XML includes")
                })?),
            };
            self.load(&base.join(path), &node.location)?
        } else {
            source.to_owned()
        };
        let id = node.attribute("refid")?.to_owned();
        let fragment = self.sources[&key]
            .fragments
            .get(&id)
            .cloned()
            .ok_or_else(|| {
                node.location.error(format!(
                    "missing <sql id=\"{id}\"> include target in `{key}`"
                ))
            })?;
        Ok((key, id, fragment))
    }
    fn validate_includes(
        &mut self,
        source: &str,
        node: &Element,
        stack: &mut Vec<(String, String)>,
        checked: &mut HashSet<(String, String)>,
    ) -> syn::Result<()> {
        for child in &node.children {
            let Content::Element(child) = child else {
                continue;
            };
            if child.tag == "include" {
                let (key, id, fragment) = self.include(source, child)?;
                let target = (key, id);
                if stack.contains(&target) {
                    let mut chain: Vec<_> = stack
                        .iter()
                        .map(|(file, id)| format!("{file}#{id}"))
                        .collect();
                    chain.push(format!("{}#{}", target.0, target.1));
                    return Err(child
                        .location
                        .error(format!("recursive SQL include: {}", chain.join(" -> "))));
                }
                if !checked.contains(&target) {
                    stack.push(target.clone());
                    self.validate_includes(&target.0, &fragment, stack, checked)?;
                    stack.pop();
                    checked.insert(target);
                }
            } else {
                self.validate_includes(source, child, stack, checked)?;
            }
        }
        Ok(())
    }
    fn validate_all_includes(&mut self) -> syn::Result<()> {
        let mut seen_sources = HashSet::new();
        let mut checked = HashSet::new();
        loop {
            let pending: Vec<_> = self
                .sources
                .keys()
                .filter(|key| !seen_sources.contains(*key))
                .cloned()
                .collect();
            if pending.is_empty() {
                break;
            }
            for key in pending {
                seen_sources.insert(key.clone());
                let source = &self.sources[&key];
                let nodes: Vec<_> = source
                    .statements
                    .iter()
                    .chain(source.fragments.iter())
                    .map(|(id, node)| (id.clone(), node.clone()))
                    .chain(
                        source
                            .direct
                            .iter()
                            .map(|node| ("<inline>".to_owned(), node.clone())),
                    )
                    .collect();
                for (id, node) in nodes {
                    self.validate_includes(
                        &key,
                        &node,
                        &mut vec![(key.clone(), id)],
                        &mut checked,
                    )?;
                }
            }
        }
        Ok(())
    }
    fn expanded_children(&mut self, source: &str, node: &Element) -> syn::Result<Vec<Content>> {
        let mut expanded = Vec::new();
        for child in &node.children {
            if let Content::Element(include) = child {
                if include.tag == "include" {
                    let (key, _, fragment) = self.include(source, include)?;
                    expanded.extend(self.expanded_children(&key, &fragment)?);
                    continue;
                }
            }
            expanded.push(child.clone());
        }
        Ok(expanded)
    }

    fn children(
        &mut self,
        source: &str,
        node: &Element,
        builder: &Ident,
    ) -> syn::Result<TokenStream> {
        let mut output = TokenStream::new();
        let mut lexer = SqlLexer::default();
        let mut text = SqlText::default();
        // Static fragments and neighboring text/CDATA form one SQL input.
        // Lexing them independently would miss delimiters split by XML, such
        // as "-<include .../>" becoming a line comment or a dollar quote.
        for child in self.expanded_children(source, node)? {
            match child {
                Content::Text(value, location) => text.push(&value, location),
                Content::Element(child) => {
                    output.extend(lexer.compile(&text, builder)?);
                    text = SqlText::default();
                    if !matches!(lexer.state, SqlState::Code) {
                        return Err(child.location.error("XML SQL elements must start outside SQL strings, quoted identifiers, and comments; use CDATA for literal markup"));
                    }
                    // Dynamic branches must not join neighboring SQL tokens
                    // into comments, dollar quotes, or escaped string prefixes.
                    // Keep this boundary even for an empty branch or a <bind>:
                    // both split the statically lexed SQL input. Static includes
                    // were flattened above and intentionally have no boundary.
                    output.extend(quote!(#builder.boundary();));
                    // Included dynamic nodes retain the file they came from,
                    // including for nested relative includes and diagnostics.
                    output.extend(self.element(&child.location.source, &child, builder)?);
                    output.extend(quote!(#builder.boundary();));
                }
            }
        }
        output.extend(lexer.compile(&text, builder)?);
        match lexer.state {
            SqlState::Code => {},
            // Keep surrounding SQL and generated separators out of a final comment.
            SqlState::LineComment => output.extend(quote!(#builder.push_sql("\n");)),
            _ => return Err(node.location.error("unclosed SQL string, quoted identifier, dollar quote, or block comment in XML element")),
        }
        Ok(output)
    }
    fn element(
        &mut self,
        source: &str,
        node: &Element,
        builder: &Ident,
    ) -> syn::Result<TokenStream> {
        match node.tag.as_str() {
            "if" => {
                let test = node.expression("test")?;
                let body = self.children(source, node, builder)?;
                Ok(quote!(if #test { #body }))
            }
            "choose" => {
                let mut output = TokenStream::new();
                let mut first = true;
                for child in &node.children {
                    if let Content::Element(branch) = child {
                        let body = self.children(source, branch, builder)?;
                        if branch.tag == "when" {
                            let test = branch.expression("test")?;
                            if first {
                                output.extend(quote!(if #test { #body }));
                                first = false;
                            } else {
                                output.extend(quote!(else if #test { #body }));
                            }
                        } else {
                            output.extend(quote!(else { #body }));
                        }
                    }
                }
                Ok(output)
            }
            "where" | "set" | "trim" => {
                let child = self.fresh("trim");
                let body = self.children(source, node, &child)?;
                let (prefix, suffix, prefix_overrides, suffix_overrides) = match node.tag.as_str() {
                    "where" => ("WHERE", "", "AND|OR", ""),
                    "set" => ("SET", "", "", ","),
                    _ => (
                        node.optional("prefix"),
                        node.optional("suffix"),
                        node.optional("prefixOverrides"),
                        node.optional("suffixOverrides"),
                    ),
                };
                let runtime = &self.runtime;
                let empty = if node.tag == "set" {
                    quote!(if !#builder.append_trimmed(#child, #prefix, #suffix, #prefix_overrides, #suffix_overrides) { return ::core::result::Result::Err(#runtime::Error::EmptySet); })
                } else {
                    quote!(#builder.append_trimmed(#child, #prefix, #suffix, #prefix_overrides, #suffix_overrides);)
                };
                Ok(quote!({ let mut #child = #builder.child(); #body #empty }))
            }
            "foreach" => {
                let collection = node.expression("collection")?;
                let item = node.identifier("item")?;
                let index = if node.attributes.contains_key("index") {
                    node.identifier("index")?
                } else {
                    self.fresh("index")
                };
                let borrowed_collection = self.fresh("collection");
                let whole = self.fresh("foreach");
                let iteration = self.fresh("item");
                let emitted = self.fresh("emitted");
                let body = self.children(source, node, &iteration)?;
                let open = node.optional("open");
                let close = node.optional("close");
                let separator = node.optional("separator");
                let runtime = &self.runtime;
                let empty = if node.optional("empty") == "skip" {
                    quote!()
                } else {
                    let name = node.attribute("collection")?;
                    quote!(else { return ::core::result::Result::Err(#runtime::Error::EmptyCollection(#name)); })
                };
                Ok(quote!({
                    let mut #whole = #builder.child();
                    let mut #emitted = false;
                    let #borrowed_collection = &(#collection);
                    for (#index, #item) in #borrowed_collection.iter().enumerate() {
                        let mut #iteration = #whole.child();
                        #body
                        if !#iteration.is_empty() {
                            if #emitted {
                                #whole.boundary();
                                #whole.push_sql(#separator);
                                #whole.boundary();
                            }
                            #whole.boundary();
                            #whole.append(#iteration);
                            #whole.boundary();
                            #emitted = true;
                        }
                    }
                    if #emitted { #builder.append_trimmed(#whole, #open, #close, "", ""); } #empty
                }))
            }
            "bind" => {
                let name = node.identifier("name")?;
                let value = node.expression("value")?;
                Ok(quote!(let #name = (#value);))
            }
            "include" => {
                let (key, _, fragment) = self.include(source, node)?;
                self.children(&key, &fragment, builder)
            }
            _ => Err(node
                .location
                .error(format!("unexpected <{}> outside its container", node.tag))),
        }
    }
}
/// Concatenated static SQL with its original XML source locations. Offsets are
/// relative to decoded text, while file and starting line/column are retained.
#[derive(Default)]
struct SqlText {
    value: String,
    origins: Vec<(usize, Location)>,
}

impl SqlText {
    fn push(&mut self, text: &str, location: Location) {
        if !text.is_empty() {
            self.origins.push((self.value.len(), location));
            self.value.push_str(text);
        }
    }

    fn location(&self, offset: usize) -> Location {
        let (start, location) = self
            .origins
            .iter()
            .rev()
            .find(|(start, _)| *start <= offset)
            .expect("SQL token offsets have an originating XML text node");
        location.offset(&self.value[*start..], offset - start)
    }
}

#[derive(Default)]
struct SqlLexer {
    state: SqlState,
}
#[derive(Default)]
enum SqlState {
    #[default]
    Code,
    Single {
        escape: bool,
    },
    Double,
    Dollar(String),
    LineComment,
    Block {
        depth: usize,
    },
}
fn identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte >= 128
}
impl SqlLexer {
    fn compile(&mut self, sql: &SqlText, builder: &Ident) -> syn::Result<TokenStream> {
        let text = &sql.value;
        let bytes = text.as_bytes();
        let mut index = 0;
        let mut plain_start = 0;
        let mut output = TokenStream::new();
        while index < bytes.len() {
            let byte = bytes[index];
            let next = bytes.get(index + 1).copied();
            match &mut self.state {
                SqlState::Code => {
                    if matches!(byte, b'#' | b'$') && next == Some(b'{') {
                        if plain_start < index {
                            let plain = &text[plain_start..index];
                            output.extend(quote!(#builder.push_sql(#plain);));
                        }
                        let (expression, end) = placeholder(text, index, &sql.location(index))?;
                        if byte == b'#' {
                            output.extend(quote! {
                                #[allow(clippy::clone_on_copy, noop_method_call)]
                                { #builder.bind((#expression).clone()); }
                            });
                        } else {
                            output.extend(quote!(#builder.push_identifier(&(#expression));));
                        }
                        index = end;
                        plain_start = index;
                        continue;
                    }
                    if byte == b'\'' {
                        let escape = index > 0
                            && matches!(bytes[index - 1], b'e' | b'E')
                            && (index < 2 || !identifier_byte(bytes[index - 2]));
                        self.state = SqlState::Single { escape };
                    } else if byte == b'"' {
                        self.state = SqlState::Double;
                    } else if byte == b'-' && next == Some(b'-') {
                        self.state = SqlState::LineComment;
                        index += 1;
                    } else if byte == b'/' && next == Some(b'*') {
                        self.state = SqlState::Block { depth: 1 };
                        index += 1;
                    } else if byte == b'$' && (index == 0 || !identifier_byte(bytes[index - 1])) {
                        if next.is_some_and(|c| c.is_ascii_digit()) {
                            return Err(sql.location(index).error("raw PostgreSQL $n placeholders are unsupported in XML; use #{expression} for typed parameters"));
                        }
                        if let Some(delimiter) = dollar_delimiter(&text[index..]) {
                            index += delimiter.len() - 1;
                            self.state = SqlState::Dollar(delimiter.to_owned());
                        }
                    }
                }
                SqlState::Single { escape } => {
                    if *escape && byte == b'\\' && next.is_some() {
                        index += 1;
                    } else if byte == b'\'' {
                        if next == Some(b'\'') {
                            index += 1;
                        } else {
                            self.state = SqlState::Code;
                        }
                    }
                }
                SqlState::Double => {
                    if byte == b'"' {
                        if next == Some(b'"') {
                            index += 1;
                        } else {
                            self.state = SqlState::Code;
                        }
                    }
                }
                SqlState::Dollar(delimiter) => {
                    if byte == b'$' && text[index..].starts_with(delimiter.as_str()) {
                        index += delimiter.len() - 1;
                        self.state = SqlState::Code;
                    }
                }
                SqlState::LineComment => {
                    if byte == b'\n' {
                        self.state = SqlState::Code;
                    }
                }
                SqlState::Block { depth } => {
                    if byte == b'/' && next == Some(b'*') {
                        *depth += 1;
                        index += 1;
                    } else if byte == b'*' && next == Some(b'/') {
                        *depth -= 1;
                        if *depth == 0 {
                            self.state = SqlState::Code;
                        }
                        index += 1;
                    }
                }
            }
            index += 1;
        }
        if plain_start < text.len() {
            let plain = &text[plain_start..];
            output.extend(quote!(#builder.push_sql(#plain);));
        }
        Ok(output)
    }
}
fn dollar_delimiter(text: &str) -> Option<&str> {
    let mut characters = text.char_indices();
    if characters.next()?.1 != '$' {
        return None;
    }
    let (offset, first) = characters.next()?;
    if first == '$' {
        return Some(&text[..offset + 1]);
    }
    // PostgreSQL accepts non-ASCII identifier characters in dollar-quote tags.
    // Work in characters to avoid slicing inside UTF-8 when a tag is invalid.
    if first != '_' && !first.is_ascii_alphabetic() && first.is_ascii() {
        return None;
    }
    for (offset, character) in characters {
        if character == '$' {
            return Some(&text[..offset + 1]);
        }
        if character != '_' && !character.is_ascii_alphanumeric() && character.is_ascii() {
            return None;
        }
    }
    None
}

fn placeholder(text: &str, start: usize, location: &Location) -> syn::Result<(Expr, usize)> {
    let expression_start = start + 2;
    let mut last_error = None;
    // Let syn find a complete expression, including nested blocks and braces in
    // strings, raw strings, macros, and closures. No handwritten Rust parser.
    for (offset, byte) in text.as_bytes()[expression_start..].iter().enumerate() {
        if *byte != b'}' {
            continue;
        }
        let end = expression_start + offset;
        match syn::parse_str::<Expr>(&text[expression_start..end]) {
            Ok(expression) => return Ok((expression, end + 1)),
            Err(error) => last_error = Some(error),
        }
    }
    match last_error {
        Some(error) => Err(location.error(format!("invalid Rust placeholder expression: {error}"))),
        None => Err(location.error("unclosed SQL placeholder; expected `}`")),
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReturnKind {
    Many,
    Optional,
    One,
    Affected,
    Unit,
    Bound,
}
fn return_kind(function: &ItemFn) -> syn::Result<ReturnKind> {
    let ReturnType::Type(_, output) = &function.sig.output else {
        return Err(syn::Error::new(
            function.sig.span(),
            "html_sql functions must return pgorm::Result<T>",
        ));
    };
    let Type::Path(result) = output.as_ref() else {
        return Err(syn::Error::new_spanned(
            output,
            "html_sql functions must return pgorm::Result<T>",
        ));
    };
    let segment = result.path.segments.last().expect("type path is nonempty");
    if segment.ident != "Result" {
        return Err(syn::Error::new_spanned(
            output,
            "html_sql functions must return pgorm::Result<T>",
        ));
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(syn::Error::new_spanned(
            output,
            "Result requires an output type",
        ));
    };
    let Some(GenericArgument::Type(inner)) = arguments.args.first() else {
        return Err(syn::Error::new_spanned(
            output,
            "Result requires an output type",
        ));
    };
    if matches!(inner, Type::Tuple(tuple) if tuple.elems.is_empty()) {
        return Ok(ReturnKind::Unit);
    }
    if let Type::Path(path) = inner {
        return Ok(
            match path
                .path
                .segments
                .last()
                .expect("type path is nonempty")
                .ident
                .to_string()
                .as_str()
            {
                "Vec" => ReturnKind::Many,
                "Option" => ReturnKind::Optional,
                "u64" => ReturnKind::Affected,
                "BoundSql" => ReturnKind::Bound,
                _ => ReturnKind::One,
            },
        );
    }
    Ok(ReturnKind::One)
}
fn check_body(function: &ItemFn) -> syn::Result<()> {
    let empty = function.block.stmts.is_empty();
    let todo = function.block.stmts.len() == 1
        && match &function.block.stmts[0] {
            Stmt::Expr(Expr::Macro(expression), _) => expression.mac.path.is_ident("todo"),
            Stmt::Macro(statement) => statement.mac.path.is_ident("todo"),
            _ => false,
        };
    if !empty && !todo {
        return Err(syn::Error::new_spanned(
            &function.block,
            "html_sql replaces the function body; use an empty body or todo!(), and move custom logic outside this function",
        ));
    }
    Ok(())
}
pub(super) fn expand(attributes: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    expand_function(
        syn::parse2(attributes)?,
        syn::parse2(input)?,
        super::runtime_path()?,
    )
}
fn expand_function(
    arguments: Arguments,
    mut function: ItemFn,
    runtime: TokenStream,
) -> syn::Result<TokenStream> {
    check_body(&function)?;
    let kind = return_kind(&function)?;
    let is_async = function.sig.asyncness.is_some();
    if is_async && kind == ReturnKind::Bound {
        return Err(syn::Error::new_spanned(
            &function.sig.output,
            "BoundSql rendering functions must be synchronous",
        ));
    }
    if !is_async && kind != ReturnKind::Bound {
        return Err(syn::Error::new_spanned(
            &function.sig.output,
            "SQL execution functions must be async; synchronous functions must return Result<BoundSql<'a>>",
        ));
    }
    if function.sig.constness.is_some()
        || function.sig.variadic.is_some()
        || function.sig.abi.is_some()
        || function.sig.unsafety.is_some()
    {
        return Err(syn::Error::new_spanned(
            &function.sig,
            "html_sql supports ordinary safe Rust functions",
        ));
    }
    let mut parameters = Vec::new();
    for argument in &function.sig.inputs {
        let FnArg::Typed(argument) = argument else {
            return Err(syn::Error::new_spanned(
                argument,
                "html_sql requires a free function with a named executor argument, not self",
            ));
        };
        let Pat::Ident(pattern) = argument.pat.as_ref() else {
            return Err(syn::Error::new_spanned(
                &argument.pat,
                "html_sql arguments must use simple names",
            ));
        };
        if pattern.subpat.is_some() || pattern.by_ref.is_some() {
            return Err(syn::Error::new_spanned(
                &argument.pat,
                "html_sql arguments must use simple names",
            ));
        }
        if pattern
            .ident
            .to_string()
            .trim_start_matches("r#")
            .starts_with("__pgorm_")
        {
            return Err(syn::Error::new_spanned(
                &pattern.ident,
                "names starting with `__pgorm_` are reserved for generated code",
            ));
        }
        parameters.push(pattern.ident.clone());
    }
    let executor = if is_async {
        Some(
            parameters
                .first()
                .ok_or_else(|| {
                    syn::Error::new_spanned(
                        &function.sig,
                        "async html_sql requires an executor as its first argument",
                    )
                })?
                .clone(),
        )
    } else {
        None
    };
    let span = arguments.source.span();
    let source_text = arguments.source.value();
    let mut compiler = Compiler {
        runtime: runtime.clone(),
        sources: HashMap::new(),
        dependencies: Vec::new(),
        span,
        next_name: 0,
    };
    let location = Location {
        source: "html_sql".to_owned(),
        line: 1,
        column: 1,
        span,
    };
    let key = if source_text.trim_start().starts_with('<') {
        let key = "<inline XML>".to_owned();
        compiler
            .sources
            .insert(key.clone(), Source::parse(&source_text, None, &key, span)?);
        key
    } else {
        let base = std::env::var("CARGO_MANIFEST_DIR")
            .map_err(|_| location.error("CARGO_MANIFEST_DIR is unavailable"))?;
        compiler.load(&Path::new(&base).join(&source_text), &location)?
    };
    let id = arguments
        .id
        .as_ref()
        .map(LitStr::value)
        .unwrap_or_else(|| function.sig.ident.to_string());
    let source = &compiler.sources[&key];
    let statement = source
        .statements
        .get(&id)
        .or_else(|| {
            source
                .direct
                .as_ref()
                .filter(|node| !node.attributes.contains_key("id") && arguments.id.is_none())
        })
        .cloned()
        .ok_or_else(|| {
            syn::Error::new(span, format!("{key}:1:1: missing SQL statement id `{id}`"))
        })?;
    if statement.tag == "select" && matches!(kind, ReturnKind::Affected | ReturnKind::Unit) {
        return Err(statement.location.error("<select> must return row data (T, Vec<T>, or Option<T>); u64 and () are reserved for affected-row mutation results"));
    }
    compiler.validate_all_includes()?;
    let builder = format_ident!("__pgorm_sql");
    let body = compiler.children(&key, &statement, &builder)?;
    let dependencies: Vec<_> = compiler
        .dependencies
        .iter()
        .map(|path| LitStr::new(&path.to_string_lossy(), span))
        .collect();
    let finish = if let Some(executor) = executor {
        let invoke = match kind {
            ReturnKind::Many => {
                quote!(#runtime::Executor::fetch_all(#executor, __pgorm_query.sql(), &__pgorm_params).await)
            }
            ReturnKind::Optional => {
                quote!(#runtime::Executor::fetch_optional(#executor, __pgorm_query.sql(), &__pgorm_params).await)
            }
            ReturnKind::One => {
                quote!(#runtime::Executor::fetch_one(#executor, __pgorm_query.sql(), &__pgorm_params).await)
            }
            ReturnKind::Affected => {
                quote!(#runtime::Executor::execute(#executor, __pgorm_query.sql(), &__pgorm_params).await)
            }
            ReturnKind::Unit => {
                quote!(#runtime::Executor::execute(#executor, __pgorm_query.sql(), &__pgorm_params).await.map(|_| ()))
            }
            ReturnKind::Bound => unreachable!(),
        };
        quote!(let __pgorm_query = #builder.finish()?; let __pgorm_params = __pgorm_query.params(); #invoke)
    } else {
        quote!(#builder.finish())
    };
    function.block = syn::parse2(quote!({
        #(const _: &str = include_str!(#dependencies);)*
        let mut #builder = #runtime::SqlBuilder::new();
        #body
        #finish
    }))?;
    Ok(quote!(#function))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn expand_test(xml: &str) -> syn::Result<TokenStream> {
        expand_function(
            Arguments {
                source: LitStr::new(xml, Span::call_site()),
                id: None,
            },
            syn::parse_quote!(
                fn query(value: i64) -> pgorm::Result<pgorm::BoundSql<'static>> {}
            ),
            quote!(::pgorm),
        )
    }
    fn error(xml: &str) -> String {
        expand_test(xml).unwrap_err().to_string()
    }
    #[test]
    fn rejects_duplicate_ids_and_unknown_tags_with_positions() {
        let duplicate = error(
            "<mapper>\n<select id=\"query\">SELECT 1</select><sql id=\"query\">x</sql></mapper>",
        );
        assert!(duplicate.contains(":2:"), "{duplicate}");
        assert!(duplicate.contains("duplicate XML id"), "{duplicate}");
        assert!(error("<select><unknown/></select>").contains("unknown SQL element"));
        assert!(error("<select><if text=\"true\">x</if></select>").contains("unknown attribute"));
    }
    #[test]
    fn rejects_missing_and_recursive_fragments() {
        assert!(error("<select><include refid=\"missing\"/></select>").contains("missing <sql"));
        let recursive = error(
            "<mapper><select id=\"query\"><include refid=\"a\"/></select><sql id=\"a\"><include refid=\"b\"/></sql><sql id=\"b\"><include refid=\"a\"/></sql></mapper>",
        );
        assert!(recursive.contains("recursive SQL include"), "{recursive}");
    }
    #[test]
    fn rejects_dtds_and_invalid_choose() {
        assert!(error("<!DOCTYPE mapper [<!ENTITY x 'sql'>]><mapper/>").contains("invalid XML"));
        assert!(
            error("<select><choose><otherwise>x</otherwise></choose></select>")
                .contains("at least one")
        );
        assert!(error("<select><when test=\"true\">x</when></select>").contains("immediate child"));
    }
    #[test]
    fn requires_closed_literals_and_typed_placeholders() {
        assert!(error("<select>SELECT '${value}</select>").contains("unclosed SQL"));
        assert!(
            error("<select>SELECT '<if test=\"true\">x</if>'</select>")
                .contains("outside SQL strings")
        );
        assert!(error("<select>SELECT $1</select>").contains("raw PostgreSQL"));
        assert!(error("<select>SELECT #{}</select>").contains("invalid Rust placeholder"));
        assert!(error("<select>SELECT #{value</select>").contains("unclosed SQL placeholder"));
    }
    #[test]
    fn lexer_preserves_sql_literals_comments_and_nested_expression_braces() {
        let tokens = expand_test(r#"<select><![CDATA[SELECT '#{x}', "${x}", $$#{x}$$, $tag$${x}$tag$, /* #{x} /* ${x} */ */ #{format!("{}", value)} -- #{x}
]]></select>"#).unwrap().to_string();
        assert_eq!(tokens.matches(". bind (").count(), 1, "{tokens}");
        assert!(tokens.contains("format !"), "{tokens}");
    }
    #[test]
    fn dollar_quote_tags_accept_unicode_without_panicking() {
        assert_eq!(dollar_delimiter("$雪$body$雪$"), Some("$雪$"));
        assert_eq!(dollar_delimiter("$a雪"), None);
        let tokens = expand_test("<select>SELECT $雪$#{unbound}$雪$, #{value}</select>")
            .unwrap()
            .to_string();
        assert_eq!(tokens.matches(". bind (").count(), 1, "{tokens}");
    }

    #[test]
    fn static_fragment_and_xml_text_boundaries_do_not_split_sql_tokens() {
        let tokens = expand_test(r#"<mapper><sql id="dash">-</sql><select id="query">SELECT 1 <include refid="dash"/>- #{missing}
, #{value}, $ta<![CDATA[g$#{missing}$tag$]]>, /<!-- XML -->* #{missing} */</select></mapper>"#)
            .unwrap().to_string();
        assert_eq!(tokens.matches(". bind (").count(), 1, "{tokens}");
    }

    #[test]
    fn concatenated_text_diagnostics_keep_the_fragment_location() {
        let message = error(
            "<mapper>\n<sql id=\"bad\">$1</sql>\n<select id=\"query\">SELECT <include refid=\"bad\"/></select>\n</mapper>",
        );
        assert!(message.contains("<inline XML>:2:15:"), "{message}");
        assert!(message.contains("raw PostgreSQL"), "{message}");
    }

    #[test]
    fn rejects_empty_and_external_dtds_too() {
        assert!(error("<!DOCTYPE select><select>SELECT 1</select>").contains("invalid XML"));
        assert!(
            error("<!DOCTYPE select SYSTEM \"file:///unused.dtd\"><select>SELECT 1</select>")
                .contains("invalid XML")
        );
    }

    #[test]
    fn dynamic_nodes_emit_sql_token_boundaries_even_for_empty_branches_and_bind() {
        let tokens = expand_test(r#"<select>SELECT 10-<if test="true">-</if><bind name="offset" value="value"/>#{offset}</select>"#)
            .unwrap().to_string();
        assert_eq!(tokens.matches(". boundary ()").count(), 4, "{tokens}");
    }

    #[test]
    fn static_includes_remain_lexically_transparent_without_dynamic_boundaries() {
        let tokens = expand_test(r#"<mapper><sql id="dash">-</sql><select id="query">SELECT 1 <include refid="dash"/>- #{ignored}
, #{value}</select></mapper>"#).unwrap().to_string();
        assert_eq!(tokens.matches(". boundary ()").count(), 0, "{tokens}");
        assert_eq!(tokens.matches(". bind (").count(), 1, "{tokens}");
    }

    #[test]
    fn foreach_separators_and_iterations_have_sql_token_boundaries() {
        let tokens = expand_test(r#"<select>SELECT 10<foreach collection="items" item="_item">-</foreach>#{value}</select>"#)
            .unwrap().to_string();
        // Two around the foreach node, two around its separator, and two
        // around every appended iteration, including an empty separator.
        assert_eq!(tokens.matches(". boundary ()").count(), 6, "{tokens}");
    }

    #[test]
    fn validates_body_and_return_dispatch() {
        let arguments = || Arguments {
            source: LitStr::new("<select>SELECT 1</select>", Span::call_site()),
            id: None,
        };
        let custom = expand_function(
            arguments(),
            syn::parse_quote!(
                async fn query(db: &impl pgorm::Executor) -> pgorm::Result<Vec<Row>> {
                    panic!()
                }
            ),
            quote!(::pgorm),
        )
        .unwrap_err();
        assert!(custom.to_string().contains("replaces the function body"));
        let rows = expand_function(
            arguments(),
            syn::parse_quote!(
                async fn query(db: &impl pgorm::Executor) -> pgorm::Result<u64> {}
            ),
            quote!(::pgorm),
        )
        .unwrap_err();
        assert!(rows.to_string().contains("must return row data"));
        let todo = expand_function(
            arguments(),
            syn::parse_quote!(
                async fn query(db: &impl pgorm::Executor) -> pgorm::Result<Vec<Row>> {
                    todo!()
                }
            ),
            quote!(::pgorm),
        )
        .unwrap()
        .to_string();
        assert!(todo.contains("fetch_all"));
    }
    #[test]
    fn accepts_editor_schema_annotation_and_rejects_other_namespaces() {
        expand_test(r#"<mapper xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="mapper.xsd"><select id="query">SELECT #{value}</select></mapper>"#).unwrap();
        assert!(
            error(r#"<select xmlns:x="urn:unknown" x:thing="x">SELECT 1</select>"#)
                .contains("unsupported namespaced attribute")
        );
    }
}
