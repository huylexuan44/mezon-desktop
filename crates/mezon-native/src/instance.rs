use std::path::PathBuf;

/// Ensures only one instance of the app runs at a time.
///
/// Unix (macOS / Linux): Unix domain socket at `$XDG_RUNTIME_DIR/mezon.sock`.
/// Windows            : Named pipe `\\.\pipe\mezon-single-instance`.
///
/// A second instance can forward a URL to the first by writing it to the
/// socket / pipe before exiting (used for deep link handling).
pub struct SingleInstance {
    #[cfg(unix)]
    socket_path: Option<PathBuf>,
    #[cfg(unix)]
    _listener: Option<std::os::unix::net::UnixListener>,

    #[cfg(windows)]
    _pipe_name: String,
    #[cfg(windows)]
    url_rx: std::sync::Mutex<Option<std::sync::mpsc::Receiver<String>>>,
}

impl SingleInstance {
    /// Try to acquire the single-instance lock.
    ///
    /// Returns `Ok(Some(instance))` — this is the first instance.
    /// Returns `Ok(None)` — another instance is already running.
    pub fn try_acquire() -> anyhow::Result<Option<Self>> {
        #[cfg(unix)]
        return Self::try_acquire_unix(None);

        #[cfg(windows)]
        return Self::try_acquire_windows(None);

        #[cfg(not(any(unix, windows)))]
        Ok(Some(Self {}))
    }

    /// Same as `try_acquire`, but additionally forwards `url` to the running
    /// first instance if one exists.
    pub fn try_acquire_or_forward(url: &str) -> anyhow::Result<Option<Self>> {
        #[cfg(unix)]
        return Self::try_acquire_unix(Some(url));

        #[cfg(windows)]
        return Self::try_acquire_windows(Some(url));

        #[cfg(not(any(unix, windows)))]
        {
            let _ = url;
            Ok(Some(Self {}))
        }
    }

    // ── Unix ──────────────────────────────────────────────────────────────────

    #[cfg(unix)]
    fn try_acquire_unix(forward_url: Option<&str>) -> anyhow::Result<Option<Self>> {
        use std::io::Write as _;

        let mut last_bind_error = None;
        for socket_path in Self::socket_paths() {
            if let Some(parent) = socket_path.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                last_bind_error = Some(e);
                continue;
            }

            if socket_path.exists() {
                match std::os::unix::net::UnixStream::connect(&socket_path) {
                    Ok(mut stream) => {
                        tracing::info!("Another instance is already running");
                        if let Some(url) = forward_url {
                            let _ = stream.write_all(url.as_bytes());
                            let safe = url.split(['?', '#']).next().unwrap_or_default();
                            tracing::debug!("Forwarded URL to running instance: {safe}");
                        }
                        return Ok(None);
                    }
                    Err(_) => {
                        // Stale socket — clean up
                        let _ = std::fs::remove_file(&socket_path);
                    }
                }
            }

            match std::os::unix::net::UnixListener::bind(&socket_path) {
                Ok(listener) => {
                    tracing::debug!("Single instance lock acquired at {}", socket_path.display());
                    return Ok(Some(Self {
                        socket_path: Some(socket_path),
                        _listener: Some(listener),
                    }));
                }
                Err(e) => {
                    last_bind_error = Some(e);
                }
            }
        }

        let error = last_bind_error
            .unwrap_or_else(|| std::io::Error::other("no Unix socket path candidates available"));

        if error.kind() == std::io::ErrorKind::PermissionDenied {
            tracing::warn!(
                "Single instance lock disabled because Unix socket binding is not permitted: {error}"
            );
            return Ok(Some(Self {
                socket_path: None,
                _listener: None,
            }));
        }

