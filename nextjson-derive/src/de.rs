//! Codegen for `NsonDeserialize` implementations.

use crate::attr::{self, ContainerAttrs, FieldAttrs};
use crate::{Fields, Input};

/// Simple line-based code builder.
struct Code {
    s: String,
}

impl Code {
    fn new() -> Code {
        Code { s: String::new() }
    }
    fn l(&mut self, line: &str) {
        self.s.push_str(line);
        self.s.push('\n');
    }
    fn out(self) -> String {
        self.s
    }
}

fn is_option(ty: &str) -> bool {
    let t = ty.trim();
    t.starts_with("Option") || t.starts_with("::core::option::Option")
}

fn field_required(f: &crate::Field, fa: &FieldAttrs, ca: &ContainerAttrs) -> bool {
    !ca.default && !fa.has_default() && !fa.skip_deserializing && !is_option(&f.ty)
}

fn default_expr(f: &crate::Field, fa: &FieldAttrs) -> String {
    match &fa.default {
        Some(d) if !d.is_empty() => format!("{d}()"),
        _ => format!("<{} as ::core::default::Default>::default()", f.ty),
    }
}

fn field_decode_expr(field: &crate::Field, fa: &FieldAttrs, cp: &str, decoder: &str) -> String {
    if let Some(p) = &fa.deserialize_with {
        format!("{p}({decoder})?")
    } else if let Some(m) = &fa.with {
        format!("{m}::deserialize({decoder})?")
    } else {
        format!(
            "<{} as {cp}::NsonDeserialize<'de>>::nextdecode({decoder})?",
            field.ty
        )
    }
}

fn write_value(idx: usize, value: &str) -> String {
    format!("__slot{idx}.write({value});")
}

fn default_write(idx: usize, init: &str) -> String {
    format!("__slot{idx}.write({init});")
}

