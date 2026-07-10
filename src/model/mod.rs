//! Playbook/package data model and WCL loading + schema validation,
//! plus the graphical editors' DocJson pipeline (AST-level extraction
//! and comment-preserving sync — see docjson.rs).

pub mod docjson;
pub mod emit;
pub mod inspect_ast;
mod load;
mod types;

pub use load::{Loaded, label_string, load};
pub use types::*;
