//! Asking the GPU scheduler to run the share ahead of a game (Settings
//! → Stream, 1.4.0).
//!
//! Windows' GPU scheduler (WDDM) runs each process's GPU queues at the
//! process's *GPU scheduling priority class*, NORMAL for everything
//! unless raised. A game that keeps the GPU at 99 % — most do, with no
//! frame cap — fills the queue, and the share's own GPU work waits
//! behind it: the desktop copy (`dxgi.rs`: `CopySubresourceRegion`,
//! the half-size `GenerateMips`, and `Map`, which blocks until the copy
//! is done) and the NVENC / AMF / Quick Sync session. The stream drops
//! to a few frames a second while the game runs fine, and recovers
//! only when the sharer caps the game's frame rate: not a matter of
//! how much GPU the share needs (a few percent) but of who goes first.
//!
//! OBS (24.0.3, "GPU priority fix") and Sunshine raise the class with
//! `D3DKMTSetProcessSchedulingPriorityClass`, REALTIME when the
//! process holds the "Increase scheduling priority" privilege
//! (`SeIncreaseBasePriorityPrivilege`: an elevated process has it,
//! disabled until enabled on the token; a plain one does not) and HIGH
//! otherwise, and set the capture device's thread priority to the
//! maximum (`IDXGIDevice::SetGPUThreadPriority(7)`). This module does
//! the same, for the share's lifetime: [`request`] when it starts,
//! [`release`] when it stops (NORMAL again, so a game played after the
//! share is not the one waiting). The cost is the game's: a frame or
//! two a second to the share, which is what the sharer asked for.
//!
//! Nothing else: `SetPriorityClass` is the CPU scheduler's (the audio
//! threads already have MMCSS), and Windows' Game Mode decides which
//! process the *OS* favours — it does not move the GPU queue.
//!
//! Elsewhere (macOS, Linux) there is no such knob; both functions are
//! no-ops there and the switch stays hidden (`settings.slint`).

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether the next capture device should ask for the maximum thread
/// priority (`dxgi.rs` reads it when it opens the duplication).
static REQUESTED: AtomicBool = AtomicBool::new(false);

/// The share is starting: raise the process's GPU priority when `on`,
/// put it back otherwise (a share restarted with the switch now off).
/// Called before the capturer opens, so its device gets the thread
/// priority too.
pub fn request(on: bool) {
    REQUESTED.store(on, Ordering::Relaxed);
    #[cfg(target_os = "windows")]
    if on {
        platform::raise();
    } else {
        platform::lower();
    }
}

/// The share stopped: the process's GPU priority goes back to NORMAL.
pub fn release() {
    REQUESTED.store(false, Ordering::Relaxed);
    #[cfg(target_os = "windows")]
    platform::lower();
}

/// Whether a share asked for it (the capture device's thread priority).
#[cfg(target_os = "windows")]
pub(crate) fn requested() -> bool {
    REQUESTED.load(Ordering::Relaxed)
}

/// Whether this platform has the knob at all (the switch shows only
/// where it does).
#[must_use]
pub const fn available() -> bool {
    cfg!(target_os = "windows")
}

