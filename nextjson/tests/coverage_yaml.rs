//! Deep coverage for the YAML codec's feature branches (tags, merge keys,
//! anchors/aliases, block scalars) using real documents.

use nextjson::formats::{self, Format};
use nextjson::Value;

fn yaml(input: &str) -> Value {
    formats::Yaml.decode(input.as_bytes()).unwrap()
}

#[test]
fn yaml_tags_force_scalar_types() {
    let v = yaml(
        "a: !!str 123\n\
         b: !!int '456'\n\
         c: !!float 7\n\
         d: !!bool 'false'\n\
         e: !!null anything\n\
         f: !!str 'quoted'\n",
    );
    assert_eq!(v["a"], Value::from("123"));
    assert_eq!(v["b"], Value::from(456_i64));
    assert_eq!(v["c"], Value::from(7.0_f64));
    assert_eq!(v["d"], Value::from(false));
    assert_eq!(v["e"], Value::Null);
    assert_eq!(v["f"], Value::from("quoted"));
}

#[test]
fn yaml_merge_keys_existing_keys_win() {
    let v = yaml(
        "base: &b\n  x: 1\n  y: 2\n\
         child:\n  <<: *b\n  y: 99\n  z: 3\n",
    );
    assert_eq!(v["child"]["x"], Value::from(1_i64));
    assert_eq!(v["child"]["y"], Value::from(99_i64)); // explicit wins
    assert_eq!(v["child"]["z"], Value::from(3_i64));
}

#[test]
fn yaml_anchors_in_sequences_and_nested_blocks() {
    let v = yaml(
        "list:\n  - &a {k: 1}\n  - *a\n  - &s hello\n  - *s\n\
         deep:\n  inner: &d\n    ref: *s\n    x: [1, 2]\n  copy: *d\n",
    );
    assert_eq!(v["list"][0], v["list"][1]);
    assert_eq!(v["list"][2], Value::from("hello"));
    assert_eq!(v["list"][3], Value::from("hello"));
    assert_eq!(v["deep"]["copy"]["ref"], Value::from("hello"));
    assert_eq!(v["deep"]["copy"]["x"][1], Value::from(2_i64));
}

#[test]
fn yaml_block_scalars_chomping_and_folding() {
    // Literal, strip, keep, folded with blank lines.
    let lit = yaml("t: |\n  a\n  b\n");
    assert_eq!(lit["t"], Value::from("a\nb\n"));
    let strip = yaml("t: |-\n  a\n  b\n\n");
    assert_eq!(strip["t"], Value::from("a\nb"));
    let keep = yaml("t: |+\n  a\n\n  b\n\n\n");
    assert_eq!(keep["t"], Value::from("a\n\nb\n\n\n"));
    let folded = yaml("t: >\n  a\n  b\n\n  c\n");
    assert_eq!(folded["t"], Value::from("a b\nc\n"));
}

#[test]
fn yaml_flow_and_block_mixed() {
    let v = yaml("top:\n  - {a: [1, 2], b: {c: true}}\n  - name: x\n    vals: [1.5, null]\n");
    assert_eq!(v["top"][0]["a"][1], Value::from(2_i64));
    assert_eq!(v["top"][0]["b"]["c"], Value::from(true));
    assert_eq!(v["top"][1]["vals"][0], Value::from(1.5_f64));
    assert_eq!(v["top"][1]["vals"][1], Value::Null);
}

#[test]
fn yaml_quoted_keys_and_colon_in_strings() {
    let v = yaml(
        r#""a:b": 1
'c d': 2
plain: "with: colon"
"#,
    );
    assert_eq!(v["a:b"], Value::from(1_i64));
    assert_eq!(v["c d"], Value::from(2_i64));
    assert_eq!(v["plain"], Value::from("with: colon"));
}

#[test]
fn yaml_document_end_marker_and_blank_lines() {
    let v = yaml("a: 1\n\n\n...\n");
    assert_eq!(v["a"], Value::from(1_i64));
    let v = yaml("---\na: 1\n...\n");
    assert_eq!(v["a"], Value::from(1_i64));
}

#[test]
fn yaml_scalar_casing_and_bool_forms() {
    let v = yaml("t1: true\nt2: True\nt3: TRUE\nf1: false\nf2: False\nf3: FALSE\nn1: null\nn2: Null\nn3: NULL\nn4: ~\n");
    assert_eq!(v["t1"], Value::from(true));
    assert_eq!(v["t2"], Value::from(true));
    assert_eq!(v["t3"], Value::from(true));
    assert_eq!(v["f1"], Value::from(false));
    assert_eq!(v["f3"], Value::from(false));
    assert_eq!(v["n1"], Value::Null);
    assert_eq!(v["n4"], Value::Null);
}

#[test]
fn yaml_special_strings_stay_strings() {
    // Date-like and colon-bearing plain scalars remain strings.
    let v = yaml("d: 2024-01-02\nurl: http://example.com\ntime: 12:30:00\n");
    assert_eq!(v["d"], Value::from("2024-01-02"));
    assert_eq!(v["url"], Value::from("http://example.com"));
    assert_eq!(v["time"], Value::from("12:30:00"));
}
