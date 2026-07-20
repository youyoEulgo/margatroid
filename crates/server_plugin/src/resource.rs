use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 3939,
        }
    }
}

pub struct ServerHandle {
    shutdown: Arc<AtomicBool>,
    address: Mutex<Option<SocketAddr>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ServerHandle {
    pub fn new() -> Self {
        Self {
            shutdown: Arc::new(AtomicBool::new(false)),
            address: Mutex::new(None),
            thread: Mutex::new(None),
        }
    }

    pub fn start(&self, config: &ServerConfig) -> Result<SocketAddr, String> {
        if let Some(address) = self.address() {
            return Ok(address);
        }

        let listener = TcpListener::bind(format!("{}:{}", config.host, config.port))
            .map_err(|error| error.to_string())?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;

        self.shutdown.store(false, Ordering::Release);
        let shutdown = self.shutdown.clone();
        let thread = std::thread::Builder::new()
            .name("margatroid-server-plugin".into())
            .spawn(move || serve(listener, shutdown))
            .map_err(|error| error.to_string())?;

        *self
            .address
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(address);
        *self
            .thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(thread);
        Ok(address)
    }

    pub fn address(&self) -> Option<SocketAddr> {
        *self
            .address
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = thread.join();
        }
        *self
            .address
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

impl Default for ServerHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn serve(listener: TcpListener, shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => handle_connection(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
}

fn handle_connection(mut stream: TcpStream) {
    let mut buffer = [0; 1024];
    let Ok(read) = stream.read(&mut buffer) else {
        return;
    };
    let request = String::from_utf8_lossy(&buffer[..read]);
    let first_line = request.lines().next().unwrap_or_default();
    let status = if first_line.starts_with("GET /health ") {
        "200 OK"
    } else {
        "404 Not Found"
    };
    let body = if status == "200 OK" {
        "ok\n"
    } else {
        "not found\n"
    };
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-length: {}\r\ncontent-type: text/plain\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}
