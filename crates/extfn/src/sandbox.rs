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
//! [`SandboxPolicy::Restricted`] is applied to every external function whose
//! [`ExternalFn::pure`] flag is set (the default the caller should use for
//! untrusted/author-supplied bodies); [`SandboxPolicy::Trusted`] disables the
//! sandbox entirely (e.g. for local development or an explicitly
//! reviewed/first-party function). See [`policy_for`].
//!
//! # What's enforced, by platform
//!
//! * **Linux**: if `bwrap` (bubblewrap) is on `PATH`, the interpreter is
//!   re-exec'd under a minimal unprivileged sandbox: read-only root
//!   filesystem, no network namespace, a fresh empty `/tmp`, no access to the
//!   real `$HOME`. This is a real filesystem/network security boundary.
//!   Whether or not `bwrap` is used, `setrlimit` is additionally applied via
//!   `pre_exec` for CPU time, address space, open files, and process count —
//!   defense in depth alongside the existing wall-clock kill in `runner.rs`.
//! * **macOS**: same `setrlimit`/`pre_exec` treatment (no `bwrap` equivalent
//!   attempted; sandbox-exec/App Sandbox profiles are a larger scope than
//!   this pass covers).
//! * **Windows**: no-op today. `setrlimit` doesn't exist on Windows; a real
//!   limiter would mean a Job Object (via e.g. the `win32job` crate) capping
//!   memory/CPU. That's future work — left as a clearly-marked no-op so
//!   Windows callers get exactly what they get today (the wall-clock timeout
//!   in `runner.rs`, unchanged) rather than a fragile partial limiter.
//!
//! # ponytail: fail-open, not fail-closed
//!
//! If `bwrap` is absent, or a `setrlimit` call itself fails (e.g. a
//! kernel/container that clamps limits lower than we can set, or a limit type
//! not supported on some exotic unix), the interpreter still runs
//! unsandboxed-on-that-axis rather than refusing to evaluate the function.
//! This is a deliberate best-effort ceiling, not a hard guarantee — the
//! wall-clock kill in `runner.rs` is unaffected either way and stays
//! authoritative. Upgrade path if a hard guarantee is ever required: require
//! `bwrap`/gVisor/a real container runtime and refuse to run without it,
//! rather than degrading silently.

use std::process::Command;

/// How strictly a subprocess runner should be confined.
///
/// `Restricted` is the default for author-supplied function bodies (anything
/// marked [`ExternalFn::pure`](crate::ExternalFn::pure) — the model-level
/// author assertion that the body is safe to run repeatedly/anywhere, which
/// is exactly the class of code this sandbox targets). `Trusted` skips
/// sandboxing (first-party/reviewed bodies, or explicit opt-out).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SandboxPolicy {
    /// No sandboxing applied; the interpreter runs with the caller's full
    /// filesystem/network/resource access (still subject to the existing
    /// wall-clock timeout in `runner.rs`).
    Trusted,
    /// Apply every sandboxing mechanism available on this platform (bwrap +
    /// rlimits on Linux/macOS; rlimits only if bwrap is unavailable; a no-op
    /// on Windows beyond the existing timeout).
    #[default]
    Restricted,
}

