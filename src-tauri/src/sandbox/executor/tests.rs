use super::*;

#[test]
fn profile_defaults_to_workspace() {
    assert_eq!(SandboxProfile::default(), SandboxProfile::Workspace);
}

#[test]
fn profile_is_active() {
    assert!(SandboxProfile::Workspace.is_active());
    assert!(SandboxProfile::ReadOnly.is_active());
    assert!(SandboxProfile::Strict.is_active());
    assert!(!SandboxProfile::Off.is_active());
}
