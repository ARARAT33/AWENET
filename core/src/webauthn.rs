//! Browser WebAuthn support for the founder passkey ceremony.
//!
//! The desktop UI serves the node on `http://localhost:41800`, which browsers
//! treat as a secure context, so `navigator.credentials` ceremonies work there.
//! This module provides the backend half of that ceremony: a minimal CBOR
//! decoder, authenticator-data and COSE-key parsing, and assertion
//! verification for Ed25519 (COSE alg -8) credentials, which the existing
//! ed25519-dalek identity stack can verify directly.
//!
//! Honest scope: attestation trust chains (attStmt signatures and certificate
//! hierarchies) are not validated here; the security of the flow rests on the
//! challenge binding, the relying-party hash check, the founder-signed
//! enrollment authorization and possession of the enrolled key at step-up.

use crate::identity::Identity;
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// The WebAuthn relying-party id this product binds to. The node UI must be
/// opened through `http://localhost:41800` for the ceremony to match.
pub const RP_ID: &str = "localhost";

/// authenticatorData flag: user presence.
pub const FLAG_UP: u8 = 0x01;
/// authenticatorData flag: attested credential data is present.
pub const FLAG_AT: u8 = 0x40;

/// COSE key types and algorithms this module accepts.
const COSE_KTY_OKP: i128 = 8;
const COSE_ALG_EDDSA: i128 = -8;
const COSE_CRV_ED25519: i128 = 6;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum WebauthnError {
    #[error("cbor structure is invalid: {0}")]
    InvalidCbor(&'static str),
    #[error("authenticator data is too short")]
    AuthenticatorDataTooShort,
    #[error("attested credential data is truncated")]
    AttestedCredentialTruncated,
    #[error("no attested credential data is present")]
    NoAttestedCredential,
    #[error("the credential key is not an Ed25519 COSE key (alg -8)")]
    NotEd25519CoseKey,
    #[error("client data is not a valid JSON object")]
    InvalidClientData,
}

/// A decoded CBOR value (the subset WebAuthn structures use).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CborValue {
    Int(i128),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<CborValue>),
    Map(Vec<(CborValue, CborValue)>),
    Bool(bool),
    Null,
}

impl CborValue {
    fn map_get(&self, key: i128) -> Option<&CborValue> {
        match self {
            CborValue::Map(entries) => entries
                .iter()
                .find(|(candidate, _)| matches!(candidate, CborValue::Int(value) if *value == key))
                .map(|(_, value)| value),
            _ => None,
        }
    }

    fn text_get(&self, key: &str) -> Option<&str> {
        match self {
            CborValue::Map(entries) => entries
                .iter()
                .find(|(candidate, _)| matches!(candidate, CborValue::Text(value) if value == key))
                .and_then(|(_, value)| match value {
                    CborValue::Text(text) => Some(text.as_str()),
                    _ => None,
                }),
            _ => None,
        }
    }

    fn bytes_get(&self, key: i128) -> Option<&[u8]> {
        match self.map_get(key) {
            Some(CborValue::Bytes(bytes)) => Some(bytes),
            _ => None,
        }
    }

    fn bytes_get_by_text(&self, key: &str) -> Option<&[u8]> {
        match self {
            CborValue::Map(entries) => entries
                .iter()
                .find(|(candidate, _)| matches!(candidate, CborValue::Text(value) if value == key))
                .and_then(|(_, value)| match value {
                    CborValue::Bytes(bytes) => Some(bytes.as_slice()),
                    _ => None,
                }),
            _ => None,
        }
    }

    fn int_get(&self, key: i128) -> Option<i128> {
        match self.map_get(key) {
            Some(CborValue::Int(value)) => Some(*value),
            _ => None,
        }
    }
}

/// Decodes one CBOR item and returns it with the number of bytes consumed.
/// Indefinite lengths are rejected: platform authenticators emit definite
/// lengths for these structures.
pub fn cbor_decode(data: &[u8]) -> Result<(CborValue, usize), WebauthnError> {
    let mut parser = CborParser { data, pos: 0 };
    let value = parser.read_item()?;
    Ok((value, parser.pos))
}

struct CborParser<'a> {
    data: &'a [u8],
    pos: usize,
}

