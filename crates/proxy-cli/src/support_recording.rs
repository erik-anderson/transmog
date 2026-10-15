//! Aggregate collection health and console input owned only during recording.
use std::{
    io::{self, IsTerminal},
    sync::Mutex,
    time::Duration,
};
use transmog_capture::support::SupportProfile;
use transmog_core::observe::{
    BoxObserverFuture, ExchangeBoundary, ObservationInterest, Observer, ObserverConfig,
    ObserverDeliveryPolicy, ObserverEvent, ObserverEventKind,
};

#[derive(Default, Clone)]
struct Counts {
    exchanges: u64,
    http: u64,
    relevant: u64,
    decrypted: u64,
    tunnels: u64,
    failed: u64,
    stream_records: u64,
}
pub(crate) struct Health {
    profile: Option<SupportProfile>,
    counts: Mutex<Counts>,
    began: std::time::Instant,
}
impl Health {
    pub(crate) fn new(profile: Option<SupportProfile>) -> Self {
        Self {
            profile,
            counts: Mutex::default(),
            began: std::time::Instant::now(),
        }
    }
    pub(crate) fn print_health(&self, elapsed: Duration) {
        let counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        eprintln!(
            "Capture health: {}s elapsed · {} exchanges · {} HTTP request observations · {} relevant · {} decrypted HTTPS · {} CONNECT tunnels · {} failures",
            elapsed.as_secs(),
            counts.exchanges,
            counts.http,
            counts.relevant,
            counts.decrypted,
            counts.tunnels,
            counts.failed
        );
        if self
            .profile
            .as_ref()
            .is_some_and(|p| !p.relevant_hosts.is_empty())
            && counts.relevant == 0
        {
            eprintln!(
                "No relevant HTTP traffic yet. Reproduce the issue and check the application's proxy configuration; this does not establish that the website is unreachable."
            );
        } else if counts.http == 0 {
            eprintln!(
                "No inspectable HTTP requests yet. CONNECT tunnels alone do not establish useful decrypted website evidence."
            );
        }
    }
    pub(crate) fn print_summary(&self, evicted: u64) {
        self.print_health(self.began.elapsed());
        let counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        println!(
            "Evidence summary: {} observed stream records · {evicted} circular exchanges evicted. HTTP/stream payload availability, body retention caps and any observer gaps are recorded in the trace; health metadata alone does not establish payload completeness. A sealed file does not establish that the issue was captured.",
            counts.stream_records
        );
    }
}
impl Observer for Health {
    fn on_event(&self, event: ObserverEvent) -> BoxObserverFuture<'_> {
        Box::pin(async move {
            let mut c = self
                .counts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match event.kind {
                ObserverEventKind::ExchangeStarted { .. } => {
                    c.exchanges = c.exchanges.saturating_add(1);
                }
                ObserverEventKind::RequestHeadObserved {
                    boundary: ExchangeBoundary::ClientRequest,
                    head,
                } => {
                    if head.method == "CONNECT" {
                        c.tunnels = c.tunnels.saturating_add(1);
                    } else {
                        c.http = c.http.saturating_add(1);
                        if self.profile.as_ref().is_none_or(|p| {
                            p.relevant_hosts.is_empty() || p.relevant(&head.target.host)
                        }) {
                            c.relevant = c.relevant.saturating_add(1);
                        }
                        if head.target.scheme == "https" {
                            c.decrypted = c.decrypted.saturating_add(1);
                        }
                    }
                }
                ObserverEventKind::StreamMessage(_) => {
                    c.stream_records = c.stream_records.saturating_add(1);
                }
                ObserverEventKind::Failed(_) => c.failed = c.failed.saturating_add(1),
                _ => {}
            }
            Ok(())
        })
    }
}
pub(crate) fn health_config() -> ObserverConfig {
    ObserverConfig {
        interest: ObservationInterest::default(),
        queue_capacity: std::num::NonZeroUsize::new(2048).unwrap_or(std::num::NonZeroUsize::MIN),
        delivery: ObserverDeliveryPolicy::WaitForCapacity,
        callback_timeout: None,
    }
}
pub(crate) fn marker_note(line: &str) -> Option<String> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line == "m" {
        Some(String::new())
    } else {
        line.strip_prefix("m ").map(str::to_owned)
    }
}

