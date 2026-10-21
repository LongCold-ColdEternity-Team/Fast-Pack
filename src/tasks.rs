//! Convert background panics into completed, retryable task failures.
use anyhow::{Result, anyhow};

pub fn run<T>(name: &str, action: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).map_err(|payload| {
        let detail = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("未知内部异常");
        anyhow!("{name}意外停止：{detail}。可以重试此任务。")
    })?
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_failed_overlay_completes_and_a_later_export_can_succeed() {
        let work = tempfile::tempdir().unwrap();
        let output = work.path().join("overlay.zip");
        let options = crate::overlay::Options::default();
        let error = crate::overlay::build(&output, &options, |_, progress| {
            if progress == 92 {
                panic!("injected export failure");
            }
        })
        .unwrap_err();
        assert!(error.to_string().contains("injected export failure"));
        assert!(
            !output.exists(),
            "a failed export must not leave a partial pack"
        );
        crate::overlay::build(&output, &options, |_, _| {}).unwrap();
        assert!(output.is_file());
    }
}
