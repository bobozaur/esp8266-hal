use std::{
    env,
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{anyhow, bail, Context, Result};

pub const DEFAULT_DUT_PORT: &str = "/dev/ttyUSB0";
pub const DEFAULT_COMPANION_PORT: &str = "/dev/ttyACM0";
const ESP_IDF_REPO: &str = "https://github.com/espressif/esp-idf.git";
const ESP_IDF_REF: &str = "v5.2.3";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionPaths {
    pub tool_root: PathBuf,
    pub idf_checkout: PathBuf,
    pub idf_tools: PathBuf,
    pub home: PathBuf,
    pub cache: PathBuf,
    pub pip_cache: PathBuf,
    pub python_env: PathBuf,
    pub tmp: PathBuf,
    pub build_dir: PathBuf,
    pub project_dir: PathBuf,
}

impl CompanionPaths {
    pub fn idf_environment(&self) -> Vec<(String, String)> {
        vec![
            (
                "IDF_PATH".to_owned(),
                self.idf_checkout.to_string_lossy().into_owned(),
            ),
            (
                "IDF_TOOLS_PATH".to_owned(),
                self.idf_tools.to_string_lossy().into_owned(),
            ),
            ("HOME".to_owned(), self.home.to_string_lossy().into_owned()),
            (
                "XDG_CACHE_HOME".to_owned(),
                self.cache.to_string_lossy().into_owned(),
            ),
            (
                "PIP_CACHE_DIR".to_owned(),
                self.pip_cache.to_string_lossy().into_owned(),
            ),
            (
                "IDF_PYTHON_ENV_PATH".to_owned(),
                self.python_env.to_string_lossy().into_owned(),
            ),
            ("TMPDIR".to_owned(), self.tmp.to_string_lossy().into_owned()),
            (
                "PYTHONPYCACHEPREFIX".to_owned(),
                self.cache.join("pycache").to_string_lossy().into_owned(),
            ),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub port: PathBuf,
    pub chip_kind: Option<String>,
    pub mac: Option<String>,
}

pub fn repo_local_paths(repo: &Path) -> CompanionPaths {
    let tool_root = repo.join(".tools").join("esp32");
    CompanionPaths {
        idf_checkout: tool_root.join("esp-idf"),
        idf_tools: tool_root.join("idf-tools"),
        home: tool_root.join("home"),
        cache: tool_root.join("cache"),
        pip_cache: tool_root.join("pip-cache"),
        python_env: tool_root.join("python-env"),
        tmp: tool_root.join("tmp"),
        build_dir: repo.join("target").join("esp32-companion"),
        project_dir: repo.join("companion").join("esp32-pico-d4"),
        tool_root,
    }
}

pub fn chip_kind_from_esptool_output(output: &str) -> Option<&'static str> {
    for line in output.lines() {
        let trimmed = line.trim();
        let Some(value) = trimmed.strip_prefix("Chip type:") else {
            continue;
        };
        let value = value.trim();
        for known in [
            "ESP32-PICO-D4",
            "ESP32-S2",
            "ESP32-S3",
            "ESP32-C3",
            "ESP32-C6",
            "ESP32-H2",
            "ESP32",
            "ESP8266EX",
        ] {
            if value.starts_with(known) {
                return Some(known);
            }
        }
        return Some("unknown");
    }

    None
}

pub fn run_cli(args: Vec<String>) -> Result<()> {
    let (command, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("usage: wifi-hw companion <doctor|setup|build|flash|command>"))?;

    match command.as_str() {
        "doctor" => {
            let options = parse_common_options(rest, true)?;
            doctor(&options)
        }
        "setup" => {
            let options = parse_common_options(rest, true)?;
            setup(&options)
        }
        "build" => {
            let options = parse_common_options(rest, true)?;
            build(&options)
        }
        "flash" => {
            let options = parse_common_options(rest, true)?;
            flash(&options)
        }
        "command" => {
            let options = parse_common_options(rest, false)?;
            send_command_cli(&options)
        }
        "--help" | "-h" | "help" => {
            print_help();
            Ok(())
        }
        other => bail!("unknown wifi-hw companion command `{other}`"),
    }
}

#[derive(Debug, Clone)]
struct CompanionCliOptions {
    repo: PathBuf,
    paths: CompanionPaths,
    dut_port: PathBuf,
    companion_port: PathBuf,
    serial_port: PathBuf,
    command: Option<String>,
}

fn parse_common_options(args: &[String], allow_empty_command: bool) -> Result<CompanionCliOptions> {
    let repo = repo_root()?;
    let paths = repo_local_paths(&repo);
    let mut dut_port = PathBuf::from(DEFAULT_DUT_PORT);
    let mut companion_port = PathBuf::from(DEFAULT_COMPANION_PORT);
    let mut serial_port = PathBuf::from(DEFAULT_COMPANION_PORT);
    let mut command_words = Vec::new();

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--dut-port" => {
                dut_port = iter
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow!("--dut-port requires a path"))?;
            }
            "--companion-port" => {
                let value = iter
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow!("--companion-port requires a path"))?;
                serial_port = value.clone();
                companion_port = value;
            }
            "--serial-port" => {
                serial_port = iter
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow!("--serial-port requires a path"))?;
            }
            value if value.starts_with('-') => bail!("unknown option `{value}`"),
            value => command_words.push(value.to_owned()),
        }
    }

    if !allow_empty_command && command_words.is_empty() {
        bail!("usage: wifi-hw companion command [--serial-port PATH] <LINE>");
    }

    Ok(CompanionCliOptions {
        repo,
        paths,
        dut_port,
        companion_port,
        serial_port,
        command: (!command_words.is_empty()).then(|| command_words.join(" ")),
    })
}