fn seen_decl(n: usize, cp: &str) -> (String, String, String) {
    // returns (declare, set(i), check(i)) as format templates with {i}
    if n <= 64 {
        (
            "let mut __seen: u64 = 0;".to_string(),
            "__seen |= 1u64 << {i};".to_string(),
            "__seen & (1u64 << {i}) != 0".to_string(),
        )
    } else {
        (
            format!(
                "let mut __seen: {cp}::__private::Vec<bool> = {cp}::__private::vec![false; {n}];"
            ),
            "__seen[{i}] = true;".to_string(),
            "__seen[{i}]".to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

pub(crate) fn deserialize_struct(
    _name: &str,
    fields: &Fields,
    input: &Input,
    cp: &str,
    has_flatten: bool,
) -> String {
    let ca = &input.cattr;
    let mut c = Code::new();
    match fields {
        Fields::Unit => {
            c.l("__d.unit()?;");
            c.l("__out.write(Self);");
            c.l("::core::result::Result::Ok(())");
        }
        Fields::Named(f) => {
            if ca.transparent {
                if f.len() != 1 {
                    return crate::err_str("nextjson: `transparent` requires exactly one field");
                }
                let field = &f[0];
                let fa = attr::field_attrs(&field.attrs);
                if fa.flatten {
                    return crate::err_str(
                        "nextjson: `transparent` cannot be combined with `flatten`",
                    );
                }
                let ident = field.ident.clone().unwrap_or_default();
                let expr = field_decode_expr(field, &fa, cp, "__d");
                c.l(&format!("let __v = {expr};"));
                c.l(&format!("__out.write(Self {{ {ident}: __v }});"));
                c.l("::core::result::Result::Ok(())");
                return c.out();
            }
            for (i, field) in f.iter().enumerate() {
                c.l(&format!(
                    "let mut __slot{i}: {cp}::private::InitSlot<{}> = {cp}::private::InitSlot::new();",
                    field.ty
                ));
            }
            let nextdecode = if has_flatten {
                gen_map_nextdecode(f, ca, cp, "__d")
            } else {
                gen_match_nextdecode(f, ca, cp, "__d")
            };
            c.l(&nextdecode);
            let assigns: Vec<String> = f
                .iter()
                .enumerate()
                .map(|(i, field)| {
                    let ident = field.ident.clone().unwrap_or_default();
                    format!("{ident}: __slot{i}.take()")
                })
                .collect();
            c.l(&format!("__out.write(Self {{ {} }});", assigns.join(", ")));
            c.l("::core::result::Result::Ok(())");
        }
        Fields::Unnamed(f) => {
            if ca.transparent {
                if f.len() != 1 {
                    return crate::err_str("nextjson: `transparent` requires exactly one field");
                }
                let field = &f[0];
                let fa = attr::field_attrs(&field.attrs);
                let expr = field_decode_expr(field, &fa, cp, "__d");
                c.l(&format!("let __v = {expr};"));
                c.l("__out.write(Self(__v));");
                c.l("::core::result::Result::Ok(())");
                return c.out();
            }
            if f.is_empty() {
                c.l("__d.begin_array()?;");
                c.l("__d.end_array()?;");
                c.l("__out.write(Self());");
                c.l("::core::result::Result::Ok(())");
                return c.out();
            }
            let count = f.len();
            c.l("__d.begin_array()?;");
            let mut names = Vec::new();
            for (i, field) in f.iter().enumerate() {
                let fa = attr::field_attrs(&field.attrs);
                let id = format!("__v{i}");
                if i > 0 {
                    c.l(&format!(
                        "if !__d.array_entry_sep()? {{ return Err({cp}::Error::invalid_length(0, \"a tuple struct\")); }}"
                    ));
                }
                if fa.skip_deserializing {
                    c.l(&format!(
                        "let {id} = {{ <{cp}::Value as {cp}::NsonDeserialize<'de>>::nextdecode(__d)?; <{} as ::core::default::Default>::default() }};",
                        field.ty
                    ));
                } else {
                    let expr = field_decode_expr(field, &fa, cp, "__d");
                    c.l(&format!("let {id} = {expr};"));
                }
                names.push(id);
            }
            c.l(&format!(
                "if __d.array_entry_sep()? {{ return Err({cp}::Error::invalid_length({count}, \"a tuple struct\")); }}"
            ));
            c.l("__d.end_array()?;");
            c.l(&format!("__out.write(Self({}));", names.join(", ")));
            c.l("::core::result::Result::Ok(())");
        }
    }
    c.out()
}

/// Key-match based object decoding.
fn gen_match_nextdecode(
    fields: &[crate::Field],
    ca: &ContainerAttrs,
    cp: &str,
    decoder: &str,
) -> String {
    let deny = ca.deny_unknown_fields;
    let tracked: Vec<(usize, &crate::Field, FieldAttrs)> = fields
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            let fa = attr::field_attrs(&f.attrs);
            if fa.skip_deserializing {
                None
            } else {
                Some((i, f, fa))
            }
        })
        .collect();

    let mut c = Code::new();

    for (i, f) in fields.iter().enumerate() {
        let fa = attr::field_attrs(&f.attrs);
        if fa.skip_deserializing {
            let init = default_expr(f, &fa);
            c.l(&default_write(i, &init));
        }
    }

    let n = tracked.len();
    let (decl, set_tpl, check_tpl) = seen_decl(n, cp);
    c.l(&decl);
    c.l(&format!("{decoder}.begin_object()?;"));
    c.l(&format!(
        "while let ::core::option::Option::Some(__key) = {decoder}.object_key()? {{"
    ));
    c.l("match __key.as_ref() {");
    for (idx, (i, f, fa)) in tracked.iter().enumerate() {
        // `idx` is the position within `tracked` (0..n) and therefore the
        // bit/`Vec<bool>` index used by `seen_decl`; `i` stays the original
        // field index used for the `__slot{i}` storage.
        let main = crate::schema::renamed_field(f, fa, ca);
        let mut pats = vec![format!("{main:?}")];
        for a in &fa.alias {
            pats.push(format!("{a:?}"));
        }
        c.l(&format!("{} => {{", pats.join(" | ")));
        c.l(&set_tpl.replace("{i}", &idx.to_string()));
        if fa.deserialize_with.is_some() || fa.with.is_some() {
            let expr = field_decode_expr(f, fa, cp, decoder);
            c.l(&format!("let __v = {expr};"));
            c.l(&write_value(*i, "__v"));
        } else {
            c.l(&format!("__slot{i}.nextdecode({decoder})?;"));
        }
        c.l("}");
    }
    c.l("_ => {");
    if deny {
        c.l(&format!(
            "return Err({cp}::Error::unknown_field(__key.into_owned()));"
        ));
    } else {
        c.l(&format!("{decoder}.skip_value()?;"));
    }
    c.l("}");
    c.l("}");
    c.l(&format!("if !{decoder}.object_entry_sep()? {{ break; }}"));
    c.l("}");
    c.l(&format!("{decoder}.end_object()?;"));

    for (idx, (i, f, fa)) in tracked.iter().enumerate() {
        let check = check_tpl.replace("{i}", &idx.to_string());
        let ident = f.ident.clone().unwrap_or_default();
        if field_required(f, fa, ca) {
            let orig = ident.clone();
            c.l(&format!(
                "if !({check}) {{ return Err({cp}::Error::missing_field({orig:?})); }};"
            ));
        } else {
            let init = default_expr(f, fa);
            let dw = default_write(*i, &init);
            c.l(&format!("if !({check}) {{ {dw} }};"));
        }
    }
    c.out()
}

/// Map-based object decoding (used with `flatten`).
fn gen_map_nextdecode(
    fields: &[crate::Field],
    ca: &ContainerAttrs,
    cp: &str,
    decoder: &str,
) -> String {
    let tracked: Vec<(usize, &crate::Field, FieldAttrs)> = fields
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            let fa = attr::field_attrs(&f.attrs);
            if fa.skip_deserializing {
                None
            } else {
                Some((i, f, fa))
            }
        })
        .collect();

    let mut c = Code::new();

    for (i, f) in fields.iter().enumerate() {
        let fa = attr::field_attrs(&f.attrs);
        if fa.skip_deserializing {
            let init = default_expr(f, &fa);
            c.l(&default_write(i, &init));
        }
    }

    c.l(&format!(
        "let mut __map = <{cp}::Map as {cp}::NsonDeserialize<'de>>::nextdecode({decoder})?;"
    ));

    // First pass: explicit fields, which remove their own keys from the map.
    for (i, f, fa) in &tracked {
        if fa.flatten {
            continue;
        }
        let ident = f.ident.clone().unwrap_or_default();
        let main = crate::schema::renamed_field(f, fa, ca);
        let mut keys = vec![format!("__map.remove({main:?})")];
        for a in &fa.alias {
            keys.push(format!(".or_else(|| __map.remove({a:?}))"));
        }
        let wv = write_value(*i, "__decoded");
        c.l("{");
        c.l(&format!("let __found = {};", keys.join("")));
        c.l("match __found {");
        c.l("::core::option::Option::Some(__val) => {");
        c.l(&format!(
            "let __decoded = {cp}::private::nextdecode_value::<{}>(__val)?;",
            f.ty
        ));
        c.l(&wv);
        c.l("}");
        if field_required(f, fa, ca) {
            let orig = ident.clone();
            c.l(&format!(
                "::core::option::Option::None => {{ return Err({cp}::Error::missing_field({orig:?})); }}"
            ));
        } else {
            let init = default_expr(f, fa);
            let dw = default_write(*i, &init);
            c.l(&format!("::core::option::Option::None => {{ {dw} }}"));
        }
        c.l("}");
        c.l("}");
    }
    // Second pass: flatten fields consume the remaining keys, regardless of
    // their declaration position (serde semantics: flatten captures the rest).
    for (i, f, fa) in &tracked {
        if !fa.flatten {
            continue;
        }
        let wv = write_value(*i, "__decoded");
        c.l("{");
        c.l(&format!(
            "let __decoded = {cp}::private::nextdecode_value::<{}>({cp}::Value::Object(__map.clone()))?;",
            f.ty
        ));
        c.l(&wv);
        c.l("}");
    }
    c.out()
}

