//! Bytes to text: BOM, UTF-8, Windows-1252 fallback and newline normalization.

use encoding_rs::{UTF_16BE, UTF_16LE, WINDOWS_1252};
use nlmx_domain::{document_type::DocumentType, parsed::ParseError};

/// The largest file read (64 MiB).
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;

pub struct Decoded {
    /// UTF-8 with `\n` line ends.
    pub text: String,
    /// The bytes were not UTF-8 and were read as Windows-1252.
    pub fallback: bool,
}

/// Decodes a text file: a UTF-8 or UTF-16 BOM decides; otherwise strict UTF-8, then
/// Windows-1252. A NUL byte means binary data (`Invalid(kind)`).
pub fn decode(bytes: &[u8], kind: DocumentType) -> Result<Decoded, ParseError> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ParseError::TooLarge);
    }
    let mut fallback = false;
    let raw = if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        std::str::from_utf8(rest)
            .map_err(|_| ParseError::Encoding)?
            .to_string()
    } else if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        UTF_16LE
            .decode_without_bom_handling_and_without_replacement(rest)
            .ok_or(ParseError::Encoding)?
            .into_owned()
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        UTF_16BE
            .decode_without_bom_handling_and_without_replacement(rest)
            .ok_or(ParseError::Encoding)?
            .into_owned()
    } else {
        if bytes.contains(&0) {
            return Err(ParseError::Invalid(kind));
        }
        match std::str::from_utf8(bytes) {
            Ok(text) => text.to_string(),
            Err(_) => {
                fallback = true;
                WINDOWS_1252
                    .decode_without_bom_handling_and_without_replacement(bytes)
                    .ok_or(ParseError::Encoding)?
                    .into_owned()
            }
        }
    };
    if raw.contains('\0') {
        return Err(ParseError::Invalid(kind));
    }
    Ok(Decoded {
        text: normalize_newlines(&raw),
        fallback,
    })
}

fn normalize_newlines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIND: DocumentType = DocumentType::Text;

    fn decoded(bytes: &[u8]) -> Decoded {
        decode(bytes, KIND).unwrap()
    }

    #[test]
    fn utf8_is_kept_and_a_bom_is_dropped() {
        let plain = decoded("ação\n".as_bytes());
        assert_eq!(plain.text, "ação\n");
        assert!(!plain.fallback);

        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice("coração".as_bytes());
        assert_eq!(decoded(&with_bom).text, "coração");
    }

    #[test]
    fn utf16_with_a_bom_is_decoded() {
        let units: Vec<u16> = "olá, mundo".encode_utf16().collect();
        let mut le = vec![0xFF, 0xFE];
        le.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        let mut be = vec![0xFE, 0xFF];
        be.extend(units.iter().flat_map(|u| u.to_be_bytes()));
        assert_eq!(decoded(&le).text, "olá, mundo");
        assert_eq!(decoded(&be).text, "olá, mundo");
        // An odd number of bytes is not UTF-16.
        assert_eq!(
            decode(&[0xFF, 0xFE, 0x41], KIND).err(),
            Some(ParseError::Encoding)
        );
    }

    #[test]
    fn other_bytes_are_read_as_windows_1252() {
        // "Ação de coração" in Windows-1252.
        let bytes = [
            0x41, 0xE7, 0xE3, 0x6F, 0x20, 0x64, 0x65, 0x20, 0x63, 0x6F, 0x72, 0x61, 0xE7, 0xE3,
            0x6F,
        ];
        let result = decoded(&bytes);
        assert_eq!(result.text, "Ação de coração");
        assert!(result.fallback);
    }

    #[test]
    fn newlines_are_normalized() {
        assert_eq!(decoded(b"a\r\nb\rc\nd").text, "a\nb\nc\nd");
    }

    #[test]
    fn binary_data_is_invalid() {
        assert_eq!(
            decode(b"abc\0def", KIND).err(),
            Some(ParseError::Invalid(KIND))
        );
        let units: Vec<u16> = "a\0b".encode_utf16().collect();
        let mut le = vec![0xFF, 0xFE];
        le.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        assert_eq!(decode(&le, KIND).err(), Some(ParseError::Invalid(KIND)));
    }

    #[test]
    fn an_invalid_utf8_file_with_a_bom_is_an_encoding_error() {
        assert_eq!(
            decode(&[0xEF, 0xBB, 0xBF, 0xFF, 0xFE, 0x41], KIND).err(),
            Some(ParseError::Encoding)
        );
    }
}
