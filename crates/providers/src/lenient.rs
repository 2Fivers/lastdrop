//! Small parsing helpers for files and tokens that hold secrets (identity, the Grok and Cursor
//! sign-ins): a string field read without buffering and overwritten when dropped, buffer wiping,
//! and the payload of a JWT.

use std::fmt;

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

/// A string field that may be missing, `null` or of another type (then `None`), read without
/// buffering the value (unlike `claude::json::Lenient`), so nothing of it lingers on the heap but
/// the string itself, which is overwritten when it drops (like `SecretString`: the fields hold
/// ids and addresses).
#[derive(Default)]
pub(crate) struct Text(pub(crate) Option<String>);

impl Drop for Text {
    fn drop(&mut self) {
        if let Some(text) = &mut self.0 {
            let len = text.len();
            text.clear();
            text.extend(std::iter::repeat_n('\0', len));
            std::hint::black_box(text.as_bytes());
        }
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Text, D::Error> {
        deserializer.deserialize_any(TextVisitor)
    }
}

struct TextVisitor;

impl<'de> Visitor<'de> for TextVisitor {
    type Value = Text;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any value")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Text, E> {
        Ok(Text(Some(v.to_owned())))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Text, E> {
        Ok(Text(Some(v)))
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Text, E> {
        Ok(Text(None))
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Text, E> {
        Ok(Text(None))
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Text, E> {
        Ok(Text(None))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Text, E> {
        Ok(Text(None))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Text, E> {
        Ok(Text(None))
    }

    fn visit_none<E: de::Error>(self) -> Result<Text, E> {
        Ok(Text(None))
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Text, D::Error> {
        deserializer.deserialize_any(TextVisitor)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Text, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Text(None))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Text, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Text(None))
    }
}

/// The text when it is there and not blank.
pub(crate) fn non_empty(text: &Option<String>) -> Option<&str> {
    text.as_deref().map(str::trim).filter(|t| !t.is_empty())
}

/// Overwrites a buffer that held a credential file (best effort, like `SecretString`'s drop).
pub(crate) fn wipe(bytes: &mut [u8]) {
    bytes.fill(0);
    std::hint::black_box(&*bytes);
}

/// A map value that is read as `T` when it is an object and as `None` when it is anything else
/// (a string, a number, a list), so one odd entry of a credentials file never fails the whole file.
/// `T`'s own fields must be lenient for an object to always read.
pub(crate) struct Object<T>(pub(crate) Option<T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Object<T>, D::Error> {
        struct ObjectVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
            type Value = Object<T>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any value")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Object<T>, A::Error> {
                T::deserialize(de::value::MapAccessDeserializer::new(map)).map(|v| Object(Some(v)))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Object<T>, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Object(None))
            }

            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Object<T>, E> {
                Ok(Object(None))
            }

            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Object<T>, E> {
                Ok(Object(None))
            }

            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Object<T>, E> {
                Ok(Object(None))
            }

            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Object<T>, E> {
                Ok(Object(None))
            }

            fn visit_str<E: de::Error>(self, _: &str) -> Result<Object<T>, E> {
                Ok(Object(None))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Object<T>, E> {
                Ok(Object(None))
            }
        }

        deserializer.deserialize_any(ObjectVisitor(std::marker::PhantomData))
    }
}

/// Takes the text out of a field as a trimmed [`SecretString`], overwriting the original string
/// (`None` when it is missing or blank).
pub(crate) fn take_secret(text: &mut Text) -> Option<lastdrop_core::secret::SecretString> {
    let raw = text.0.take()?;
    let secret = lastdrop_core::secret::SecretString::new(raw.trim().to_owned());
    // Back into a `Text`, whose drop overwrites it.
    drop(Text(Some(raw)));
    (!secret.is_empty()).then_some(secret)
}

/// The payload of a JWT (`header.payload.signature`, base64url) as bytes: read, not verified.
/// `None` when it is not a JWT. The caller wipes the bytes once parsed.
pub(crate) fn jwt_payload(token: &str) -> Option<Vec<u8>> {
    let mut parts = token.trim().split('.');
    let (_header, payload, _signature) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    base64url_decode(payload)
}

/// Decodes base64url (RFC 4648 §5), with or without `=` padding. `None` on any other character or
/// an impossible length.
pub(crate) fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    if text.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => {
                wipe(&mut out);
                return None;
            }
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{base64url_decode, jwt_payload};

    #[test]
    fn base64url_round_trips_the_rfc_4648_vectors() {
        for (encoded, plain) in [
            ("", ""),
            ("Zg", "f"),
            ("Zm8", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg", "foob"),
            ("Zm9vYmE", "fooba"),
            ("Zm9vYmFy", "foobar"),
            ("Zm9vYg==", "foob"),
        ] {
            assert_eq!(
                base64url_decode(encoded).as_deref(),
                Some(plain.as_bytes()),
                "{encoded}"
            );
        }
        // The URL alphabet: `-` and `_` instead of `+` and `/`.
        assert_eq!(base64url_decode("-_8").as_deref(), Some(&[0xfb, 0xff][..]));
        assert_eq!(base64url_decode("+/8"), None);
        assert_eq!(base64url_decode("Z"), None, "an impossible length");
        assert_eq!(base64url_decode("Zm9v!"), None);
    }

    #[test]
    fn a_jwt_payload_is_its_middle_part() {
        assert_eq!(
            jwt_payload("aGVhZA.Zm9vYmFy.c2ln").as_deref(),
            Some(&b"foobar"[..])
        );
        assert_eq!(
            jwt_payload(" aGVhZA.Zm9v.c2ln \n").as_deref(),
            Some(&b"foo"[..])
        );
        assert_eq!(jwt_payload("aGVhZA.Zm9v"), None, "two parts");
        assert_eq!(jwt_payload("a.b.c.d"), None, "four parts");
        assert_eq!(jwt_payload("a.***.c"), None);
    }
}
