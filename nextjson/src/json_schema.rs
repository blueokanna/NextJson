//! JSON Schema (draft-07 style) generator — a product of the schema innovation.

use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::map::Map;
use crate::number::Number;
use crate::schema::TypeSchema;
use crate::value::Value;

/// Convert a [`TypeSchema`] into a JSON Schema description.
pub(crate) fn from_schema(schema: TypeSchema) -> Value {
    let mut obj = Map::new();
    fill_schema(schema, &mut obj);
    Value::Object(obj)
}

fn fill_schema(schema: TypeSchema, obj: &mut Map) {
    match schema {
        TypeSchema::Unit => {
            obj.insert("type".into(), Value::String("null".into()));
        }
        TypeSchema::Bool => {
            obj.insert("type".into(), Value::String("boolean".into()));
        }
        TypeSchema::I8
        | TypeSchema::I16
        | TypeSchema::I32
        | TypeSchema::I64
        | TypeSchema::I128
        | TypeSchema::Isize => {
            obj.insert("type".into(), Value::String("integer".into()));
        }
        TypeSchema::U8
        | TypeSchema::U16
        | TypeSchema::U32
        | TypeSchema::U64
        | TypeSchema::U128
        | TypeSchema::Usize => {
            obj.insert("type".into(), Value::String("integer".into()));
            obj.insert("minimum".into(), Value::Number(Number::U64(0)));
        }
        TypeSchema::F32 | TypeSchema::F64 => {
            obj.insert("type".into(), Value::String("number".into()));
        }
        TypeSchema::Char | TypeSchema::Str => {
            obj.insert("type".into(), Value::String("string".into()));
        }
        TypeSchema::Bytes => {
            obj.insert("type".into(), Value::String("array".into()));
            let mut items = Map::new();
            items.insert("type".into(), Value::String("integer".into()));
            items.insert("minimum".into(), Value::Number(Number::U64(0)));
            items.insert("maximum".into(), Value::Number(Number::U64(255)));
            obj.insert("items".into(), Value::Object(items));
        }
        TypeSchema::Opaque => {}
        TypeSchema::Seq(inner) => {
            obj.insert("type".into(), Value::String("array".into()));
            let mut items = Map::new();
            fill_schema(*inner, &mut items);
            obj.insert("items".into(), Value::Object(items));
        }
        TypeSchema::Map(inner) => {
            obj.insert("type".into(), Value::String("object".into()));
            let mut extra = Map::new();
            fill_schema(*inner, &mut extra);
            obj.insert("additionalProperties".into(), Value::Object(extra));
        }
        TypeSchema::Optional(inner) => {
            fill_schema(*inner, obj);
            obj.insert("nullable".into(), Value::Bool(true));
        }
        TypeSchema::Tuple(items) => {
            obj.insert("type".into(), Value::String("array".into()));
            let arr: Vec<Value> = items
                .iter()
                .map(|&t| {
                    let mut m = Map::new();
                    fill_schema(t, &mut m);
                    Value::Object(m)
                })
                .collect();
            obj.insert("items".into(), Value::Array(arr));
            obj.insert(
                "minItems".into(),
                Value::Number(Number::U64(items.len() as u64)),
            );
            obj.insert(
                "maxItems".into(),
                Value::Number(Number::U64(items.len() as u64)),
            );
        }
        TypeSchema::Struct(s) => {
            obj.insert("type".into(), Value::String("object".into()));
            obj.insert("title".into(), Value::String(s.name.to_string()));
            let mut props = Map::new();
            let mut required = Vec::new();
            for f in s.fields {
                let mut fm = Map::new();
                fill_schema(f.ty, &mut fm);
                props.insert(f.name.to_string(), Value::Object(fm));
                if f.required {
                    required.push(Value::String(f.name.to_string()));
                }
            }
            obj.insert("properties".into(), Value::Object(props));
            if !required.is_empty() {
                obj.insert("required".into(), Value::Array(required));
            }
            obj.insert("additionalProperties".into(), Value::Bool(!s.transparent));
        }
        TypeSchema::Enum(e) => {
            obj.insert("title".into(), Value::String(e.name.to_string()));
            if e.untagged {
                let one_of: Vec<Value> = e
                    .variants
                    .iter()
                    .map(|v| {
                        let mut m = Map::new();
                        fill_schema(v.ty, &mut m);
                        Value::Object(m)
                    })
                    .collect();
                obj.insert("oneOf".into(), Value::Array(one_of));
            } else if e.tag.is_none()
                && e.content.is_none()
                && e.variants.iter().all(|v| v.ty == TypeSchema::Unit)
            {
                obj.insert("type".into(), Value::String("string".into()));
                let enums: Vec<Value> = e
                    .variants
                    .iter()
                    .map(|v| Value::String(v.name.to_string()))
                    .collect();
                obj.insert("enum".into(), Value::Array(enums));
            } else {
                let one_of: Vec<Value> = e
                    .variants
                    .iter()
                    .map(|v| {
                        let m = if let Some(tag) = e.tag {
                            let mut tag_props = Map::new();
                            tag_props.insert(
                                tag.to_string(),
                                Value::Object(Map::from_iter(vec![(
                                    "enum".to_string(),
                                    Value::Array(vec![Value::String(v.name.to_string())]),
                                )])),
                            );
                            let mut content = Map::new();
                            fill_schema(v.ty, &mut content);
                            let mut merged = tag_props;
                            for (k, val) in content.iter() {
                                merged.insert(k.to_string(), val.clone());
                            }
                            merged
                        } else {
                            let mut inner = Map::new();
                            fill_schema(v.ty, &mut inner);
                            inner
                        };
                        Value::Object(m)
                    })
                    .collect();
                obj.insert("oneOf".into(), Value::Array(one_of));
            }
        }
    }
}