impl CborParser<'_> {
    fn read_item(&mut self) -> Result<CborValue, WebauthnError> {
        let head = self.read_byte()?;
        let major = head >> 5;
        let info = head & 0x1f;
        let argument = self.read_argument(info)?;
        match major {
            0 => Ok(CborValue::Int(argument)),
            1 => {
                let magnitude = argument
                    .checked_add(1)
                    .ok_or(WebauthnError::InvalidCbor("integer magnitude overflow"))?;
                Ok(CborValue::Int(-magnitude))
            }
            2 => {
                let length = usize::try_from(argument)
                    .map_err(|_| WebauthnError::InvalidCbor("byte string length overflow"))?;
                let bytes = self.read_slice(length)?;
                Ok(CborValue::Bytes(bytes.to_vec()))
            }
            3 => {
                let length = usize::try_from(argument)
                    .map_err(|_| WebauthnError::InvalidCbor("text length overflow"))?;
                let bytes = self.read_slice(length)?;
                let text = String::from_utf8(bytes.to_vec())
                    .map_err(|_| WebauthnError::InvalidCbor("text is not valid UTF-8"))?;
                Ok(CborValue::Text(text))
            }
            4 => {
                let length = usize::try_from(argument)
                    .map_err(|_| WebauthnError::InvalidCbor("array length overflow"))?;
                let mut items = Vec::with_capacity(length.min(64));
                for _ in 0..length {
                    items.push(self.read_item()?);
                }
                Ok(CborValue::Array(items))
            }
            5 => {
                let length = usize::try_from(argument)
                    .map_err(|_| WebauthnError::InvalidCbor("map length overflow"))?;
                let mut entries = Vec::with_capacity(length.min(64));
                for _ in 0..length {
                    let key = self.read_item()?;
                    let value = self.read_item()?;
                    entries.push((key, value));
                }
                Ok(CborValue::Map(entries))
            }
            7 => match argument {
                20 => Ok(CborValue::Bool(false)),
                21 => Ok(CborValue::Bool(true)),
                22 | 23 => Ok(CborValue::Null),
                _ => Err(WebauthnError::InvalidCbor("unsupported simple value")),
            },
            _ => Err(WebauthnError::InvalidCbor("unsupported major type")),
        }
    }

    fn read_argument(&mut self, info: u8) -> Result<i128, WebauthnError> {
        match info {
            0..=23 => Ok(i128::from(info)),
            24 => Ok(i128::from(self.read_byte()?)),
            25 => {
                let bytes = self.read_slice(2)?;
                Ok(i128::from(u16::from_be_bytes([bytes[0], bytes[1]])))
            }
            26 => {
                let bytes = self.read_slice(4)?;
                Ok(i128::from(u32::from_be_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3],
                ])))
            }
            27 => {
                let bytes = self.read_slice(8)?;
                Ok(i128::from(u64::from_be_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ])))
            }
            31 => Err(WebauthnError::InvalidCbor(
                "indefinite lengths are not accepted",
            )),
            _ => Err(WebauthnError::InvalidCbor("reserved additional info")),
        }
    }

    fn read_byte(&mut self) -> Result<u8, WebauthnError> {
        let byte = *self
            .data
            .get(self.pos)
            .ok_or(WebauthnError::InvalidCbor("truncated input"))?;
        self.pos += 1;
        Ok(byte)
    }

    fn read_slice(&mut self, length: usize) -> Result<&[u8], WebauthnError> {
        let end = self
            .pos
            .checked_add(length)
            .ok_or(WebauthnError::InvalidCbor("length overflow"))?;
        if end > self.data.len() {
            return Err(WebauthnError::InvalidCbor("truncated input"));
        }
        let start = self.pos;
        self.pos = end;
        Ok(&self.data[start..end])
    }
}

/// The attested credential inside an authenticator-data block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttestedCredential {
    pub credential_id: Vec<u8>,
    pub public_key: [u8; 32],
}

/// A parsed authenticator-data block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatorData {
    pub rp_id_hash: [u8; 32],
    pub flags: u8,
    pub sign_count: u32,
    pub attested_credential: Option<AttestedCredential>,
}

/// SHA-256 of the relying-party id, as WebAuthn embeds it in authenticator
/// data.
pub fn rp_id_hash() -> [u8; 32] {
    Sha256::digest(RP_ID.as_bytes()).into()
}

