//! Independent wire checks against the locked benchmark reference codecs.
use nextjson::formats::{Format, MsgPack, Ron};

#[test]
fn shared_float_notation_is_accepted_by_yaml_and_toml() {
    #[derive(nextjson::NsonSerialize, serde::Deserialize)]
    struct Field {
        value: f64,
    }
    for value in [
        0.0,
        -0.0,
        1e-6,
        1e16,
        1e100,
        f64::from_bits(1),
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::MIN,
    ] {
        let field = Field { value };
        let yaml = nextjson::formats::Yaml.encode(&field).unwrap();
        let actual: Field = serde_yaml::from_slice(&yaml).unwrap();
        assert_eq!(actual.value.to_bits(), value.to_bits());
        let toml = nextjson::formats::Toml.encode(&field).unwrap();
        let actual: Field = toml::from_str(std::str::from_utf8(&toml).unwrap()).unwrap();
        assert_eq!(actual.value.to_bits(), value.to_bits());
    }
}

#[test]
fn ron_options_and_string_escapes_cross_decode() {
    for value in [
        'a', '\'', '\\', '"', '\0', '\u{8}', '\n', '\r', '\t', '中', '🙂',
    ] {
        assert_eq!(
            ron::de::from_bytes::<char>(&Ron.encode(&value).unwrap()).unwrap(),
            value
        );
        assert_eq!(
            Ron.decode::<char>(ron::to_string(&value).unwrap().as_bytes())
                .unwrap(),
            value
        );
    }
    for value in [None, Some(None), Some(Some(()))] {
        let wire = Ron.encode(&value).unwrap();
        assert_eq!(
            ron::de::from_bytes::<Option<Option<()>>>(&wire).unwrap(),
            value
        );
        let wire = ron::to_string(&value).unwrap();
        assert_eq!(
            Ron.decode::<Option<Option<()>>>(wire.as_bytes()).unwrap(),
            value
        );
    }
    for value in ["\0\u{8}\u{c}\n\t\r\"\\中文🙂", "unescaped ASCII"] {
        assert_eq!(
            ron::de::from_bytes::<String>(&Ron.encode(value).unwrap()).unwrap(),
            value
        );
        assert_eq!(
            Ron.decode::<String>(ron::to_string(value).unwrap().as_bytes())
                .unwrap(),
            value
        );
    }
}

#[test]
fn messagepack_sized_sequences_match_reference_bytes() {
    for size in [0, 15, 16, 65535, 65536] {
        let value: Vec<u64> = (0..size).collect();
        let actual = MsgPack.encode(&value).unwrap();
        let reference = rmp_serde::to_vec(&value).unwrap();
        assert_eq!(actual, reference);
        assert_eq!(MsgPack.decode::<Vec<u64>>(&reference).unwrap(), value);
    }
}
