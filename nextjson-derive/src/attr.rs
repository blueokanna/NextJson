//! Attribute (`#[njson(...)]`) parsing.

use crate::{join, split_top, P};
use proc_macro::{Delimiter, TokenTree};

/// A single `name = value` or bare `name` item inside `#[njson(...)]`.
#[derive(Clone, Debug)]
pub(crate) enum Meta {
    Flag(String),
    Named(String, String),
}

impl Meta {
    pub fn name(&self) -> &str {
        match self {
            Meta::Flag(n) | Meta::Named(n, _) => n,
        }
    }
    pub fn value(&self) -> Option<&str> {
        match self {
            Meta::Named(_, v) => Some(v),
            Meta::Flag(_) => None,
        }
    }
}

/// Extract metas from one attribute group's inner tokens.
///
/// Accepts `#[njson(...)]`, `#[nextjson(...)]`, and `#[serde(...)]` so that
/// existing serde-derived types migrate without rewriting their attributes.
pub(crate) fn metas_from_attr(inner: &[TokenTree]) -> Vec<Meta> {
    let mut p = P { toks: inner, i: 0 };
    if !p.is_ident("njson") && !p.is_ident("nextjson") && !p.is_ident("serde") {
        return Vec::new();
    }
    p.next();
    let Some(TokenTree::Group(g)) = p.next() else {
        return Vec::new();
    };
    if g.delimiter() != Delimiter::Parenthesis {
        return Vec::new();
    }
    let toks: Vec<TokenTree> = g.stream().into_iter().collect();
    split_top(&toks, ',')
        .iter()
        .map(|item| meta_from_item(item))
        .collect()
}

fn meta_from_item(item: &[TokenTree]) -> Meta {
    let mut p = P { toks: item, i: 0 };
    let name = p.expect_ident().unwrap_or_default();
    if p.eat_punct('=') {
        Meta::Named(name, join(&item[p.i..]).trim().to_string())
    } else if let Some(TokenTree::Group(g)) = p.next() {
        // serde's nested form: `bound(serialize = "...", deserialize = "...")`
        // or `rename_all(serialize = "A", deserialize = "B")`. The inner
        // tokens are joined so the caller can split them directionally.
        if g.delimiter() == Delimiter::Parenthesis {
            let inner: Vec<TokenTree> = g.stream().into_iter().collect();
            Meta::Named(name, join(&inner).trim().to_string())
        } else {
            Meta::Flag(name)
        }
    } else {
        Meta::Flag(name)
    }
}

/// Strip surrounding quotes from a string literal token.
fn unquote(s: &str) -> Option<String> {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        Some(t[1..t.len() - 1].to_string())
    } else {
        None
    }
}

/// Container-level attributes.
#[derive(Clone)]
pub(crate) struct ContainerAttrs {
    pub unknown: Vec<String>,
    pub rename_all: Option<String>,
    pub rename_all_ser: Option<String>,
    pub rename_all_de: Option<String>,
    pub rename_all_fields: Option<String>,
    pub rename_all_fields_ser: Option<String>,
    pub rename_all_fields_de: Option<String>,
    pub rename: Option<String>,
    pub tag: Option<String>,
    pub content: Option<String>,
    pub untagged: bool,
    pub transparent: bool,
    pub deny_unknown_fields: bool,
    pub default: bool,
    pub default_path: Option<String>,
    pub bound: Option<String>,
    /// Directional `bound(serialize = "...", deserialize = "...")`.
    pub bound_ser: Option<String>,
    pub bound_de: Option<String>,
    pub crate_path: String,
    /// `#[serde(into = "Type")]`: serialize by converting `&self` first.
    pub into: Option<String>,
    /// `#[serde(from = "Type")]`: deserialize into `Type` then convert.
    pub from: Option<String>,
    /// `#[serde(try_from = "Type")]`: like `from` but fallibly.
    pub try_from: Option<String>,
    /// `#[serde(remote = "Type")]`: implement the traits for an external type.
    pub remote: Option<String>,
    /// `#[serde(expecting = "...")]`: overrides the default type-description
    /// used in deserialization type-mismatch / length-mismatch messages.
    pub expecting: Option<String>,
    /// `#[njson(max_depth = N)]`: maximum container nesting allowed below
    /// this type (schema safety policy).
    pub max_depth: Option<u64>,
}

