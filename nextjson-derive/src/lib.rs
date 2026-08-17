//! Zero-dependency derive macros for `nextjson`.
//!
//! Implemented entirely with the standard `proc_macro` API: no `syn`, no
//! `quote`, no `proc-macro2`. The input `TokenStream` is parsed by a
//! hand-written recursive-descent parser into a small AST, and the output is
//! generated as text and re-parsed.
//!
//! ## Forward compatibility contract
//!
//! A derive macro only ever receives a single item's tokens, and this parser
//! interprets a deliberately **stable grammar subset**: the item header
//! (`struct` / `enum` + name), the generic parameter list, the `where`
//! clause, and the field / variant structure, plus `#[njson]` /
//! `#[nextjson]` / `#[serde]` attributes. Everything inside a field or
//! generic *type position* is carried through verbatim as an opaque token
//! sequence, so new Rust syntax that appears in type positions (new
//! literals, `impl Trait` forms, associated-type paths, ...) needs no parser
//! change — it is round-tripped unchanged.
//!
//! The risk of future item-level grammar changes is handled defensively:
//! `parse_input` requires that **every** input token be consumed. If a future
//! Rust release extends item syntax in a way this parser does not understand,
//! the macro fails with a loud `compile_error!` naming the leftover tokens
//! instead of silently generating impls from a mis-parsed subset.

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![doc(html_root_url = "https://docs.rs/nextjson-derive")]

extern crate proc_macro;

use proc_macro::{Delimiter, Spacing, TokenStream, TokenTree};
use std::str::FromStr;

mod attr;
mod case;
mod de;
mod schema;
mod ser;

pub(crate) use attr::{ContainerAttrs, FieldAttrs, Meta, VariantAttrs};

/// Parse a string into a TokenStream.
pub(crate) fn ts(s: &str) -> TokenStream {
    TokenStream::from_str(s)
        .unwrap_or_else(|e| panic!("nextjson-derive: invalid generated tokens: {e:?}"))
}

/// Build a `compile_error!` expansion from a message.
pub(crate) fn err(msg: &str) -> TokenStream {
    ts(&format!("::core::compile_error!({:?});", msg))
}

/// Error string returned by codegen helpers.
pub(crate) fn err_str(msg: &str) -> String {
    msg.to_string()
}

/// Token cursor over a slice of TokenTrees.
pub(crate) struct P<'a> {
    pub toks: &'a [TokenTree],
    pub i: usize,
}

