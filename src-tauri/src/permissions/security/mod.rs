//! Security sub-checks — the defense-in-depth layers that run AFTER the
//! rule/mode decision (rules can allow, security never trusts the allow):
//! unified bash analysis, filesystem path validation, network policy, and
//! sensitive-file edits.

pub mod bash;
pub mod filesystem;
pub mod network;
pub mod sensitive;
