//! The DocJson pipeline: the document shapes (docjson), AST → DocJson
//! extraction (inspect_ast), and the comment-preserving sync back onto
//! WCL source (emit). Consumed by the CLI's hidden
//! `__wcl-inspect`/`__wcl-render` subcommands for external tooling.

pub mod docjson;
pub mod emit;
pub mod inspect_ast;