/// On Windows, one inbox PowerShell helper owns Console.ReadKey only after setup.
/// Killing and joining it before drain returns input ownership before any later prompt.
/// No input thread can remain blocked on stdin or consume an encryption/cleanup answer.
pub(crate) struct MarkerInput {
    active: bool,
    #[cfg(unix)]
    terminal: Option<std::fs::File>,
    #[cfg(unix)]
    line: Vec<u8>,
    #[cfg(unix)]
    oversized: bool,
    #[cfg(windows)]
    child: Option<std::process::Child>,
    #[cfg(windows)]
    reader: Option<std::thread::JoinHandle<()>>,
    #[cfg(windows)]
    receiver: Option<std::sync::mpsc::Receiver<String>>,
}
impl MarkerInput {
    pub(crate) fn start() -> Self {
        let mut input = Self {
            active: io::stdin().is_terminal(),
            #[cfg(unix)]
            terminal: None,
            #[cfg(unix)]
            line: Vec::new(),
            #[cfg(unix)]
            oversized: false,
            #[cfg(windows)]
            child: None,
            #[cfg(windows)]
            reader: None,
            #[cfg(windows)]
            receiver: None,
        };
        if !input.active {
            println!(
                "Incident marking disabled for redirected/noninteractive input. Ctrl+C still stops recording."
            );
            return input;
        }
        #[cfg(windows)]
        if let Err(error) = input.start_windows() {
            input.stop();
            eprintln!(
                "Incident console input is unavailable: {error}. Ctrl+C still stops recording."
            );
            return input;
        }
        #[cfg(unix)]
        if let Err(error) = input.start_unix() {
            input.stop();
            eprintln!(
                "Incident console input is unavailable: {error}. Ctrl+C still stops recording."
            );
            return input;
        }
        println!(
            "Issue happened here: type m then Enter, or m followed by a short note. Notes are saved in the trace."
        );
        input
    }
    #[cfg(windows)]
    fn start_windows(&mut self) -> io::Result<()> {
        use std::io::{BufRead, BufReader, Read};
        use std::process::{Command, Stdio};
        let executable = std::path::PathBuf::from(
            std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()),
        )
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut child = Command::new(executable)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                CONSOLE_INPUT,
            ])
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Console input pipe unavailable"))?;
        let (sender, receiver) = std::sync::mpsc::sync_channel(4);
        let reader = std::thread::Builder::new().name("transmog-incident-input".into()).spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.by_ref().take(16385).read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if line.len() > 16384 {
                            eprintln!("Incident input exceeded its bound; no marker recorded.");
                            break;
                        }
                        if let Ok(value) = serde_json::from_str::<String>(&line)
                            && sender.try_send(value).is_err() {
                            eprintln!("Incident input queue full or closed; that marker was not recorded.");
                        }
                    }
                }
            }
        });
        match reader {
            Ok(reader) => {
                self.child = Some(child);
                self.reader = Some(reader);
                self.receiver = Some(receiver);
                Ok(())
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(e)
            }
        }
    }
    pub(crate) fn poll(&mut self) -> Option<String> {
        if !self.active {
            return None;
        }
        #[cfg(windows)]
        {
            match self
                .receiver
                .as_ref()
                .map(std::sync::mpsc::Receiver::try_recv)
            {
                Some(Ok(value)) => return Some(value),
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                    self.stop();
                    eprintln!("Incident input closed. Ctrl+C still stops recording.");
                }
                _ => {}
            }
            None
        }
        #[cfg(unix)]
        {
            use std::io::Read;
            // A separate open description keeps O_NONBLOCK off setup/cleanup stdin.
            // Bound each turn, including unterminated or oversized terminal input.
            for _ in 0..4097 {
                let mut byte = [0];
                match self.terminal.as_mut()?.read(&mut byte) {
                    Ok(0) => {
                        self.stop();
                        eprintln!("Incident input closed. Ctrl+C still stops recording.");
                        return None;
                    }
                    Ok(_) if byte[0] == b'\n' => {
                        if self.oversized {
                            self.line.clear();
                            self.oversized = false;
                            eprintln!("Incident input exceeded its bound; no marker recorded.");
                            return None;
                        }
                        return Some(
                            String::from_utf8_lossy(&std::mem::take(&mut self.line)).into_owned(),
                        );
                    }
                    Ok(_) => {
                        if self.line.len() < 4096 {
                            self.line.push(byte[0]);
                        } else {
                            self.oversized = true;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => return None,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => return None,
                    Err(error) => {
                        self.stop();
                        eprintln!("Incident input closed: {error}. Ctrl+C still stops recording.");
                        return None;
                    }
                }
            }
            None
        }
        #[cfg(not(any(windows, unix)))]
        {
            None
        }
    }
    #[cfg(unix)]
    fn start_unix(&mut self) -> io::Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let flags = i32::try_from(rustix::fs::OFlags::NONBLOCK.bits()).map_err(io::Error::other)?;
        self.terminal = Some(
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open("/dev/tty")?,
        );
        Ok(())
    }
    pub(crate) fn stop(&mut self) {
        self.active = false;
        #[cfg(unix)]
        {
            self.terminal.take();
            self.line.clear();
        }
        #[cfg(windows)]
        {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            self.receiver.take();
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }
}
impl Drop for MarkerInput {
    fn drop(&mut self) {
        self.stop();
    }
}
#[cfg(windows)]
const CONSOLE_INPUT: &str = r#"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$line = New-Object System.Text.StringBuilder
while ($true) {
 if (-not [Console]::KeyAvailable) { Start-Sleep -Milliseconds 40; continue }
 $key = [Console]::ReadKey($true)
 if ($key.Key -eq 'Enter') { [Console]::Out.WriteLine((ConvertTo-Json -InputObject $line.ToString() -Compress)); [Console]::Error.WriteLine(); $line.Clear() | Out-Null }
 elseif ($key.Key -eq 'Backspace') { if ($line.Length -gt 0) { $line.Length--; [Console]::Error.Write("`b `b") } }
 elseif (-not [char]::IsControl($key.KeyChar)) { if ($line.Length -lt 2048) { $line.Append($key.KeyChar) | Out-Null; [Console]::Error.Write($key.KeyChar) } else { [Console]::Error.Write([char]7) } }
}
"#;