/// Parses an authenticator-data block. When the attested-credential flag is
/// set, the embedded COSE key must be an Ed25519 (OKP, alg -8) key, which is
/// the only credential type this product can verify.
pub fn parse_authenticator_data(data: &[u8]) -> Result<AuthenticatorData, WebauthnError> {
    if data.len() < 37 {
        return Err(WebauthnError::AuthenticatorDataTooShort);
    }
    let mut rp_id_hash = [0u8; 32];
    rp_id_hash.copy_from_slice(&data[0..32]);
    let flags = data[32];
    let sign_count = u32::from_be_bytes([data[33], data[34], data[35], data[36]]);
    let attested_credential = if flags & FLAG_AT != 0 {
        let rest = &data[37..];
        if rest.len() < 18 {
            return Err(WebauthnError::AttestedCredentialTruncated);
        }
        let credential_len = u16::from_be_bytes([rest[16], rest[17]]) as usize;
        if rest.len() < 18 + credential_len {
            return Err(WebauthnError::AttestedCredentialTruncated);
        }
        let credential_id = rest[18..18 + credential_len].to_vec();
        let cose_bytes = &rest[18 + credential_len..];
        let (cose, consumed) = cbor_decode(cose_bytes)?;
        if cose_bytes.len() < consumed {
            return Err(WebauthnError::AttestedCredentialTruncated);
        }
        let kty = cose.int_get(1).ok_or(WebauthnError::NotEd25519CoseKey)?;
        let alg = cose.int_get(3).ok_or(WebauthnError::NotEd25519CoseKey)?;
        let crv = cose.int_get(-1).ok_or(WebauthnError::NotEd25519CoseKey)?;
        let public_key_bytes = cose.bytes_get(-2).ok_or(WebauthnError::NotEd25519CoseKey)?;
        if kty != COSE_KTY_OKP || alg != COSE_ALG_EDDSA || crv != COSE_CRV_ED25519 {
            return Err(WebauthnError::NotEd25519CoseKey);
        }
        let mut public_key = [0u8; 32];
        if public_key_bytes.len() != public_key.len() {
            return Err(WebauthnError::NotEd25519CoseKey);
        }
        public_key.copy_from_slice(public_key_bytes);
        Some(AttestedCredential {
            credential_id,
            public_key,
        })
    } else {
        None
    };
    Ok(AuthenticatorData {
        rp_id_hash,
        flags,
        sign_count,
        attested_credential,
    })
}

/// Parses a WebAuthn attestation object and returns the authenticator data
/// together with the attestation statement format identifier. The attestation
/// statement itself is not cryptographically validated; the caller binds the
/// credential through the founder-signed enrollment instead.
pub fn parse_attestation_object(
    bytes: &[u8],
) -> Result<(AuthenticatorData, String), WebauthnError> {
    let (value, consumed) = cbor_decode(bytes)?;
    if consumed == 0 {
        return Err(WebauthnError::InvalidCbor("empty attestation object"));
    }
    let auth_data_bytes = value
        .bytes_get_by_text("authData")
        .ok_or(WebauthnError::InvalidCbor(
            "attestation object has no authData",
        ))?;
    let format = value
        .text_get("fmt")
        .ok_or(WebauthnError::InvalidCbor("attestation object has no fmt"))?
        .to_string();
    let authenticator_data = parse_authenticator_data(auth_data_bytes)?;
    Ok((authenticator_data, format))
}

/// The message an assertion signs: authenticatorData || SHA-256(clientDataJSON).
pub fn assertion_message(authenticator_data: &[u8], client_data_json: &[u8]) -> Vec<u8> {
    let mut message = authenticator_data.to_vec();
    message.extend_from_slice(&Sha256::digest(client_data_json));
    message
}

/// Verifies an Ed25519 WebAuthn assertion. The challenge binding against
/// clientDataJSON and the relying-party hash must be checked by the caller on
/// the parsed data; this function re-parses the block, requires the
/// user-presence flag, and verifies the signature over the assertion message.
pub fn verify_assertion(
    public_key: &[u8; 32],
    authenticator_data: &[u8],
    client_data_json: &[u8],
    signature: &[u8; 64],
) -> bool {
    let Ok(data) = parse_authenticator_data(authenticator_data) else {
        return false;
    };
    if data.flags & FLAG_UP == 0 {
        return false;
    }
    let message = assertion_message(authenticator_data, client_data_json);
    Identity::verify(public_key, &message, signature)
}

/// Reads a string field from the client data JSON.
pub fn client_data_field(client_data_json: &str, field: &str) -> Option<String> {
    let value: Value = serde_json::from_str(client_data_json).ok()?;
    value.get(field)?.as_str().map(str::to_string)
}

/// Unpadded base64url encoding, as WebAuthn uses for the clientDataJSON
/// challenge field.
pub fn base64url_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(triple >> 18) as usize & 63] as char);
        out.push(TABLE[(triple >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(triple >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[triple as usize & 63] as char);
        }
    }
    out
}