impl<'a> P<'a> {
    pub fn peek(&self) -> Option<&TokenTree> {
        self.toks.get(self.i)
    }
    pub fn next(&mut self) -> Option<TokenTree> {
        let t = self.toks.get(self.i).cloned();
        if t.is_some() {
            self.i += 1;
        }
        t
    }
    pub fn is_ident(&self, s: &str) -> bool {
        matches!(self.peek(), Some(TokenTree::Ident(id)) if id.to_string() == s)
    }
    pub fn is_punct(&self, ch: char) -> bool {
        matches!(self.peek(), Some(TokenTree::Punct(p)) if p.as_char() == ch)
    }
    pub fn eat_ident(&mut self, s: &str) -> bool {
        if self.is_ident(s) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    pub fn eat_punct(&mut self, ch: char) -> bool {
        if self.is_punct(ch) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    pub fn expect_ident(&mut self) -> Option<String> {
        match self.next() {
            Some(TokenTree::Ident(id)) => Some(id.to_string()),
            _ => None,
        }
    }
}

/// Join tokens into a re-parseable string, preserving `Joint` spacing so
/// that punctuation sequences (`::`, `'a`, `->`, `>>`) stay adjacent.
pub(crate) fn join(toks: &[TokenTree]) -> String {
    let mut s = String::new();
    let mut no_space = false;
    for t in toks {
        if !s.is_empty() && !no_space {
            s.push(' ');
        }
        no_space = false;
        match t {
            TokenTree::Punct(p) => {
                s.push_str(&p.to_string());
                no_space = p.spacing() == Spacing::Joint;
            }
            _ => s.push_str(&t.to_string()),
        }
    }
    s
}

/// Split tokens at a top-level separator.
///
/// Angle brackets are tracked so that generic types such as
/// `BTreeMap<String, i32>` stay on a single side of the split.
pub(crate) fn split_top(toks: &[TokenTree], sep: char) -> Vec<Vec<TokenTree>> {
    let mut out: Vec<Vec<TokenTree>> = Vec::new();
    let mut cur: Vec<TokenTree> = Vec::new();
    let mut angle: usize = 0;
    for tt in toks {
        match tt {
            TokenTree::Group(_) => cur.push(tt.clone()),
            TokenTree::Punct(p) if p.as_char() == '<' => {
                angle += 1;
                cur.push(tt.clone());
            }
            TokenTree::Punct(p) if p.as_char() == '>' => {
                angle = angle.saturating_sub(1);
                cur.push(tt.clone());
            }
            TokenTree::Punct(p) if angle == 0 && p.as_char() == sep => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(tt.clone()),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(Vec::new());
    }
    out
}

/// Read a `<...>` group. proc_macro does not group angle brackets, so this
/// scans for the matching `>` while ignoring `->` arrow tokens.
pub(crate) fn read_angle(p: &mut P) -> Option<Vec<TokenTree>> {
    if !p.eat_punct('<') {
        return None;
    }
    let mut depth = 1usize;
    let mut out = Vec::new();
    while let Some(tt) = p.next() {
        match &tt {
            TokenTree::Punct(c) if c.as_char() == '<' => {
                depth += 1;
                out.push(tt);
            }
            TokenTree::Punct(c)
                if c.as_char() == '-'
                    && matches!(p.peek(), Some(TokenTree::Punct(n)) if n.as_char() == '>') =>
            {
                out.push(tt);
                out.push(p.next().unwrap());
            }
            TokenTree::Punct(c) if c.as_char() == '>' => {
                if depth == 1 {
                    return Some(out);
                }
                depth -= 1;
                out.push(tt);
            }
            _ => out.push(tt),
        }
    }
    None
}

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParamKind {
    Lifetime,
    Type,
    Const,
}

#[derive(Clone)]
pub(crate) struct GenericParam {
    pub kind: ParamKind,
    pub full: String,
    pub name: String,
}

#[derive(Clone, Default)]
pub(crate) struct Generics {
    pub params: Vec<GenericParam>,
    pub where_preds: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct Field {
    pub ident: Option<String>,
    pub ty: String,
    pub attrs: Vec<attr::Meta>,
}

#[derive(Clone)]
pub(crate) enum Fields {
    Unit,
    Named(Vec<Field>),
    Unnamed(Vec<Field>),
}

impl Fields {
    pub fn iter(&self) -> core::slice::Iter<'_, Field> {
        match self {
            Fields::Unit => [].iter(),
            Fields::Named(fields) | Fields::Unnamed(fields) => fields.iter(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Variant {
    pub ident: String,
    pub fields: Fields,
    pub attrs: Vec<attr::Meta>,
}

#[derive(Clone)]
pub(crate) enum Data {
    Struct(Fields),
    Enum(Vec<Variant>),
}

#[derive(Clone)]
pub(crate) struct Input {
    pub ident: String,
    pub generics: Generics,
    pub data: Data,
    pub cattr: ContainerAttrs,
}

// ---------------------------------------------------------------------------
// Attribute collection
// ---------------------------------------------------------------------------

/// Collect leading `#[...]` attribute groups.
fn parse_attrs(p: &mut P) -> Vec<Vec<TokenTree>> {
    let mut out = Vec::new();
    while p.is_punct('#') {
        p.next();
        if let Some(TokenTree::Group(g)) = p.next() {
            if g.delimiter() == Delimiter::Bracket {
                out.push(g.stream().into_iter().collect());
            }
        }
    }
    out
}

/// Extract `njson` / `nextjson` metas from a set of attribute groups.
fn collect_metas(groups: &[Vec<TokenTree>]) -> Vec<attr::Meta> {
    let mut out = Vec::new();
    for g in groups {
        out.extend(attr::metas_from_attr(g));
    }
    out
}

// ---------------------------------------------------------------------------
// Top-level parse
// ---------------------------------------------------------------------------

pub(crate) fn parse_input(input: TokenStream) -> Result<Input, String> {
    let toks: Vec<TokenTree> = input.into_iter().collect();
    let mut p = P { toks: &toks, i: 0 };

    let attrs = parse_attrs(&mut p);
    let cattr = ContainerAttrs::from_metas(&collect_metas(&attrs));
    eat_visibility(&mut p);

    let is_enum = if p.eat_ident("struct") {
        false
    } else if p.eat_ident("enum") {
        true
    } else {
        return Err("nextjson: expected `struct` or `enum`".into());
    };

    let ident = p
        .expect_ident()
        .ok_or_else(|| "nextjson: expected type name".to_string())?;

    let mut generics = Generics::default();
    if let Some(inner) = read_angle(&mut p) {
        generics = parse_generics(&inner);
    }

    let data = if !is_enum
        && matches!(p.peek(), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis)
    {
        let Some(TokenTree::Group(body)) = p.next() else {
            return Err("nextjson: expected a tuple struct body".into());
        };
        if p.eat_ident("where") {
            parse_where_clause(&mut p, &mut generics, false);
        }
        // Tuple structs terminate with `;`; consume it so the trailing-input
        // check below sees a fully parsed item.
        p.eat_punct(';');
        let inner: Vec<TokenTree> = body.stream().into_iter().collect();
        Data::Struct(Fields::Unnamed(parse_unnamed_fields(&inner)))
    } else {
        if p.eat_ident("where") {
            parse_where_clause(&mut p, &mut generics, true);
        }
        if !is_enum {
            match p.next() {
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
                    let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                    Data::Struct(Fields::Named(parse_named_fields(&inner)))
                }
                Some(TokenTree::Punct(pc)) if pc.as_char() == ';' => Data::Struct(Fields::Unit),
                _ => return Err("nextjson: expected a struct body".into()),
            }
        } else {
            match p.next() {
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
                    let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                    Data::Enum(parse_variants(&inner))
                }
                _ => return Err("nextjson: expected an enum body".into()),
            }
        }
    };

    // The derive input must be a single item. Any tokens left over mean the
    // hand-written parser did not understand part of the declaration; refuse
    // to generate code from a silently mis-parsed subset (this is the
    // forward-compatibility guard: if a future Rust release extends item
    // syntax, the macro fails loudly instead of emitting wrong impls).
    if p.i != toks.len() {
        return Err(format!(
            "nextjson: cannot parse trailing tokens: {}",
            join(&toks[p.i..])
        ));
    }

    Ok(Input {
        ident,
        generics,
        data,
        cattr,
    })
}

fn parse_where_clause(p: &mut P<'_>, generics: &mut Generics, has_braced_body: bool) {
    let mut tokens = Vec::new();
    while let Some(token) = p.peek() {
        let is_body = has_braced_body
            && p.i + 1 == p.toks.len()
            && matches!(token, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace);
        if is_body || matches!(token, TokenTree::Punct(punct) if punct.as_char() == ';') {
            break;
        }
        if let Some(token) = p.next() {
            tokens.push(token);
        }
    }
    for piece in split_top(&tokens, ',') {
        let predicate = join(&piece).trim().to_string();
        if !predicate.is_empty() {
            generics.where_preds.push(predicate);
        }
    }
}

fn parse_generics(inner: &[TokenTree]) -> Generics {
    let mut g = Generics::default();
    for item in split_top(inner, ',') {
        if item.is_empty() {
            continue;
        }
        let declaration = strip_generic_default(&item);
        let mut p = P {
            toks: &declaration,
            i: 0,
        };
        if p.is_punct('\'') {
            p.next();
            let name = p.expect_ident().unwrap_or_default();
            g.params.push(GenericParam {
                kind: ParamKind::Lifetime,
                full: join(&declaration),
                name: format!("'{name}"),
            });
        } else if p.eat_ident("const") {
            let name = p.expect_ident().unwrap_or_default();
            g.params.push(GenericParam {
                kind: ParamKind::Const,
                full: join(&declaration),
                name,
            });
        } else {
            let name = p.expect_ident().unwrap_or_default();
            g.params.push(GenericParam {
                kind: ParamKind::Type,
                full: join(&declaration),
                name,
            });
        }
    }
    g
}

fn strip_generic_default(tokens: &[TokenTree]) -> Vec<TokenTree> {
    let mut angle_depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            TokenTree::Punct(punct) if punct.as_char() == '<' => angle_depth += 1,
            TokenTree::Punct(punct) if punct.as_char() == '>' => {
                angle_depth = angle_depth.saturating_sub(1);
            }
            TokenTree::Punct(punct) if punct.as_char() == '=' && angle_depth == 0 => {
                return tokens[..index].to_vec();
            }
            _ => {}
        }
    }
    tokens.to_vec()
}

fn parse_named_fields(inner: &[TokenTree]) -> Vec<Field> {
    split_top(inner, ',')
        .iter()
        .filter(|s| !s.is_empty())
        .map(|piece| parse_named_field(piece))
        .collect()
}

/// Consume an optional `pub` visibility specifier (`pub`, `pub(crate)`,
/// `pub(super)`, `pub(in path)`). In the proc-macro token stream the
/// parenthesized part arrives as a `Group` with `Parenthesis` delimiter, not
/// as a `Punct('(')`, so it must be matched as a group.
pub(crate) fn eat_visibility(p: &mut P<'_>) {
    if !p.eat_ident("pub") {
        return;
    }
    if matches!(p.peek(), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis) {
        p.next();
    }
}

fn parse_named_field(piece: &[TokenTree]) -> Field {
    let mut p = P { toks: piece, i: 0 };
    let attrs = parse_attrs(&mut p);
    eat_visibility(&mut p);
    // Find the field separator ':' at top level, excluding '::'.
    let mut colon = None;
    let mut j = p.i;
    while j < piece.len() {
        match &piece[j] {
            TokenTree::Punct(c) if c.as_char() == ':' => {
                if matches!(piece.get(j + 1), Some(TokenTree::Punct(n)) if n.as_char() == ':') {
                    j += 2;
                    continue;
                }
                colon = Some(j);
                break;
            }
            _ => j += 1,
        }
    }
    let (ident, ty) = match colon {
        Some(c) => (
            Some(join(&piece[p.i..c]).trim().to_string()),
            join(&piece[c + 1..]).trim().to_string(),
        ),
        None => (None, join(&piece[p.i..]).trim().to_string()),
    };
    let mut field_metas = collect_metas(&attrs);
    // `PhantomData` fields are not part of the data model (serde semantics):
    // skip them on serialize and default them on deserialize. Normalizing at
    // parse time keeps every codegen path (ser / de / schema) consistent.
    if crate::schema::is_phantom_data(&ty) {
        field_metas.push(Meta::Flag("skip".to_string()));
    }
    Field {
        ident,
        ty,
        attrs: field_metas,
    }
}

fn parse_unnamed_fields(inner: &[TokenTree]) -> Vec<Field> {
    split_top(inner, ',')
        .iter()
        .filter(|s| !s.is_empty())
        .map(|piece| {
            let mut p = P { toks: piece, i: 0 };
            let attrs = parse_attrs(&mut p);
            eat_visibility(&mut p);
            Field {
                ident: None,
                ty: join(&piece[p.i..]).trim().to_string(),
                attrs: collect_metas(&attrs),
            }
        })
        .collect()
}

fn parse_variants(inner: &[TokenTree]) -> Vec<Variant> {
    split_top(inner, ',')
        .iter()
        .filter(|s| !s.is_empty())
        .map(|piece| {
            let mut p = P { toks: piece, i: 0 };
            let attrs = parse_attrs(&mut p);
            let ident = p.expect_ident().unwrap_or_default();
            let fields = match p.next() {
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
                    let inner2: Vec<TokenTree> = g.stream().into_iter().collect();
                    Fields::Named(parse_named_fields(&inner2))
                }
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
                    let inner2: Vec<TokenTree> = g.stream().into_iter().collect();
                    Fields::Unnamed(parse_unnamed_fields(&inner2))
                }
                _ => Fields::Unit,
            };
            Variant {
                ident,
                fields,
                attrs: collect_metas(&attrs),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Generic helpers for code generation
// ---------------------------------------------------------------------------

/// Build `(impl_generics, ty_generics, where_clause)` for the impl header.
pub(crate) fn build_generics(
    input: &Input,
    cp: &str,
    de: bool,
    has_flatten: bool,
    has_borrow: bool,
) -> (String, String, String) {
    let g = &input.generics;
    let c = &input.cattr;
    let name = input.ident.clone();
    // `remote` implements the traits for an external type; conversion bounds
    // that mention `Self` must refer to that type instead of the mirror. The
    // remote path already carries its generic arguments, while a local type
    // must be written with the mirror's type parameters applied.
    let (self_ty, remote_typed) = match &c.remote {
        Some(r) => (r.clone(), true),
        None => (name.clone(), false),
    };

    let mut impl_params: Vec<String> = g.params.iter().map(|p| p.full.clone()).collect();
    if de {
        impl_params.insert(0, "'de".to_string());
    }
    let impl_generics = if impl_params.is_empty() {
        String::new()
    } else {
        format!("<{}>", impl_params.join(", "))
    };

    let names: Vec<String> = g.params.iter().map(|p| p.name.clone()).collect();
    let ty_generics = if names.is_empty() {
        String::new()
    } else {
        format!("<{}>", names.join(", "))
    };
    // The fully-instantiated `Self` for conversion bounds: for local types the
    // type parameters must be applied (`Dst<T>`), for remote types the path
    // already names them (`external::Foreign<T>`).
    let self_ty_inst = if remote_typed {
        self_ty.clone()
    } else {
        format!("{self_ty}{ty_generics}")
    };

    // The type's own where-clause predicates are ALWAYS required to name the
    // type, so they are kept unconditionally. The `bound` attribute only
    // replaces the *auto-generated per-type-parameter* bounds; serde behaves
    // the same way.
    let mut preds: Vec<String> = g.where_preds.clone();

    let directional = if de {
        c.bound_de.as_ref()
    } else {
        c.bound_ser.as_ref()
    };
    let bound = directional.or(c.bound.as_ref());
    let auto_bound = |p: &GenericParam| -> Option<String> {
        if p.kind != ParamKind::Type {
            return None;
        }
        if de && has_flatten {
            Some(format!(
                "{0}: for<'__n> {1}::NsonDeserialize<'__n>",
                p.name, cp
            ))
        } else if de {
            Some(format!("{}: {}::NsonDeserialize<'de>", p.name, cp))
        } else {
            Some(format!("{}: {}::NsonSerialize", p.name, cp))
        }
    };
    if let Some(bound) = bound {
        let cleaned = bound.trim().trim_matches('"');
        if !cleaned.is_empty() {
            for s in cleaned.split(',') {
                let s = s.trim();
                if !s.is_empty() {
                    preds.push(s.to_string());
                }
            }
        }
    } else {
        for p in g.params.iter() {
            if let Some(b) = auto_bound(p) {
                preds.push(b);
            }
        }
    }

    // Missing-field fallbacks that call `Default::default()` must be able to
    // name the type parameters they fall back on, so every type parameter
    // receives a `Default` bound when any fallback can fire for a generic
    // field. This mirrors serde, which adds `T: Default` for exactly the same
    // attribute combinations.
    if de && de_uses_type_param_default(input) {
        for p in g.params.iter() {
            if p.kind == ParamKind::Type {
                preds.push(format!("{}: ::core::default::Default", p.name));
            }
        }
    }

    // Conversion attributes add their own bounds.
    if de {
        if let Some(from) = &c.from {
            preds.push(format!(
                "{from}: {cp}::NsonDeserialize<'de> + ::core::convert::Into<{self_ty_inst}>"
            ));
        }
        if let Some(from) = &c.try_from {
            preds.push(format!(
                "{from}: {cp}::NsonDeserialize<'de> + ::core::convert::TryInto<{self_ty_inst}>"
            ));
            preds.push(format!(
                "<{from} as ::core::convert::TryInto<{self_ty_inst}>>::Error: ::core::fmt::Display"
            ));
        }
    } else {
        if let Some(into) = &c.into {
            preds.push(format!("{self_ty_inst}: ::core::clone::Clone"));
            preds.push(format!("{self_ty_inst}: ::core::convert::Into<{into}>"));
            preds.push(format!("{into}: {cp}::NsonSerialize"));
            preds.push(format!("{into}: {cp}::NsonSchema"));
        }
    }

    if de && has_borrow {
        for p in g.params.iter() {
            if p.kind == ParamKind::Lifetime {
                preds.push(format!("'de: {}", p.name));
            }
        }
    }

    let where_clause = if preds.is_empty() {
        String::new()
    } else {
        format!(" where {}", preds.join(", "))
    };

    (impl_generics, ty_generics, where_clause)
}

/// Validate attribute combinations that serde rejects at compile time.
///
/// Returns an error message when the combination is invalid. Called by both
/// derive entry points so the rejection is identical regardless of which
/// macro is expanded first.
fn validate_input(input: &Input) -> Option<String> {
    if let Some(name) = input.cattr.unknown.first() {
        return Some(format!(
            "nextjson: unsupported container attribute `{name}`; refusing to ignore wire semantics"
        ));
    }
    fn unknown_field_attribute(fields: &Fields) -> Option<String> {
        for field in fields.iter() {
            if let Some(name) = attr::field_attrs(&field.attrs).unknown.into_iter().next() {
                return Some(format!(
                    "nextjson: unsupported field attribute `{name}`; refusing to ignore wire semantics"
                ));
            }
        }
        None
    }
    match &input.data {
        Data::Struct(fields) => {
            if let Some(message) = unknown_field_attribute(fields) {
                return Some(message);
            }
        }
        Data::Enum(variants) => {
            for variant in variants {
                if let Some(name) = attr::variant_attrs(&variant.attrs)
                    .unknown
                    .into_iter()
                    .next()
                {
                    return Some(format!(
                        "nextjson: unsupported variant attribute `{name}`; refusing to ignore wire semantics"
                    ));
                }
                if let Some(message) = unknown_field_attribute(&variant.fields) {
                    return Some(message);
                }
            }
        }
    }
    // `transparent` is only meaningful on single-field structs.
    if input.cattr.transparent {
        match &input.data {
            Data::Enum(_) => {
                return Some("nextjson: `transparent` is not supported on enums".to_string());
            }
            Data::Struct(Fields::Named(f)) if f.len() != 1 => {
                return Some("nextjson: `transparent` requires exactly one field".to_string());
            }
            Data::Struct(Fields::Unnamed(f)) if f.len() != 1 => {
                return Some("nextjson: `transparent` requires exactly one field".to_string());
            }
            _ => {}
        }
    }
    // `flatten` splices a nested map into the parent object, which is
    // impossible for positional (unnamed) shapes.
    if type_has_flag_on(input, |f, fa| fa.flatten && f.ident.is_none()) {
        return Some(
            "nextjson: `flatten` is not allowed on tuple structs or tuple variants".to_string(),
        );
    }
    // `flatten` + `skip_serializing_if` conflicts: the decision to skip would
    // depend on the flattened value, which serde rejects up front.
    if type_has_flag_on(input, |_f, fa| {
        fa.flatten && fa.skip_serializing_if.is_some()
    }) {
        return Some(
            "nextjson: `flatten` cannot be combined with `skip_serializing_if`".to_string(),
        );
    }
    None
}

/// Like [`type_has_flag`] but the predicate also sees the field (needed to
/// tell named from unnamed fields).
fn type_has_flag_on<F: Fn(&Field, &FieldAttrs) -> bool>(input: &Input, f: F) -> bool {
    let check = |fld: &Field| f(fld, &attr::field_attrs(&fld.attrs));
    match &input.data {
        Data::Struct(fields) => fields.iter().any(check),
        Data::Enum(variants) => variants.iter().any(|v| v.fields.iter().any(check)),
    }
}

/// Emit the `NsonSchema` + `NsonSerialize` impls.
pub(crate) fn generate_impls(input: &Input) -> TokenStream {
    if let Some(msg) = validate_input(input) {
        return err(&msg);
    }
    let cp = input.cattr.crate_path.clone();
    let name = input.ident.clone();
    // `remote` implements the traits for the external type itself. The remote
    // path already names its generic arguments, so the mirror's type-generics
    // are not appended a second time (`Foreign<T><T>` would not parse).
    let (target, use_tg) = match &input.cattr.remote {
        Some(r) => (r.clone(), false),
        None => (name.clone(), true),
    };
    let (ig, tg, wc) = build_generics(input, &cp, false, false, false);
    let tg_part = if use_tg { tg.as_str() } else { "" };
    let body = if let Some(into) = &input.cattr.into {
        // `into = "T"`: serialize by converting `self` to `T` first.
        format!(
            "let __v: {into} = ::core::convert::Into::into(self.clone());\n\
             <{into} as {cp}::NsonSerialize>::nextencode(&__v, __e)"
        )
    } else {
        match &input.data {
            Data::Struct(f) => ser::serialize_struct(&name, f, input, &cp),
            Data::Enum(v) => ser::serialize_enum(&name, v, input, &cp),
        }
    };
    let out = format!(
        "#[automatically_derived]\n\
         impl {ig} {cp}::NsonSchema for {target}{tg_part}{wc} {{\n\
         \x20   const SCHEMA: {cp}::TypeSchema = {schema_expr};\n\
         }}\n\
         #[automatically_derived]\n\
         impl {ig} {cp}::NsonSerialize for {target}{tg_part}{wc} {{\n\
         \x20   fn nextencode<__E: {cp}::FormatEncoder>(&self, __e: &mut __E) -> ::core::result::Result<(), __E::Error> {{\n\
         {body}\n\
         \x20   }}\n\
         }}",
        schema_expr = schema::schema_expr(input, &cp)
    );
    ts(&out)
}

/// Emit the `NsonDeserialize` impl.
pub(crate) fn generate_de_impl(input: &Input) -> TokenStream {
    if let Some(msg) = validate_input(input) {
        return err(&msg);
    }
    let cp = input.cattr.crate_path.clone();
    let name = input.ident.clone();
    // Container-level `default` supplies missing-field values from a `Self`
    // instance, which only makes sense for structs (serde rejects it on
    // enums as well).
    if matches!(&input.data, Data::Enum(_)) && input.cattr.has_default() {
        return err("nextjson: `default` is not supported on enums");
    }
    let has_flatten = type_has_flag(input, |fa| fa.flatten);
    let has_borrow = type_has_flag(input, |fa| fa.borrow);
    if has_flatten && type_has_with(input) {
        return err("nextjson: `flatten` cannot be combined with `with` / `deserialize_with`");
    }
    if has_flatten && input.cattr.deny_unknown_fields {
        // flatten consumes every remaining key, so unknown-field rejection is
        // silently impossible; serde rejects this combination at compile time.
        return err("nextjson: `deny_unknown_fields` cannot be combined with `flatten`");
    }
    let (target, use_tg) = match &input.cattr.remote {
        Some(r) => (r.clone(), false),
        None => (name.clone(), true),
    };
    let (ig, tg, wc) = build_generics(input, &cp, true, has_flatten, has_borrow);
    let tg_part = if use_tg { tg.as_str() } else { "" };
    // `expecting = "..."` overrides the default `type_name`-based description
    // used in type-mismatch and length-mismatch error messages.
    let expecting = match &input.cattr.expecting {
        Some(e) => format!("\n     fn expecting() -> &'static str {{ {:?} }}\n", e),
        None => String::new(),
    };
    let body = if let Some(from) = &input.cattr.from {
        // `from = "T"`: deserialize a `T` then convert into `Self`.
        format!(
            "let __v: {from} = <{from} as {cp}::NsonDeserialize<'de>>::nextdecode(__d)?;\n\
             __out.write(::core::convert::Into::into(__v));\n\
             ::core::result::Result::Ok(())"
        )
    } else if let Some(from) = &input.cattr.try_from {
        // `try_from = "T"`: deserialize a `T` then fallibly convert.
        format!(
            "let __v: {from} = <{from} as {cp}::NsonDeserialize<'de>>::nextdecode(__d)?;\n\
             let __c: Self = ::core::convert::TryInto::try_into(__v).map_err(|__e| {{\n\
             \x20   {cp}::FormatError::custom({cp}::__private::ToString::to_string(&__e))\n\
             }})?;\n\
             __out.write(__c);\n\
             ::core::result::Result::Ok(())"
        )
    } else {
        match &input.data {
            Data::Struct(f) => de::deserialize_struct(&name, f, input, &cp, has_flatten),
            Data::Enum(v) => de::deserialize_enum(&name, v, input, &cp, has_flatten),
        }
    };
    let out = format!(
        "#[automatically_derived]\n\
         impl {ig} {cp}::NsonDeserialize<'de> for {target}{tg_part}{wc} {{\n\
         {expecting}\
         \x20   fn nextdecode_into<__D: {cp}::FormatDecoder<'de>>(\n\
         \x20       __d: &mut __D,\n\
         \x20       __out: &mut {cp}::DecodeSlot<Self>,\n\
         \x20   ) -> ::core::result::Result<(), __D::Error> {{\n\
         \x20       __d.set_expecting(Self::expecting());\n\
         {body}\n\
         \x20   }}\n\
         }}"
    );
    ts(&out)
}

fn type_has_flag<F: Fn(&FieldAttrs) -> bool>(input: &Input, f: F) -> bool {
    match &input.data {
        Data::Struct(fields) => fields.iter().any(|fld| f(&attr::field_attrs(&fld.attrs))),
        Data::Enum(variants) => variants
            .iter()
            .any(|v| v.fields.iter().any(|fld| f(&attr::field_attrs(&fld.attrs)))),
    }
}

/// Whether the generated deserializer can fall back to `Default::default()`
/// for a field whose type is a generic parameter.
///
/// True when the container has a default, when any field has a bare
/// `default` attribute, or when any field is `skip_deserializing` without an
/// explicit `default = "path"` (which would supply its own value). Field-level
/// `default = "path"` does not need the bound. `PhantomData` fields are
/// excluded: `PhantomData<T>: Default` holds for every `T` with no bound.
fn de_uses_type_param_default(input: &Input) -> bool {
    if input.cattr.has_default() {
        return true;
    }
    let mut found = false;
    let mut scan = |f: &Field| {
        if crate::schema::is_phantom_data(&f.ty) {
            return;
        }
        let fa = attr::field_attrs(&f.attrs);
        let bare_default = fa.default == Some(String::new());
        let skip_without_path =
            fa.skip_deserializing && !matches!(&fa.default, Some(d) if !d.is_empty());
        if bare_default || skip_without_path {
            found = true;
        }
    };
    match &input.data {
        Data::Struct(fields) => {
            for f in fields.iter() {
                scan(f);
            }
        }
        Data::Enum(variants) => {
            for v in variants {
                for f in v.fields.iter() {
                    scan(f);
                }
            }
        }
    }
    found
}

fn type_has_with(input: &Input) -> bool {
    type_has_flag(input, |fa| {
        fa.with.is_some() || fa.deserialize_with.is_some()
    })
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

#[proc_macro_derive(NsonSerialize, attributes(njson, nextjson, serde))]
/// Derive NextJson's native serialization contract and compile-time schema.
///
/// Configuration is accepted through `#[njson(...)]` (and, for migration
/// convenience, `#[serde(...)]`). The generated implementation writes
/// directly through `NsonSerialize::nextencode` and exposes
/// `NsonSchema::SCHEMA` without depending on another macro framework.
pub fn derive_serialize(input: TokenStream) -> TokenStream {
    match parse_input(input) {
        Ok(ast) => generate_impls(&ast),
        Err(e) => err(&e),
    }
}

#[proc_macro_derive(NsonDeserialize, attributes(njson, nextjson, serde))]
/// Derive NextJson's native decoding contract.
///
/// Configuration is accepted through `#[njson(...)]` (and, for migration
/// convenience, `#[serde(...)]`). The generated implementation decodes
/// through checked `DecodeSlot` state and uses normal Rust drop semantics for
/// partially initialized fields.
pub fn derive_deserialize(input: TokenStream) -> TokenStream {
    match parse_input(input) {
        Ok(ast) => generate_de_impl(&ast),
        Err(e) => err(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The `proc_macro` API cannot be used in unit tests (it panics outside a
    // macro expansion), so these tests build the small AST by hand and cover
    // the pure logic: attribute parsing, validation, and impl-generics
    // construction. Parser token-level behavior is exercised by the workspace
    // integration tests, which compile real derives.

    fn meta_flag(name: &str) -> Meta {
        Meta::Flag(name.to_string())
    }
    fn meta_named(name: &str, value: &str) -> Meta {
        Meta::Named(name.to_string(), value.to_string())
    }
    fn named_field(ident: &str, ty: &str, metas: Vec<Meta>) -> Field {
        Field {
            ident: Some(ident.to_string()),
            ty: ty.to_string(),
            attrs: metas,
        }
    }
    fn unnamed_field(ty: &str, metas: Vec<Meta>) -> Field {
        Field {
            ident: None,
            ty: ty.to_string(),
            attrs: metas,
        }
    }
    fn cattr(metas: &[Meta]) -> ContainerAttrs {
        ContainerAttrs::from_metas(metas)
    }
    fn struct_named(cattr: ContainerAttrs, fields: Vec<Field>) -> Input {
        Input {
            ident: "S".into(),
            generics: Generics::default(),
            data: Data::Struct(Fields::Named(fields)),
            cattr,
        }
    }
    fn generic_input(
        ident: &str,
        params: Vec<GenericParam>,
        where_preds: Vec<String>,
        data: Data,
        cattr: ContainerAttrs,
    ) -> Input {
        Input {
            ident: ident.to_string(),
            generics: Generics {
                params,
                where_preds,
            },
            data,
            cattr,
        }
    }

    // -- validate_input ------------------------------------------------------

    #[test]
    fn validate_rejects_transparent_enum() {
        let input = Input {
            ident: "E".into(),
            generics: Generics::default(),
            data: Data::Enum(vec![Variant {
                ident: "A".into(),
                fields: Fields::Unit,
                attrs: vec![],
            }]),
            cattr: cattr(&[meta_flag("transparent")]),
        };
        let msg = validate_input(&input).expect("must reject transparent enum");
        assert!(msg.contains("transparent"));
    }

    #[test]
    fn validate_rejects_transparent_multi_field() {
        let input = struct_named(
            cattr(&[meta_flag("transparent")]),
            vec![
                named_field("a", "i32", vec![]),
                named_field("b", "i32", vec![]),
            ],
        );
        assert!(validate_input(&input).is_some());
    }

    #[test]
    fn validate_rejects_flatten_on_tuple() {
        let input = Input {
            ident: "T".into(),
            generics: Generics::default(),
            data: Data::Struct(Fields::Unnamed(vec![unnamed_field(
                "std::collections::BTreeMap<String, i32>",
                vec![meta_flag("flatten")],
            )])),
            cattr: cattr(&[]),
        };
        let msg = validate_input(&input).expect("must reject flatten on tuple field");
        assert!(msg.contains("flatten"), "unexpected error: {msg}");
    }

    #[test]
    fn validate_rejects_flatten_with_skip_if() {
        let input = struct_named(
            cattr(&[]),
            vec![named_field(
                "m",
                "std::collections::BTreeMap<String, i32>",
                vec![
                    meta_flag("flatten"),
                    meta_named("skip_serializing_if", "Option::is_none"),
                ],
            )],
        );
        let msg = validate_input(&input).expect("must reject flatten + skip_serializing_if");
        assert!(msg.contains("flatten"), "unexpected error: {msg}");
    }

    #[test]
    fn validate_rejects_unknown_wire_attributes() {
        let input = struct_named(
            cattr(&[]),
            vec![named_field(
                "a",
                "i32",
                vec![meta_flag("not_actually_supported")],
            )],
        );
        let message = validate_input(&input).expect("unknown attribute must be rejected");
        assert!(message.contains("not_actually_supported"));
        assert!(message.contains("refusing to ignore"));
    }

    #[test]
    fn validate_accepts_valid_combinations() {
        let ok = struct_named(
            cattr(&[]),
            vec![
                named_field("a", "i32", vec![]),
                named_field(
                    "m",
                    "std::collections::BTreeMap<String, i32>",
                    vec![meta_flag("flatten")],
                ),
            ],
        );
        assert!(validate_input(&ok).is_none());

        let w = Input {
            ident: "W".into(),
            generics: Generics::default(),
            data: Data::Struct(Fields::Unnamed(vec![unnamed_field("i32", vec![])])),
            cattr: cattr(&[meta_flag("transparent")]),
        };
        assert!(validate_input(&w).is_none());
    }

    // -- build_generics ------------------------------------------------------

    #[test]
    fn build_generics_keeps_where_clause_with_bound() {
        // `bound` must replace only the auto bounds; the struct's own where
        // clause must always survive.
        let input = generic_input(
            "S",
            vec![GenericParam {
                kind: ParamKind::Type,
                full: "T".into(),
                name: "T".into(),
            }],
            vec!["T: core::fmt::Debug".into()],
            Data::Struct(Fields::Named(vec![named_field("v", "T", vec![])])),
            cattr(&[meta_named("bound", "\"T: Clone\"")]),
        );
        let (_, _, wc) = build_generics(&input, "::nextjson", false, false, false);
        assert!(wc.contains("Clone"), "missing user bound: {wc}");
        assert!(wc.contains("Debug"), "missing struct where clause: {wc}");
    }

    #[test]
    fn build_generics_instantiates_conversion_self_type() {
        // Conversion bounds must name `Dst<T>`, never bare `Dst`.
        let input = generic_input(
            "Dst",
            vec![GenericParam {
                kind: ParamKind::Type,
                full: "T".into(),
                name: "T".into(),
            }],
            vec![],
            Data::Struct(Fields::Named(vec![named_field("x", "T", vec![])])),
            cattr(&[meta_named("from", "\"Src<T>\"")]),
        );
        let (_, _, wc) = build_generics(&input, "::nextjson", true, false, false);
        assert!(
            wc.contains("Into<Dst<T>>"),
            "missing instantiated self: {wc}"
        );
        assert!(!wc.contains("Into<Dst>"), "bare self type: {wc}");
    }

    #[test]
    fn build_generics_adds_default_bound_for_generic_default() {
        // Container `default` on a generic struct must add `T: Default`.
        let input = generic_input(
            "S",
            vec![GenericParam {
                kind: ParamKind::Type,
                full: "T".into(),
                name: "T".into(),
            }],
            vec![],
            Data::Struct(Fields::Named(vec![named_field("v", "T", vec![])])),
            cattr(&[meta_flag("default")]),
        );
        let (_, _, wc) = build_generics(&input, "::nextjson", true, false, false);
        assert!(
            wc.contains("T: ::core::default::Default"),
            "missing Default bound: {wc}"
        );
    }

    #[test]
    fn build_generics_remote_never_appends_generics_twice() {
        // The impl target for `remote` carries its own generic arguments.
        let input = generic_input(
            "Mirror",
            vec![GenericParam {
                kind: ParamKind::Type,
                full: "T".into(),
                name: "T".into(),
            }],
            vec![],
            Data::Struct(Fields::Named(vec![named_field("x", "T", vec![])])),
            cattr(&[meta_named("remote", "external::Foreign<T>")]),
        );
        let (_, _, wc) = build_generics(&input, "::nextjson", false, false, false);
        assert!(
            wc.contains("T: ::nextjson::NsonSerialize"),
            "missing auto bound: {wc}"
        );
    }

    // -- default-bound detection ---------------------------------------------

    #[test]
    fn default_bound_not_needed_for_phantom_data() {
        // A `PhantomData` field must not force `T: Default`.
        let input = struct_named(
            cattr(&[]),
            vec![
                named_field("_m", "core::marker::PhantomData<T>", vec![]),
                named_field("n", "i32", vec![]),
            ],
        );
        assert!(!de_uses_type_param_default(&input));

        // With a container default the bound is required again.
        let input2 = struct_named(
            cattr(&[meta_flag("default")]),
            vec![named_field("v", "T", vec![])],
        );
        assert!(de_uses_type_param_default(&input2));
    }

    // -- PhantomData detection ----------------------------------------------

    #[test]
    fn phantom_detection_spellings() {
        assert!(crate::schema::is_phantom_data("PhantomData<T>"));
        assert!(crate::schema::is_phantom_data(
            "core::marker::PhantomData<T>"
        ));
        assert!(crate::schema::is_phantom_data(
            "::core::marker::PhantomData<T>"
        ));
        assert!(crate::schema::is_phantom_data(
            "std::marker::PhantomData<T>"
        ));
        assert!(!crate::schema::is_phantom_data("Vec<T>"));
        assert!(!crate::schema::is_phantom_data("Option<PhantomData<T>>"));
        assert!(!crate::schema::is_phantom_data("Phantom"));
    }
}
