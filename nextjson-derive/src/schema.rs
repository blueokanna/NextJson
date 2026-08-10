//! Codegen for the `const SCHEMA: TypeSchema` expression.

use crate::attr::{self, ContainerAttrs, FieldAttrs};
use crate::{Data, Fields, Input};

/// Generate the schema constant expression for a type.
pub(crate) fn schema_expr(input: &Input, cp: &str) -> String {
    let name = input.ident.clone();
    let ca = &input.cattr;
    match &input.data {
        Data::Struct(fields) => struct_schema(fields, ca, cp, &name),
        Data::Enum(variants) => enum_schema(variants, ca, cp, &name),
    }
}

fn struct_schema(fields: &Fields, ca: &ContainerAttrs, cp: &str, name: &str) -> String {
    match fields {
        Fields::Unit => format!("{cp}::TypeSchema::Unit"),
        Fields::Named(f) => {
            let transparent = ca.transparent;
            let items: Vec<String> = f
                .iter()
                .map(|field| {
                    let fa = attr::field_attrs(&field.attrs);
                    field_schema_tokens(field, &fa, ca, cp)
                })
                .collect();
            format!(
                "{cp}::TypeSchema::Struct(&{cp}::StructSchema {{ \
                 name: {name:?}, transparent: {transparent}, fields: &[{}] }})",
                items.join(", ")
            )
        }
        Fields::Unnamed(f) => {
            let items: Vec<String> = f
                .iter()
                .map(|field| {
                    let fa = attr::field_attrs(&field.attrs);
                    field_type_schema(&field.ty, fa.skip_serializing, cp)
                })
                .collect();
            format!("{cp}::TypeSchema::Tuple(&[{}])", items.join(", "))
        }
    }
}

fn enum_schema(variants: &[crate::Variant], ca: &ContainerAttrs, cp: &str, name: &str) -> String {
    let items: Vec<String> = variants
        .iter()
        .map(|v| {
            let va = attr::variant_attrs(&v.attrs);
            let serialized = renamed_variant(v, &va, ca);
            let ty = variant_type_schema(v, &va, &serialized, ca, cp);
            format!(
                "{cp}::VariantSchema {{ name: {serialized:?}, orig: {:?}, ty: {ty} }}",
                v.ident
            )
        })
        .collect();
    let tag = ca
        .tag
        .as_ref()
        .map(|t| format!("::core::option::Option::Some({t:?})"))
        .unwrap_or_else(|| "::core::option::Option::None".to_string());
    let content = ca
        .content
        .as_ref()
        .map(|c| format!("::core::option::Option::Some({c:?})"))
        .unwrap_or_else(|| "::core::option::Option::None".to_string());
    format!(
        "{cp}::TypeSchema::Enum(&{cp}::EnumSchema {{ \
         name: {name:?}, tag: {tag}, content: {content}, \
         untagged: {}, default_tag: \"type\", variants: &[{}] }})",
        ca.untagged,
        items.join(", ")
    )
}

fn variant_type_schema(
    v: &crate::Variant,
    va: &crate::VariantAttrs,
    serialized_name: &str,
    ca: &ContainerAttrs,
    cp: &str,
) -> String {
    match &v.fields {
        Fields::Unit => format!("{cp}::TypeSchema::Unit"),
        Fields::Unnamed(f) if f.len() == 1 => field_type_schema(&f[0].ty, va.skip_serializing, cp),
        Fields::Unnamed(f) => {
            let items: Vec<String> = f
                .iter()
                .map(|field| field_type_schema(&field.ty, false, cp))
                .collect();
            format!("{cp}::TypeSchema::Tuple(&[{}])", items.join(", "))
        }
        Fields::Named(f) => {
            let items: Vec<String> = f
                .iter()
                .map(|field| {
                    let fa = attr::field_attrs(&field.attrs);
                    field_schema_tokens(field, &fa, ca, cp)
                })
                .collect();
            format!(
                "{cp}::TypeSchema::Struct(&{cp}::StructSchema {{ \
                 name: {serialized_name:?}, transparent: false, fields: &[{}] }})",
                items.join(", ")
            )
        }
    }
}

