//! Playbook/package data model and WCL loading + schema validation.

mod load;
mod types;

pub use load::{Loaded, load};
pub use types::*;
