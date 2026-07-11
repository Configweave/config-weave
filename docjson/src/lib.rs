//! The graphical editors' DocJson pipeline: the document shapes
//! (docjson), AST → DocJson extraction (inspect_ast), and the
//! comment-preserving sync back onto WCL source (emit). Shared by the
//! CLI (`__wcl-inspect`/`__wcl-render`) and weave-server, which parses
//! package.wcl in-process for the GUI's package API docs.

pub mod docjson;
pub mod emit;
pub mod inspect_ast;