// ---------------------------------------------------------------------------
// Enum struct variants
// ---------------------------------------------------------------------------

/// Decode a struct variant into local slots and construct `Self::Variant`.
fn gen_variant_struct_nextdecode(
    variant: &str,
    fields: &[crate::Field],
    ca: &ContainerAttrs,
    cp: &str,
    decoder: &str,
    has_flatten: bool,
) -> String {
    let mut c = Code::new();
    c.l("{");
    for (i, f) in fields.iter().enumerate() {
        c.l(&format!(
            "let mut __slot{i}: {cp}::private::InitSlot<{}> = {cp}::private::InitSlot::new();",
            f.ty
        ));
    }
    let nextdecode = if has_flatten {
        gen_map_nextdecode(fields, ca, cp, decoder)
    } else {
        gen_match_nextdecode(fields, ca, cp, decoder)
    };
    c.l(&nextdecode);
    let mut assigns = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        let ident = f.ident.clone().unwrap_or_default();
        assigns.push(format!("{ident}: __slot{i}.take()"));
    }
    c.l(&format!("Self::{variant} {{ {} }}", assigns.join(", ")));
    c.l("}");
    c.out()
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

pub(crate) fn deserialize_enum(
    _name: &str,
    variants: &[crate::Variant],
    input: &Input,
    cp: &str,
    has_flatten: bool,
) -> String {
    let ca = &input.cattr;
    if ca.untagged && (ca.tag.is_some() || ca.content.is_some()) {
        return crate::err_str("nextjson: `untagged` cannot be combined with `tag` / `content`");
    }
    if let Some(tag) = &ca.tag {
        if let Some(content) = &ca.content {
            deserialize_adjacent(variants, tag, content, ca, cp, has_flatten)
        } else {
            deserialize_internal(variants, tag, ca, cp, has_flatten)
        }
    } else if ca.untagged {
        deserialize_untagged(variants, ca, cp, has_flatten)
    } else {
        deserialize_external(variants, ca, cp, has_flatten)
    }
}