fn print_help() {
    println!("usage: wifi-hw companion <doctor|setup|build|flash|command>");
    println!("  doctor [--dut-port PATH] [--companion-port PATH]");
    println!("  setup");
    println!("  build");
    println!("  flash [--serial-port PATH]");
    println!("  command [--serial-port PATH] <LINE>");
}

fn doctor(options: &CompanionCliOptions) -> Result<()> {
    ensure_base_host_tools()?;

    let dut = probe_device(&options.dut_port)
        .with_context(|| format!("failed to probe DUT on {}", options.dut_port.display()))?;
    let companion = probe_device(&options.companion_port).with_context(|| {
        format!(
            "failed to probe companion on {}",
            options.companion_port.display()
        )
    })?;

    println!("wifi-hw companion doctor: ok");
    println!("repo={}", options.repo.display());
    println!("esp-idf-ref={ESP_IDF_REF}");
    println!("dut-port={}", dut.port.display());
    println!("dut-chip={}", dut.chip_kind.as_deref().unwrap_or("unknown"));
    println!("dut-mac={}", dut.mac.as_deref().unwrap_or("unknown"));
    println!("companion-port={}", companion.port.display());
    println!(
        "companion-chip={}",
        companion.chip_kind.as_deref().unwrap_or("unknown")
    );
    println!(
        "companion-mac={}",
        companion.mac.as_deref().unwrap_or("unknown")
    );
    println!("idf-checkout={}", options.paths.idf_checkout.display());
    println!("idf-tools={}", options.paths.idf_tools.display());
    Ok(())
}

fn setup(options: &CompanionCliOptions) -> Result<()> {
    ensure_base_host_tools()?;
    create_repo_local_directories(&options.paths)?;

    if !options.paths.idf_checkout.exists() {
        run_logged(
            options.paths.tool_root.join("clone.log"),
            &options.repo,
            &[],
            "git",
            [
                OsString::from("clone"),
                OsString::from("--recursive"),
                OsString::from("--branch"),
                OsString::from(ESP_IDF_REF),
                OsString::from(ESP_IDF_REPO),
                options.paths.idf_checkout.as_os_str().to_owned(),
            ],
        )
        .context("failed to clone ESP-IDF repo-locally")?;
    } else {
        run_logged(
            options.paths.tool_root.join("submodule-update.log"),
            &options.paths.idf_checkout,
            &[],
            "git",
            [
                OsString::from("submodule"),
                OsString::from("update"),
                OsString::from("--init"),
                OsString::from("--recursive"),
            ],
        )
        .context("failed to update ESP-IDF submodules")?;
    }

    ensure_repo_local_python_env(options)?;

    let install_log = options.paths.tool_root.join("install.log");
    let command = format!(
        "set -euo pipefail; cd \"{}\"; bash ./install.sh esp32",
        options.paths.idf_checkout.display()
    );
    run_bash_logged(
        &install_log,
        &options.repo,
        &options.paths.idf_environment(),
        &command,
    )
    .context("failed to install repo-local ESP-IDF tools")?;

    println!("wifi-hw companion setup: ok");
    println!("idf-checkout={}", options.paths.idf_checkout.display());
    println!("idf-tools={}", options.paths.idf_tools.display());
    Ok(())
}

