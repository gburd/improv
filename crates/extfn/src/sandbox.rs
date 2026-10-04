//! Best-effort OS sandbox for the subprocess runners (Python/R/Julia/Pure).
//!
//! The Wasm runtime (see `wasm.rs`) is already sandboxed by construction: it
//! runs in-process via the `wasmi` interpreter, which has no syscall access
//! and no host imports linked, so untrusted Wasm bodies cannot touch the
//! filesystem or network regardless of OS. This module hardens the
//! *subprocess* runtimes instead, which spawn a real OS process (`python3`,
//! `Rscript`, `julia`, `pure`) that would otherwise have the full permissions
//! of the Improv process.
//!
//! # Policy
//!
//! The host decides, never the model: [`SandboxPolicy::Restricted`] applies to
//! every external function unless the operator sets `IMPROV_EXTFN_TRUSTED=1`,
//! which selects [`SandboxPolicy::Trusted`] (no sandbox) for the whole
//! process. See [`policy_for`].
//!
//! # What's enforced, by platform
//!
//! * **Linux**: the interpreter is re-exec'd under `bwrap` (bubblewrap), an
//!   unprivileged filesystem/network sandbox, and `setrlimit` ceilings (CPU
//!   time, address space, open files, process count) are applied via
//!   `pre_exec`, alongside the wall-clock kill in `runner.rs`.
//! * **macOS / Windows**: there is no `bwrap`, so `Restricted` refuses to run
//!   (see below). With the opt-out set, macOS gets the `setrlimit` ceilings
//!   and Windows gets only the wall-clock timeout (a Job Object limiter is
//!   future work).
//!
//! # Fail closed
//!
//! `Restricted` without `bwrap` returns [`ExtFnError::Sandbox`] instead of
//! running the body unconfined, unless the operator sets
//! `IMPROV_EXTFN_ALLOW_UNSANDBOXED=1`, which falls back to `setrlimit` only.
//! A `setrlimit` failure in `pre_exec` aborts the spawn. A limit an ancestor
//! already lowered is kept (the ceiling is clamped to the existing hard limit,
//! never raised).

use crate::ExtFnError;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The refusal returned under `Restricted` when `bwrap` is missing.
const NO_BWRAP: &str =
    "bwrap not found; install bubblewrap or set IMPROV_EXTFN_ALLOW_UNSANDBOXED=1";

/// How strictly a subprocess runner should be confined.
///
/// `Restricted` is the default for every body. `Trusted` skips sandboxing and
/// only the operator selects it (`IMPROV_EXTFN_TRUSTED=1`, see `policy_for`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SandboxPolicy {
    /// No sandboxing applied; the interpreter runs with the caller's full
    /// filesystem/network/resource access (still subject to the existing
    /// wall-clock timeout in `runner.rs`).
    Trusted,
    /// `bwrap` + rlimits; refuses to run without `bwrap` unless the operator
    /// sets `IMPROV_EXTFN_ALLOW_UNSANDBOXED=1` (then rlimits only).
    #[default]
    Restricted,
}

/// The policy for every subprocess runner. The **host** decides, never the
/// model: `Restricted` unless the operator sets `IMPROV_EXTFN_TRUSTED=1`
/// (read once per process). [`ExternalFn::pure`](crate::ExternalFn::pure) is
/// model data, so it keeps only its caching meaning; letting it choose the
/// sandbox would let a shared model opt itself out.
pub fn policy_for() -> SandboxPolicy {
    static POLICY: OnceLock<SandboxPolicy> = OnceLock::new();
    *POLICY.get_or_init(|| policy_from_env(std::env::var_os("IMPROV_EXTFN_TRUSTED").as_deref()))
}

/// `IMPROV_EXTFN_TRUSTED` value to policy: exactly `1` means `Trusted`.
fn policy_from_env(trusted: Option<&std::ffi::OsStr>) -> SandboxPolicy {
    if trusted.is_some_and(|v| v == "1") {
        SandboxPolicy::Trusted
    } else {
        SandboxPolicy::Restricted
    }
}

/// `IMPROV_EXTFN_ALLOW_UNSANDBOXED=1`: the operator accepts running
/// `Restricted` bodies with rlimits only when `bwrap` is missing.
fn allow_unsandboxed() -> bool {
    static ALLOW: OnceLock<bool> = OnceLock::new();
    *ALLOW.get_or_init(|| {
        std::env::var_os("IMPROV_EXTFN_ALLOW_UNSANDBOXED").is_some_and(|v| v == "1")
    })
}

