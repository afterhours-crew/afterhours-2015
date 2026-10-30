// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Typed graph values. Math programs use separate scalar and Vec3 register banks.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Value {
    Text(Text),
    Integer(i32),
    Unsigned32(u32),
    /// Native UInt64 properties (including GameId); never narrow to a score.
    Unsigned64(u64),
    Float(f32),
    Boolean(bool),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
    /// Right, up, forward and translation vectors, in that order. No padding.
    Transform([[f32; 3]; 4]),
}

/// Owned native CString content, bounded by the StringProperty wire contract.
/// Empty and unready are distinct; embedded terminators are rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Text(std::sync::Arc<str>);

impl serde::Serialize for Text {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Text {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(&value).map_err(|_| serde::de::Error::custom("invalid bounded graph text"))
    }
}

impl Text {
    pub fn new(value: &str) -> Result<Self, super::Error> {
        if value.len() > crate::replication::entity::MAX_STRING_BYTES
            || value.as_bytes().contains(&0)
        {
            return Err(super::Error::Bound);
        }
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_owned_and_bounded_in_bytes_with_no_embedded_terminator() {
        let mut source = String::from("event");
        let text = Text::new(&source).unwrap();
        source.clear();
        assert_eq!(text.as_str(), "event");
        assert_eq!(Text::new("").unwrap().as_str(), "");
        assert!(Text::new(&"a".repeat(1023)).is_ok());
        assert!(Text::new(&"a".repeat(1024)).is_err());
        assert!(Text::new(&"ö".repeat(512)).is_err());
        assert!(Text::new("event\0suffix").is_err());
    }
}