fn deserialize_external(
    variants: &[crate::Variant],
    ca: &ContainerAttrs,
    cp: &str,
    has_flatten: bool,
) -> String {
    let mut c = Code::new();
    c.l("__d.begin_object()?;");
    c.l(&format!(
        "let __key = __d.object_key()?.ok_or_else(|| {cp}::Error::invalid_length(0, \"an enum variant\"))?;"
    ));
    c.l("match __key.as_ref() {");
    for v in variants {
        let va = attr::variant_attrs(&v.attrs);
        if va.skip_deserializing {
            continue;
        }
        let vname = crate::schema::renamed_variant(v, &va, ca);
        let body = match &v.fields {
            Fields::Unit => format!("__d.unit()?; __out.write(Self::{});", v.ident),
            Fields::Unnamed(f) if f.len() == 1 => {
                let field = &f[0];
                let fa = attr::field_attrs(&field.attrs);
                let expr = field_decode_expr(field, &fa, cp, "__d");
                format!("let __v = {expr}; __out.write(Self::{}(__v));", v.ident)
            }
            Fields::Unnamed(f) => {
                let mut sub = Code::new();
                sub.l("__d.begin_array()?;");
                let mut names = Vec::new();
                for (i, field) in f.iter().enumerate() {
                    let fa = attr::field_attrs(&field.attrs);
                    let id = format!("__v{i}");
                    if i > 0 {
                        sub.l(&format!(
                            "if !__d.array_entry_sep()? {{ return Err({cp}::Error::invalid_length(0, \"a tuple variant\")); }}"
                        ));
                    }
                    if fa.skip_deserializing {
                        sub.l(&format!(
                            "let {id} = {{ <{cp}::Value as {cp}::NsonDeserialize<'de>>::nextdecode(__d)?; <{} as ::core::default::Default>::default() }};",
                            field.ty
                        ));
                    } else {
                        let expr = field_decode_expr(field, &fa, cp, "__d");
                        sub.l(&format!("let {id} = {expr};"));
                    }
                    names.push(id);
                }
                sub.l(&format!(
                    "if __d.array_entry_sep()? {{ return Err({cp}::Error::custom(\"too many elements in tuple variant\")); }}"
                ));
                sub.l("__d.end_array()?;");
                sub.l(&format!(
                    "__out.write(Self::{}({}));",
                    v.ident,
                    names.join(", ")
                ));
                sub.out()
            }
            Fields::Named(f) => {
                let constructed =
                    gen_variant_struct_nextdecode(&v.ident, f, ca, cp, "__d", has_flatten);
                format!("__out.write({constructed});")
            }
        };
        c.l(&format!("{vname:?} => {{"));
        c.l(&body);
        c.l("}");
    }
    c.l(&format!(
        "_ => return Err({cp}::Error::unknown_variant(__key.into_owned())),"
    ));
    c.l("}");
    c.l(&format!(
        "if __d.object_entry_sep()? {{ return Err({cp}::Error::custom(\"expected a single-variant enum object\")); }}"
    ));
    c.l("__d.end_object()?;");
    c.l("::core::result::Result::Ok(())");
    c.out()
}