/// Decodes unpadded or padded base64url text.
pub fn base64url_decode(text: &str) -> Result<Vec<u8>, WebauthnError> {
    let mut sextets = Vec::new();
    for character in text.chars() {
        let value = match character {
            '=' => break,
            'A'..='Z' => character as u8 - b'A',
            'a'..='z' => character as u8 - b'a' + 26,
            '0'..='9' => character as u8 - b'0' + 52,
            '-' => 62,
            '_' => 63,
            _ => return Err(WebauthnError::InvalidCbor("invalid base64url character")),
        };
        sextets.push(value);
    }
    let mut out = Vec::new();
    for chunk in sextets.chunks(4) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let b3 = u32::from(chunk.get(3).copied().unwrap_or(0));
        let triple = (b0 << 18) | (b1 << 12) | (b2 << 6) | b3;
        out.push((triple >> 16) as u8);
        if chunk.len() > 2 {
            out.push((triple >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(triple as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    // Minimal CBOR encoder used to build realistic authenticator structures.
    fn encode_head(major: u8, argument: u64) -> Vec<u8> {
        let major = major << 5;
        if argument <= 23 {
            return vec![major | argument as u8];
        }
        if argument <= u64::from(u8::MAX) {
            return vec![major | 24, argument as u8];
        }
        if argument <= u64::from(u16::MAX) {
            let mut out = vec![major | 25];
            out.extend_from_slice(&(argument as u16).to_be_bytes());
            return out;
        }
        let mut out = vec![major | 26];
        out.extend_from_slice(&(argument as u32).to_be_bytes());
        out
    }

    fn encode_bytes(bytes: &[u8]) -> Vec<u8> {
        let mut out = encode_head(2, bytes.len() as u64);
        out.extend_from_slice(bytes);
        out
    }

    fn encode_text(text: &str) -> Vec<u8> {
        let mut out = encode_head(3, text.len() as u64);
        out.extend_from_slice(text.as_bytes());
        out
    }

    fn encode_int(value: i64) -> Vec<u8> {
        if value >= 0 {
            encode_head(0, value as u64)
        } else {
            encode_head(1, (-value - 1) as u64)
        }
    }

    fn encode_map(entries: Vec<(Vec<u8>, Vec<u8>)>) -> Vec<u8> {
        let mut out = encode_head(5, entries.len() as u64);
        for (key, value) in entries {
            out.extend_from_slice(&key);
            out.extend_from_slice(&value);
        }
        out
    }

    fn cose_ed25519_key(public_key: &[u8; 32]) -> Vec<u8> {
        encode_map(vec![
            (encode_int(1), encode_int(8)),             // kty: OKP
            (encode_int(3), encode_int(-8)),            // alg: EdDSA
            (encode_int(-1), encode_int(6)),            // crv: Ed25519
            (encode_int(-2), encode_bytes(public_key)), // x
        ])
    }

    fn authenticator_data_with_credential(
        public_key: &[u8; 32],
        credential_id: &[u8],
        flags: u8,
    ) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&rp_id_hash());
        data.push(flags | FLAG_AT);
        data.extend_from_slice(&7u32.to_be_bytes());
        data.extend_from_slice(&[0u8; 16]); // aaguid
        data.extend_from_slice(&(credential_id.len() as u16).to_be_bytes());
        data.extend_from_slice(credential_id);
        data.extend_from_slice(&cose_ed25519_key(public_key));
        data
    }

    fn plain_authenticator_data(flags: u8) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&rp_id_hash());
        data.push(flags);
        data.extend_from_slice(&42u32.to_be_bytes());
        data
    }

    #[test]
    fn parses_attestation_object_with_ed25519_credential() {
        let credential = identity("webauthn-cred");
        let credential_id = b"founder-credential-id-01".to_vec();
        let auth_data =
            authenticator_data_with_credential(&credential.public.public_key, &credential_id, 0);
        let attestation = encode_map(vec![
            (encode_text("fmt"), encode_text("none")),
            (encode_text("attStmt"), encode_map(vec![])),
            (encode_text("authData"), encode_bytes(&auth_data)),
        ]);
        let (parsed, format) = parse_attestation_object(&attestation).expect("valid attestation");
        assert_eq!(format, "none");
        assert_eq!(parsed.rp_id_hash, rp_id_hash());
        assert_eq!(parsed.sign_count, 7);
        let attested = parsed.attested_credential.expect("attested credential");
        assert_eq!(attested.credential_id, credential_id);
        assert_eq!(attested.public_key, credential.public.public_key);
    }

    #[test]
    fn rejects_non_ed25519_cose_keys() {
        let credential = identity("webauthn-es256");
        let mut auth_data = Vec::new();
        auth_data.extend_from_slice(&rp_id_hash());
        auth_data.push(FLAG_AT);
        auth_data.extend_from_slice(&0u32.to_be_bytes());
        auth_data.extend_from_slice(&[0u8; 16]);
        let credential_id = b"cred".to_vec();
        auth_data.extend_from_slice(&(credential_id.len() as u16).to_be_bytes());
        auth_data.extend_from_slice(&credential_id);
        // an ES256 key: kty 2, alg -7, with an arbitrary byte-string x
        let es256 = encode_map(vec![
            (encode_int(1), encode_int(2)),
            (encode_int(3), encode_int(-7)),
            (encode_int(-1), encode_int(1)),
            (encode_int(-2), encode_bytes(&credential.public.public_key)),
        ]);
        auth_data.extend_from_slice(&es256);
        assert_eq!(
            parse_authenticator_data(&auth_data).unwrap_err(),
            WebauthnError::NotEd25519CoseKey
        );
    }

    #[test]
    fn rejects_wrong_relying_party_and_missing_presence() {
        let credential = identity("webauthn-rp");
        let mut wrong_rp = plain_authenticator_data(FLAG_UP);
        wrong_rp[0] ^= 1;
        let parsed = parse_authenticator_data(&wrong_rp).unwrap();
        assert_ne!(parsed.rp_id_hash, rp_id_hash());
        // user presence flag missing: assertion verification fails
        let client_data = r#"{"type":"webauthn.get","challenge":"abc"}"#;
        let message = assertion_message(&plain_authenticator_data(0), client_data.as_bytes());
        let signature = credential.sign(&message);
        assert!(!verify_assertion(
            &credential.public.public_key,
            &plain_authenticator_data(0),
            client_data.as_bytes(),
            &signature
        ));
    }

    #[test]
    fn verifies_a_valid_assertion_and_rejects_tampering() {
        let credential = identity("webauthn-assert");
        let auth_data = plain_authenticator_data(FLAG_UP);
        let challenge = base64url_encode(&[1, 2, 3, 4]);
        let client_data = format!("{{\"type\":\"webauthn.get\",\"challenge\":\"{challenge}\"}}");
        let message = assertion_message(&auth_data, client_data.as_bytes());
        let signature = credential.sign(&message);
        assert!(verify_assertion(
            &credential.public.public_key,
            &auth_data,
            client_data.as_bytes(),
            &signature
        ));
        // a different client data breaks the signature
        assert!(!verify_assertion(
            &credential.public.public_key,
            &auth_data,
            b"{\"type\":\"webauthn.get\",\"challenge\":\"other\"}",
            &signature
        ));
        // a different key cannot verify it
        let stranger = identity("webauthn-stranger");
        assert!(!verify_assertion(
            &stranger.public.public_key,
            &auth_data,
            client_data.as_bytes(),
            &signature
        ));
    }

    #[test]
    fn cbor_rejects_truncated_and_indefinite_input() {
        assert!(cbor_decode(&[]).is_err());
        assert!(cbor_decode(&[0x5f]).is_err());
        assert!(cbor_decode(&[0x38]).is_err());
    }

    #[test]
    fn base64url_roundtrips() {
        let cases: [&[u8]; 5] = [
            &[],
            &[0],
            &[1, 2],
            &[1, 2, 3],
            &[7, 99, 128, 250, 61, 17, 203],
        ];
        for case in cases {
            let encoded = base64url_encode(case);
            assert_eq!(base64url_decode(&encoded).unwrap(), case.to_vec());
        }
        assert_eq!(base64url_decode("aGVsbG8=").unwrap(), b"hello".to_vec());
        assert_eq!(base64url_decode("AAEC").unwrap(), vec![0, 1, 2]);
        assert!(base64url_decode("a*").is_err());
    }

    #[test]
    fn client_data_fields_are_read_from_json() {
        let data =
            r#"{"type":"webauthn.create","challenge":"AbCd","origin":"http://localhost:41800"}"#;
        assert_eq!(
            client_data_field(data, "type").as_deref(),
            Some("webauthn.create")
        );
        assert_eq!(
            client_data_field(data, "challenge").as_deref(),
            Some("AbCd")
        );
        assert_eq!(client_data_field(data, "missing"), None);
        assert_eq!(client_data_field("not json", "type"), None);
    }
}
