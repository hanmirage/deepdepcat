//! Windows Job Object — process-tree isolation for spawned commands.
//!
//! A Job Object lets us terminate an entire process tree (the shell plus
//! everything it spawned) instead of just the direct child. On timeout or
//! cancellation the whole tree is killed, so `npm run dev` / `git push`
//! grandchildren never linger as orphans.
//!
//! The job is attached to an already-spawned process via
//! `AssignProcessToJobObject` (the attribute-list path requires nightly
//! `windows_process_extensions_raw_attribute`, so post-spawn assignment is
//! the stable choice). The race window between spawn and assignment is tiny
//! and acceptable for the kill-on-timeout use case; assignment failure just
//! degrades to "no tree isolation" (the direct child is still killed).
//!
//! A restricted variant ([`JobObject::create_restricted`]) additionally sets
//! the job security filter (`JOB_OBJECT_SECURITY_RESTRICTED_TOKEN` +
//! `JOB_OBJECT_SECURITY_NO_ADMIN`), so every process assigned to the job
//! runs under a restricted token: admin SIDs are stripped and dangerous
//! privileges removed. This is the Windows restricted-token downgrade — the
//! same mechanism an admin-bypassing sandbox uses — applied at the job level
//! so the spawn path (tokio/std Command) stays unchanged.

use std::os::windows::io::{AsRawHandle, RawHandle};
use std::sync::Mutex;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_REMOVED, TOKEN_PRIVILEGES,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    JobObjectSecurityLimitInformation, SetInformationJobObject, TerminateJobObject,
    JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOBOBJECT_SECURITY_LIMIT_INFORMATION, JOB_OBJECT_LIMIT, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_SECURITY_NO_ADMIN,
    JOB_OBJECT_SECURITY_RESTRICTED_TOKEN,
};

/// Privileges stripped from the restricted job's processes (beyond what
/// NO_ADMIN already removes). `SeDebugPrivilege` is the highest-risk
/// privilege that can appear on a non-admin token (arbitrary process handle
/// access); the remaining admin-equivalent privileges are already
/// neutralized by stripping the Administrators group SID via NO_ADMIN.
const STRIP_PRIVILEGES: &[&str] = &["SeDebugPrivilege"];

/// Owns a Windows Job Object used to terminate a spawned process tree.
pub struct JobObject {
    handle: HANDLE,
    /// Whether the tree has been released to outlive this object. Serializes
    /// the preserve/terminate decision so the two race safely.
    preserve_descendants: Mutex<bool>,
}

impl Drop for JobObject {
    fn drop(&mut self) {
        // Closing the last handle with KILL_ON_JOB_CLOSE set terminates every
        // member still in the job.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

// A job handle is a Windows kernel handle — safe to share across threads
// (the API is externally synchronized, and `preserve_descendants` has its own
// mutex). The raw `*mut c_void` inside HANDLE is not auto-`Send`, so assert it.
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl JobObject {
    /// Create a Job Object configured to terminate all members when its last
    /// handle closes.
    pub fn create() -> std::io::Result<Self> {
        let handle = unsafe {
            CreateJobObjectW(None, None).map_err(|e| std::io::Error::other(e.to_string()))?
        };
        let job = Self {
            handle,
            preserve_descendants: Mutex::new(false),
        };
        job.set_limit_flags(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK)?;
        Ok(job)
    }

    /// Create a Job Object with the restricted-token security filter.
    ///
    /// Every process assigned to the job runs under a restricted token:
    /// admin SIDs are stripped (`JOB_OBJECT_SECURITY_NO_ADMIN`) and
    /// `SeDebugPrivilege` is removed (`JOB_OBJECT_SECURITY_RESTRICTED_TOKEN`
    /// applied to the caller's token). `SeDebugPrivilege` is the highest-risk
    /// privilege that can appear on a non-admin token (arbitrary process
    /// handle access); the remaining admin-equivalent privileges are already
    /// neutralized by stripping the Administrators group SID.
    ///
    /// The security filter is set BEFORE any process is assigned, so no race
    /// exists. Best-effort: if the filter cannot be applied (e.g. the calling
    /// process cannot enumerate its own token), the plain job is returned
    /// with tree isolation still intact — the downgrade is additional
    /// defense, not the primary guarantee.
    pub fn create_restricted() -> std::io::Result<Self> {
        let job = Self::create()?;

        // Build the privilege to strip: the first entry of STRIP_PRIVILEGES.
        let mut debug_luid = windows::Win32::Foundation::LUID::default();
        let debug_name: Vec<u16> = STRIP_PRIVILEGES[0].encode_utf16().collect();
        let ok = unsafe {
            LookupPrivilegeValueW(
                None,
                windows::core::PCWSTR(debug_name.as_ptr()),
                &mut debug_luid,
            )
            .is_ok()
        };
        let mut priv_to_delete = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: debug_luid,
                Attributes: SE_PRIVILEGE_REMOVED,
            }],
        };
        if !ok {
            priv_to_delete.PrivilegeCount = 0;
        }