fn field_schema_tokens(
    field: &crate::Field,
    fa: &FieldAttrs,
    ca: &ContainerAttrs,
    cp: &str,
) -> String {
    let orig = field.ident.clone().unwrap_or_default();
    let serialized = renamed_field(field, fa, ca);
    let required =
        !ca.default && !fa.has_default() && !fa.skip_deserializing && !is_option_type(&field.ty);
    let flattened = fa.flatten;
    let ty = field_type_schema(&field.ty, fa.skip_serializing, cp);
    format!(
        "{cp}::FieldSchema {{ name: {serialized:?}, orig: {orig:?}, \
         required: {required}, flattened: {flattened}, ty: {ty} }}"
    )
}

/// The serialized name of a field.
pub(crate) fn renamed_field(field: &crate::Field, fa: &FieldAttrs, ca: &ContainerAttrs) -> String {
    if let Some(r) = &fa.rename {
        return r.clone();
    }
    let orig = field.ident.clone().unwrap_or_default();
    match &ca.rename_all {
        Some(rule) => crate::case::apply(rule, &orig),
        None => orig,
    }
}

/// The serialized name of a variant.
pub(crate) fn renamed_variant(
    v: &crate::Variant,
    va: &crate::VariantAttrs,
    ca: &ContainerAttrs,
) -> String {
    if let Some(r) = &va.rename {
        return r.clone();
    }
    let orig = v.ident.clone();
    let rule = va.rename_all.as_ref().or(ca.rename_all.as_ref());
    match rule {
        Some(rule) => crate::case::apply(rule, &orig),
        None => orig,
    }
}

/// Whether a type is `Option<...>`.
pub(crate) fn is_option_type(ty: &str) -> bool {
    let t = ty.trim();
    t.starts_with("Option")
        || t.starts_with("::core::option::Option")
        || t.starts_with("core::option::Option")
}

/// Schema expression for a field type.
pub(crate) fn field_type_schema(ty: &str, skipped: bool, cp: &str) -> String {
    if skipped {
        return format!("{cp}::TypeSchema::Opaque");
    }
    known_type_schema(ty, cp).unwrap_or_else(|| format!("<{ty} as {cp}::NsonSchema>::SCHEMA"))
}

