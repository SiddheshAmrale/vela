//! Single-instance + automation channel: a per-user named pipe on Windows,
//! a Unix domain socket elsewhere.
//!
//! Protocol: the client writes one request terminated by NUL, the server
//! answers with one reply terminated by NUL. Request = lines; first line is
//! the verb (`open`, `enqueue`, `action`, `state`, `mpv`, `get`, `shot`, `quit`).

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Request {
    pub text: String,
    pub reply: mpsc::Sender<String>,
}

fn user_tag() -> String {
    let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "user".into());
    user.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// Write `request\0`, read until `\0`.
fn exchange(stream: &mut (impl Read + Write), request: &str) -> Option<String> {
    let mut msg = request.as_bytes().to_vec();
    msg.push(0);
    stream.write_all(&msg).ok()?;
    let mut reply = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                reply.extend_from_slice(&buf[..n]);
                if reply.last() == Some(&0) {
                    reply.pop();
                    break;
                }
            }
        }
    }
    Some(String::from_utf8_lossy(&reply).into_owned())
}

/// Read one NUL-terminated request, hand it to the app, write the reply.
fn serve_one(stream: &mut (impl Read + Write), on_request: &impl Fn(Request)) {
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                data.extend_from_slice(&buf[..n]);
                if data.last() == Some(&0) {
                    data.pop();
                    break;
                }
                if data.len() > 4 * 1024 * 1024 {
                    return;
                }
            }
        }
    }
    let (tx, rx) = mpsc::channel();
    on_request(Request { text: String::from_utf8_lossy(&data).into_owned(), reply: tx });
    let mut reply = rx.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| "timeout".into()).into_bytes();
    reply.push(0);
    let _ = stream.write_all(&reply);
    let _ = stream.flush();
}

// ---------------------------------------------------------------------------
#[cfg(windows)]
mod imp {
    use super::*;
    use std::fs::File;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    pub fn endpoint() -> String {
        format!(r"\\.\pipe\Vela-{}", user_tag())
    }

    pub fn send(request: &str, timeout: Duration) -> Option<String> {
        let start = Instant::now();
        loop {
            match std::fs::OpenOptions::new().read(true).write(true).open(endpoint()) {
                Ok(mut f) => return exchange(&mut f, request),
                // 231 = ERROR_PIPE_BUSY: a server exists but is serving someone else
                Err(e) if e.raw_os_error() == Some(231) && start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(15)),
                Err(_) => return None,
            }
        }
    }

    fn create(first: bool) -> Option<HANDLE> {
        let name: Vec<u16> = endpoint().encode_utf16().chain(std::iter::once(0)).collect();
        let mode = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        let h = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                std::ptr::null(),
            )
        };
        (h != INVALID_HANDLE_VALUE).then_some(h)
    }

    pub struct Server(usize);

    pub fn claim() -> Option<Server> {
        create(true).map(|h| Server(h as usize))
    }

    impl Server {
        pub fn spawn(self, on_request: impl Fn(Request) + Send + 'static) {
            let first = self.0;
            let _ = std::thread::Builder::new().name("vela-ipc".into()).spawn(move || {
                let mut handle = first as HANDLE;
                loop {
                let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                // File takes ownership of the handle (closed on drop).
                let mut pipe = unsafe { File::from_raw_handle(handle as _) };
                if connected {
                    serve_one(&mut pipe, &on_request);
                    unsafe { DisconnectNamedPipe(handle) };
                }
                drop(pipe);
                match create(false).or_else(|| {
                    std::thread::sleep(Duration::from_millis(200));
                    create(false)
                }) {
                    Some(h) => handle = h,
                    None => return,
                }
                }
            });
        }
    }
}

// ---------------------------------------------------------------------------
#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    pub fn endpoint() -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        dir.join(format!("vela-{}.sock", user_tag()))
    }

    pub fn send(request: &str, timeout: Duration) -> Option<String> {
        let start = Instant::now();
        loop {
            match UnixStream::connect(endpoint()) {
                Ok(mut s) => {
                    let _ = s.set_read_timeout(Some(Duration::from_secs(12)));
                    return exchange(&mut s, request);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(15)),
                Err(_) => return None,
            }
        }
    }

    pub struct Server(UnixListener);

    pub fn claim() -> Option<Server> {
        let path = endpoint();
        match UnixListener::bind(&path) {
            Ok(l) => Some(Server(l)),
            Err(_) => {
                // A socket file exists: live instance, or left over from a crash?
                if UnixStream::connect(&path).is_ok() {
                    return None;
                }
                let _ = std::fs::remove_file(&path);
                UnixListener::bind(&path).ok().map(Server)
            }
        }
    }

    impl Server {
        pub fn spawn(self, on_request: impl Fn(Request) + Send + 'static) {
            let _ = std::thread::Builder::new().name("vela-ipc".into()).spawn(move || {
                for mut stream in self.0.incoming().flatten() {
                    serve_one(&mut stream, &on_request);
                }
            });
        }
    }
}

pub use imp::{claim, send, Server};
