use super::*;
use serde_json::json;

/// Minimal deserializer that hands an owned byte buffer to the visitor, which
/// serde_json itself never does for string input.
struct OwnedBytes(Vec<u8>);

impl<'de> Deserializer<'de> for OwnedBytes {
    type Error = serde::de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> std::result::Result<V::Value, Self::Error> {
        visitor.visit_byte_buf(self.0)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
    }
}

#[test]
fn text_accepts_owned_byte_buffers_and_rejects_non_strings() {
    // Owned buffer path keeps lone-surrogate WTF-8 bytes intact.
    let surrogate = vec![0xed, 0xa0, 0x80];
    let text = match Text::deserialize(OwnedBytes(surrogate.clone())) {
        Ok(text) => text,
        Err(error) => panic!("owned bytes should deserialize: {error}"),
    };
    assert_eq!(text.0, surrogate);
    let mut quoted = vec![];
    quote(&text.0, &mut quoted);
    assert_eq!(quoted, br#""\ud800""#);

    let plain = match Text::deserialize(OwnedBytes(b"PUBLIC-text".to_vec())) {
        Ok(text) => text,
        Err(error) => panic!("owned bytes should deserialize: {error}"),
    };
    assert_eq!(plain.0, b"PUBLIC-text");

    // Non-string JSON reports the visitor's expectation.
    for input in ["1", "true", "null"] {
        let error = match serde_json::from_str::<Text>(input) {
            Ok(_) => panic!("non-string JSON must not deserialize as text: {input}"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("expected a JSON string"),
            "{input}: {error}"
        );
    }
}

#[test]
fn field_requires_an_object_and_finds_named_members() {
    for input in ["[1]", "\"PUBLIC-string\"", "42"] {
        let raw: &RawValue = serde_json::from_str(input).unwrap();
        let error = field(raw, "descriptor").expect_err("non-object JSON must not yield fields");
        assert!(
            error.to_string().contains("expected a JSON object"),
            "{input}: {error}"
        );
    }

    let raw: &RawValue =
        serde_json::from_str(r#"{"descriptor":{"name":"PUBLIC-a"},"other":1}"#).unwrap();
    assert_eq!(
        field(raw, "descriptor").unwrap().map(RawValue::get),
        Some(r#"{"name":"PUBLIC-a"}"#)
    );
    assert!(field(raw, "missing").unwrap().is_none());
}

#[test]
fn stringify_escapes_quotes_backslashes_and_short_control_characters() {
    for (input, expected) in [
        (r#""q\"b""#, r#""q\"b""#),
        (r#""\\""#, r#""\\""#),
        (r#""\b\f""#, r#""\b\f""#),
        (r#""a\u0008b\u000cc""#, r#""a\bb\fc""#),
        (r#""\u0001\u001f""#, r#""\u0001\u001f""#),
        (r#"{"\"k\\":"\f\b"}"#, r#"{"\"k\\":"\f\b"}"#),
    ] {
        assert_eq!(
            String::from_utf8(stringify(input).unwrap()).unwrap(),
            expected,
            "{input}"
        );
    }
}

#[test]
fn result_digest_matches_raw_envelope_digest_and_rejects_non_array_descriptors() {
    let run_id = "PUBLIC-run";
    let result = json!({"answerText":"PUBLIC-answer","n":1.0});
    let descriptors = json!([{"name":"PUBLIC-a","size":2}, null]);
    let digest = result_digest(run_id, &result, &descriptors).unwrap();

    let expected = r#"{"runId":"PUBLIC-run","result":{"answerText":"PUBLIC-answer","n":1},"artifacts":[{"name":"PUBLIC-a","size":2},null]}"#;
    assert_eq!(digest, crate::artifacts::sha256(expected.as_bytes()));

    let raw_result: &RawValue =
        serde_json::from_str(r#"{"answerText":"PUBLIC-answer","n":1.0}"#).unwrap();
    let first: &RawValue =
        serde_json::from_str(r#"{"descriptor":{"name":"PUBLIC-a","size":2}}"#).unwrap();
    let second: &RawValue = serde_json::from_str(r#"{"other":true}"#).unwrap();
    assert_eq!(
        digest,
        result_digest_raw(run_id, raw_result, &[first, second]).unwrap()
    );

    assert_ne!(
        digest,
        result_digest("PUBLIC-other-run", &result, &descriptors).unwrap()
    );

    let error = result_digest(run_id, &result, &json!({"name":"PUBLIC-a"}))
        .expect_err("object descriptors must be rejected");
    assert_eq!(error.to_string(), "Remote descriptors must be an array");
}
