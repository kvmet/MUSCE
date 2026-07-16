//! Password hashing for account credentials: argon2id, PHC-string encoded.
//!
//! This is the one CPU-heavy authentication primitive, so it is a pure function
//! pair with no storage of its own: it turns a password into a self-describing PHC
//! string (algorithm, cost parameters, and a random salt all embedded in the
//! output) and checks a password against such a string. Where that string is kept
//! is a separate concern.
//!
//! argon2 is *deliberately* slow, so a verify must never run on the sim thread: at
//! a 10 Hz tick that would stall every connected player. Both functions are meant
//! to be called from the async side (the cold/off-thread path), never inside a
//! tick.

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

/// A password could not be hashed. Not expected for well-formed input under the
/// default parameters; it is returned rather than unwrapped so a hashing failure is
/// never silently mistaken for a produced hash.
#[derive(Debug)]
pub struct HashError(argon2::password_hash::Error);

impl std::fmt::Display for HashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "could not hash password: {}", self.0)
    }
}

impl std::error::Error for HashError {}

/// A stored hash string could not be parsed (corrupt or truncated data). Distinct
/// from a wrong password, which is `Ok(false)`: a malformed stored hash must surface
/// as an error and never read as an ordinary failed login, so the two are never
/// confused.
#[derive(Debug)]
pub struct VerifyError(argon2::password_hash::Error);

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed password hash: {}", self.0)
    }
}

impl std::error::Error for VerifyError {}

/// Hash `password` into a self-describing PHC string: argon2id, default
/// OWASP-recommended cost parameters, and a fresh random salt drawn from the OS
/// CSPRNG, all embedded in the returned string. Two calls on the same password
/// return different strings (different salts); both verify. CPU-heavy by design;
/// call off the sim thread.
pub fn hash_password(password: &str) -> Result<String, HashError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(HashError)
}

/// Check `password` against a stored PHC string. `Ok(true)` is a match; `Ok(false)`
/// is a valid hash the password does not satisfy; `Err` is a hash string that would
/// not parse. A wrong password and corrupt stored data are kept distinct so a
/// storage fault is never counted as an ordinary failed login. CPU-heavy by design;
/// call off the sim thread.
pub fn verify_password(password: &str, phc: &str) -> Result<bool, VerifyError> {
    let parsed = PasswordHash::new(phc).map_err(VerifyError)?;
    match Argon2::default().verify_password(password.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(other) => Err(VerifyError(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_roundtrips() {
        let phc = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &phc).unwrap());
    }

    #[test]
    fn wrong_password_is_false_not_error() {
        let phc = hash_password("hunter2").unwrap();
        assert_eq!(
            verify_password("hunter3", &phc).unwrap(),
            false,
            "a wrong password is a clean non-match, not an error"
        );
    }

    #[test]
    fn each_hash_gets_its_own_salt() {
        // Same password, two hashes: the embedded salts differ, so the strings do
        // too, yet both verify. Guards against a fixed or missing salt.
        let a = hash_password("same").unwrap();
        let b = hash_password("same").unwrap();
        assert_ne!(a, b, "two hashes of one password must not be identical");
        assert!(verify_password("same", &a).unwrap());
        assert!(verify_password("same", &b).unwrap());
    }

    #[test]
    fn corrupt_hash_is_an_error_not_a_failed_login() {
        // The critical distinction: a stored hash that will not parse must be an
        // Err, never Ok(false). Otherwise corrupt storage reads as "wrong password"
        // and hides the fault.
        assert!(verify_password("anything", "not-a-phc-string").is_err());
        assert!(verify_password("anything", "").is_err());
    }

    #[test]
    fn empty_password_roundtrips_and_still_discriminates() {
        // The primitive holds no password policy (min length and the like are a
        // higher layer's call), so the empty string must hash and verify like any
        // other, and a non-empty password must not match it.
        let phc = hash_password("").unwrap();
        assert!(verify_password("", &phc).unwrap());
        assert!(!verify_password("x", &phc).unwrap());
    }
}
