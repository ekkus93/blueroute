use std::fmt;
use std::str::FromStr;

use crate::{CoreError, ErrorKind};

const ID_BYTES: usize = 16;
const ID_HEX_CHARS: usize = ID_BYTES * 2;

fn encode_hex(bytes: &[u8; ID_BYTES]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(ID_HEX_CHARS);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn decode_hex(value: &str) -> Result<[u8; ID_BYTES], CoreError> {
    if value.len() != ID_HEX_CHARS {
        return Err(CoreError::new(
            ErrorKind::InvalidInput,
            format!("identifier must contain exactly {ID_HEX_CHARS} hexadecimal characters"),
        ));
    }

    let input = value.as_bytes();
    let mut bytes = [0_u8; ID_BYTES];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let high = decode_nibble(input[index * 2]).ok_or_else(|| {
            CoreError::new(ErrorKind::InvalidInput, "identifier contains non-hexadecimal data")
        })?;
        let low = decode_nibble(input[index * 2 + 1]).ok_or_else(|| {
            CoreError::new(ErrorKind::InvalidInput, "identifier contains non-hexadecimal data")
        })?;
        *byte = (high << 4) | low;
    }
    Ok(bytes)
}

macro_rules! define_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name([u8; ID_BYTES]);

        impl $name {
            pub const fn from_bytes(bytes: [u8; ID_BYTES]) -> Self {
                Self(bytes)
            }

            pub const fn as_bytes(&self) -> &[u8; ID_BYTES] {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&encode_hex(&self.0))
            }
        }

        impl FromStr for $name {
            type Err = CoreError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                decode_hex(value).map(Self)
            }
        }
    };
}

define_id!(NodeId, "Stable authorization identity for a BlueRoute node.");
define_id!(NetworkId, "Stable identity for a logical BlueRoute network.");
define_id!(LinkId, "Stable identity for a BlueRoute PAN link observation.");
define_id!(SegmentId, "Stable identity for a routed PAN segment.");

/// A user-editable presentation name that is intentionally separate from identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DisplayName(String);

impl DisplayName {
    pub const MAX_CHARS: usize = 64;

    pub fn new(value: impl Into<String>) -> Result<Self, CoreError> {
        let value = value.into();
        let trimmed = value.trim();
        let char_count = trimmed.chars().count();
        if char_count == 0 {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                "display name cannot be empty",
            ));
        }
        if char_count > Self::MAX_CHARS {
            return Err(CoreError::new(
                ErrorKind::InvalidInput,
                format!("display name cannot exceed {} characters", Self::MAX_CHARS),
            ));
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DisplayName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_have_deterministic_lowercase_serialization() {
        let id = NodeId::from_bytes([
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
            0xdd, 0xee, 0xff,
        ]);
        assert_eq!(id.to_string(), "00112233445566778899aabbccddeeff");
    }

    #[test]
    fn identifiers_parse_uppercase_and_round_trip_canonically() {
        let id: NetworkId = "00112233445566778899AABBCCDDEEFF".parse().unwrap();
        assert_eq!(id.to_string(), "00112233445566778899aabbccddeeff");
    }

    #[test]
    fn invalid_identifiers_are_rejected() {
        assert!("abc".parse::<NodeId>().is_err());
        assert!("zz112233445566778899aabbccddeeff".parse::<NodeId>().is_err());
    }

    #[test]
    fn display_name_is_not_part_of_node_identity() {
        let id = NodeId::from_bytes([7; ID_BYTES]);
        let first = DisplayName::new("Lab laptop").unwrap();
        let second = DisplayName::new("Renamed laptop").unwrap();

        assert_ne!(first, second);
        assert_eq!(id, NodeId::from_bytes([7; ID_BYTES]));
    }
}