fn deserialize_internal(
    variants: &[crate::Variant],
    tag: &str,
    ca: &ContainerAttrs,
    cp: &str,
    has_flatten: bool,
) -> String {
    let mut c = Code::new();
    c.l(&format!(
        "let __entries = {cp}::private::read_object_map(__d)?;"
    ));
    c.l(&format!(
        "let mut __tag: ::core::option::Option<{cp}::__private::String> = ::core::option::Option::None;"
    ));
    c.l(&format!(
        "let mut __rest: {cp}::__private::Vec<({cp}::__private::Cow<'de, str>, {cp}::__private::Vec<{cp}::private::Token<'de>>)> = {cp}::__private::Vec::new();"
    ));
    c.l("for (__k, __v) in __entries {");
    c.l(&format!(
        "if __k == {tag:?} {{ __tag = ::core::option::Option::Some({cp}::private::token_to_string(&__v)?); }} else {{ __rest.push((__k, __v)); }}"
    ));
    c.l("}");
    c.l(&format!(
        "let __tag = __tag.ok_or_else(|| {cp}::Error::missing_field({tag:?}))?;"
    ));
    c.l("let __tag_ref = __tag.clone();");
    c.l("match __tag_ref.as_str() {");
    for v in variants {
        let va = attr::variant_attrs(&v.attrs);
        if va.skip_deserializing {
            continue;
        }
        let vname = crate::schema::renamed_variant(v, &va, ca);
        let body = match &v.fields {
            Fields::Unit => format!("__out.write(Self::{});", v.ident),
            Fields::Unnamed(f) if f.len() == 1 => {
                let field = &f[0];
                let fa = attr::field_attrs(&field.attrs);
                let expr = field_decode_expr(field, &fa, cp, "__sub");
                format!(
                    "let __tokens = {cp}::private::tokens_to_object(__rest); \
                     let mut __sub = {cp}::private::from_tokens(__tokens); \
                     let __sub = &mut __sub; \
                     let __v = {expr}; \
                     __out.write(Self::{}(__v));",
                    v.ident
                )
            }
            Fields::Unnamed(f) => {
                let tys: Vec<String> = f.iter().map(|x| x.ty.clone()).collect();
                let mut pats = Vec::new();
                let mut names = Vec::new();
                for (i, field) in f.iter().enumerate() {
                    let fa = attr::field_attrs(&field.attrs);
                    if fa.skip_deserializing {
                        pats.push("_".to_string());
                        names.push(format!(
                            "<{} as ::core::default::Default>::default()",
                            field.ty
                        ));
                    } else {
                        let id = format!("__v{i}");
                        pats.push(id.clone());
                        names.push(id);
                    }
                }
                format!(
                    "let __tokens = {cp}::private::tokens_to_object(__rest); \
                     let mut __sub = {cp}::private::from_tokens(__tokens); \
                     let __sub = &mut __sub; \
                     let ({}) = <({}) as {cp}::NsonDeserialize<'de>>::nextdecode(__sub)?; \
                     __out.write(Self::{}({}));",
                    pats.join(", "),
                    tys.join(", "),
                    v.ident,
                    names.join(", ")
                )
            }
            Fields::Named(f) => {
                let constructed =
                    gen_variant_struct_nextdecode(&v.ident, f, ca, cp, "__sub", has_flatten);
                format!(
                    "let __tokens = {cp}::private::tokens_to_object(__rest); \
                     let mut __sub = {cp}::private::from_tokens(__tokens); \
                     let __sub = &mut __sub; \
                     let __constructed = {constructed}; \
                     __out.write(__constructed);"
                )
            }
        };
        c.l(&format!("{vname:?} => {{"));
        c.l(&body);
        c.l("}");
    }
    c.l(&format!(
        "_ => return Err({cp}::Error::unknown_variant(__tag)),"
    ));
    c.l("}");
    c.l("::core::result::Result::Ok(())");
    c.out()
}

