//! Build + verify of the Rung-5 `.airmem` signed memory bundle. Canon-only.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod binding;
pub mod format;
pub mod merkle;

pub use binding::{binding_hash, binding_signing_bytes, decode_ed25519_key, verify_binding_internal};
pub use format::{Airmem, AirmemItem, Binding, BindingPayload, ItemClass, Manifest, canonical_json, FORMAT_VERSION};
