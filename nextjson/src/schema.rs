//! Compile-time schema: one of the core innovations of `nextjson`.
//!
//! Every [`NsonSerialize`](crate::NsonSerialize) type carries a
//! `const SCHEMA: TypeSchema` - a metadata tree constructed at compile time
//! and introspectable at runtime.

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

/// Inclusive integer range of an integer schema (`(min, max)`), if it is an
/// integer type. Shared by the validator and the compatibility checker.
pub(crate) fn integer_range(schema: TypeSchema) -> Option<(i128, i128)> {
    match schema {
        TypeSchema::I8 => Some((i8::MIN as i128, i8::MAX as i128)),
        TypeSchema::I16 => Some((i16::MIN as i128, i16::MAX as i128)),
        TypeSchema::I32 => Some((i32::MIN as i128, i32::MAX as i128)),
        TypeSchema::I64 => Some((i64::MIN as i128, i64::MAX as i128)),
        TypeSchema::I128 => Some((i128::MIN, i128::MAX)),
        TypeSchema::Isize => Some((isize::MIN as i128, isize::MAX as i128)),
        TypeSchema::U8 => Some((0, u8::MAX as i128)),
        TypeSchema::U16 => Some((0, u16::MAX as i128)),
        TypeSchema::U32 => Some((0, u32::MAX as i128)),
        TypeSchema::U64 => Some((0, u64::MAX as i128)),
        // `u128` can exceed `i128::MAX`; callers handle that special case.
        TypeSchema::U128 => Some((0, i128::MAX)),
        TypeSchema::Usize => Some((0, usize::MAX as i128)),
        _ => None,
    }
}

/// Whether the schema is a floating-point type.
pub(crate) fn is_float_type(schema: TypeSchema) -> bool {
    matches!(schema, TypeSchema::F32 | TypeSchema::F64)
}

/// A declared safety policy attached to a schema node.
///
/// Policies are compile-time declarations (const-constructible) that turn the
/// schema from "what data looks like" into "what data is allowed to enter the
/// system". They are declared through derive attributes such as
/// `#[njson(max_str_len = 32)]` / `#[njson(sensitive)]` and enforced at runtime
/// by [`crate::validate`](crate::validate).
///
/// The limits are shape-driven: when validating a value, `max_str_len` applies
/// to strings, `max_items` to arrays and objects, `min` / `max` to numbers.
/// A policy on a field whose value does not have the matching shape is simply
/// not triggered. `sensitive` is metadata: the validator reports the path but
/// never fails on it, so logging and monitoring code can redact the value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// Maximum string length in Unicode scalar values.
    pub max_str_len: Option<u64>,
    /// Maximum number of elements (arrays) or entries (objects).
    pub max_items: Option<u64>,
    /// Inclusive numeric lower bound.
    pub min: Option<i128>,
    /// Inclusive numeric upper bound.
    pub max: Option<i128>,
    /// Whether the value is sensitive (must not be logged or exposed).
    pub sensitive: bool,
}

/// Compile-time description of a struct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StructSchema {
    /// Type name.
    pub name: &'static str,
    /// Whether `#[njson(transparent)]`.
    pub transparent: bool,
    /// Maximum container nesting allowed below this struct
    /// (`#[njson(max_depth = N)]`).
    pub max_depth: Option<u64>,
    /// Whether unknown fields are rejected
    /// (`#[njson(deny_unknown_fields)]`).
    pub deny_unknown_fields: bool,
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
    /// Declared safety policy for this field.
    pub policy: Policy,
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
    /// Maximum container nesting allowed below this enum
    /// (`#[njson(max_depth = N)]`).
    pub max_depth: Option<u64>,
    /// Whether unknown fields are rejected
    /// (`#[njson(deny_unknown_fields)]`).
    pub deny_unknown_fields: bool,
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
    /// Declared safety policy for a newtype variant's contained field
    /// (empty for unit / tuple / struct variants, whose fields carry their
    /// own policies).
    pub policy: Policy,
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
            max_depth: Some(4),
            deny_unknown_fields: true,
            fields: &[FieldSchema {
                name: "x",
                orig: "x",
                required: true,
                flattened: false,
                policy: Policy {
                    min: Some(0),
                    max: Some(100),
                    max_str_len: None,
                    max_items: None,
                    sensitive: false,
                },
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