fn deserialize_adjacent(
    variants: &[crate::Variant],
    tag: &str,
    content: &str,
    ca: &ContainerAttrs,
    cp: &str,
    has_flatten: bool,
) -> String {
    let mut c = Code::new();
    c.l(&format!(
        "let __entries = {cp}::private::read_object_map(__d)?;"
    ));
    c.l(&format!(
        "let mut __tag: ::core::option::Option<{cp}::__private::String> = ::core::option::Option::None;"
    ));
    c.l(&format!(
        "let mut __content: ::core::option::Option<{cp}::__private::Vec<{cp}::private::Token<'de>>> = ::core::option::Option::None;"
    ));
    c.l("for (__k, __v) in __entries {");
    c.l(&format!(
        "if __k == {tag:?} {{ __tag = ::core::option::Option::Some({cp}::private::token_to_string(&__v)?); }} \
         else if __k == {content:?} {{ __content = ::core::option::Option::Some(__v); }} \
         else {{ return Err({cp}::Error::unknown_field(__k.into_owned())); }}"
    ));
    c.l("}");
    c.l(&format!(
        "let __tag = __tag.ok_or_else(|| {cp}::Error::missing_field({tag:?}))?;"
    ));
    c.l("let __tag_ref = __tag.clone();");
    c.l("match __tag_ref.as_str() {");
    for v in variants {
        let va = attr::variant_attrs(&v.attrs);
        if va.skip_deserializing {
            continue;
        }
        let vname = crate::schema::renamed_variant(v, &va, ca);
        let body = match &v.fields {
            Fields::Unit => format!(
                "if __content.is_some() {{ return Err({cp}::Error::custom(\"adjacently tagged unit variant must not carry content\")); }} \
                 __out.write(Self::{});",
                v.ident
            ),
            Fields::Unnamed(f) if f.len() == 1 => {
                let field = &f[0];
                let fa = attr::field_attrs(&field.attrs);
                let expr = field_decode_expr(field, &fa, cp, "__sub");
                format!(
                    "let __c = __content.ok_or_else(|| {cp}::Error::missing_field({content:?}))?; \
                     let mut __sub = {cp}::private::from_tokens(__c); \
                     let __sub = &mut __sub; \
                     let __v = {expr}; \
                     __out.write(Self::{}(__v));",
                    v.ident
                )
            }
            Fields::Unnamed(f) => {
                let tys: Vec<String> = f.iter().map(|x| x.ty.clone()).collect();
                let mut pats = Vec::new();
                let mut names = Vec::new();
                for (i, field) in f.iter().enumerate() {
                    let fa = attr::field_attrs(&field.attrs);
                    if fa.skip_deserializing {
                        pats.push("_".to_string());
                        names.push(format!("<{} as ::core::default::Default>::default()", field.ty));
                    } else {
                        let id = format!("__v{i}");
                        pats.push(id.clone());
                        names.push(id);
                    }
                }
                format!(
                    "let __c = __content.ok_or_else(|| {cp}::Error::missing_field({content:?}))?; \
                     let mut __sub = {cp}::private::from_tokens(__c); \
                     let __sub = &mut __sub; \
                     let ({}) = <({}) as {cp}::NsonDeserialize<'de>>::nextdecode(__sub)?; \
                     __out.write(Self::{}({}));",
                    pats.join(", "),
                    tys.join(", "),
                    v.ident,
                    names.join(", ")
                )
            }
            Fields::Named(f) => {
                let constructed = gen_variant_struct_nextdecode(&v.ident, f, ca, cp, "__sub", has_flatten);
                format!(
                    "let __c = __content.ok_or_else(|| {cp}::Error::missing_field({content:?}))?; \
                     let mut __sub = {cp}::private::from_tokens(__c); \
                     let __sub = &mut __sub; \
                     let __constructed = {constructed}; \
                     __out.write(__constructed);"
                )
            }
        };
        c.l(&format!("{vname:?} => {{"));
        c.l(&body);
        c.l("}");
    }
    c.l(&format!(
        "_ => return Err({cp}::Error::unknown_variant(__tag)),"
    ));
    c.l("}");
    c.l("::core::result::Result::Ok(())");
    c.out()
}