impl ContainerAttrs {
    pub fn has_default(&self) -> bool {
        self.default || self.default_path.is_some()
    }

    pub fn from_metas(metas: &[Meta]) -> ContainerAttrs {
        let mut c = ContainerAttrs {
            unknown: Vec::new(),
            rename_all: None,
            rename_all_ser: None,
            rename_all_de: None,
            rename_all_fields: None,
            rename_all_fields_ser: None,
            rename_all_fields_de: None,
            rename: None,
            tag: None,
            content: None,
            untagged: false,
            transparent: false,
            deny_unknown_fields: false,
            default: false,
            default_path: None,
            bound: None,
            bound_ser: None,
            bound_de: None,
            crate_path: "::nextjson".to_string(),
            into: None,
            from: None,
            try_from: None,
            remote: None,
            expecting: None,
            max_depth: None,
        };
        for m in metas {
            match m.name() {
                "untagged" => c.untagged = true,
                "transparent" => c.transparent = true,
                "deny_unknown_fields" => c.deny_unknown_fields = true,
                "rename_all" => {
                    if let Some(v) = m.value() {
                        match split_directional(v) {
                            Some((ser, de)) => {
                                c.rename_all_ser = ser;
                                c.rename_all_de = de;
                            }
                            None => c.rename_all = unquote(v),
                        }
                    }
                }
                "rename_all_fields" => {
                    if let Some(v) = m.value() {
                        match split_directional(v) {
                            Some((ser, de)) => {
                                c.rename_all_fields_ser = ser;
                                c.rename_all_fields_de = de;
                            }
                            None => c.rename_all_fields = unquote(v),
                        }
                    }
                }
                "rename" => c.rename = m.value().and_then(unquote),
                "tag" => c.tag = m.value().and_then(unquote),
                "content" => c.content = m.value().and_then(unquote),
                "default" => {
                    if let Some(v) = m.value() {
                        if let Some(q) = unquote(v) {
                            c.default_path = Some(q);
                        } else {
                            c.default = true;
                        }
                    } else {
                        c.default = true;
                    }
                }
                "bound" => {
                    if let Some(v) = m.value() {
                        match split_directional(v) {
                            Some((ser, de)) => {
                                c.bound_ser = ser.map(clean_bound);
                                c.bound_de = de.map(clean_bound);
                            }
                            None => c.bound = unquote(v).map(clean_bound),
                        }
                    }
                }
                "into" => c.into = m.value().and_then(unquote),
                "from" => c.from = m.value().and_then(unquote),
                "try_from" => c.try_from = m.value().and_then(unquote),
                "remote" => c.remote = m.value().and_then(unquote),
                "expecting" => c.expecting = m.value().and_then(unquote),
                "crate" => {
                    if let Some(v) = m.value() {
                        if let Some(q) = unquote(v) {
                            c.crate_path = q;
                        } else if !v.is_empty() {
                            c.crate_path = v.to_string();
                        }
                    }
                }
                "max_depth" => c.max_depth = parse_u64(m.value()),
                _ => c.unknown.push(m.name().to_string()),
            }
        }
        c
    }
}

/// Split a serde directional value of the form
/// `serialize = "A", deserialize = "B"` into its two halves.
///
/// Returns `None` when the value is a plain string (both directions equal).
fn split_directional(v: &str) -> Option<(Option<String>, Option<String>)> {
    let t = v.trim();
    if !(t.contains("serialize") || t.contains("deserialize")) {
        return None;
    }
    let mut ser = None;
    let mut de = None;
    for piece in split_directional_pieces(t) {
        if let Some(rest) = piece.strip_prefix("serialize") {
            ser = Some(parse_directional_value(rest));
        } else if let Some(rest) = piece.strip_prefix("deserialize") {
            de = Some(parse_directional_value(rest));
        }
    }
    Some((ser, de))
}

