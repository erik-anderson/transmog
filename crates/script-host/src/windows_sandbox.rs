//! Small audited Win32 boundary used before any untrusted code is read.

use std::{
    ffi::c_void,
    mem::{size_of, zeroed},
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr::{null, null_mut},
    time::{SystemTime, UNIX_EPOCH},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, HANDLE},
    Security::{
        FreeSid, GetTokenInformation,
        Isolation::{CreateAppContainerProfile, DeleteAppContainerProfile},
        SECURITY_CAPABILITIES, TOKEN_QUERY, TokenIsAppContainer,
    },
    System::{
        Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_UILIMIT_DESKTOP,
            JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
            JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES,
            JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
            JOB_OBJECT_UILIMIT_WRITECLIPBOARD, JOBOBJECT_BASIC_UI_RESTRICTIONS,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicUIRestrictions,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        },
        SystemInformation::GetWindowsDirectoryW,
        Threading::{
            CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
            GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, OpenProcessToken,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ResumeThread,
            STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this wrapper exclusively owns a valid kernel handle.
            unsafe { CloseHandle(self.0) };
        }
    }
}

struct Profile {
    name: Vec<u16>,
    sid: windows_sys::Win32::Security::PSID,
}

impl Drop for Profile {
    fn drop(&mut self) {
        // SAFETY: both values came from CreateAppContainerProfile and remain
        // valid until this cleanup after the sandboxed child has exited.
        unsafe {
            FreeSid(self.sid);
            DeleteAppContainerProfile(self.name.as_ptr());
        }
    }
}

struct AttributeList {
    bytes: Vec<usize>,
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        // SAFETY: initialization succeeded and the aligned backing allocation
        // remains live for this call.
        unsafe { DeleteProcThreadAttributeList(self.as_ptr()) };
    }
}

impl AttributeList {
    fn as_ptr(&mut self) -> windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST {
        self.bytes.as_mut_ptr().cast()
    }
}

pub(super) fn bootstrap(max_heap_bytes: usize) -> Result<i32, String> {
    // The profile is unique per bootstrap, so scripts cannot share its private
    // storage even if a future native-engine defect exposes file APIs.
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "sandbox clock is unavailable")?
        .as_nanos();
    let profile_name = wide(&format!("Transmog.Script.{}.{}", std::process::id(), nonce));
    let display_name = wide("Transmog isolated traffic script");
    let description = wide("Ephemeral zero-capability Transmog script host");
    let mut sid = null_mut();
    // SAFETY: all strings are terminated and output storage is valid.
    let profile_result = unsafe {
        CreateAppContainerProfile(
            profile_name.as_ptr(),
            display_name.as_ptr(),
            description.as_ptr(),
            null(),
            0,
            &mut sid,
        )
    };
    if profile_result < 0 || sid.is_null() {
        return Err("AppContainer profile creation failed".to_owned());
    }
    let _profile = Profile {
        name: profile_name,
        sid,
    };

    let mut attribute_bytes = 0_usize;
    // SAFETY: the documented sizing call intentionally passes a null buffer.
    unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut attribute_bytes) };
    if attribute_bytes == 0 {
        return Err("AppContainer attribute sizing failed".to_owned());
    }
    let words = attribute_bytes.div_ceil(size_of::<usize>());
    let mut attributes = AttributeList {
        bytes: vec![0_usize; words],
    };
    // SAFETY: the aligned allocation has the exact requested byte capacity.
    if unsafe { InitializeProcThreadAttributeList(attributes.as_ptr(), 1, 0, &mut attribute_bytes) }
        == 0
    {
        return Err("AppContainer attribute initialization failed".to_owned());
    }
    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: sid,
        Capabilities: null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    // SAFETY: the attribute list and capabilities outlive CreateProcessW.
    if unsafe {
        UpdateProcThreadAttribute(
            attributes.as_ptr(),
            0,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            (&raw const capabilities).cast::<c_void>(),
            size_of::<SECURITY_CAPABILITIES>(),
            null_mut(),
            null(),
        )
    } == 0
    {
        return Err("AppContainer security capability setup failed".to_owned());
    }

    let job = create_job(max_heap_bytes)?;
    let executable = std::env::current_exe().map_err(|_| "host executable is unavailable")?;
    let mut command_line = command_line(&executable);
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb =
        u32::try_from(size_of::<STARTUPINFOEXW>()).map_err(|_| "startup structure is invalid")?;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    // SAFETY: standard handles are inherited from the trusted bootstrap and
    // are the only IPC authority exposed to the sandbox.
    unsafe {
        startup.StartupInfo.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
        startup.StartupInfo.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
        startup.StartupInfo.hStdError = GetStdHandle(STD_ERROR_HANDLE);
    }
    startup.lpAttributeList = attributes.as_ptr();
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    let environment = minimal_environment()?;
    let executable_wide = wide_path(&executable);
    let flags = CREATE_SUSPENDED
        | CREATE_NO_WINDOW
        | CREATE_UNICODE_ENVIRONMENT
        | EXTENDED_STARTUPINFO_PRESENT;
    // SAFETY: every pointer remains valid for the duration of CreateProcessW;
    // the child starts suspended before untrusted bytes can be processed.
    if unsafe {
        CreateProcessW(
            executable_wide.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            flags,
            environment.as_ptr().cast(),
            null(),
            (&raw const startup.StartupInfo),
            &mut process,
        )
    } == 0
    {
        // SAFETY: immediately reads the calling thread's last-error value.
        let code = unsafe { GetLastError() };
        return Err(format!("AppContainer process creation failed ({code})"));
    }
    let process_handle = Handle(process.hProcess);
    let thread_handle = Handle(process.hThread);
    // SAFETY: process_handle names the still-suspended child and job is valid.
    if unsafe { AssignProcessToJobObject(job.0, process_handle.0) } == 0 {
        return Err("script Job Object assignment failed".to_owned());
    }
    // SAFETY: thread_handle is the primary suspended thread.
    if unsafe { ResumeThread(thread_handle.0) } == u32::MAX {
        return Err("sandboxed script process could not resume".to_owned());
    }
    // SAFETY: process_handle remains valid for the whole wait.
    unsafe { WaitForSingleObject(process_handle.0, INFINITE) };
    let mut exit_code = 70_u32;
    // SAFETY: output storage and process handle are valid.
    unsafe { GetExitCodeProcess(process_handle.0, &mut exit_code) };
    Ok(i32::try_from(exit_code).unwrap_or(70))
}

