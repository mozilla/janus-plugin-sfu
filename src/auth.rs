use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::error::Error;
use crate::messages::RoomId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedToken {
    pub join_hub: bool,
    pub kick_users: bool,
    pub room_ids: Option<Vec<RoomId>>
}

impl ValidatedToken {
    pub fn may_join(&self, room_id: &RoomId) -> bool {
        if self.join_hub {
            if let Some(allowed_rooms) = &self.room_ids {
                if allowed_rooms.contains(room_id) { // this token explicitly lets you in this room
                    true
                } else { // this token lets you in some rooms, but not this one
                    false
                }
            } else { // this token lets you in any room
                true
            }
        } else {
            false // this token disallows joining entirely
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct UserClaims {
    #[serde(default)]
    join_hub: bool,
    #[serde(default)]
    kick_users: bool,
    #[serde(default)]
    room_ids: Option<Vec<RoomId>>
}

impl ValidatedToken {
    pub fn from_str(value: &str, key: &[u8]) -> Result<ValidatedToken, Box<dyn Error>> {
        let validation = Validation::new(Algorithm::RS512);
        let dk = DecodingKey::from_rsa_der(key);
        let token_data = decode::<UserClaims>(value, &dk, &validation)?;
        Ok(ValidatedToken {
            join_hub: token_data.claims.join_hub,
            kick_users: token_data.claims.kick_users,
            room_ids: token_data.claims.room_ids,
        })
    }
}
