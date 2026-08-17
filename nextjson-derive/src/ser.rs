//! Codegen for `NsonSerialize` implementations.

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

pub(crate) fn serialize_struct(name: &str, fields: &Fields, input: &Input, cp: &str) -> String {
    let ca = &input.cattr;
    let mut c = Code::new();
    match fields {
        Fields::Unit => {
            c.l("__e.write_null()?;");
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
                c.l(&field_ser_call(field, &fa, cp));
                c.l("::core::result::Result::Ok(())");
                return c.out();
            }
            c.l("__e.begin_object()?;");
            for field in f {
                let fa = attr::field_attrs(&field.attrs);
                if fa.skip_serializing {
                    continue;
                }
                let ident = field.ident.clone().unwrap_or_default();
                let key = crate::schema::renamed_field(field, &fa, ca, true);
                if fa.flatten {
                    c.l(&format!(
                        "{cp}::private::flatten_serialize(&self.{ident}, __e)?;"
                    ));
                    continue;
                }
                let call = field_ser_call(field, &fa, cp);
                if let Some(pred) = &fa.skip_serializing_if {
                    let subject = match &fa.getter {
                        Some(g) => format!("{g}(&self)"),
                        None => format!("&self.{ident}"),
                    };
                    c.l(&format!(
                        "if !({pred})({subject}) {{ __e.key({key:?})?; {call} }}"
                    ));
                } else {
                    c.l(&format!("__e.key({key:?})?;"));
                    c.l(&call);
                }
            }
            c.l("__e.end_object()?;");
            c.l("::core::result::Result::Ok(())");
        }
        Fields::Unnamed(f) => {
            if ca.transparent {
                if f.len() != 1 {
                    return crate::err_str("nextjson: `transparent` requires exactly one field");
                }
                let field = &f[0];
                let fa = attr::field_attrs(&field.attrs);
                c.l(&field_ser_call_indexed(field, 0, &fa, cp));
                c.l("::core::result::Result::Ok(())");
                return c.out();
            }
            if f.is_empty() {
                c.l("__e.begin_array()?;");
                c.l("__e.end_array()?;");
                c.l("::core::result::Result::Ok(())");
                return c.out();
            }
            c.l("__e.begin_array()?;");
            for (i, field) in f.iter().enumerate() {
                let fa = attr::field_attrs(&field.attrs);
                if fa.skip_serializing {
                    continue;
                }
                c.l("__e.separator()?;");
                c.l(&field_ser_call_indexed(field, i, &fa, cp));
            }
            c.l("__e.end_array()?;");
            c.l("::core::result::Result::Ok(())");
        }
    }
    let _ = name;
    c.out()
}

fn field_ser_call(field: &crate::Field, fa: &FieldAttrs, cp: &str) -> String {
    let ident = field.ident.clone().unwrap_or_default();
    if let Some(p) = &fa.serialize_with {
        let subject = match &fa.getter {
            Some(g) => format!("{g}(&self)"),
            None => format!("&self.{ident}"),
        };
        format!("{p}({subject}, __e)?;")
    } else if let Some(m) = &fa.with {
        let subject = match &fa.getter {
            Some(g) => format!("{g}(&self)"),
            None => format!("&self.{ident}"),
        };
        format!("{m}::serialize({subject}, __e)?;")
    } else if let Some(g) = &fa.getter {
        // The getter returns a reference to the field; `&T: NsonSerialize`
        // forwards through the blanket impl, so the field type never needs
        // to be named (the external type's field may not even be a `String`
        // spelled the same way).
        format!("{cp}::NsonSerialize::nextencode({g}(&self), __e)?;")
    } else {
        format!(
            "<{} as {cp}::NsonSerialize>::nextencode(&self.{ident}, __e)?;",
            field.ty
        )
    }
}

