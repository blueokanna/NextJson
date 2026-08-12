//! The cross_format relay entry and Error Display branch are covered

use nextjson::cross_format::{self, EventSink};
use nextjson::Error;

#[test]
fn cross_format_writer_and_pretty_entry_points() {
    let json = br#"{"a":[1,2],"b":"x"}"#;

    // writer form
    let mut sink = Vec::new();
    cross_format::json_to_cbor_writer(json, &mut sink).unwrap();
    assert!(!sink.is_empty());
    let mut sink2 = Vec::new();
    cross_format::cbor_to_json_writer(&sink, &mut sink2).unwrap();
    assert!(!sink2.is_empty());

    let pretty = cross_format::cbor_to_json_pretty(&sink).unwrap();
    assert!(String::from_utf8_lossy(&pretty).contains('\n'));
    let _ =
        cross_format::cbor_to_json_with_config(&sink, nextjson::EncodeConfig::default()).unwrap();

    struct Collect(Vec<String>);
    impl EventSink for Collect {
        fn null(&mut self) -> Result<(), Error> {
            self.0.push("null".into());
            Ok(())
        }
        fn boolean(&mut self, v: bool) -> Result<(), Error> {
            self.0.push(v.to_string());
            Ok(())
        }
        fn number(&mut self, v: nextjson::Number) -> Result<(), Error> {
            self.0.push(v.to_string());
            Ok(())
        }
        fn string(&mut self, s: &str) -> Result<(), Error> {
            self.0.push(s.to_string());
            Ok(())
        }
        fn begin_object(&mut self) -> Result<(), Error> {
            self.0.push("{".into());
            Ok(())
        }
        fn object_key(&mut self, k: &str) -> Result<(), Error> {
            self.0.push(k.into());
            Ok(())
        }
        fn end_object(&mut self) -> Result<(), Error> {
            self.0.push("}".into());
            Ok(())
        }
        fn begin_array(&mut self) -> Result<(), Error> {
            self.0.push("[".into());
            Ok(())
        }
        fn end_array(&mut self) -> Result<(), Error> {
            self.0.push("]".into());
            Ok(())
        }
    }
    let mut c = Collect(Vec::new());
    cross_format::json_into(br#"{"k":7}"#, &mut c).unwrap();
    assert!(c.0.len() >= 4);
    let mut c = Collect(Vec::new());
    cross_format::cbor_into(&sink, &mut c).unwrap();
    assert!(c.0.len() >= 4);

    // Error message: Incorrect input
    assert!(cross_format::json_to_cbor(b"{bad").is_err());
    assert!(cross_format::cbor_to_json(b"\xff").is_err());
}

#[test]
fn error_display_all_kinds() {
    use nextjson::{from_str, Value};
    assert_eq!(Error::custom("m").to_string(), "m");
    assert!(Error::missing_field("f").to_string().contains("f"));
    assert!(Error::unknown_field("k".into()).to_string().contains("k"));
    assert!(Error::unknown_variant("v".into()).to_string().contains("v"));
    assert!(Error::invalid_length(2, "a tuple")
        .to_string()
        .contains("2"));
    assert!(Error::invalid_type("x", "y").to_string().contains("x"));
    assert!(from_str::<Value>("")
        .unwrap_err()
        .to_string()
        .contains("end of input"));
    assert!(from_str::<Value>("1e")
        .unwrap_err()
        .to_string()
        .contains("invalid number"));
    assert!(from_str::<Value>("[1,2,]")
        .unwrap_err()
        .to_string()
        .contains("expected"));
    assert!(from_str::<Value>(r#""\q""#)
        .unwrap_err()
        .to_string()
        .contains("escape"));
    let deep = "[".repeat(200);
    assert!(from_str::<Value>(&deep)
        .unwrap_err()
        .to_string()
        .contains("recursion"));
}
