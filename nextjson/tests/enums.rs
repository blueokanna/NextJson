//! 派生宏：枚举的四种标签模式集成测试。

use nextjson::{from_str, to_string, Number, NsonDeserialize, NsonSerialize, Value};

// ---------------------------------------------------------------------------
// 外部标签（默认）
// ---------------------------------------------------------------------------

#[test]
fn external_unit_variants() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    enum Color {
        Red,
        Green,
        Blue,
    }
    assert_eq!(to_string(&Color::Red).unwrap(), r#"{"Red":null}"#);
    let back: Color = from_str(r#"{"Green":null}"#).unwrap();
    assert_eq!(back, Color::Green);
    assert!(from_str::<Color>(r#"{"Purple":null}"#).is_err());
}

#[test]
fn external_newtype_variant() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    enum Shape {
        Circle(f64),
        Rect { w: f64, h: f64 },
    }
    assert_eq!(to_string(&Shape::Circle(1.5)).unwrap(), r#"{"Circle":1.5}"#);
    let back: Shape = from_str(r#"{"Circle":2.0}"#).unwrap();
    assert_eq!(back, Shape::Circle(2.0));

    let r = Shape::Rect { w: 3.0, h: 4.0 };
    assert_eq!(to_string(&r).unwrap(), r#"{"Rect":{"w":3.0,"h":4.0}}"#);
    let back: Shape = from_str(r#"{"Rect":{"h":4.0,"w":3.0}}"#).unwrap();
    assert_eq!(back, r);
}

#[test]
fn external_tuple_variant() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    enum Event {
        Point(i32, i32),
    }
    assert_eq!(to_string(&Event::Point(1, 2)).unwrap(), r#"{"Point":[1,2]}"#);
    let back: Event = from_str(r#"{"Point":[1,2]}"#).unwrap();
    assert_eq!(back, Event::Point(1, 2));
    // 元素数量不符报错。
    assert!(from_str::<Event>(r#"{"Point":[1]}"#).is_err());
}

#[test]
fn rename_all_variants() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[njson(rename_all = "snake_case")]
    enum Status {
        InProgress,
        Done,
    }
    assert_eq!(
        to_string(&Status::InProgress).unwrap(),
        r#"{"in_progress":null}"#
    );
    let back: Status = from_str(r#"{"done":null}"#).unwrap();
    assert_eq!(back, Status::Done);
}

#[test]
fn variant_rename() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    enum E {
        #[njson(rename = "first")]
        First,
    }
    assert_eq!(to_string(&E::First).unwrap(), r#"{"first":null}"#);
    let back: E = from_str(r#"{"first":null}"#).unwrap();
    assert_eq!(back, E::First);
}

// ---------------------------------------------------------------------------
// 内部标签
// ---------------------------------------------------------------------------

#[test]
fn internally_tagged() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[njson(tag = "kind")]
    enum Msg {
        Hello { name: String },
        Bye,
    }
    let m = Msg::Hello { name: "alice".into() };
    assert_eq!(
        to_string(&m).unwrap(),
        r#"{"kind":"Hello","name":"alice"}"#
    );
    let back: Msg = from_str(r#"{"name":"bob","kind":"Hello"}"#).unwrap();
    assert_eq!(back, Msg::Hello { name: "bob".into() });

    assert_eq!(to_string(&Msg::Bye).unwrap(), r#"{"kind":"Bye"}"#);
    let back: Msg = from_str(r#"{"kind":"Bye"}"#).unwrap();
    assert_eq!(back, Msg::Bye);
    // 缺失标签报错。
    assert!(from_str::<Msg>(r#"{"name":"x"}"#).is_err());
    // 未知变体报错。
    assert!(from_str::<Msg>(r#"{"kind":"Nope"}"#).is_err());
}

#[test]
fn internally_tagged_newtype_map() {
    use std::collections::BTreeMap;
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[njson(tag = "type")]
    enum Cmd {
        Set(BTreeMap<String, i32>),
    }
    let c = Cmd::Set(BTreeMap::from([("k".to_string(), 1)]));
    assert_eq!(to_string(&c).unwrap(), r#"{"type":"Set","k":1}"#);
    let back: Cmd = from_str(r#"{"type":"Set","k":2}"#).unwrap();
    assert_eq!(back, Cmd::Set(BTreeMap::from([("k".to_string(), 2)])));
}

// ---------------------------------------------------------------------------
// 邻接标签
// ---------------------------------------------------------------------------

#[test]
fn adjacently_tagged() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[njson(tag = "t", content = "c")]
    enum Op {
        Add(i32, i32),
        Named { id: String },
        Clear,
    }
    let a = Op::Add(1, 2);
    assert_eq!(to_string(&a).unwrap(), r#"{"t":"Add","c":[1,2]}"#);
    let back: Op = from_str(r#"{"t":"Add","c":[3,4]}"#).unwrap();
    assert_eq!(back, Op::Add(3, 4));

    let n = Op::Named { id: "x".into() };
    assert_eq!(
        to_string(&n).unwrap(),
        r#"{"t":"Named","c":{"id":"x"}}"#
    );
    let back: Op = from_str(r#"{"t":"Named","c":{"id":"y"}}"#).unwrap();
    assert_eq!(back, Op::Named { id: "y".into() });

    assert_eq!(to_string(&Op::Clear).unwrap(), r#"{"t":"Clear"}"#);
    let back: Op = from_str(r#"{"t":"Clear"}"#).unwrap();
    assert_eq!(back, Op::Clear);
    // 单元变体不能带内容。
    assert!(from_str::<Op>(r#"{"t":"Clear","c":1}"#).is_err());
    // 未知字段报错。
    assert!(from_str::<Op>(r#"{"t":"Add","c":[1,2],"extra":1}"#).is_err());
}

// ---------------------------------------------------------------------------
// 无标签
// ---------------------------------------------------------------------------

#[test]
fn untagged() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[njson(untagged)]
    enum Value {
        Num(f64),
        Text(String),
        Flag(bool),
        List(Vec<i32>),
        Obj { x: i32 },
    }
    let v = Value::Num(1.5);
    assert_eq!(to_string(&v).unwrap(), "1.5");
    assert_eq!(from_str::<Value>("2.5").unwrap(), Value::Num(2.5));

    assert_eq!(to_string(&Value::Text("hi".into())).unwrap(), r#""hi""#);
    assert_eq!(from_str::<Value>(r#""yo""#).unwrap(), Value::Text("yo".into()));

    assert_eq!(to_string(&Value::Flag(true)).unwrap(), "true");
    assert_eq!(from_str::<Value>("false").unwrap(), Value::Flag(false));

    assert_eq!(to_string(&Value::List(vec![1, 2])).unwrap(), "[1,2]");
    assert_eq!(from_str::<Value>("[3]").unwrap(), Value::List(vec![3]));

    let o = Value::Obj { x: 7 };
    assert_eq!(to_string(&o).unwrap(), r#"{"x":7}"#);
    assert_eq!(from_str::<Value>(r#"{"x":9}"#).unwrap(), Value::Obj { x: 9 });

    // 全都不匹配报错。
    assert!(from_str::<Value>("null").is_err());
}

#[test]
fn untagged_fallback_order() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[njson(untagged)]
    enum Either {
        Int(i64),
        Float(f64),
    }
    // "42" 匹配第一个变体。
    assert_eq!(from_str::<Either>("42").unwrap(), Either::Int(42));
    // "4.5" 在第一个变体失败后回退到第二个。
    assert_eq!(from_str::<Either>("4.5").unwrap(), Either::Float(4.5));
}

// ---------------------------------------------------------------------------
// 综合：泛型枚举 + 属性组合
// ---------------------------------------------------------------------------

#[test]
fn generic_enum() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    enum Tree<T> {
        Leaf(T),
        Node {
            left: Box<Tree<T>>,
            right: Box<Tree<T>>,
        },
    }
    let t = Tree::Node {
        left: Box::new(Tree::Leaf(1)),
        right: Box::new(Tree::Leaf(2)),
    };
    let text = to_string(&t).unwrap();
    assert_eq!(
        text,
        r#"{"Node":{"left":{"Leaf":1},"right":{"Leaf":2}}}"#
    );
    let back: Tree<i32> = from_str(&text).unwrap();
    assert_eq!(back, t);
}

#[test]
fn enum_with_defaults_and_skips() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    #[njson(tag = "kind")]
    enum W {
        A {
            x: i32,
            #[njson(default)]
            y: i32,
            #[njson(skip_serializing, default)]
            secret: String,
        },
    }
    let w = W::A {
        x: 1,
        y: 2,
        secret: "s".into(),
    };
    assert_eq!(to_string(&w).unwrap(), r#"{"kind":"A","x":1,"y":2}"#);
    let back: W = from_str(r#"{"kind":"A","x":5}"#).unwrap();
    match back {
        W::A { x, y, secret } => {
            assert_eq!(x, 5);
            assert_eq!(y, 0);
            assert_eq!(secret, "");
        }
    }
}

#[test]
fn schema_of_works() {
    #[derive(NsonSerialize)]
    #[njson(rename_all = "camelCase")]
    struct Person {
        first_name: String,
        age: u32,
        tags: Vec<String>,
        active: bool,
    }
    let s = nextjson::schema_of::<Person>();
    assert_eq!(s.name(), "Person");
    let json_schema = nextjson::to_json_schema::<Person>();
    assert_eq!(json_schema["type"], "object".into());
    assert_eq!(json_schema["properties"]["firstName"]["type"], "string".into());
    assert_eq!(json_schema["properties"]["age"]["type"], "integer".into());
    assert_eq!(json_schema["properties"]["tags"]["type"], "array".into());
    assert_eq!(json_schema["properties"]["active"]["type"], "boolean".into());
}

#[test]
fn to_value_from_value() {
    #[derive(NsonSerialize, NsonDeserialize, Debug, PartialEq)]
    struct S {
        a: i32,
        b: Vec<String>,
    }
    let s = S {
        a: 1,
        b: vec!["x".into(), "y".into()],
    };
    let v = nextjson::to_value(&s).unwrap();
    assert_eq!(v["a"], Value::Number(Number::U64(1)));
    assert_eq!(v["b"][1], "y".into());
    let back: S = nextjson::from_value(v).unwrap();
    assert_eq!(back, s);
}