fn build(options: &CompanionCliOptions) -> Result<()> {
    ensure_base_host_tools()?;
    ensure_setup_complete(&options.paths)?;
    create_repo_local_directories(&options.paths)?;
    if !options.paths.project_dir.exists() {
        bail!(
            "companion project missing at {}",
            options.paths.project_dir.display()
        );
    }

    let build_log = options.paths.build_dir.join("build.log");
    let command = format!(
        "set -euo pipefail; \
         export IDF_TARGET=esp32; \
         source \"{idf}/export.sh\" >/dev/null; \
         idf.py -C \"{project}\" -B \"{build}\" build",
        idf = options.paths.idf_checkout.display(),
        project = options.paths.project_dir.display(),
        build = options.paths.build_dir.display(),
    );
    run_bash_logged(
        &build_log,
        &options.repo,
        &options.paths.idf_environment(),
        &command,
    )
    .context("failed to build ESP32 companion firmware")?;

    println!("wifi-hw companion build: ok");
    println!("build-dir={}", options.paths.build_dir.display());
    Ok(())
}

fn flash(options: &CompanionCliOptions) -> Result<()> {
    ensure_base_host_tools()?;
    ensure_setup_complete(&options.paths)?;
    ensure_built(&options.paths)?;

    let flash_log = options.paths.build_dir.join("flash.log");
    let command = format!(
        "set -euo pipefail; \
         export IDF_TARGET=esp32; \
         source \"{idf}/export.sh\" >/dev/null; \
         idf.py -C \"{project}\" -B \"{build}\" -p \"{port}\" flash",
        idf = options.paths.idf_checkout.display(),
        project = options.paths.project_dir.display(),
        build = options.paths.build_dir.display(),
        port = options.serial_port.display(),
    );
    run_bash_logged(
        &flash_log,
        &options.repo,
        &options.paths.idf_environment(),
        &command,
    )
    .context("failed to flash ESP32 companion firmware")?;

    println!("wifi-hw companion flash: ok");
    println!("serial-port={}", options.serial_port.display());
    Ok(())
}

fn send_command_cli(options: &CompanionCliOptions) -> Result<()> {
    let command = options
        .command
        .as_deref()
        .ok_or_else(|| anyhow!("missing command line"))?;
    let response = send_command(
        &options.serial_port,
        command,
        std::time::Duration::from_secs(10),
    )
    .with_context(|| {
        format!(
            "failed to send `{command}` to {}",
            options.serial_port.display()
        )
    })?;

    print!("{response}");
    if !response.ends_with('\n') {
        println!();
    }
    Ok(())
}

fn ensure_base_host_tools() -> Result<()> {
    for program in ["bash", "git", "python3"] {
        command_available(program)?;
    }
    let _ = esptool_program()?;
    Ok(())
}

fn create_repo_local_directories(paths: &CompanionPaths) -> Result<()> {
    for path in [
        &paths.tool_root,
        &paths.idf_tools,
        &paths.home,
        &paths.cache,
        &paths.pip_cache,
        &paths.python_env,
        &paths.tmp,
        &paths.build_dir,
    ] {
        fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    }
    Ok(())
}

