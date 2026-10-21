use super::*;

const ROLE: &str = "FAST_PACK_GUARD_TEST_ROLE";
const TRACE: &str = "FAST_PACK_GUARD_TEST_TRACE";
const NATIVE_FAILURE: i32 = 0xC0000005_u32 as i32;

#[test]
fn current_process_identity_can_be_checked_before_creating_a_runtime_lease() {
    assert!(
        process_creation(std::process::id())
            .flatten()
            .is_some_and(|creation| creation > 0)
    );
}

#[test]
fn normal_runtime_close_does_not_trigger_recovery() {
    let work = tempfile::tempdir().unwrap();
    let lease = RuntimeLease::create(work.path(), 10, 123).unwrap();
    lease.finish();
    assert!(!recover_unclean(work.path(), |_, _| false));
}

#[test]
fn runtime_failure_is_recorded_once_and_live_instances_are_preserved() {
    let work = tempfile::tempdir().unwrap();
    let alive = RuntimeLease::create(work.path(), 10, 123).unwrap();
    let failed = RuntimeLease::create(work.path(), 20, 456).unwrap();
    let failed_path = failed.0.clone();
    let _unacknowledged = failed; // No clean-close acknowledgement was made.
    assert!(recover_unclean(work.path(), |pid, stamp| pid == 10 && stamp == 123));
    assert!(!failed_path.exists());
    assert!(alive.0.exists());
    assert!(
        work.path()
            .join("logs/renderer-unclean-20-456.json")
            .is_file()
    );
    assert!(!recover_unclean(work.path(), |pid, stamp| pid == 10 && stamp == 123));
    alive.finish();
}

#[test]
fn child_fixture() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let backend = std::env::var("ICED_BACKEND").unwrap();
    if let Some(path) = std::env::var_os(TRACE) {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "{backend}").unwrap();
    }
    if role == "pre-render-failure" {
        eprintln!("fixture failure before renderer");
        std::process::exit(7);
    }
    entered();
    if role == "close-before-ready" {
        std::process::exit(0);
    }
    if backend == "wgpu" && matches!(role.as_str(), "native-gpu-failure" | "both-fail") {
        std::process::exit(NATIVE_FAILURE);
    }
    if backend == "wgpu" && role == "hung-gpu" {
        std::thread::sleep(Duration::from_secs(10));
    }
    if backend == "tiny-skia" && role == "both-fail" {
        eprintln!("software fixture failed");
        std::process::exit(8);
    }
    ready();
    std::thread::sleep(Duration::from_millis(40));
    if role == "failure-after-ready" {
        std::process::exit(NATIVE_FAILURE);
    }
    std::process::exit(0);
}

fn command(backend: &str, role: &str, trace: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "renderer_guard::tests::child_fixture",
            "--nocapture",
        ])
        .env("ICED_BACKEND", backend)
        .env(ROLE, role)
        .env(TRACE, trace);
    command
}

fn trace(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn finish(result: Launch) -> i32 {
    match result {
        Launch::Started(mut child) => child.wait().unwrap().code().unwrap_or(1),
        Launch::Finished(code) => code,
        Launch::DirectSoftware => panic!("expected a supervised UI"),
    }
}

#[test]
fn explicit_backend_legacy_and_remembered_software_respect_retry_policy() {
    assert_eq!(plan(false, false, false, true, false), Plan::Guard);
    assert_eq!(plan(false, false, false, true, true), Plan::Software);
    assert_eq!(plan(false, false, true, true, true), Plan::Guard);
    assert_eq!(plan(false, false, true, false, true), Plan::Software);
    assert_eq!(plan(true, false, true, true, true), Plan::Direct);
    assert_eq!(plan(true, true, false, true, false), Plan::Software);
}

#[test]
fn native_gpu_exit_retries_software_once_and_remembers_only_success() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("renderer.json");
    let log = temp.path().join("attempts.txt");
    let result = supervise(
        |backend| command(backend, "native-gpu-failure", &log),
        &cache,
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(finish(result), 0);
    assert_eq!(trace(&log), ["wgpu", "tiny-skia"]);
    assert!(saved_software(&cache));
}

#[test]
fn successful_gpu_retry_clears_remembered_compatibility_mode() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("renderer.json");
    remember(&cache, true).unwrap();
    let log = temp.path().join("attempts.txt");
    assert_eq!(
        finish(
            supervise(
                |backend| command(backend, "ready", &log),
                &cache,
                Duration::from_secs(5)
            )
            .unwrap()
        ),
        0
    );
    assert_eq!(trace(&log), ["wgpu"]);
    assert!(!saved_software(&cache));
}

