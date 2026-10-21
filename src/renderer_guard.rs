//! Keep native graphics-driver failures outside the startup supervisor.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, GetLastError, HANDLE, WAIT_FAILED,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateEventW, EVENT_MODIFY_STATE, GetExitCodeProcess, GetProcessTimes,
    OpenEventW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, SetEvent, WaitForMultipleObjects,
    WaitForSingleObject,
};

const SESSION: &str = "FAST_PACK_RENDER_SESSION";
const TIMEOUT: Duration = Duration::from_secs(20);
static READY: AtomicBool = AtomicBool::new(false);
static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct SavedRenderer {
    software: bool,
}

fn saved_software(path: &Path) -> bool {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SavedRenderer>(&bytes).ok())
        .is_some_and(|value| value.software)
}

fn remember(path: &Path, software: bool) -> Result<()> {
    let directory = path.parent().context("渲染配置目录无效")?;
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(&serde_json::to_vec(&SavedRenderer { software })?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).context("无法保存渲染兼容设置")?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct GpuLease {
    pid: u32,
    creation: u64,
}

/// A normal return clears the lease. Native crashes and process aborts leave it
/// behind so the next launch can choose software without restarting lost work.
pub struct RuntimeLease(std::path::PathBuf);

impl RuntimeLease {
    fn create(directory: &Path, pid: u32, creation: u64) -> Result<Self> {
        let directory = directory.join("runtime");
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("gpu-{pid}-{creation}.json"));
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        file.write_all(&serde_json::to_vec(&GpuLease { pid, creation })?)?;
        file.persist_noclobber(&path).map_err(|error| error.error)?;
        Ok(Self(path))
    }

    pub fn finish(self) {
        let _ = fs::remove_file(self.0);
    }
}

// None means the process cannot be inspected; do not mistake that for a crash.
fn process_creation(pid: u32) -> Option<Option<u64>> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return (GetLastError() == ERROR_INVALID_PARAMETER).then_some(None);
        }
        let mut code = 0;
        if GetExitCodeProcess(handle, &mut code) == 0 {
            CloseHandle(handle);
            return None;
        }
        if code != 259 {
            // STILL_ACTIVE
            CloseHandle(handle);
            return Some(None);
        }
        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0;
        CloseHandle(handle);
        ok.then_some(Some(
            (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime),
        ))
    }
}

pub fn runtime_lease() -> Option<RuntimeLease> {
    if std::env::var("ICED_BACKEND").as_deref() != Ok("wgpu") {
        return None;
    }
    let directory = crate::preferences::state_directory().ok()?;
    let pid = std::process::id();
    let creation = process_creation(pid)??;
    RuntimeLease::create(&directory, pid, creation).ok()
}

fn recover_unclean(directory: &Path, alive: impl Fn(u32, u64) -> bool) -> bool {
    let Ok(entries) = fs::read_dir(directory.join("runtime")) else {
        return false;
    };
    let mut unclean = false;
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("gpu-")
            || !entry
                .metadata()
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= 4096)
        {
            continue;
        }
        let Some(lease) = fs::read(entry.path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<GpuLease>(&bytes).ok())
        else {
            continue;
        };
        if alive(lease.pid, lease.creation) {
            continue;
        }
        unclean = true;
        let logs = directory.join("logs");
        if fs::create_dir_all(&logs).is_ok() {
            let _ = fs::rename(
                entry.path(),
                logs.join(format!(
                    "renderer-unclean-{}-{}.json",
                    lease.pid, lease.creation
                )),
            );
        }
    }
    unclean
}

fn wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

struct Event(HANDLE);
impl Event {
    fn new(name: &str) -> Result<Self> {
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, wide(name).as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error()).context("无法建立界面启动信号");
        }
        Ok(Self(handle))
    }
    fn signaled(&self) -> bool {
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
    }
}
impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct Session {
    name: String,
    entered: Event,
    ready: Event,
}
impl Session {
    fn new() -> Result<Self> {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let name = format!(
            "Local\\FastPack-Renderer-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        Ok(Self {
            entered: Event::new(&format!("{name}-entered"))?,
            ready: Event::new(&format!("{name}-ready"))?,
            name,
        })
    }
}

fn signal(suffix: &str) {
    let Some(name) = std::env::var_os(SESSION) else {
        return;
    };
    let name = format!("{}-{suffix}", name.to_string_lossy());
    let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, wide(&name).as_ptr()) };
    if !handle.is_null() {
        let event = Event(handle);
        unsafe {
            SetEvent(event.0);
        }
    }
}

pub fn pending() -> bool {
    std::env::var_os(SESSION).is_some() && !READY.load(Ordering::Relaxed)
}
pub fn entered() {
    signal("entered");
}
pub fn ready() {
    if pending() {
        signal("ready");
        READY.store(true, Ordering::Relaxed);
    }
}

enum Attempt {
    Ready(Child),
    Exited {
        code: i32,
        entered: bool,
        detail: String,
    },
    Timeout {
        entered: bool,
    },
}