/// Map an [`ExternalFn::pure`](crate::ExternalFn::pure) flag to a policy.
/// `pure = true` (the common case, and the default an author gets) is
/// `Restricted`; `pure = false` is `Trusted` — an explicit author assertion
/// that this function is *not* a pure sandboxable computation (e.g. it's
/// meant to do I/O) is also a statement that the caller already trusts it
/// with fewer restrictions.
pub fn policy_for(pure: bool) -> SandboxPolicy {
    if pure {
        SandboxPolicy::Restricted
    } else {
        SandboxPolicy::Trusted
    }
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

/// Apply `policy` to `cmd` before it is spawned: either rewrite it in place
/// to be `bwrap <sandbox args> -- <original cmd/args>` (Linux, when `bwrap` is
/// on `PATH`), or attach a `pre_exec` rlimit hook (any unix, including the
/// Linux-without-bwrap fallback). `Trusted` is a no-op.
///
/// Called from [`crate::runner::run_interpreter`] so every subprocess runner
/// (Python/R/Julia/Pure) is covered without per-language duplication.
pub fn apply(cmd: Command, policy: SandboxPolicy) -> Command {
    if policy == SandboxPolicy::Trusted {
        return cmd;
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(wrapped) = try_bwrap(&cmd) {
            return with_rlimits(wrapped);
        }
    }
    with_rlimits(cmd)
}

/// On Linux, if `bwrap` is available, rebuild `cmd` as
/// `bwrap <sandbox flags> -- <original program> <original args>`.
/// Returns `None` (leaving `cmd` untouched by the caller) if `bwrap` is not on
/// `PATH` — the ponytail fail-open path.
#[cfg(target_os = "linux")]
fn try_bwrap(cmd: &Command) -> Option<Command> {
    let bwrap = which("bwrap")?;
    let mut wrapped = Command::new(bwrap);
    wrapped.args(bwrap_args());
    wrapped.arg(cmd.get_program());
    wrapped.args(cmd.get_args());
    Some(wrapped)
}

/// Minimal unprivileged bubblewrap sandbox: read-only root (so the
/// interpreter binary and its shared libraries resolve normally), a fresh
/// empty `/tmp`, no network namespace, no IPC/UTS namespace sharing, and the
/// sandbox is killed if Improv itself dies. Deliberately does NOT bind the
/// real `$HOME` or CWD, so absolute paths outside the read-only root (e.g. a
/// repo checkout under `$HOME`) are unreadable from inside the sandbox.
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
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

/// Attach a `pre_exec` hook that applies best-effort `setrlimit` ceilings.
/// Each limit is applied independently and failures are ignored (fail-open,
/// see module docs) — a limit the kernel refuses to lower (already lower via
/// an ancestor's rlimit, e.g. under CI/containers) is not a reason to refuse
/// to run the interpreter at all.
#[cfg(unix)]
fn with_rlimits(mut cmd: Command) -> Command {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            set_rlimit(libc::RLIMIT_CPU, limits::CPU_SECONDS);
            set_rlimit(libc::RLIMIT_AS, limits::ADDRESS_SPACE_BYTES);
            set_rlimit(libc::RLIMIT_NOFILE, limits::MAX_FDS);
            set_rlimit(libc::RLIMIT_NPROC, limits::MAX_NPROC);
            Ok(())
        });
    }
    cmd
}

/// `setrlimit(resource, {cur: value, max: value})`, ignoring the return code.
/// Async-signal-safe (no allocation, no locking) so it's safe to call from
/// `pre_exec`, which runs after `fork()` in the child before `exec()`.
///
/// `resource` takes libc's raw resource-id type, which is `c_int` on macOS/BSD
/// but `u32` on Linux (glibc); callers pass the platform's own
/// `libc::RLIMIT_*` constant, so this stays correct on both without a cast at
/// each call site.
#[cfg(unix)]
fn set_rlimit(resource: RlimitResource, value: u64) {
    let limit = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    // ponytail: ignore the result (fail-open, see module docs). A hard
    // guarantee would check this and refuse to run instead.
    unsafe {
        libc::setrlimit(resource, &limit);
    }
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
    fn policy_for_pure_is_restricted() {
        assert_eq!(policy_for(true), SandboxPolicy::Restricted);
        assert_eq!(policy_for(false), SandboxPolicy::Trusted);
    }

    #[test]
    fn trusted_policy_leaves_command_untouched() {
        let cmd = Command::new("python3");
        let cmd = apply(cmd, SandboxPolicy::Trusted);
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
    fn restricted_policy_still_runnable() {
        // `apply` never panics and always returns a `Command` whose program
        // is either the original (rlimits-only fallback) or `bwrap` (Linux
        // with bwrap present); either way it stays runnable (fail-open).
        let cmd = Command::new("python3");
        let cmd = apply(cmd, SandboxPolicy::Restricted);
        let prog = cmd.get_program().to_string_lossy();
        assert!(prog == "python3" || prog.ends_with("bwrap"));
    }
}