fn field_ser_call_indexed(field: &crate::Field, i: usize, fa: &FieldAttrs, cp: &str) -> String {
    if let Some(p) = &fa.serialize_with {
        format!("{p}(&self.{i}, __e)?;")
    } else if let Some(m) = &fa.with {
        format!("{m}::serialize(&self.{i}, __e)?;")
    } else {
        format!(
            "<{} as {cp}::NsonSerialize>::nextencode(&self.{i}, __e)?;",
            field.ty
        )
    }
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

pub(crate) fn serialize_enum(
    name: &str,
    variants: &[crate::Variant],
    input: &Input,
    cp: &str,
) -> String {
    let ca = &input.cattr;
    let _ = name;
    if ca.transparent {
        return crate::err_str("nextjson: `transparent` is not supported on enums");
    }
    if ca.untagged && (ca.tag.is_some() || ca.content.is_some()) {
        return crate::err_str("nextjson: `untagged` cannot be combined with `tag` / `content`");
    }
    let mut c = Code::new();
    if let Some(tag) = &ca.tag {
        if let Some(content) = &ca.content {
            enum_arms(&mut c, variants, &Mode::Adjacent { tag, content }, ca, cp);
            c.l("::core::result::Result::Ok(())");
        } else {
            enum_arms(&mut c, variants, &Mode::Internal { tag }, ca, cp);
            c.l("::core::result::Result::Ok(())");
        }
    } else if ca.untagged {
        enum_arms(&mut c, variants, &Mode::Untagged, ca, cp);
        c.l("::core::result::Result::Ok(())");
    } else {
        c.l("__e.begin_object()?;");
        enum_arms(&mut c, variants, &Mode::External, ca, cp);
        c.l("__e.end_object()?;");
        c.l("::core::result::Result::Ok(())");
    }
    c.out()
}

enum Mode<'a> {
    External,
    Internal { tag: &'a str },
    Adjacent { tag: &'a str, content: &'a str },
    Untagged,
}

fn enum_arms(
    c: &mut Code,
    variants: &[crate::Variant],
    mode: &Mode,
    ca: &ContainerAttrs,
    cp: &str,
) {
    c.l("match self {");
    for v in variants {
        let va = attr::variant_attrs(&v.attrs);
        let vname = crate::schema::renamed_variant(v, &va, ca, true);
        if va.skip_serializing {
            c.l(&format!("Self::{} => {{}}", v.ident));
            continue;
        }
        let pat = variant_pat(&v.fields, &v.ident);
        let body = variant_body(&v.fields, &vname, mode, ca, cp, &va);
        c.l(&format!("{pat} => {{"));
        c.l(&body);
        c.l("}");
    }
    c.l("}");
}

fn variant_pat(fields: &Fields, ident: &str) -> String {
    match fields {
        Fields::Unit => format!("Self::{ident}"),
        Fields::Unnamed(f) if f.len() == 1 => format!("Self::{ident}(__v0)"),
        Fields::Unnamed(f) => {
            let binds: Vec<String> = (0..f.len()).map(|i| format!("__v{i}")).collect();
            format!("Self::{ident}({})", binds.join(", "))
        }
        Fields::Named(f) => {
            let pats: Vec<String> = f
                .iter()
                .filter(|fl| !attr::field_attrs(&fl.attrs).skip_serializing)
                .map(|fl| fl.ident.clone().unwrap_or_default())
                .collect();
            format!("Self::{ident} {{ {}, .. }}", pats.join(", "))
        }
    }
}

fn variant_body(
    fields: &Fields,
    vname: &str,
    mode: &Mode,
    ca: &ContainerAttrs,
    cp: &str,
    va: &crate::VariantAttrs,
) -> String {
    match mode {
        Mode::External => external_body(fields, vname, ca, cp, va),
        Mode::Internal { tag } => internal_body(fields, vname, tag, ca, cp, va),
        Mode::Adjacent { tag, content } => adjacent_body(fields, vname, tag, content, ca, cp, va),
        Mode::Untagged => untagged_body(fields, ca, cp, va),
    }
}

fn external_body(
    fields: &Fields,
    vname: &str,
    ca: &ContainerAttrs,
    cp: &str,
    va: &crate::VariantAttrs,
) -> String {
    let mut c = Code::new();
    c.l(&format!("__e.key({vname:?})?;"));
    content_write(&mut c, fields, ca, cp, va);
    c.out()
}

fn content_write(
    c: &mut Code,
    fields: &Fields,
    ca: &ContainerAttrs,
    cp: &str,
    va: &crate::VariantAttrs,
) {
    match fields {
        Fields::Unit => c.l("__e.write_null()?;"),
        Fields::Unnamed(f) if f.len() == 1 => {
            let fa = attr::newtype_field_attrs(&attr::field_attrs(&f[0].attrs), va);
            c.l(&variant_field_call(&f[0], &fa, "__v0", cp));
        }
        Fields::Unnamed(f) => {
            c.l("__e.begin_array()?;");
            for (i, field) in f.iter().enumerate() {
                let fa = attr::field_attrs(&field.attrs);
                if fa.skip_serializing {
                    continue;
                }
                c.l("__e.separator()?;");
                c.l(&variant_field_call(field, &fa, &format!("__v{i}"), cp));
            }
            c.l("__e.end_array()?;");
        }
        Fields::Named(f) => {
            c.l("__e.begin_object()?;");
            for field in f {
                let fa = attr::field_attrs(&field.attrs);
                if fa.skip_serializing {
                    continue;
                }
                let ident = field.ident.clone().unwrap_or_default();
                let key = crate::schema::renamed_variant_field(field, &fa, va, ca, true);
                let call = variant_field_call(field, &fa, &ident, cp);
                if let Some(pred) = &fa.skip_serializing_if {
                    c.l(&format!(
                        "if !({pred})({ident}) {{ __e.key({key:?})?; {call} }}"
                    ));
                } else {
                    c.l(&format!("__e.key({key:?})?;"));
                    c.l(&call);
                }
            }
            c.l("__e.end_object()?;");
        }
    }
}

fn variant_field_call(_field: &crate::Field, fa: &FieldAttrs, binder: &str, cp: &str) -> String {
    if let Some(p) = &fa.serialize_with {
        format!("{p}({binder}, __e)?;")
    } else if let Some(m) = &fa.with {
        format!("{m}::serialize({binder}, __e)?;")
    } else {
        format!("{cp}::NsonSerialize::nextencode({binder}, __e)?;")
    }
}

fn internal_body(
    fields: &Fields,
    vname: &str,
    tag: &str,
    ca: &ContainerAttrs,
    cp: &str,
    va: &crate::VariantAttrs,
) -> String {
    let mut c = Code::new();
    match fields {
        Fields::Unit => {
            c.l(&format!("__e.begin_object()?; __e.key({tag:?})?; __e.write_str({vname:?})?; __e.end_object()?;"));
        }
        Fields::Unnamed(f) => {
            let expr = if f.len() == 1 {
                "__v0".to_string()
            } else {
                let binds: Vec<String> = (0..f.len()).map(|i| format!("__v{i}")).collect();
                format!("&({})", binds.join(", "))
            };
            c.l(&format!("let __val = {cp}::to_value({expr})?;"));
            c.l(&format!(
                "{cp}::private::write_tagged_object(__e, {tag:?}, {vname:?}, __val)?;"
            ));
        }
        Fields::Named(f) => {
            c.l(&format!("let mut __m = {cp}::Map::new();"));
            for field in f {
                let fa = attr::field_attrs(&field.attrs);
                if fa.skip_serializing {
                    continue;
                }
                let ident = field.ident.clone().unwrap_or_default();
                let key = crate::schema::renamed_variant_field(field, &fa, va, ca, true);
                if let Some(pred) = &fa.skip_serializing_if {
                    c.l(&format!(
                        "if !({pred})({ident}) {{ __m.insert({key:?}.to_string(), {cp}::to_value({ident})?); }}"
                    ));
                } else {
                    c.l(&format!(
                        "__m.insert({key:?}.to_string(), {cp}::to_value({ident})?);"
                    ));
                }
            }
            c.l(&format!(
                "{cp}::private::write_tagged_object(__e, {tag:?}, {vname:?}, {cp}::Value::Object(__m))?;"
            ));
        }
    }
    let _ = ca;
    c.out()
}

fn adjacent_body(
    fields: &Fields,
    vname: &str,
    tag: &str,
    content: &str,
    ca: &ContainerAttrs,
    cp: &str,
    va: &crate::VariantAttrs,
) -> String {
    let mut c = Code::new();
    c.l("__e.begin_object()?;");
    c.l(&format!("__e.key({tag:?})?; __e.write_str({vname:?})?;"));
    match fields {
        Fields::Unit => {}
        Fields::Unnamed(f) if f.len() == 1 => {
            let fa = attr::newtype_field_attrs(&attr::field_attrs(&f[0].attrs), va);
            c.l(&format!("__e.key({content:?})?;"));
            c.l(&variant_field_call(&f[0], &fa, "__v0", cp));
        }
        Fields::Unnamed(f) => {
            c.l(&format!("__e.key({content:?})?;"));
            c.l("__e.begin_array()?;");
            for (i, field) in f.iter().enumerate() {
                let fa = attr::field_attrs(&field.attrs);
                if fa.skip_serializing {
                    continue;
                }
                c.l("__e.separator()?;");
                c.l(&variant_field_call(field, &fa, &format!("__v{i}"), cp));
            }
            c.l("__e.end_array()?;");
        }
        Fields::Named(f) => {
            c.l(&format!("__e.key({content:?})?;"));
            c.l("__e.begin_object()?;");
            for field in f {
                let fa = attr::field_attrs(&field.attrs);
                if fa.skip_serializing {
                    continue;
                }
                let ident = field.ident.clone().unwrap_or_default();
                let key = crate::schema::renamed_variant_field(field, &fa, va, ca, true);
                c.l(&format!("__e.key({key:?})?;"));
                c.l(&variant_field_call(field, &fa, &ident, cp));
            }
            c.l("__e.end_object()?;");
        }
    }
    c.l("__e.end_object()?;");
    c.out()
}

fn untagged_body(
    fields: &Fields,
    ca: &ContainerAttrs,
    cp: &str,
    va: &crate::VariantAttrs,
) -> String {
    let mut c = Code::new();
    content_write(&mut c, fields, ca, cp, va);
    c.out()
}