/// Resource ceilings applied via `setrlimit` under [`SandboxPolicy::Restricted`].
/// Generous enough not to break legitimate small computations, tight enough to
/// blunt runaway memory growth and fork bombs.
mod limits {
    /// CPU time, seconds. Defense-in-depth alongside the wall-clock kill in
    /// `runner.rs` (which also bounds wall time even for CPU-light but
    /// I/O-blocked processes).
    pub const CPU_SECONDS: u64 = 30;
    /// Address space, bytes (1 GiB). `RLIMIT_AS` bounds *virtual* memory,
    /// which for CPython/R/Julia's allocators is a reasonable proxy for RSS
    /// without needing cgroups.
    pub const ADDRESS_SPACE_BYTES: u64 = 1 << 30;
    /// Max open file descriptors.
    pub const MAX_FDS: u64 = 256;
    /// Max processes/threads for this (real) uid. `RLIMIT_NPROC` is scoped to
    /// the whole uid system-wide, not the sandboxed process's own subtree, so
    /// this must stay well above however many processes/threads the *host*
    /// already owns (a dev machine or CI runner commonly has hundreds), not
    /// just above what one interpreter needs — a low ceiling here doesn't
    /// just blunt the user's fork bomb, it can make `bwrap`'s own namespace
    /// setup (or Python's threads) fail with EAGAIN before the user body even
    /// runs. 4096 is comfortably above typical host process counts while still
    /// bounding a runaway fork loop (which would hit thousands of processes
    /// within its first second).
    pub const MAX_NPROC: u64 = 4096;
}

/// Apply `policy` to `cmd` before it is spawned: rewrite it as
/// `bwrap <sandbox args> -- <original cmd/args>` with a `pre_exec` rlimit
/// hook. Without `bwrap` this fails closed (see the module docs). `Trusted`
/// is a no-op.
///
/// Called from [`crate::runner::run_interpreter`] so every subprocess runner
/// (Python/R/Julia/Pure) is covered without per-language duplication.
pub fn apply(cmd: Command, policy: SandboxPolicy) -> Result<Command, ExtFnError> {
    apply_with(cmd, policy, bwrap_path(), allow_unsandboxed())
}

/// [`apply`] with the `bwrap` lookup and the opt-out injected (for tests).
fn apply_with(
    cmd: Command,
    policy: SandboxPolicy,
    bwrap: Option<PathBuf>,
    allow_unsandboxed: bool,
) -> Result<Command, ExtFnError> {
    if policy == SandboxPolicy::Trusted {
        return Ok(cmd);
    }
    match bwrap {
        Some(bwrap) => Ok(with_rlimits(wrap_bwrap(&bwrap, &cmd))),
        None if allow_unsandboxed => Ok(with_rlimits(cmd)),
        None => Err(ExtFnError::Sandbox(NO_BWRAP.into())),
    }
}

/// `bwrap` on `PATH`. Bubblewrap is Linux-only, so `None` elsewhere.
fn bwrap_path() -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        which("bwrap")
    } else {
        None
    }
}

/// Rebuild `cmd` as `bwrap <sandbox flags> -- <original program> <args>`.
fn wrap_bwrap(bwrap: &Path, cmd: &Command) -> Command {
    let mut wrapped = Command::new(bwrap);
    wrapped.args(bwrap_args());
    wrapped.arg(cmd.get_program());
    wrapped.args(cmd.get_args());
    wrapped
}

/// Minimal unprivileged bubblewrap sandbox: read-only root (so the
/// interpreter binary and its shared libraries resolve normally), a fresh
/// empty `/tmp`, no network namespace, no IPC/UTS namespace sharing, and the
/// sandbox is killed if Improv itself dies. Deliberately does NOT bind the
/// real `$HOME` or CWD, so absolute paths outside the read-only root (e.g. a
/// repo checkout under `$HOME`) are unreadable from inside the sandbox.
fn bwrap_args() -> Vec<&'static str> {
    vec![
        "--ro-bind",
        "/",
        "/",
        "--tmpfs",
        "/tmp",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--unshare-net",
        "--unshare-ipc",
        "--unshare-uts",
        "--die-with-parent",
        "--",
    ]
}

/// Search `PATH` for `name`, the same way the shell would. `std::process`
/// doesn't expose this, so it's a small manual walk.
fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        candidate.is_file().then_some(candidate)
    })
}

/// libc's raw resource-id type: `c_int` on macOS/BSD, `u32` on Linux (glibc).
/// A type alias lets [`set_rlimit`] take whichever `libc::RLIMIT_*` constants
/// resolve to on the target, without a manual cast at each of its call sites.
#[cfg(target_os = "linux")]
type RlimitResource = u32;
#[cfg(all(unix, not(target_os = "linux")))]
type RlimitResource = libc::c_int;

