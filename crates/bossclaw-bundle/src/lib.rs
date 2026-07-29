//! Build + verify of the Rung-5 `.airmem` signed memory bundle. Canon-only.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod binding;
pub mod build;
pub mod format;
pub mod merkle;
pub mod resolver;
pub mod verify;

pub use binding::{binding_hash, binding_signing_bytes, decode_ed25519_key, verify_binding_internal};
pub use build::{build_bundle, BuildInput, ItemInput};
pub use format::{Airmem, AirmemItem, Binding, BindingPayload, ItemClass, Manifest, canonical_json, FORMAT_VERSION};
pub use resolver::{IdentityResolver, OfflineResolver};
pub use verify::{
    object_keys_are_ascii, verify, IdentityLevel, Verdict, VerifyError, BINDING_PURPOSE,
};
