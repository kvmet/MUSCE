//! Account authentication and identity primitives.
//!
//! Currently: the [`Account`] record (identity, credential, capability names,
//! engine-owned status, opaque app data) and password hashing (see [`password`]).
//! How accounts are stored is a separate, still-being-designed concern and
//! deliberately not decided here.

mod account;
mod password;

pub use account::{Account, AccountId, AccountStatus, ParseStatusError};
pub use password::{HashError, VerifyError, hash_password, verify_password};
