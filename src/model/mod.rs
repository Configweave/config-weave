//! Playbook/package data model and WCL loading + schema validation.
//! The DocJson pipeline lives in the shared `weave-docjson` crate
//! (re-exported here so `model::docjson` etc. keep working).

mod load;
mod types;

pub use weave_docjson::{docjson, emit, inspect_ast};

pub use load::{Loaded, label_string, load};
pub use types::*;
