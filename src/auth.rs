use super::jwt;
use super::jwt::{Algorithm, Validation};
use messages::UserId;
use std::fmt;
use std::error::Error;

#[derive(Debug)]
struct SubjectParseError;

impl Error for SubjectParseError {
    fn description(&self) -> &str {
        "Failed to parse JWT subject; expected User:UID."
    }
}

impl fmt::Display for SubjectParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.description())
    }
}

fn parse_user_subject(sub: &str) -> Result<UserId, SubjectParseError> {
    let separator_idx = sub.find(':').ok_or(SubjectParseError)?;
    let namespace = &sub[0..separator_idx];
    let ident = &sub[separator_idx+1..];
    if namespace == "User" {
        ident.parse().map_err(|_| SubjectParseError)
    } else {
        Err(SubjectParseError)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedToken {
    pub sub: UserId,
}

#[derive(Debug, Serialize, Deserialize)]
struct UserClaims {
   sub: String,
}

impl ValidatedToken {
    pub fn from_str(value: &str, key: &[u8]) -> Result<ValidatedToken, Box<Error>> {
        let mut validation = Validation::new(Algorithm::RS512);
        validation.validate_exp = false;
        let token_data = jwt::decode::<UserClaims>(value, key, &validation)?;
        let subject = parse_user_subject(&token_data.claims.sub)?;
        Ok(ValidatedToken { sub: subject })
    }
}
