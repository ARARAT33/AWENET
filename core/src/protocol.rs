use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_MESSAGE_TYPE_LEN: usize = 64;
pub const MAX_REQUEST_ID_LEN: usize = 128;
pub const MAX_PAYLOAD_LEN: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProtocolEnvelope {
    pub version: u16,
    pub message_type: String,
    pub request_id: String,
    pub payload: Vec<u8>,
}

impl ProtocolEnvelope {
    pub fn new(
        message_type: impl Into<String>,
        request_id: impl Into<String>,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            message_type: message_type.into(),
            request_id: request_id.into(),
            payload,
        }
    }

    pub fn validate(&self) -> bool {
        self.version == PROTOCOL_VERSION
            && !self.message_type.is_empty()
            && self.message_type.len() <= MAX_MESSAGE_TYPE_LEN
            && !self.request_id.is_empty()
            && self.request_id.len() <= MAX_REQUEST_ID_LEN
            && self.payload.len() <= MAX_PAYLOAD_LEN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_envelope_passes() {
        assert!(ProtocolEnvelope::new("data", "req", vec![1, 2]).validate());
    }

    #[test]
    fn invalid_version_and_oversized_fields_fail() {
        let mut p = ProtocolEnvelope::new("data", "req", vec![]);
        p.version = PROTOCOL_VERSION + 1;
        assert!(!p.validate());

        let p = ProtocolEnvelope::new("x".repeat(MAX_MESSAGE_TYPE_LEN + 1), "req", vec![]);
        assert!(!p.validate());

        let p = ProtocolEnvelope::new("data", "r".repeat(MAX_REQUEST_ID_LEN + 1), vec![]);
        assert!(!p.validate());
    }
}