pub fn probe_device(port: &Path) -> Result<DeviceInfo> {
    let output = Command::new(esptool_program()?)
        .arg("--port")
        .arg(port)
        .arg("chip-id")
        .output()
        .with_context(|| format!("failed to invoke esptool for {}", port.display()))?;

    if !output.status.success() {
        bail!(
            "esptool chip-id failed for {}:\n{}{}",
            port.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(DeviceInfo {
        port: port.to_path_buf(),
        chip_kind: chip_kind_from_esptool_output(&combined).map(str::to_owned),
        mac: combined
            .lines()
            .find_map(|line| line.trim().strip_prefix("MAC:").map(str::trim))
            .map(str::to_owned),
    })
}

fn ensure_setup_complete(paths: &CompanionPaths) -> Result<()> {
    if !paths.idf_checkout.join("export.sh").exists() {
        bail!(
            "ESP-IDF checkout is missing at {}; run `cargo run -p xtask -- wifi-hw companion setup` first",
            paths.idf_checkout.display()
        );
    }
    Ok(())
}

fn ensure_built(paths: &CompanionPaths) -> Result<()> {
    if !paths.build_dir.join("flasher_args.json").exists() {
        bail!(
            "companion firmware is not built under {}; run `cargo run -p xtask -- wifi-hw companion build` first",
            paths.build_dir.display()
        );
    }
    Ok(())
}

fn ensure_repo_local_python_env(options: &CompanionCliOptions) -> Result<()> {
    let virtualenv_python = python_env_python(&options.paths);
    if python_env_has_pip(&virtualenv_python) {
        return Ok(());
    }
    if options.paths.python_env.exists() {
        fs::remove_dir_all(&options.paths.python_env).with_context(|| {
            format!(
                "failed to remove broken Python environment {}",
                options.paths.python_env.display()
            )
        })?;
    }

    let bootstrap_dir = options.paths.tool_root.join("virtualenv-bootstrap");
    fs::create_dir_all(&bootstrap_dir)
        .with_context(|| format!("failed to create {}", bootstrap_dir.display()))?;
    let mut bootstrap_env = options.paths.idf_environment();
    bootstrap_env.push((
        "VIRTUALENV_OVERRIDE_APP_DATA".to_owned(),
        options
            .paths
            .cache
            .join("virtualenv")
            .to_string_lossy()
            .into_owned(),
    ));

    run_logged(
        options.paths.tool_root.join("bootstrap-virtualenv-pip.log"),
        &options.repo,
        &bootstrap_env,
        "python3",
        [
            OsString::from("-m"),
            OsString::from("pip"),
            OsString::from("install"),
            OsString::from("--upgrade"),
            OsString::from("--target"),
            bootstrap_dir.as_os_str().to_owned(),
            OsString::from("virtualenv"),
        ],
    )
    .context("failed to bootstrap repo-local virtualenv support")?;

    run_logged(
        options
            .paths
            .tool_root
            .join("bootstrap-virtualenv-create.log"),
        &options.repo,
        &{
            let mut env = bootstrap_env.clone();
            env.push((
                "PYTHONPATH".to_owned(),
                bootstrap_dir.to_string_lossy().into_owned(),
            ));
            env
        },
        "python3",
        [
            OsString::from("-m"),
            OsString::from("virtualenv"),
            OsString::from("--clear"),
            OsString::from("--python"),
            OsString::from("python3"),
            options.paths.python_env.as_os_str().to_owned(),
        ],
    )
    .context("failed to create repo-local Python environment for ESP-IDF")?;

    if !python_env_has_pip(&virtualenv_python) {
        bail!(
            "repo-local Python environment at {} is missing pip after bootstrap",
            options.paths.python_env.display()
        );
    }

    Ok(())
}

fn command_available(program: &str) -> Result<()> {
    let status = Command::new("bash")
        .arg("-lc")
        .arg(format!("command -v {program}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("failed to run shell while checking `{program}`"))?;

    if status.success() {
        Ok(())
    } else {
        bail!("required command `{program}` is not available")
    }
}

fn esptool_program() -> Result<&'static str> {
    if command_available("esptool").is_ok() {
        return Ok("esptool");
    }
    if command_available("esptool.py").is_ok() {
        return Ok("esptool.py");
    }
    bail!("required command `esptool` or `esptool.py` is not available")
}

fn repo_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().map(Path::to_path_buf).ok_or_else(|| {
        anyhow!(
            "failed to resolve repo root from {}",
            manifest_dir.display()
        )
    })
}

fn python_env_python(paths: &CompanionPaths) -> PathBuf {
    paths.python_env.join("bin").join("python")
}

fn python_env_has_pip(python: &Path) -> bool {
    if !python.exists() {
        return false;
    }

    Command::new(python)
        .args(["-m", "pip", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn run_logged<I, S>(
    log_path: PathBuf,
    cwd: &Path,
    env_overrides: &[(String, String)],
    program: &str,
    args: I,
) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut log = fs::File::create(&log_path)
        .with_context(|| format!("failed to create {}", log_path.display()))?;

    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    writeln!(
        log,
        "$ {} {}",
        program,
        args.iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    )?;

    let output = Command::new(program)
        .args(&args)
        .current_dir(cwd)
        .envs(env_overrides.iter().cloned())
        .output()
        .with_context(|| format!("failed to run `{program}`"))?;
    log.write_all(&output.stdout)?;
    log.write_all(&output.stderr)?;

    if output.status.success() {
        Ok(())
    } else {
        bail!("{program} failed; see {}", log_path.display())
    }
}

fn run_bash_logged(
    log_path: &Path,
    cwd: &Path,
    env_overrides: &[(String, String)],
    command: &str,
) -> Result<()> {
    run_logged(
        log_path.to_path_buf(),
        cwd,
        env_overrides,
        "bash",
        [OsString::from("-lc"), OsString::from(command)],
    )
}

pub struct CommandSession {
    port: Box<dyn serialport::SerialPort>,
}

impl CommandSession {
    pub fn open(port: &Path) -> Result<Self> {
        let mut serial = serialport::new(port.to_string_lossy(), 115_200)
            .timeout(std::time::Duration::from_millis(250))
            .open()
            .with_context(|| format!("failed to open serial port {}", port.display()))?;
        serial
            .write_data_terminal_ready(false)
            .context("failed to clear companion DTR")?;
        serial
            .write_request_to_send(false)
            .context("failed to clear companion RTS")?;
        drain_serial_noise(&mut *serial, std::time::Duration::from_millis(300));
        Ok(Self { port: serial })
    }

    pub fn send_command(&mut self, line: &str, timeout: std::time::Duration) -> Result<String> {
        drain_serial_noise(&mut *self.port, std::time::Duration::from_millis(20));
        self.port
            .write_all(line.as_bytes())
            .context("failed to write command bytes")?;
        self.port
            .write_all(b"\n")
            .context("failed to write command newline")?;
        self.port.flush().context("failed to flush serial port")?;

        let start = std::time::Instant::now();
        let mut response = Vec::new();
        let mut chunk = [0u8; 256];
        while start.elapsed() < timeout {
            match self.port.read(&mut chunk) {
                Ok(len) if len > 0 => {
                    response.extend_from_slice(&chunk[..len]);
                    if let Some(line) = extract_terminal_line(&response) {
                        return Ok(line);
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) => return Err(error).context("failed to read serial response"),
            }
        }

        let received = String::from_utf8_lossy(&response);
        bail!(
            "timed out waiting for `{line}` response after {:.1}s; received: {}",
            timeout.as_secs_f32(),
            received.trim()
        )
    }
}

pub fn send_command(port: &Path, line: &str, timeout: std::time::Duration) -> Result<String> {
    let mut session = CommandSession::open(port)?;
    session.send_command(line, timeout)
}

fn drain_serial_noise(port: &mut dyn serialport::SerialPort, duration: std::time::Duration) {
    let deadline = std::time::Instant::now() + duration;
    let mut chunk = [0u8; 256];
    while std::time::Instant::now() < deadline {
        match port.read(&mut chunk) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
}

fn extract_terminal_line(buffer: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(buffer);
    let mut last_ready = None;
    let mut last_terminal = None;

    for line in text.lines() {
        let trimmed = line.trim_matches(|ch: char| ch == '\0' || ch.is_control());
        if let Some(ready_index) = trimmed.find("READY ") {
            last_ready = Some(trimmed[ready_index..].to_owned());
        }
        if let Some(ok_index) = trimmed.find("OK ") {
            last_terminal = Some(trimmed[ok_index..].to_owned());
            continue;
        }
        if let Some(err_index) = trimmed.find("ERR ") {
            last_terminal = Some(trimmed[err_index..].to_owned());
        }
    }

    match (last_ready, last_terminal) {
        (Some(ready), Some(terminal)) => Some(format!("{ready}\n{terminal}")),
        (None, Some(terminal)) => Some(terminal),
        (Some(_), None) => None,
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::extract_terminal_line;

    #[test]
    fn terminal_line_parser_prefers_clean_ok_line() {
        let buffer =
            b"x\x00y\xffREADY companion esp32-pico-d4 mac=4c:c3:82:82:32:80\njunk\nOK PONG\n";
        assert_eq!(
            extract_terminal_line(buffer).as_deref(),
            Some("READY companion esp32-pico-d4 mac=4c:c3:82:82:32:80\nOK PONG")
        );
    }

    #[test]
    fn terminal_line_parser_handles_err_without_ready() {
        let buffer = b"\x00\x01ERR code=sta-timeout detail=15000ms\n";
        assert_eq!(
            extract_terminal_line(buffer).as_deref(),
            Some("ERR code=sta-timeout detail=15000ms")
        );
    }

    #[test]
    fn terminal_line_parser_does_not_finish_on_ready_alone() {
        let buffer = b"READY companion esp32-pico-d4 mac=4c:c3:82:82:32:80\n";
        assert_eq!(extract_terminal_line(buffer).as_deref(), None);
    }
}