/// Attach a `pre_exec` hook that applies the `setrlimit` ceilings. Any
/// failure is returned from the hook, which aborts the spawn (fail closed).
#[cfg(unix)]
fn with_rlimits(mut cmd: Command) -> Command {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            set_rlimit(libc::RLIMIT_CPU, limits::CPU_SECONDS)?;
            set_rlimit(libc::RLIMIT_AS, limits::ADDRESS_SPACE_BYTES)?;
            set_rlimit(libc::RLIMIT_NOFILE, limits::MAX_FDS)?;
            set_rlimit(libc::RLIMIT_NPROC, limits::MAX_NPROC)?;
            Ok(())
        });
    }
    cmd
}

/// `setrlimit(resource, {cur: v, max: v})` with `v = min(value, hard limit)`,
/// so a limit an ancestor already lowered is kept rather than refused (raising
/// a hard limit fails with `EPERM`). Async-signal-safe (two syscalls, no
/// allocation), as `pre_exec` requires.
///
/// `resource` takes libc's raw resource-id type, which is `c_int` on macOS/BSD
/// but `u32` on Linux (glibc); callers pass the platform's own
/// `libc::RLIMIT_*` constant, so this stays correct on both without a cast at
/// each call site.
#[cfg(unix)]
fn set_rlimit(resource: RlimitResource, value: u64) -> std::io::Result<()> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(resource, &mut limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let value = (value as libc::rlim_t).min(limit.rlim_max);
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    if unsafe { libc::setrlimit(resource, &limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Windows: no rlimit equivalent is applied here. Rely on the wall-clock
/// timeout in `runner.rs` only; a Job Object memory/CPU limiter (via the
/// `win32job` crate) is future work.
#[cfg(not(unix))]
fn with_rlimits(cmd: Command) -> Command {
    cmd
}

/// Test-only hook exercising the rlimits-only fallback path directly
/// (skipping the `bwrap`-lookup branch entirely), so a test asserting
/// "rlimits-only still runs the interpreter" doesn't depend on whether this
/// machine happens to have `bwrap` installed.
#[cfg(test)]
pub(crate) fn apply_rlimits_only_for_test(cmd: Command) -> Command {
    with_rlimits(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_is_host_decided() {
        use std::ffi::OsStr;
        assert_eq!(policy_from_env(None), SandboxPolicy::Restricted);
        assert_eq!(
            policy_from_env(Some(OsStr::new("0"))),
            SandboxPolicy::Restricted
        );
        assert_eq!(
            policy_from_env(Some(OsStr::new("true"))),
            SandboxPolicy::Restricted
        );
        assert_eq!(
            policy_from_env(Some(OsStr::new("1"))),
            SandboxPolicy::Trusted
        );
        if std::env::var_os("IMPROV_EXTFN_TRUSTED").is_none() {
            assert_eq!(policy_for(), SandboxPolicy::Restricted);
        }
    }

    #[test]
    fn trusted_policy_leaves_command_untouched() {
        let cmd = Command::new("python3");
        let cmd = apply(cmd, SandboxPolicy::Trusted).expect("trusted");
        assert_eq!(cmd.get_program(), "python3");
        assert_eq!(cmd.get_args().count(), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bwrap_lookup_matches_path_probe() {
        // Consistency check against the same probe the runner tests use: if
        // `which bwrap` finds it, our manual PATH walk must agree.
        let on_path = std::process::Command::new("which")
            .arg("bwrap")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert_eq!(which("bwrap").is_some(), on_path);
    }

    #[test]
    fn restricted_without_bwrap_is_refused() {
        let err = apply_with(
            Command::new("python3"),
            SandboxPolicy::Restricted,
            None,
            false,
        )
        .unwrap_err();
        match err {
            ExtFnError::Sandbox(m) => assert_eq!(m, NO_BWRAP),
            other => panic!("expected Sandbox, got {other:?}"),
        }
    }

    #[test]
    fn restricted_without_bwrap_opt_out_uses_rlimits_only() {
        let cmd = apply_with(
            Command::new("python3"),
            SandboxPolicy::Restricted,
            None,
            true,
        )
        .expect("opt-out runs");
        assert_eq!(cmd.get_program(), "python3");
    }

    #[test]
    fn restricted_with_bwrap_wraps_the_interpreter() {
        let mut cmd = Command::new("python3");
        cmd.arg("-");
        let cmd = apply_with(
            cmd,
            SandboxPolicy::Restricted,
            Some("/usr/bin/bwrap".into()),
            false,
        )
        .expect("wraps");
        assert_eq!(cmd.get_program(), "/usr/bin/bwrap");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(&args[args.len() - 3..], ["--", "python3", "-"]);
    }

    #[cfg(unix)]
    #[test]
    fn set_rlimit_reports_failure() {
        // An unknown resource id is EINVAL: the error must surface (and so
        // abort the spawn from `pre_exec`), not be swallowed.
        assert!(set_rlimit(9999, 1).is_err());
        assert!(set_rlimit(libc::RLIMIT_NOFILE, u64::MAX).is_ok());
    }
}