/// Split a directional attribute body on commas that are not inside quotes.
fn split_directional_pieces(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_quote = false;
    for (i, ch) in s.char_indices() {
        match ch {
            '"' => in_quote = !in_quote,
            ',' if !in_quote => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        out.push(s[start..].trim());
    }
    out
}

/// Parse the `= "value"` tail of a directional piece.
fn parse_directional_value(rest: &str) -> String {
    let t = rest.trim();
    let value = t
        .strip_prefix('=')
        .map(|s| s.trim())
        .unwrap_or(t)
        .trim_matches('"')
        .trim();
    value.to_string()
}

/// Clean a bound predicate: strip quotes and outer whitespace.
fn clean_bound(bound: String) -> String {
    bound.trim().trim_matches('"').trim().to_string()
}

/// Field-level attributes.
#[derive(Clone, Default)]
pub(crate) struct FieldAttrs {
    pub unknown: Vec<String>,
    pub rename: Option<String>,
    pub rename_ser: Option<String>,
    pub rename_de: Option<String>,
    pub alias: Vec<String>,
    /// `None` = absent, `Some("")` = bare `default`, `Some("path")` = `default = "path"`.
    pub default: Option<String>,
    pub skip_serializing: bool,
    pub skip_deserializing: bool,
    pub skip_serializing_if: Option<String>,
    pub serialize_with: Option<String>,
    pub deserialize_with: Option<String>,
    pub with: Option<String>,
    pub flatten: bool,
    pub borrow: bool,
    /// `#[serde(getter = "path")]`: read the field through this accessor.
    pub getter: Option<String>,
    /// `#[njson(max_str_len = N)]`: maximum string length (safety policy).
    pub max_str_len: Option<u64>,
    /// `#[njson(max_items = N)]`: maximum elements / entries (safety policy).
    pub max_items: Option<u64>,
    /// `#[njson(min = N)]`: inclusive numeric lower bound (safety policy).
    pub min: Option<i128>,
    /// `#[njson(max = N)]`: inclusive numeric upper bound (safety policy).
    pub max: Option<i128>,
    /// `#[njson(sensitive)]`: mark the value as sensitive (safety policy).
    pub sensitive: bool,
}

impl FieldAttrs {
    pub fn has_default(&self) -> bool {
        self.default.is_some()
    }
}

pub(crate) fn field_attrs(metas: &[Meta]) -> FieldAttrs {
    let mut f = FieldAttrs::default();
    for m in metas {
        match m.name() {
            "skip" => {
                f.skip_serializing = true;
                f.skip_deserializing = true;
            }
            "skip_serializing" => f.skip_serializing = true,
            "skip_deserializing" => f.skip_deserializing = true,
            "default" => {
                f.default = match m.value() {
                    Some(v) => unquote(v).or(Some(v.to_string())),
                    None => Some(String::new()),
                };
            }
            "flatten" => f.flatten = true,
            "borrow" => f.borrow = true,
            "rename" => {
                if let Some(value) = m.value() {
                    match split_directional(value) {
                        Some((ser, de)) => {
                            f.rename_ser = ser;
                            f.rename_de = de;
                        }
                        None => f.rename = unquote(value),
                    }
                }
            }
            "alias" => {
                if let Some(v) = m.value().and_then(unquote) {
                    f.alias.push(v);
                }
            }
            "skip_serializing_if" => f.skip_serializing_if = path_of(m),
            "serialize_with" => f.serialize_with = path_of(m),
            "deserialize_with" => f.deserialize_with = path_of(m),
            "with" => f.with = path_of(m),
            "getter" => f.getter = path_of(m),
            "max_str_len" => f.max_str_len = parse_u64(m.value()),
            "max_items" => f.max_items = parse_u64(m.value()),
            "min" => f.min = parse_i128(m.value()),
            "max" => f.max = parse_i128(m.value()),
            "sensitive" => f.sensitive = true,
            _ => f.unknown.push(m.name().to_string()),
        }
    }
    f
}

/// Parse an optional quoted-or-bare unsigned integer attribute value.
fn parse_u64(v: Option<&str>) -> Option<u64> {
    let s = v.map(|s| s.trim().trim_matches('"').trim()).unwrap_or("");
    s.parse().ok()
}

/// Parse an optional quoted-or-bare signed integer attribute value.
fn parse_i128(v: Option<&str>) -> Option<i128> {
    let s = v.map(|s| s.trim().trim_matches('"').trim()).unwrap_or("");
    s.parse().ok()
}

fn path_of(m: &Meta) -> Option<String> {
    m.value()
        .map(|v| unquote(v).unwrap_or_else(|| v.to_string()))
}

/// Variant-level attributes.
#[derive(Clone, Default)]
pub(crate) struct VariantAttrs {
    pub unknown: Vec<String>,
    pub rename: Option<String>,
    pub rename_ser: Option<String>,
    pub rename_de: Option<String>,
    pub rename_all: Option<String>,
    pub rename_all_ser: Option<String>,
    pub rename_all_de: Option<String>,
    pub skip_serializing: bool,
    pub skip_deserializing: bool,
    pub alias: Vec<String>,
    pub other: bool,
    /// `#[serde(serialize_with = "path")]` on a newtype variant: applies to
    /// the single contained field (serde semantics).
    pub serialize_with: Option<String>,
    /// `#[serde(deserialize_with = "path")]` on a newtype variant.
    pub deserialize_with: Option<String>,
    /// `#[serde(with = "module")]` on a newtype variant.
    pub with: Option<String>,
    /// `#[njson(max_str_len = N)]` on a newtype variant: applies to the
    /// single contained field (serde semantics).
    pub max_str_len: Option<u64>,
    /// `#[njson(max_items = N)]` on a newtype variant.
    pub max_items: Option<u64>,
    /// `#[njson(min = N)]` on a newtype variant.
    pub min: Option<i128>,
    /// `#[njson(max = N)]` on a newtype variant.
    pub max: Option<i128>,
    /// `#[njson(sensitive)]` on a newtype variant.
    pub sensitive: bool,
}

pub(crate) fn variant_attrs(metas: &[Meta]) -> VariantAttrs {
    let mut v = VariantAttrs::default();
    for m in metas {
        match m.name() {
            "skip" => {
                v.skip_serializing = true;
                v.skip_deserializing = true;
            }
            "skip_serializing" => v.skip_serializing = true,
            "skip_deserializing" => v.skip_deserializing = true,
            "rename" => {
                if let Some(value) = m.value() {
                    match split_directional(value) {
                        Some((ser, de)) => {
                            v.rename_ser = ser;
                            v.rename_de = de;
                        }
                        None => v.rename = unquote(value),
                    }
                }
            }
            "rename_all" => {
                if let Some(value) = m.value() {
                    match split_directional(value) {
                        Some((ser, de)) => {
                            v.rename_all_ser = ser;
                            v.rename_all_de = de;
                        }
                        None => v.rename_all = unquote(value),
                    }
                }
            }
            "alias" => {
                if let Some(a) = m.value().and_then(unquote) {
                    v.alias.push(a);
                }
            }
            "other" => v.other = true,
            "serialize_with" => v.serialize_with = path_of(m),
            "deserialize_with" => v.deserialize_with = path_of(m),
            "with" => v.with = path_of(m),
            "max_str_len" => v.max_str_len = parse_u64(m.value()),
            "max_items" => v.max_items = parse_u64(m.value()),
            "min" => v.min = parse_i128(m.value()),
            "max" => v.max = parse_i128(m.value()),
            "sensitive" => v.sensitive = true,
            _ => v.unknown.push(m.name().to_string()),
        }
    }
    v
}

/// The effective field attributes for a newtype variant's single field.
///
/// serde applies variant-level `serialize_with` / `deserialize_with` / `with`
/// to the contained field of a newtype variant. A field-level attribute wins
/// when both are present. Safety-policy attributes follow the same rule.
pub(crate) fn newtype_field_attrs(fa: &FieldAttrs, va: &VariantAttrs) -> FieldAttrs {
    let mut out = fa.clone();
    if out.serialize_with.is_none() && out.deserialize_with.is_none() && out.with.is_none() {
        if let Some(w) = &va.with {
            out.with = Some(w.clone());
        }
        if let Some(s) = &va.serialize_with {
            out.serialize_with = Some(s.clone());
        }
        if let Some(d) = &va.deserialize_with {
            out.deserialize_with = Some(d.clone());
        }
    }
    // Safety-policy attributes: field-level wins, variant-level fills gaps.
    if out.max_str_len.is_none() {
        out.max_str_len = va.max_str_len;
    }
    if out.max_items.is_none() {
        out.max_items = va.max_items;
    }
    if out.min.is_none() {
        out.min = va.min;
    }
    if out.max.is_none() {
        out.max = va.max;
    }
    if !out.sensitive {
        out.sensitive = va.sensitive;
    }
    out
}
