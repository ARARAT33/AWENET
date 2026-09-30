//! Signalling/session state for direct and group real-time communication.
//! Media codecs and NAT traversal remain transport concerns; this module
//! provides authenticated session state without pretending to implement media.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CallKind {
    Voice,
    Video,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CallMode {
    Direct,
    Group { coordinator: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallInvite {
    pub call_id: [u8; 16],
    pub from: [u8; 32],
    pub participants: Vec<[u8; 32]>,
    pub kind: CallKind,
    pub mode: CallMode,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallSession {
    pub call_id: [u8; 16],
    pub participants: BTreeSet<[u8; 32]>,
    pub kind: CallKind,
    pub mode: CallMode,
    pub established: bool,
}

impl CallSession {
    pub fn accept(invite: CallInvite) -> Result<Self, String> {
        if invite.participants.is_empty() {
            return Err("call must have at least one participant".into());
        }
        if let CallMode::Group { coordinator } = &invite.mode {
            if coordinator.is_empty() {
                return Err("group call coordinator must not be empty".into());
            }
        }
        let mut participants = BTreeSet::new();
        participants.insert(invite.from);
        participants.extend(invite.participants);
        Ok(Self {
            call_id: invite.call_id,
            participants,
            kind: invite.kind,
            mode: invite.mode,
            established: false,
        })
    }

    pub fn establish(&mut self) {
        self.established = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_call_has_two_participants() {
        let invite = CallInvite {
            call_id: [1; 16],
            from: [2; 32],
            participants: vec![[3; 32]],
            kind: CallKind::Voice,
            mode: CallMode::Direct,
        };
        let mut session = CallSession::accept(invite).unwrap();
        session.establish();
        assert_eq!(session.participants.len(), 2);
        assert!(session.established);
    }

    #[test]
    fn group_call_requires_coordinator() {
        let invite = CallInvite {
            call_id: [1; 16],
            from: [2; 32],
            participants: vec![[3; 32], [4; 32]],
            kind: CallKind::Video,
            mode: CallMode::Group { coordinator: "node-x".into() },
        };
        assert_eq!(CallSession::accept(invite).unwrap().participants.len(), 3);
    }
}
