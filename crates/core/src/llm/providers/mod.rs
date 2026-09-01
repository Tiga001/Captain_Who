//! Vendor-owned wire protocols.
//!
//! The modules in this namespace intentionally do not depend on one another. They may share
//! provider-neutral Chat Completions framing and message helpers, but every vendor owns its
//! payload policy, private continuation codec, usage projection and error semantics.

pub(super) mod deepseek;
pub(super) mod moonshot;
