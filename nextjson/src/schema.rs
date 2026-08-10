//! Compile-time schema: one of the core innovations of `nextjson`.
//!
//! Every [`NsonSerialize`](crate::NsonSerialize) type carries a
//! `const SCHEMA: TypeSchema` — a metadata tree constructed at compile time
//! and introspectable at runtime. serde's derives leave no such shape behind.

/// Compile-time description of a type's structure.
///
/// `Copy` and reference-only, so it can be constructed in `const` context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeSchema {
    /// Unit (`()`, unit structs, unit variants).
    Unit,
    /// `bool`
    Bool,
    /// `i8`
    I8,
    /// `i16`
    I16,
    /// `i32`
    I32,
    /// `i64`
    I64,
    /// `i128`
    I128,
    /// `isize`
    Isize,
    /// `u8`
    U8,
    /// `u16`
    U16,
    /// `u32`
    U32,
    /// `u64`
    U64,
    /// `u128`
    U128,
    /// `usize`
    Usize,
    /// `f32`
    F32,
    /// `f64`
    F64,
    /// `char`
    Char,
    /// string (`String` / `&str` / `Cow<str>`)
    Str,
    /// byte sequence (`&[u8]`)
    Bytes,
    /// unknown / opaque type (e.g. `skip_serializing` fields)
    Opaque,
    /// sequence (`Vec<T>`, `[T; N]`, slices)
    Seq(&'static TypeSchema),
    /// map (`HashMap<K, V>` etc.), describing the value type
    Map(&'static TypeSchema),
    /// `Option<T>`
    Optional(&'static TypeSchema),
    /// fixed-size tuple
    Tuple(&'static [TypeSchema]),
    /// struct
    Struct(&'static StructSchema),
    /// enum
    Enum(&'static EnumSchema),
}

impl TypeSchema {
    /// Human-readable short type name.
    pub fn name(&self) -> &'static str {
        match self {
            TypeSchema::Unit => "unit",
            TypeSchema::Bool => "bool",
            TypeSchema::I8 => "i8",
            TypeSchema::I16 => "i16",
            TypeSchema::I32 => "i32",
            TypeSchema::I64 => "i64",
            TypeSchema::I128 => "i128",
            TypeSchema::Isize => "isize",
            TypeSchema::U8 => "u8",
            TypeSchema::U16 => "u16",
            TypeSchema::U32 => "u32",
            TypeSchema::U64 => "u64",
            TypeSchema::U128 => "u128",
            TypeSchema::Usize => "usize",
            TypeSchema::F32 => "f32",
            TypeSchema::F64 => "f64",
            TypeSchema::Char => "char",
            TypeSchema::Str => "string",
            TypeSchema::Bytes => "bytes",
            TypeSchema::Opaque => "opaque",
            TypeSchema::Seq(_) => "sequence",
            TypeSchema::Map(_) => "map",
            TypeSchema::Optional(inner) => inner.name(),
            TypeSchema::Tuple(_) => "tuple",
            TypeSchema::Struct(s) => s.name,
            TypeSchema::Enum(e) => e.name,
        }
    }

    /// Whether this is a JSON object.
    pub fn is_object(&self) -> bool {
        matches!(self, TypeSchema::Struct(_) | TypeSchema::Map(_))
    }
}

/// Compile-time description of a struct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StructSchema {
    /// Type name.
    pub name: &'static str,
    /// Whether `#[njson(transparent)]`.
    pub transparent: bool,
    /// Field list.
    pub fields: &'static [FieldSchema],
}

/// Compile-time description of a struct field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldSchema {
    /// Serialized field name (after `rename` / `rename_all`).
    pub name: &'static str,
    /// Original Rust field name (for error messages).
    pub orig: &'static str,
    /// Whether the field is required.
    pub required: bool,
    /// Whether `#[njson(flatten)]`.
    pub flattened: bool,
    /// Field type.
    pub ty: TypeSchema,
}

/// Compile-time description of an enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnumSchema {
    /// Type name.
    pub name: &'static str,
    /// `#[njson(tag = "...")]` (internally tagged).
    pub tag: Option<&'static str>,
    /// `#[njson(content = "...")]` (adjacently tagged).
    pub content: Option<&'static str>,
    /// Whether `#[njson(untagged)]`.
    pub untagged: bool,
    /// Default tag field name for internal / adjacent tagging.
    pub default_tag: &'static str,
    /// Variant list.
    pub variants: &'static [VariantSchema],
}

/// Compile-time description of an enum variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VariantSchema {
    /// Serialized variant name.
    pub name: &'static str,
    /// Original Rust variant name.
    pub orig: &'static str,
    /// Variant content type.
    pub ty: TypeSchema,
}

/// Compile-time schema provider.
///
/// [`NsonSerialize`](crate::NsonSerialize) uses this as a supertrait, so any
/// serializable type can be introspected via [`schema_of`](crate::schema_of).
pub trait NsonSchema {
    /// The compile-time structural description.
    const SCHEMA: TypeSchema;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_const_constructible() {
        const S: TypeSchema = TypeSchema::Struct(&StructSchema {
            name: "Foo",
            transparent: false,
            fields: &[FieldSchema {
                name: "x",
                orig: "x",
                required: true,
                flattened: false,
                ty: TypeSchema::F64,
            }],
        });
        assert_eq!(S.name(), "Foo");
        assert!(S.is_object());
    }

    #[test]
    fn optional_name_unwraps() {
        assert_eq!(TypeSchema::Optional(&TypeSchema::I32).name(), "i32");
    }
}
