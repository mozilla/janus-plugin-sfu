use jsonwebtoken::{decode, Algorithm, Validation};
use serde::{Deserialize, Serialize};
use std::error::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedToken {
    pub join_hub: bool,
    pub kick_users: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct UserClaims {
    join_hub: bool,
    kick_users: bool,
}

impl ValidatedToken {
    pub fn from_str(value: &str, key: &[u8]) -> Result<ValidatedToken, Box<dyn Error>> {
        let validation = Validation::new(Algorithm::RS512);
        let token_data = decode::<UserClaims>(value, key, &validation)?;
        Ok(ValidatedToken {
            join_hub: token_data.claims.join_hub,
            kick_users: token_data.claims.kick_users,
        })
    }
}
