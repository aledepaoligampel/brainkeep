#![forbid(unsafe_op_in_unsafe_fn)]
#[cfg(not(unix))]
compile_error!(
    "The initial local Brainkeep prototype supports Unix; Windows requires a transport/permissions adapter."
);
pub mod access;
pub mod files;
pub mod panel_query;
pub mod security;
pub mod store;

pub mod compact;
