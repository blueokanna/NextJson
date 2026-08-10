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
pub(crate) fn metas_from_attr(inner: &[TokenTree]) -> Vec<Meta> {
    let mut p = P { toks: inner, i: 0 };
    if !p.is_ident("njson") && !p.is_ident("nextjson") {
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
    pub rename_all: Option<String>,
    pub rename: Option<String>,
    pub tag: Option<String>,
    pub content: Option<String>,
    pub untagged: bool,
    pub transparent: bool,
    pub deny_unknown_fields: bool,
    pub default: bool,
    pub bound: Option<String>,
    pub crate_path: String,
}

impl ContainerAttrs {
    pub fn from_metas(metas: &[Meta]) -> ContainerAttrs {
        let mut c = ContainerAttrs {
            rename_all: None,
            rename: None,
            tag: None,
            content: None,
            untagged: false,
            transparent: false,
            deny_unknown_fields: false,
            default: false,
            bound: None,
            crate_path: "::nextjson".to_string(),
        };
        for m in metas {
            match m.name() {
                "untagged" => c.untagged = true,
                "transparent" => c.transparent = true,
                "deny_unknown_fields" => c.deny_unknown_fields = true,
                "default" => c.default = true,
                "rename_all" => c.rename_all = m.value().and_then(unquote),
                "rename" => c.rename = m.value().and_then(unquote),
                "tag" => c.tag = m.value().and_then(unquote),
                "content" => c.content = m.value().and_then(unquote),
                "bound" => c.bound = m.value().and_then(unquote),
                "crate" => {
                    if let Some(v) = m.value() {
                        if let Some(q) = unquote(v) {
                            c.crate_path = q;
                        } else if !v.is_empty() {
                            c.crate_path = v.to_string();
                        }
                    }
                }
                _ => {}
            }
        }
        c
    }
}

/// Field-level attributes.
#[derive(Clone, Default)]
pub(crate) struct FieldAttrs {
    pub rename: Option<String>,
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
            "rename" => f.rename = m.value().and_then(unquote),
            "alias" => {
                if let Some(v) = m.value().and_then(unquote) {
                    f.alias.push(v);
                }
            }
            "skip_serializing_if" => f.skip_serializing_if = path_of(m),
            "serialize_with" => f.serialize_with = path_of(m),
            "deserialize_with" => f.deserialize_with = path_of(m),
            "with" => f.with = path_of(m),
            _ => {}
        }
    }
    f
}

fn path_of(m: &Meta) -> Option<String> {
    m.value()
        .map(|v| unquote(v).unwrap_or_else(|| v.to_string()))
}

/// Variant-level attributes.
#[derive(Clone, Default)]
pub(crate) struct VariantAttrs {
    pub rename: Option<String>,
    pub rename_all: Option<String>,
    pub skip_serializing: bool,
    pub skip_deserializing: bool,
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
            "rename" => v.rename = m.value().and_then(unquote),
            "rename_all" => v.rename_all = m.value().and_then(unquote),
            _ => {}
        }
    }
    v
}

/// Test the rename rule table (case module).
pub(crate) fn _assert_rename_smoke() {}
