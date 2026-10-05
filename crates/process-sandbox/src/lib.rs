#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

//! Shared, narrowly scoped operating-system boundary for hostile-input helper
//! processes. It grants no `AppContainer` capabilities and exposes only inherited
//! standard handles.

#[cfg(windows)]
mod windows;

/// Trusted labels used to create one ephemeral `AppContainer` profile.
#[derive(Clone, Copy, Debug)]
pub struct SandboxIdentity {
    /// Short ASCII profile namespace, such as `Script` or `Preview`.
    pub profile_namespace: &'static str,
    /// Operator-facing profile display name.
    pub display_name: &'static str,
    /// Operator-facing profile description.
    pub description: &'static str,
}

/// Re-executes the current helper binary inside an ephemeral zero-capability
/// `AppContainer` and a kill-on-close Job Object.
///
/// The child receives only inherited standard handles and the `--sandboxed`
/// argument. `process_memory_bytes` is the exact Job Object process limit.
///
/// # Errors
/// Returns a redaction-safe platform or resource setup failure.
pub fn sandbox_bootstrap(
    identity: SandboxIdentity,
    process_memory_bytes: usize,
) -> Result<i32, String> {
    if identity.profile_namespace.is_empty()
        || identity.profile_namespace.len() > 32
        || !identity
            .profile_namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
        || identity.display_name.is_empty()
        || identity.description.is_empty()
        || process_memory_bytes < 64 * 1024 * 1024
    {
        return Err("sandbox configuration is invalid".to_owned());
    }
    #[cfg(windows)]
    {
        windows::bootstrap(identity, process_memory_bytes)
    }
    #[cfg(not(windows))]
    {
        let _ = (identity, process_memory_bytes);
        Err("the helper sandbox is supported only on Windows".to_owned())
    }
}

/// Verifies that the current helper is inside both an `AppContainer` and a Job
/// Object before it reads hostile input.
///
/// # Errors
/// Returns a redaction-safe failure when either boundary is absent.
pub fn verify_current_process() -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::verify_current_process()
    }
    #[cfg(not(windows))]
    {
        Err("the helper sandbox is supported only on Windows".to_owned())
    }
}
