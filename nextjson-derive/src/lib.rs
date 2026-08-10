//! Zero-dependency derive macros for `nextjson`.
//!
//! Implemented entirely with the standard `proc_macro` API: no `syn`, no
//! `quote`, no `proc-macro2`. The input `TokenStream` is parsed by a
//! hand-written recursive-descent parser into a small AST, and the output is
//! generated as text and re-parsed.

extern crate proc_macro;

use proc_macro::{Delimiter, Ident, Spacing, TokenStream, TokenTree};
use std::str::FromStr;

mod attr;
mod case;
mod de;
mod schema;
mod ser;

pub(crate) use attr::{ContainerAttrs, FieldAttrs, VariantAttrs};

/// Parse a string into a TokenStream.
/// Parse a string into a TokenStream.
pub(crate) fn ts(s: &str) -> TokenStream {
    TokenStream::from_str(s).unwrap_or_else(|e| {
        panic!("nextjson-derive: invalid generated tokens: {e:?}")
    })
}

/// Build a `compile_error!` expansion from a message.
pub(crate) fn err(msg: &str) -> TokenStream {
    ts(&format!("::core::compile_error!({:?})", msg))
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
    pub fn iter(&self) -> Vec<&Field> {
        match self {
            Fields::Unit => Vec::new(),
            Fields::Named(f) | Fields::Unnamed(f) => f.iter().collect(),
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

    if p.eat_ident("where") {
        let mut cur = Vec::new();
        loop {
            match p.peek() {
                Some(TokenTree::Group(g))
                    if matches!(g.delimiter(), Delimiter::Brace | Delimiter::Parenthesis) =>
                {
                    break
                }
                Some(_) => cur.push(p.next().unwrap()),
                None => break,
            }
        }
        for piece in split_top(&cur, ',') {
            let s = join(&piece).trim().to_string();
            if !s.is_empty() && s != ";" {
                generics.where_preds.push(s);
            }
        }
    }

    let data = if !is_enum {
        match p.next() {
            Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
                let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                Data::Struct(Fields::Named(parse_named_fields(&inner)))
            }
            Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
                let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                Data::Struct(Fields::Unnamed(parse_unnamed_fields(&inner)))
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
    };

    Ok(Input {
        ident,
        generics,
        data,
        cattr,
    })
}

fn parse_generics(inner: &[TokenTree]) -> Generics {
    let mut g = Generics::default();
    for item in split_top(inner, ',') {
        if item.is_empty() {
            continue;
        }
        let mut p = P { toks: &item, i: 0 };
        if p.is_punct('\'') {
            p.next();
            let name = p.expect_ident().unwrap_or_default();
            g.params.push(GenericParam {
                kind: ParamKind::Lifetime,
                full: join(&item),
                name: format!("'{name}"),
            });
        } else if p.eat_ident("const") {
            let name = p.expect_ident().unwrap_or_default();
            g.params.push(GenericParam {
                kind: ParamKind::Const,
                full: join(&item),
                name,
            });
        } else {
            let name = p.expect_ident().unwrap_or_default();
            g.params.push(GenericParam {
                kind: ParamKind::Type,
                full: join(&item),
                name,
            });
        }
    }
    g
}

fn parse_named_fields(inner: &[TokenTree]) -> Vec<Field> {
    split_top(inner, ',')
        .iter()
        .filter(|s| !s.is_empty())
        .map(|piece| parse_named_field(piece))
        .collect()
}

fn parse_named_field(piece: &[TokenTree]) -> Field {
    let mut p = P { toks: piece, i: 0 };
    let attrs = parse_attrs(&mut p);
    if p.eat_ident("pub") && p.is_punct('(') {
        p.next();
        p.next();
    }
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
    match colon {
        Some(c) => Field {
            ident: Some(join(&piece[p.i..c]).trim().to_string()),
            ty: join(&piece[c + 1..]).trim().to_string(),
            attrs: collect_metas(&attrs),
        },
        None => Field {
            ident: None,
            ty: join(&piece[p.i..]).trim().to_string(),
            attrs: collect_metas(&attrs),
        },
    }
}

fn parse_unnamed_fields(inner: &[TokenTree]) -> Vec<Field> {
    split_top(inner, ',')
        .iter()
        .filter(|s| !s.is_empty())
        .map(|piece| {
            let mut p = P { toks: piece, i: 0 };
            let attrs = parse_attrs(&mut p);
            if p.eat_ident("pub") && p.is_punct('(') {
                p.next();
                p.next();
            }
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

    let mut preds: Vec<String> = if let Some(bound) = &c.bound {
        let cleaned = bound.trim().trim_matches('"');
        if cleaned.is_empty() {
            Vec::new()
        } else {
            cleaned
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        }
    } else {
        let mut v: Vec<String> = g.where_preds.clone();
        for p in g.params.iter() {
            if p.kind != ParamKind::Type {
                continue;
            }
            if de && has_flatten {
                v.push(format!("{0}: for<'__n> {1}::NsonDeserialize<'__n>", p.name, cp));
            } else if de {
                v.push(format!("{}: {}::NsonDeserialize<'de>", p.name, cp));
            } else {
                v.push(format!("{}: {}::NsonSerialize", p.name, cp));
            }
        }
        v
    };

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

/// Emit the `NsonSchema` + `NsonSerialize` impls.
pub(crate) fn generate_impls(input: &Input) -> TokenStream {
    let cp = input.cattr.crate_path.clone();
    let name = input.ident.clone();
    let (ig, tg, wc) = build_generics(input, &cp, false, false, false);
    let schema_expr = schema::schema_expr(input, &cp);
    let body = match &input.data {
        Data::Struct(f) => ser::serialize_struct(&name, f, input, &cp),
        Data::Enum(v) => ser::serialize_enum(&name, v, input, &cp),
    };
    let out = format!(
        "#[automatically_derived]\n\
         impl {ig} {cp}::NsonSchema for {name}{tg}{wc} {{\n\
         \x20   const SCHEMA: {cp}::TypeSchema = {schema_expr};\n\
         }}\n\
         #[automatically_derived]\n\
         impl {ig} {cp}::NsonSerialize for {name}{tg}{wc} {{\n\
         \x20   fn encode<__W: {cp}::Write>(&self, __e: &mut {cp}::Encoder<__W>) -> {cp}::Result<()> {{\n\
         {body}\n\
         \x20   }}\n\
         }}"
    );
    ts(&out)
}

/// Emit the `NsonDeserialize` impl.
pub(crate) fn generate_de_impl(input: &Input) -> TokenStream {
    let cp = input.cattr.crate_path.clone();
    let name = input.ident.clone();
    let has_flatten = type_has_flag(input, |fa| fa.flatten);
    let has_borrow = type_has_flag(input, |fa| fa.borrow);
    if has_flatten && type_has_with(input) {
        return err("nextjson: `flatten` cannot be combined with `with` / `deserialize_with`");
    }
    let (ig, tg, wc) = build_generics(input, &cp, true, has_flatten, has_borrow);
    let body = match &input.data {
        Data::Struct(f) => de::deserialize_struct(&name, f, input, &cp, has_flatten),
        Data::Enum(v) => de::deserialize_enum(&name, v, input, &cp, has_flatten),
    };
    let out = format!(
        "#[automatically_derived]\n\
         impl {ig} {cp}::NsonDeserialize<'de> for {name}{tg}{wc} {{\n\
         \x20   fn decode_into(\n\
         \x20       __d: &mut {cp}::Decoder<'de>,\n\
         \x20       __out: &mut ::core::mem::MaybeUninit<Self>,\n\
         \x20   ) -> {cp}::Result<()> {{\n\
         {body}\n\
         \x20   }}\n\
         }}"
    );
    ts(&out)
}

fn type_has_flag<F: Fn(&FieldAttrs) -> bool>(input: &Input, f: F) -> bool {
    match &input.data {
        Data::Struct(fields) => fields.iter().iter().any(|fld| f(&attr::field_attrs(&fld.attrs))),
        Data::Enum(variants) => variants
            .iter()
            .any(|v| v.fields.iter().iter().any(|fld| f(&attr::field_attrs(&fld.attrs)))),
    }
}

fn type_has_with(input: &Input) -> bool {
    type_has_flag(input, |fa| fa.with.is_some() || fa.deserialize_with.is_some())
}

/// Build a `proc_macro::Ident` (kept for API symmetry).
#[allow(dead_code)]
pub(crate) fn ident(name: &str) -> Ident {
    Ident::new(name, proc_macro::Span::call_site())
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

#[proc_macro_derive(NsonSerialize, attributes(njson, nextjson))]
pub fn derive_serialize(input: TokenStream) -> TokenStream {
    match parse_input(input) {
        Ok(ast) => generate_impls(&ast),
        Err(e) => err(&e),
    }
}

#[proc_macro_derive(NsonDeserialize, attributes(njson, nextjson))]
pub fn derive_deserialize(input: TokenStream) -> TokenStream {
    match parse_input(input) {
        Ok(ast) => generate_de_impl(&ast),
        Err(e) => err(&e),
    }
}