#[cfg(target_os = "windows")]
mod platform {
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows::core::{w, PCWSTR};
    use windows::Wdk::Graphics::Direct3D::{
        D3DKMTSetProcessSchedulingPriorityClass, D3DKMT_SCHEDULINGPRIORITYCLASS,
        D3DKMT_SCHEDULINGPRIORITYCLASS_HIGH, D3DKMT_SCHEDULINGPRIORITYCLASS_NORMAL,
        D3DKMT_SCHEDULINGPRIORITYCLASS_REALTIME,
    };
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_NOT_ALL_ASSIGNED, HANDLE, LUID,
    };
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// The class is raised: `lower` has something to put back.
    static RAISED: AtomicBool = AtomicBool::new(false);
    /// The privilege was asked for once; the token does not change.
    static PRIVILEGE_ASKED: AtomicBool = AtomicBool::new(false);

    /// Enable `SeIncreaseBasePriorityPrivilege` on our token, which
    /// REALTIME needs. An elevated process holds it disabled; a plain
    /// one does not hold it, and `AdjustTokenPrivileges` then succeeds
    /// with `ERROR_NOT_ALL_ASSIGNED` — reported here as an error, so
    /// the log says which case this is.
    fn enable_increase_priority() -> windows::core::Result<()> {
        let mut token = HANDLE::default();
        // SAFETY: this process's pseudo-handle, and a handle written
        // here, closed below whatever `enable_on` says.
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &raw mut token,
            )?;
        }
        let adjusted = enable_on(token);
        // SAFETY: the handle opened above, closed once.
        let _ = unsafe { CloseHandle(token) };
        adjusted
    }

    /// The privilege on an open token (`TOKEN_ADJUST_PRIVILEGES |
    /// TOKEN_QUERY`).
    fn enable_on(token: HANDLE) -> windows::core::Result<()> {
        let mut luid = LUID::default();
        let name = w!("SeIncreaseBasePriorityPrivilege");
        // SAFETY: the structures are built here and outlive the calls;
        // the token is open for adjusting (the caller's).
        unsafe {
            LookupPrivilegeValueW(PCWSTR::null(), name, &raw mut luid)?;
            let privileges = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            AdjustTokenPrivileges(token, false, Some(&raw const privileges), 0, None, None)?;
            if GetLastError() == ERROR_NOT_ALL_ASSIGNED {
                return Err(ERROR_NOT_ALL_ASSIGNED.into());
            }
        }
        Ok(())
    }

    fn set_class(class: D3DKMT_SCHEDULINGPRIORITYCLASS) -> Result<(), i32> {
        // SAFETY: a plain call on this process's pseudo-handle.
        let status = unsafe { D3DKMTSetProcessSchedulingPriorityClass(GetCurrentProcess(), class) };
        // NT_SUCCESS: zero and the informational values are success.
        if status.0 >= 0 {
            Ok(())
        } else {
            Err(status.0)
        }
    }

    pub(super) fn raise() {
        if RAISED.load(Ordering::Relaxed) {
            return;
        }
        if !PRIVILEGE_ASKED.swap(true, Ordering::Relaxed) {
            match enable_increase_priority() {
                Ok(()) => log::info!("GPU priority: the increase-priority privilege is enabled"),
                Err(e) => log::info!(
                    "GPU priority: no increase-priority privilege ({e}); realtime needs \
                     Baucord started as administrator, high does not"
                ),
            }
        }
        for (class, name) in [
            (D3DKMT_SCHEDULINGPRIORITYCLASS_REALTIME, "realtime"),
            (D3DKMT_SCHEDULINGPRIORITYCLASS_HIGH, "high"),
        ] {
            match set_class(class) {
                Ok(()) => {
                    RAISED.store(true, Ordering::Relaxed);
                    log::info!("GPU priority: {name} for the share (Windows GPU scheduler)");
                    return;
                }
                Err(status) => {
                    log::debug!("GPU priority: {name} refused, NTSTATUS {status:#x}");
                }
            }
        }
        log::warn!(
            "GPU priority: Windows refused every class; the share's GPU work queues behind \
             the game's"
        );
    }

    pub(super) fn lower() {
        if !RAISED.swap(false, Ordering::Relaxed) {
            return;
        }
        match set_class(D3DKMT_SCHEDULINGPRIORITYCLASS_NORMAL) {
            Ok(()) => log::info!("GPU priority: normal again"),
            Err(status) => {
                log::warn!("GPU priority: could not go back to normal, NTSTATUS {status:#x}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_release_are_harmless_everywhere() {
        request(true);
        request(false);
        release();
        assert_eq!(available(), cfg!(target_os = "windows"));
    }
}