fn create_job(max_heap_bytes: usize) -> Result<Handle, String> {
    // SAFETY: null security attributes/name request an anonymous job.
    let raw = unsafe { CreateJobObjectW(null(), null()) };
    if raw.is_null() {
        return Err("script Job Object creation failed".to_owned());
    }
    let job = Handle(raw);
    let process_memory = max_heap_bytes
        .saturating_mul(3)
        .saturating_add(128 * 1024 * 1024);
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    limits.ProcessMemoryLimit = process_memory;
    // SAFETY: fixed-size information structure is fully initialized.
    if unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                .map_err(|_| "Job Object limit structure is invalid")?,
        )
    } == 0
    {
        return Err("script Job Object limits failed".to_owned());
    }
    let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
            | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
            | JOB_OBJECT_UILIMIT_EXITWINDOWS
            | JOB_OBJECT_UILIMIT_GLOBALATOMS
            | JOB_OBJECT_UILIMIT_HANDLES
            | JOB_OBJECT_UILIMIT_READCLIPBOARD
            | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
            | JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    };
    // SAFETY: fixed-size UI information structure is fully initialized.
    if unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectBasicUIRestrictions,
            (&raw const ui).cast(),
            u32::try_from(size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>())
                .map_err(|_| "Job Object UI structure is invalid")?,
        )
    } == 0
    {
        return Err("script Job Object UI restrictions failed".to_owned());
    }
    Ok(job)
}

pub(super) fn verify_current_process() -> Result<(), String> {
    let mut in_job = 0;
    // SAFETY: the pseudo-handle is always valid and output storage is valid.
    if unsafe { IsProcessInJob(GetCurrentProcess(), null_mut(), &mut in_job) } == 0 || in_job == 0 {
        return Err("script host is not in a Job Object".to_owned());
    }
    let mut token: HANDLE = null_mut();
    // SAFETY: output storage is valid; TOKEN_QUERY is read-only.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err("script host token cannot be inspected".to_owned());
    }
    let token = Handle(token);
    let mut app_container = 0_u32;
    let mut returned = 0_u32;
    // SAFETY: the token and fixed-size output storage are valid.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenIsAppContainer,
            (&raw mut app_container).cast(),
            u32::try_from(size_of::<u32>()).map_err(|_| "token query size is invalid")?,
            &mut returned,
        )
    } == 0
        || app_container == 0
    {
        return Err("script host is not an AppContainer".to_owned());
    }
    Ok(())
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn wide_path(value: &Path) -> Vec<u16> {
    value.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn command_line(executable: &Path) -> Vec<u16> {
    let mut value = String::from("\"");
    value.push_str(&executable.to_string_lossy());
    value.push_str("\" --sandboxed");
    wide(&value)
}

fn minimal_environment() -> Result<Vec<u16>, String> {
    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: the mutable UTF-16 buffer has the advertised capacity.
    let length = unsafe {
        GetWindowsDirectoryW(
            buffer.as_mut_ptr(),
            u32::try_from(buffer.len()).map_err(|_| "Windows path buffer is invalid")?,
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err("Windows directory is unavailable".to_owned());
    }
    buffer.truncate(length);
    let directory =
        String::from_utf16(&buffer).map_err(|_| "Windows directory encoding is invalid")?;
    let mut environment = Vec::new();
    let mut items = vec![
        format!("SystemRoot={directory}"),
        format!("WINDIR={directory}"),
    ];
    for name in ["LOCALAPPDATA", "TEMP", "TMP", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(name) {
            items.push(format!("{name}={}", value.to_string_lossy()));
        }
    }
    items.sort_by_key(|value| value.to_ascii_lowercase());
    for item in items {
        environment.extend(item.encode_utf16());
        environment.push(0);
    }
    environment.push(0);
    Ok(environment)
}