        let security_limits = JOBOBJECT_SECURITY_LIMIT_INFORMATION {
            SecurityLimitFlags: JOB_OBJECT_SECURITY_RESTRICTED_TOKEN | JOB_OBJECT_SECURITY_NO_ADMIN,
            // NULL JobToken → the restriction is based on the calling
            // process's primary token (the app), with the privileges above
            // removed.
            JobToken: HANDLE(std::ptr::null_mut()),
            SidsToDisable: std::ptr::null_mut(),
            PrivilegesToDelete: &mut priv_to_delete as *mut TOKEN_PRIVILEGES,
            RestrictedSids: std::ptr::null_mut(),
        };

        let result = unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectSecurityLimitInformation,
                &security_limits as *const JOBOBJECT_SECURITY_LIMIT_INFORMATION as *const _,
                std::mem::size_of::<JOBOBJECT_SECURITY_LIMIT_INFORMATION>() as u32,
            )
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, "Job security filter failed — running without restricted token");
        }
        Ok(job)
    }

    fn set_limit_flags(&self, flags: JOB_OBJECT_LIMIT) -> std::io::Result<()> {
        let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: flags,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = unsafe {
            SetInformationJobObject(
                self.handle,
                JobObjectExtendedLimitInformation,
                &limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        result.map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Assign an already-spawned process to this job.
    ///
    /// Assignment is not retroactive: descendants created before this call
    /// completes are not guaranteed to become members.
    pub fn assign_process(&self, process_handle: RawHandle) -> std::io::Result<()> {
        unsafe {
            AssignProcessToJobObject(self.handle, HANDLE(process_handle))
                .map_err(|e| std::io::Error::other(e.to_string()))
        }
    }

    /// Assign a tokio child to this job via its raw handle.
    pub fn assign_child(&self, child: &tokio::process::Child) -> std::io::Result<()> {
        match child.raw_handle() {
            Some(handle) => self.assign_process(handle),
            None => Err(std::io::Error::other("child has no handle")),
        }
    }

    /// Terminate every process currently assigned to the job.
    pub fn terminate(&self) -> std::io::Result<()> {
        let preserve = self
            .preserve_descendants
            .lock()
            .map_err(|_| std::io::Error::other("job state lock poisoned"))?;
        if *preserve {
            return Ok(());
        }
        unsafe {
            TerminateJobObject(self.handle, 1).map_err(|e| std::io::Error::other(e.to_string()))
        }
    }
}

impl AsRawHandle for JobObject {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::AsRawHandle;

    #[test]
    fn create_succeeds() {
        let job = JobObject::create().expect("job object creation should succeed");
        assert_ne!(job.as_raw_handle(), std::ptr::null_mut());
    }

    #[test]
    fn create_restricted_succeeds() {
        // The restricted variant must create a valid job with the security
        // filter applied (or degrade gracefully without failing).
        let job = JobObject::create_restricted().expect("restricted job creation should succeed");
        assert_ne!(job.as_raw_handle(), std::ptr::null_mut());
        // Killing still works on the restricted job.
        job.terminate()
            .expect("terminate restricted job should succeed");
    }

    #[tokio::test]
    async fn restricted_job_kills_spawned_process() {
        // End-to-end: spawn a long-running child under a restricted job and
        // verify tree termination still works (the security filter must not
        // break the kill-on-timeout path).
        let job = JobObject::create_restricted().expect("restricted job creation should succeed");
        let mut child = tokio::process::Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn cmd");
        job.assign_child(&child)
            .expect("assign spawned process to job");
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        job.terminate().expect("terminate job");
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .expect("child should exit promptly after job termination")
            .expect("wait should succeed");
        assert!(!status.success(), "terminated process should not exit 0");
    }

    #[test]
    fn create_then_drop_is_safe() {
        // KILL_ON_JOB_CLOSE fires when the last handle closes — dropping an
        // empty job must not error or panic.
        let job = JobObject::create().expect("job object creation should succeed");
        drop(job);
    }

    #[tokio::test]
    async fn terminate_kills_spawned_process() {
        // Spawn a cmd that launches a long-running child, assign the shell to
        // a job, then terminate — the whole tree (cmd + child) must die.
        let job = JobObject::create().expect("job object creation should succeed");

        let mut child = tokio::process::Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn cmd");

        job.assign_child(&child)
            .expect("assign spawned process to job");

        // Give ping a moment to actually start.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        job.terminate().expect("terminate job");

        let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .expect("child should exit promptly after job termination")
            .expect("wait should succeed");

        assert!(!status.success(), "terminated process should not exit 0");
    }
}