        Err(error.into())
    }

    #[cfg(unix)]
    fn socket_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();

        if let Some(runtime_dir) = dirs::runtime_dir() {
            paths.push(runtime_dir.join("mezon.sock"));
        }

        let user = std::env::var("USER")
            .ok()
            .filter(|user| !user.is_empty())
            .unwrap_or_else(|| "user".to_owned());

        paths.push(
            std::env::temp_dir()
                .join(format!("mezon-desktop-{user}"))
                .join("mezon.sock"),
        );

        paths
    }

    // ── Windows ───────────────────────────────────────────────────────────────

    #[cfg(windows)]
    const PIPE_NAME: &'static str = r"\\.\pipe\mezon-single-instance";

    #[cfg(windows)]
    fn try_acquire_windows(forward_url: Option<&str>) -> anyhow::Result<Option<Self>> {
        use std::io::Write as _;

        match std::fs::OpenOptions::new()
            .write(true)
            .open(Self::PIPE_NAME)
        {
            Ok(mut pipe) => {
                tracing::info!("Another instance is already running (Windows named pipe)");
                if let Some(url) = forward_url {
                    let _ = pipe.write_all(url.as_bytes());
                    let safe = url.split(['?', '#']).next().unwrap_or_default();
                    tracing::debug!("Forwarded URL to running instance via named pipe: {safe}");
                }
                return Ok(None);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // No server yet — we become the server.
            }
            Err(e) => {
                tracing::warn!("Named pipe connect error (treating as no server): {e}");
            }
        }

        let url_rx = Self::create_pipe_server()?;

        tracing::debug!("Single instance lock acquired (Windows named pipe)");
        Ok(Some(Self {
            _pipe_name: Self::PIPE_NAME.to_owned(),
            url_rx: std::sync::Mutex::new(Some(url_rx)),
        }))
    }

    #[cfg(windows)]
    fn create_pipe_server() -> anyhow::Result<std::sync::mpsc::Receiver<String>> {
        use std::time::Duration;
        use windows::Win32::Foundation::{
            ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE,
        };
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND,
        };
        use windows::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
            PIPE_WAIT,
        };

        const MIN_BACKOFF: Duration = Duration::from_millis(100);
        const MAX_BACKOFF: Duration = Duration::from_secs(5);

        let pipe_name: Vec<u16> = Self::PIPE_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        // so a second server creation attempt fails (mutex semantics).
        let handle = unsafe {
            CreateNamedPipeW(
                windows::core::PCWSTR(pipe_name.as_ptr()),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                None,
            )
        };

        if handle == INVALID_HANDLE_VALUE {
            return Err(anyhow::anyhow!(
                "CreateNamedPipeW failed (another instance may already hold the lock): {}",
                std::io::Error::last_os_error()
            ));
        }

        // Extract raw pointer value; HANDLE is Copy so just store the value
        let raw = unsafe { handle.0 as usize };

        let (tx, rx) = std::sync::mpsc::channel::<String>();

        // Transfer ownership into the listener thread.
        // The thread keeps the handle alive and continuously accepts + reads connections.
        std::thread::Builder::new()
            .name("mezon-single-instance".into())
            .spawn(move || {
                let h = windows::Win32::Foundation::HANDLE(raw as *mut std::ffi::c_void);
                let mut backoff = MIN_BACKOFF;
                loop {
                    let connected =
                        match unsafe { windows::Win32::System::Pipes::ConnectNamedPipe(h, None) } {
                            Ok(()) => true,
                            Err(_) => unsafe { GetLastError() == ERROR_PIPE_CONNECTED },
                        };
                    if !connected {
                        unsafe {
                            let _ = windows::Win32::System::Pipes::DisconnectNamedPipe(h);
                        }
                        std::thread::sleep(backoff);
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }
                    backoff = MIN_BACKOFF;

                    let mut buf = [0u8; 4096];
                    let mut bytes_read = 0u32;
                    let ok = unsafe {
                        windows::Win32::Storage::FileSystem::ReadFile(
                            h,
                            Some(&mut buf),
                            Some(&mut bytes_read),
                            None,
                        )
                        .is_ok()
                    };
                    if ok && bytes_read > 0 {
                        if let Ok(url) = std::str::from_utf8(&buf[..bytes_read as usize]) {
                            let url = url.trim().to_owned();
                            let safe = url.split(['?', '#']).next().unwrap_or_default();
                            tracing::debug!(
                                "Named pipe: received URL from secondary instance: {safe}"
                            );
                            let _ = tx.send(url);
                        }
                    }
                    unsafe {
                        let _ = windows::Win32::System::Pipes::DisconnectNamedPipe(h);
                    }
                }
            })
            .map_err(|e| {
                anyhow::anyhow!("Failed to spawn single-instance pipe listener thread: {e}")
            })?;

        Ok(rx)
    }

    // ── Shared: URL forwarding listener ───────────────────────────────────────

    /// Spawn a thread that accepts connections and calls `callback` with any
    /// URL strings sent by secondary instances.
    pub fn listen_for_urls(&self, callback: impl Fn(String) + Send + 'static) {
        #[cfg(unix)]
        self.listen_for_urls_unix(callback);

        #[cfg(windows)]
        self.listen_for_urls_windows(callback);

        #[cfg(not(any(unix, windows)))]
        {
            let _ = callback;
        }
    }

    #[cfg(unix)]
    fn listen_for_urls_unix(&self, callback: impl Fn(String) + Send + 'static) {
        let Some(listener) = &self._listener else {
            tracing::debug!(
                "Unix URL forwarding disabled because single-instance lock is disabled"
            );
            let _ = callback;
            return;
        };

        let listener = match listener.try_clone() {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("Could not clone UnixListener for URL forwarding: {e}");
                return;
            }
        };

        std::thread::spawn(move || {
            use std::io::{BufReader, Read as _};
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => {
                        let mut buf = String::new();
                        let ok = BufReader::new(s)
                            .take(4096)
                            .read_to_string(&mut buf)
                            .is_ok();
                        if ok && !buf.is_empty() {
                            let url = buf.trim().to_owned();
                            let safe = url.split(['?', '#']).next().unwrap_or_default();
                            tracing::debug!(
                                "Unix socket: received deep link from secondary instance: {safe}"
                            );
                            callback(url);
                        }
                    }
                    Err(e) => {
                        if Self::is_transient_accept_error(&e) {
                            tracing::debug!("Transient Unix socket accept error, retrying: {e}");
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            continue;
                        }
                        tracing::warn!("Unix socket accept error (stopping URL forwarding): {e}");
                        break;
                    }
                }
            }
        });
    }

    #[cfg(unix)]
    fn is_transient_accept_error(e: &std::io::Error) -> bool {
        use std::io::ErrorKind;
        if matches!(
            e.kind(),
            ErrorKind::ConnectionAborted | ErrorKind::Interrupted | ErrorKind::WouldBlock
        ) {
            return true;
        }
        matches!(e.raw_os_error(), Some(23) | Some(24) | Some(55) | Some(105))
    }

    #[cfg(windows)]
    fn listen_for_urls_windows(&self, callback: impl Fn(String) + Send + 'static) {
        let Some(rx) = self.url_rx.lock().ok().and_then(|mut guard| guard.take()) else {
            tracing::debug!("Windows URL forwarding already wired or unavailable");
            let _ = callback;
            return;
        };

        if let Err(e) = std::thread::Builder::new()
            .name("mezon-pipe-url-listener".into())
            .spawn(move || {
                for url in rx {
                    let safe = url.split(['?', '#']).next().unwrap_or_default();
                    tracing::debug!("Named pipe URL listener: delivering {safe}");
                    callback(url);
                }
            })
        {
            tracing::warn!("Failed to spawn pipe URL listener thread: {e}");
        }
    }
}

// ── Cleanup ───────────────────────────────────────────────────────────────────

#[cfg(unix)]
impl Drop for SingleInstance {
    fn drop(&mut self) {
        if let Some(socket_path) = &self.socket_path {
            let _ = std::fs::remove_file(socket_path);
        }
    }
}