fn deserialize_untagged(
    variants: &[crate::Variant],
    ca: &ContainerAttrs,
    cp: &str,
    has_flatten: bool,
) -> String {
    let mut c = Code::new();
    c.l("let __mark = __d.save();");
    for v in variants {
        let va = attr::variant_attrs(&v.attrs);
        if va.skip_deserializing {
            continue;
        }
        let body = match &v.fields {
            Fields::Unit => format!("__d.unit()?; ::core::result::Result::Ok(Self::{})", v.ident),
            Fields::Unnamed(f) if f.len() == 1 => {
                let field = &f[0];
                let fa = attr::field_attrs(&field.attrs);
                let expr = field_decode_expr(field, &fa, cp, "__d");
                format!(
                    "let __v = {expr}; ::core::result::Result::Ok(Self::{}(__v))",
                    v.ident
                )
            }
            Fields::Unnamed(f) => {
                let tys: Vec<String> = f.iter().map(|x| x.ty.clone()).collect();
                let mut pats = Vec::new();
                let mut names = Vec::new();
                for (i, field) in f.iter().enumerate() {
                    let fa = attr::field_attrs(&field.attrs);
                    if fa.skip_deserializing {
                        pats.push("_".to_string());
                        names.push(format!(
                            "<{} as ::core::default::Default>::default()",
                            field.ty
                        ));
                    } else {
                        let id = format!("__v{i}");
                        pats.push(id.clone());
                        names.push(id);
                    }
                }
                format!(
                    "let ({}) = <({}) as {cp}::NsonDeserialize<'de>>::nextdecode(__d)?; \
                     ::core::result::Result::Ok(Self::{}({}))",
                    pats.join(", "),
                    tys.join(", "),
                    v.ident,
                    names.join(", ")
                )
            }
            Fields::Named(f) => {
                let constructed =
                    gen_variant_struct_nextdecode(&v.ident, f, ca, cp, "__d", has_flatten);
                format!("let __v = {constructed}; ::core::result::Result::Ok(__v)")
            }
        };
        c.l("{");
        c.l("__d.restore(__mark);");
        c.l(&format!(
            "let __result: {cp}::Result<Self> = (|| -> {cp}::Result<Self> {{"
        ));
        c.l(&body);
        c.l("})();");
        c.l("if let ::core::result::Result::Ok(__v) = __result {");
        c.l("__out.write(__v);");
        c.l("return ::core::result::Result::Ok(());");
        c.l("}");
        c.l("}");
    }
    c.l(&format!(
        "::core::result::Result::Err({cp}::Error::custom(\"data did not match any variant of untagged enum\"))"
    ));
    c.out()
}
