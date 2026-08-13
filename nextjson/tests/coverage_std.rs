//! Round trips for every standard-library `NsonSerialize` / `NsonDeserialize`
//! implementation (smart pointers, collections, ranges, durations). These
//! impls are exercised nowhere else, so they dominate the `ser.rs` / `de.rs`
//! coverage gap.

use nextjson::{NsonDeserialize, NsonSerialize};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, LinkedList, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

fn rt<T>(value: &T)
where
    T: NsonSerialize + for<'de> NsonDeserialize<'de> + PartialEq + std::fmt::Debug,
{
    let bytes = nextjson::nextencode(value).unwrap();
    let back: T = nextjson::nextdecode(&bytes).unwrap();
    assert_eq!(&back, value);
}

#[test]
fn smart_pointer_roundtrips() {
    rt(&Box::new(7_i32));
    rt(&Rc::new(3_i32));
    rt(&Arc::new(4_i32));
    rt(&Cell::new(5_i32));
    rt(&RefCell::new(6_i32));
    // `&T` serializes through the underlying type (owned decode target).
    let v = 9_i32;
    let bytes = nextjson::nextencode(&(&v)).unwrap();
    assert_eq!(nextjson::nextdecode::<i32>(&bytes).unwrap(), 9);
}

#[test]
fn collection_roundtrips() {
    rt(&vec![1_i32, 2, 3]);
    rt(&[1_i32, 2, 3]);
    // Unsized slice: serializes, decodes as owned `Vec`.
    let slice: &[i32] = &[1, 2, 3];
    let bytes = nextjson::nextencode(&slice).unwrap();
    assert_eq!(
        nextjson::nextdecode::<Vec<i32>>(&bytes).unwrap(),
        vec![1, 2, 3]
    );
    rt(&VecDeque::from(vec![1_i32, 2]));
    rt(&LinkedList::from([1_i32, 2]));
    rt(&BTreeMap::from([(1_i32, "a".to_string())]));
    rt(&BTreeSet::from([1_i32, 2]));
    // BinaryHeap has no PartialEq; verify encode + decode separately.
    let mut heap = BinaryHeap::new();
    heap.push(3_i32);
    heap.push(1);
    let bytes = nextjson::nextencode(&heap).unwrap();
    let mut back: BinaryHeap<i32> = nextjson::nextdecode(&bytes).unwrap();
    assert_eq!(back.pop(), Some(3));
    assert_eq!(back.pop(), Some(1));
    rt(&String::from("s"));
    rt(&Box::<str>::from("boxed"));
}

#[test]
fn cow_and_boxed_bytes_roundtrip() {
    let s: Cow<'static, str> = Cow::Borrowed("cow");
    let bytes = nextjson::nextencode(&s).unwrap();
    // The decoded Cow borrows from `bytes`, so use an inferred lifetime.
    let back: Cow<'_, str> = nextjson::nextdecode(&bytes).unwrap();
    assert_eq!(back, "cow");

    let owned: Cow<'static, str> = Cow::Owned(String::from("owned"));
    let bytes = nextjson::nextencode(&owned).unwrap();
    let back: Cow<'_, str> = nextjson::nextdecode(&bytes).unwrap();
    assert_eq!(back, "owned");

    let b: Box<[u8]> = Box::from(&b"bytes"[..]);
    let bytes = nextjson::nextencode(&b).unwrap();
    assert_eq!(nextjson::nextdecode::<Box<[u8]>>(&bytes).unwrap(), b);

    // `Cow<[u8]>` only has a deserializer (bytes form); decode a JSON string.
    let decoded: Cow<'static, [u8]> = nextjson::nextdecode(br#""abc""#).unwrap();
    assert_eq!(decoded.as_ref(), b"abc");
}

#[test]
fn range_and_duration_roundtrips() {
    rt(&(1_i32..5));
    rt(&(1_i32..));
    rt(&(1_i32..=5));
    rt(&(..5));
    rt(&(..=5));
    rt(&Duration::new(1, 500_000_000));
    rt(&Duration::from_millis(0));
}

#[test]
fn tuple_unit_char_bool_roundtrips() {
    // A tuple containing `&str` cannot satisfy the HRTB deserialize bound
    // (the borrow is tied to one input lifetime), so decode to an owned form.
    let t = (1_i32, "two");
    let bytes = nextjson::nextencode(&t).unwrap();
    assert_eq!(
        nextjson::nextdecode::<(i32, String)>(&bytes).unwrap(),
        (1, String::from("two"))
    );
    rt(&(1_i32, 2_i32, 3_i32));
    rt(&());
    rt(&'x');
    rt(&true);
    rt(&false);
}

#[test]
fn borrowed_and_owned_str_surface() {
    // &str decode borrows when unescaped.
    let input = r#""borrow-me""#;
    let v: &str = nextjson::from_str(input).unwrap();
    assert_eq!(v, "borrow-me");
    assert_eq!(v.as_ptr(), input.as_ptr().wrapping_add(1));
    // escaped strings materialize owned.
    let v: String = nextjson::from_str(r#""a\nb""#).unwrap();
    assert_eq!(v, "a\nb");
}
