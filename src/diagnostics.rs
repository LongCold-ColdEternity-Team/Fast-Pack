//! Local panic reports for failures which are hard to reproduce on another PC.
use std::io::Write;
use std::sync::{
    Once,
    atomic::{AtomicU64, Ordering},
};

static INSTALLED: Once = Once::new();
static NEXT: AtomicU64 = AtomicU64::new(0);

pub fn install() {
    INSTALLED.call_once(|| {
        let Ok(directory) = crate::preferences::state_directory() else {
            return;
        };
        install_at(directory.join("logs"));
    });
}

fn install_at(directory: std::path::PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("unknown panic");
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_millis());
        let report = serde_json::json!({
            "version":env!("CARGO_PKG_VERSION"), "timestamp_ms":timestamp,
            "thread":std::thread::current().name(), "message":message,
            "location":info.location().map(|where_| format!("{}:{}:{}", where_.file(), where_.line(), where_.column())),
            "backtrace":std::backtrace::Backtrace::force_capture().to_string().chars().take(24000).collect::<String>()
        });
        // Logging must never replace the original failure with another panic.
        let _ = (|| -> anyhow::Result<()> {
            std::fs::create_dir_all(&directory)?;
            let mut file = tempfile::NamedTempFile::new_in(&directory)?;
            file.write_all(&serde_json::to_vec_pretty(&report)?)?;
            let name = format!(
                "panic-{timestamp}-{}-{}.json",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            file.persist_noclobber(directory.join(name))
                .map_err(|error| error.error)?;
            Ok(())
        })();
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    #[test]
    fn panic_fixture() {
        let Some(directory) = std::env::var_os("FAST_PACK_DIAGNOSTICS_TEST_DIR") else {
            return;
        };
        super::install_at(directory.into());
        let result: anyhow::Result<()> = fast_pack::tasks::run("diagnostic fixture", || {
            panic!("recoverable fixture failure")
        });
        assert!(result.is_err());
    }

    #[test]
    fn a_recovered_worker_panic_creates_a_local_report_without_exiting() {
        use std::os::windows::process::CommandExt;
        let work = tempfile::tempdir().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "diagnostics::tests::panic_fixture",
                "--nocapture",
            ])
            .env("FAST_PACK_DIAGNOSTICS_TEST_DIR", work.path())
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .output()
            .unwrap();
        assert!(result.status.success());
        let path = std::fs::read_dir(work.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(report["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(report["message"], "recoverable fixture failure");
        assert!(
            report["location"]
                .as_str()
                .unwrap()
                .contains("diagnostics.rs")
        );
        assert!(!report["backtrace"].as_str().unwrap().is_empty());
    }
}
