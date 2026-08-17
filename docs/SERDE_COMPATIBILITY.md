# Serde compatibility contract

NextJson is not a drop-in implementation of Serde. Accepting `#[serde(...)]`
is a migration aid for verified attributes; it does not make Serde traits,
visitors, adapters, data models, or errors interchangeable.

## Verified derive behavior

| Area | Contract |
| --- | --- |
| Names | Shared and directional `rename`, `rename_all`, and `rename_all_fields` are honored. |
| Missing data | Field/container `default`, `Option`, directional skip, and aliases follow the tested derive behavior. |
| Duplicate typed fields | Rejected with `duplicate field`, matching Serde derive. Dynamic `Value` maps retain their documented last-wins policy. |
| Enums | External, internal, adjacent, untagged, aliases, and unit `other` are covered by integration tests. |
| Flatten | Object/map roots only. Serialization is direct and allocation-free; deserialization stages remaining entries. |
| Unknown attributes | Rejected at derive time. Wire-affecting metadata is never silently ignored. |
| Custom codecs | `with`, `serialize_with`, and `deserialize_with` use `FormatEncoder`/`FormatDecoder` signatures, not Serde `Serializer`/`Deserializer` signatures. Existing Serde adapter functions must be ported. |

## Deliberate model differences

- There is no Serde `Visitor`, `SeqAccess`, `MapAccess`, or `DeserializeSeed`.
  `NsonDeserialize::nextdecode_into` writes through a checked `DecodeSlot`.
- Error variants and locations are NextJson's contract. Parser errors carry
  byte positions; errors created by generic derive code are semantic errors
  and are not claimed to be byte-for-byte equal to `serde_json::Error`.
- `#[serde(crate = "...")]` names a crate in Serde's trait universe. For a
  NextJson derive it must name the NextJson crate path, so one shared value is
  not generally valid for both derive systems.
- Text and binary formats expose different data models through
  `is_human_readable`; format-specific Serde behavior is not inferred.

## Cryptography and chain types

No blanket compatibility is claimed for external crates.

| Type family | Built-in coverage | Required verification |
| --- | --- | --- |
| Rust integers | Exact through `u128`/`i128`; CBOR uses tags 2/3 inside that domain. | Larger external integers need an explicit decimal/hex/byte adapter with overflow and canonical-form tests. |
| Fixed bytes | `[u8; N]` is a generic sequence; `Bytes<'a>` selects native byte strings where supported. | Verify exact length, owned/borrowed behavior, and the intended human-readable spelling. |
| Newtypes | Transparent and remote helper generation is available inside NextJson's trait model. | It does not import a foreign crate's Serde implementation; confirm visibility, conversion, schema, and directional representation against the real type. |
| Curve points/signatures | No built-in semantic adapter. | Decode to fixed bytes, enforce length/prefix/canonical encoding, then call the curve library's validated constructor. Never treat arbitrary bytes as a valid point. |
| Feature-gated foreign types | No implicit implementation. | Compile and round-trip every enabled feature combination in the downstream crate. |

An external adapter is considered covered only when its real crate version and
feature set are compiled in the downstream project and tested against known
wire vectors. A same-named attribute or a local stand-in is not that proof.