fn failure_detail(file: &mut File) -> String {
    let mut bytes = Vec::new();
    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.take(16 * 1024).read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

fn attempt(mut command: Command, timeout: Duration) -> Result<Attempt> {
    let session = Session::new()?;
    let mut errors = tempfile::tempfile()?;
    // The child inherits this process-local error mode. A WER popup must not
    // prevent the supervisor from observing a native driver exception.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    }
    let mut child = command
        .env(SESSION, &session.name)
        .creation_flags(CREATE_NO_WINDOW)
        .stderr(Stdio::from(errors.try_clone()?))
        .spawn()
        .context("无法启动界面进程")?;
    let handles = [session.ready.0, child.as_raw_handle().cast()];
    let outcome = unsafe {
        WaitForMultipleObjects(
            2,
            handles.as_ptr(),
            0,
            timeout.as_millis().min(u32::MAX as u128 - 1) as u32,
        )
    };
    if outcome == WAIT_OBJECT_0 {
        return Ok(Attempt::Ready(child));
    }
    if outcome == WAIT_OBJECT_0 + 1 {
        let status = child.wait()?;
        return Ok(Attempt::Exited {
            code: status.code().unwrap_or(1),
            entered: session.entered.signaled(),
            detail: failure_detail(&mut errors),
        });
    }
    // Only terminate the UI process created by this attempt, before it has
    // acknowledged a usable frame. Never restart an established work session.
    let wait_error = (outcome == WAIT_FAILED).then(std::io::Error::last_os_error);
    let _ = child.kill();
    let _ = child.wait();
    if outcome == WAIT_TIMEOUT {
        return Ok(Attempt::Timeout {
            entered: session.entered.signaled(),
        });
    }
    if let Some(error) = wait_error {
        return Err(error).context("无法等待界面启动");
    }
    bail!("界面启动返回未知状态")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plan {
    Direct,
    Software,
    Guard,
}
fn plan(explicit: bool, software: bool, retry: bool, gpu: bool, saved: bool) -> Plan {
    if software {
        Plan::Software
    } else if explicit {
        Plan::Direct
    } else if !gpu || saved && !retry {
        Plan::Software
    } else {
        Plan::Guard
    }
}

// A launch failure before process creation cannot be isolated; run the existing
// process in software instead. A successfully started helper never recurses.
enum Launch {
    DirectSoftware,
    Started(Child),
    Finished(i32),
}
fn supervise(
    mut command: impl FnMut(&str) -> Command,
    cache: &Path,
    timeout: Duration,
) -> Result<Launch> {
    match attempt(command("wgpu"), timeout) {
        Ok(Attempt::Ready(child)) => {
            if saved_software(cache) {
                let _ = remember(cache, false);
            }
            return Ok(Launch::Started(child));
        }
        Ok(Attempt::Exited { code: 0, .. }) => return Ok(Launch::Finished(0)),
        Ok(Attempt::Exited {
            code,
            entered: false,
            detail,
        }) => {
            bail!("界面初始化之前退出（0x{:08X}）：{detail}", code as u32);
        }
        Ok(Attempt::Timeout { entered: false }) => bail!("界面初始化之前启动超时"),
        Err(_) => return Ok(Launch::DirectSoftware),
        _ => crate::startup::record("gpu_startup_failed"),
    }
    match attempt(command("tiny-skia"), timeout)? {
        Attempt::Ready(child) => {
            let _ = remember(cache, true);
            crate::startup::record("software_startup_recovered");
            Ok(Launch::Started(child))
        }
        Attempt::Exited { code: 0, .. } => Ok(Launch::Finished(0)),
        Attempt::Exited { code, detail, .. } => {
            bail!(
                "GPU 启动失败，软件渲染也未能启动（0x{:08X}）：{detail}",
                code as u32
            )
        }
        Attempt::Timeout { .. } => bail!("GPU 启动失败，软件渲染启动超时"),
    }
}

fn use_software() {
    // Called before any application threads or graphics initialization.
    unsafe {
        std::env::set_var("ICED_BACKEND", "tiny-skia");
    }
}

pub fn launch_if_needed() -> Result<Option<i32>> {
    if std::env::var_os(SESSION).is_some() {
        return Ok(None);
    }
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let software = arguments
        .iter()
        .any(|value| value == "--software-rendering");
    let retry = arguments.iter().any(|value| value == "--retry-gpu");
    let initial = plan(
        std::env::var_os("ICED_BACKEND").is_some(),
        software,
        retry,
        crate::windows_compat::prefers_gpu(),
        false,
    );
    match initial {
        Plan::Direct => Ok(None),
        Plan::Software => {
            use_software();
            Ok(None)
        }
        Plan::Guard => {
            let Ok(directory) = crate::preferences::state_directory() else {
                use_software();
                return Ok(None);
            };
            let cache = directory.join("renderer.json");
            if recover_unclean(&directory, |pid, creation| {
                process_creation(pid).is_none_or(|actual| actual == Some(creation))
            }) {
                let _ = remember(&cache, true);
                crate::startup::record("previous_gpu_session_unclean");
            }
            if saved_software(&cache) && !retry {
                use_software();
                return Ok(None);
            }
            let executable = std::env::current_exe()?;
            match supervise(
                |backend| {
                    let mut command = Command::new(&executable);
                    command.args(&arguments).env("ICED_BACKEND", backend);
                    command
                },
                &cache,
                TIMEOUT,
            )? {
                Launch::DirectSoftware => {
                    use_software();
                    Ok(None)
                }
                Launch::Finished(code) => Ok(Some(code)),
                // A usable UI now owns the session. Release the supervisor's
                // unpacked executable and leave only that UI process running.
                Launch::Started(_child) => Ok(Some(0)),
            }
        }
    }
}

#[cfg(test)]
#[path = "renderer_guard_tests.rs"]
mod tests;