fn known_type_schema(ty: &str, cp: &str) -> Option<String> {
    let t = ty.trim();
    // Primitives and common std/alloc types are mapped by simple prefixes.
    let simple = match t {
        "bool" => Some("Bool"),
        "char" => Some("Char"),
        "i8" => Some("I8"),
        "i16" => Some("I16"),
        "i32" => Some("I32"),
        "i64" => Some("I64"),
        "i128" => Some("I128"),
        "isize" => Some("Isize"),
        "u8" => Some("U8"),
        "u16" => Some("U16"),
        "u32" => Some("U32"),
        "u64" => Some("U64"),
        "u128" => Some("U128"),
        "usize" => Some("Usize"),
        "f32" => Some("F32"),
        "f64" => Some("F64"),
        "str" | "String" | "&str" => Some("Str"),
        "()" => Some("Unit"),
        _ => None,
    };
    if let Some(s) = simple {
        return Some(format!("{cp}::TypeSchema::{s}"));
    }

    // Generic containers: Vec<T>, Option<T>, Box<T>, maps, tuples, arrays, refs, slices.
    let inner = |arg: &str| {
        known_type_schema(arg, cp).unwrap_or_else(|| format!("<{arg} as {cp}::NsonSchema>::SCHEMA"))
    };

    if let Some(arg) = generic_arg(t, "Option") {
        return Some(format!("{cp}::TypeSchema::Optional(&{})", inner(&arg)));
    }
    for name in [
        "Vec",
        "VecDeque",
        "LinkedList",
        "BinaryHeap",
        "HashSet",
        "BTreeSet",
    ] {
        if let Some(arg) = generic_arg(t, name) {
            return Some(format!("{cp}::TypeSchema::Seq(&{})", inner(&arg)));
        }
    }
    for name in ["Box", "Rc", "Arc", "Cell", "RefCell", "Mutex", "RwLock"] {
        if let Some(arg) = generic_arg(t, name) {
            return Some(inner(&arg));
        }
    }
    if let Some(arg) = generic_arg(t, "Cow") {
        let a = arg.trim();
        if a == "str" || a.ends_with("str") {
            return Some(format!("{cp}::TypeSchema::Str"));
        }
        return Some(format!("{cp}::TypeSchema::Seq(&{})", inner(a)));
    }
    if let Some(v) = map_value_arg(t) {
        return Some(format!("{cp}::TypeSchema::Map(&{})", inner(&v)));
    }
    if t.starts_with('(') && t.ends_with(')') {
        let body = &t[1..t.len() - 1];
        let parts = split_commas(body);
        let items: Vec<String> = parts.iter().map(|p| inner(p.trim())).collect();
        return Some(format!("{cp}::TypeSchema::Tuple(&[{}])", items.join(", ")));
    }
    if t.starts_with('[') && t.ends_with(']') {
        // array or slice
        let body = &t[1..t.len() - 1];
        if let Some(semi) = body.find(';') {
            let elem = body[..semi].trim();
            return Some(format!("{cp}::TypeSchema::Seq(&{})", inner(elem)));
        }
        let elem = body.trim();
        if elem == "u8" {
            return Some(format!("{cp}::TypeSchema::Bytes"));
        }
        return Some(format!("{cp}::TypeSchema::Seq(&{})", inner(elem)));
    }
    if let Some(rest) = t.strip_prefix('&') {
        let rest = rest.trim();
        if rest.starts_with('[') {
            return known_type_schema(rest, cp);
        }
        if rest == "str" {
            return Some(format!("{cp}::TypeSchema::Str"));
        }
        return known_type_schema(rest, cp);
    }
    for name in ["Duration"] {
        if t == name {
            return Some(format!("{cp}::TypeSchema::U64"));
        }
    }
    for name in ["PathBuf", "IpAddr", "Ipv4Addr", "Ipv6Addr", "SocketAddr"] {
        if t == name {
            return Some(format!("{cp}::TypeSchema::Str"));
        }
    }
    for name in ["Number", "Value"] {
        if t == name {
            return Some(format!("{cp}::TypeSchema::Opaque"));
        }
    }
    if t == "Map" {
        return Some(format!("{cp}::TypeSchema::Map(&{cp}::TypeSchema::Opaque)"));
    }
    if t == "PhantomData" || t.starts_with("PhantomData<") {
        return Some(format!("{cp}::TypeSchema::Unit"));
    }
    if let Some(arg) = generic_arg(t, "Range").or_else(|| generic_arg(t, "RangeInclusive")) {
        let a = inner(&arg);
        return Some(format!("{cp}::TypeSchema::Tuple(&[{a}, {a}])"));
    }
    None
}

/// Extract the single type argument of `Name<Arg>`.
fn generic_arg(t: &str, name: &str) -> Option<String> {
    let t = t.trim();
    let idx = t.find('<')?;
    let base = t[..idx].trim();
    let base = base.rsplit("::").next().unwrap_or(base);
    if base != name {
        return None;
    }
    let rest = &t[idx + 1..];
    let end = rest.rfind('>')?;
    Some(rest[..end].to_string())
}

/// Extract the value type argument of `HashMap<K, V>` / `BTreeMap<K, V>`.
fn map_value_arg(t: &str) -> Option<String> {
    let _ = t.find("HashMap");
    let _ = t.find("BTreeMap");
    let arg = generic_arg(t, "HashMap").or_else(|| generic_arg(t, "BTreeMap"))?;
    let parts = split_commas(&arg);
    if parts.len() == 2 {
        Some(parts[1].trim().to_string())
    } else {
        None
    }
}

/// Split a string on commas at depth 0 (respecting <>()[]).
fn split_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '<' | '(' | '[' => {
                depth += 1;
                cur.push(c);
            }
            '>' | ')' | ']' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}