#[test]
fn software_failure_does_not_create_a_retry_loop_or_remember_a_broken_backend() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("renderer.json");
    let log = temp.path().join("attempts.txt");
    let error = supervise(
        |backend| command(backend, "both-fail", &log),
        &cache,
        Duration::from_secs(5),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("software fixture failed"));
    assert_eq!(trace(&log), ["wgpu", "tiny-skia"]);
    assert!(!cache.exists());
}

#[test]
fn errors_before_rendering_and_normal_early_close_never_restart() {
    for role in ["pre-render-failure", "close-before-ready"] {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("renderer.json");
        let log = temp.path().join("attempts.txt");
        let result = supervise(
            |backend| command(backend, role, &log),
            &cache,
            Duration::from_secs(5),
        );
        if role == "pre-render-failure" {
            assert!(
                result
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("fixture failure before renderer")
            );
        } else {
            assert!(matches!(result.unwrap(), Launch::Finished(0)));
        }
        assert_eq!(trace(&log), ["wgpu"]);
        assert!(!cache.exists());
    }
}

#[test]
fn established_work_sessions_are_not_restarted_on_later_failures() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("renderer.json");
    let log = temp.path().join("attempts.txt");
    let result = supervise(
        |backend| command(backend, "failure-after-ready", &log),
        &cache,
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(finish(result), NATIVE_FAILURE);
    assert_eq!(trace(&log), ["wgpu"]);
    assert!(!cache.exists());
}

#[test]
fn hung_gpu_is_terminated_and_software_recovers() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("renderer.json");
    let log = temp.path().join("attempts.txt");
    let result = supervise(
        |backend| command(backend, "hung-gpu", &log),
        &cache,
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(finish(result), 0);
    assert_eq!(trace(&log), ["wgpu", "tiny-skia"]);
    assert!(saved_software(&cache));
}

#[test]
fn helper_launch_failure_can_use_the_current_process_in_software() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("renderer.json");
    let result = supervise(
        |_| Command::new(temp.path().join("does-not-exist.exe")),
        &cache,
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(matches!(result, Launch::DirectSoftware));
    assert!(!cache.exists());
}

#[test]
#[ignore = "requires the freshly built desktop executable"]
fn native_gpu_failure_recovers_a_real_software_ui() {
    let executable = std::env::var_os("FAST_PACK_REAL_RENDER_FIXTURE").expect("desktop executable");
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("renderer.json");
    let log = temp.path().join("attempts.txt");
    let capture = temp.path().join("software.png");
    let result = supervise(
        |backend| {
            if backend == "wgpu" {
                command(backend, "native-gpu-failure", &log)
            } else {
                let mut c = Command::new(&executable);
                c.env("ICED_BACKEND", backend)
                    .env("FAST_PACK_CAPTURE", &capture)
                    .env("FAST_PACK_CAPTURE_SIZE", "1040x780");
                c
            }
        },
        &cache,
        Duration::from_secs(20),
    )
    .unwrap();
    assert_eq!(finish(result), 0);
    assert!(saved_software(&cache));
    let image = image::open(capture).unwrap();
    assert_eq!((image.width(), image.height()), (1040, 780));
}
