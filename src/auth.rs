use super::jwt;
use super::jwt::{Algorithm, Validation};
use super::serde::de::{self, Deserialize, Deserializer, Unexpected, Visitor};
use messages::UserId;
use std::fmt;
use std::error::Error;

/// The symmetric encryption key for user JWTs. For production use, store somewhere private.
const USER_AUTH_KEY: &'static str = "JpWapNaJQ4HU1spmFCb5EyWxJAwKXiCl8677nd2GWYCurPYXYksMsHIV3J8zsYvN";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserToken {
    pub sub: UserId,
}

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

impl<'de> Deserialize<'de> for UserToken {

    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {

        #[derive(Debug, Serialize, Deserialize)]
        struct UserClaims {
            pub sub: String,
        }

        struct UserTokenVisitor;
        impl<'de> Visitor<'de> for UserTokenVisitor {
            type Value = UserToken;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a JWT token")
            }

            fn visit_str<E>(self, value: &str) -> Result<UserToken, E> where E: de::Error {
                let result = jwt::decode::<UserClaims>(value, USER_AUTH_KEY.as_bytes(), &Validation::new(Algorithm::HS512));
                if let Ok(token_data) = result {
                    if let Ok(subject) = parse_user_subject(&token_data.claims.sub) {
                        return Ok(UserToken { sub: subject })
                    }
                }
                Err(E::invalid_value(Unexpected::Str("Invalid JWT."), &self))
            }
        }
        deserializer.deserialize_str(UserTokenVisitor)
    }
}
