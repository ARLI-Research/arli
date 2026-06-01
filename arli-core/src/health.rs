//! Health check HTTP endpoint — basic observability.
//!
//! Starts a tiny HTTP server that responds to GET /health with JSON status.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Health check server — minimal HTTP on a given port.
pub struct HealthServer {
    port: u16,
    ready: Arc<RwLock<bool>>,
    metrics: Arc<RwLock<HashMap<String, String>>>,
}

impl HealthServer {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            ready: Arc::new(RwLock::new(false)),
            metrics: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Mark the server as ready (returns 200 instead of 503).
    pub async fn set_ready(&self, ready: bool) {
        *self.ready.write().await = ready;
    }

    /// Set a metric key-value pair.
    pub async fn set_metric(&self, key: &str, value: &str) {
        self.metrics
            .write()
            .await
            .insert(key.to_string(), value.to_string());
    }

    /// Start serving HTTP on the configured port.
    pub async fn serve(&self) {
        let listener = match tokio::net::TcpListener::bind(format!("0.0.0.0:{}", self.port)).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("Health server failed to bind port {}: {}", self.port, e);
                return;
            }
        };

        tracing::info!("Health server listening on port {}", self.port);

        let ready = self.ready.clone();
        let metrics = self.metrics.clone();

        loop {
            match listener.accept().await {
                Ok((mut stream, _addr)) => {
                    let ready = ready.clone();
                    let metrics = metrics.clone();

                    tokio::spawn(async move {
                        use tokio::io::{AsyncReadExt, AsyncWriteExt};

                        let mut buf = [0u8; 1024];
                        let n = match stream.read(&mut buf).await {
                            Ok(n) if n > 0 => n,
                            _ => return,
                        };

                        let request = String::from_utf8_lossy(&buf[..n]);
                        let first_line = request.lines().next().unwrap_or("");

                        let (status, body) = if first_line.starts_with("GET /health") {
                            let is_ready = *ready.read().await;
                            let m = metrics.read().await;

                            let metrics_json: Vec<String> = m
                                .iter()
                                .map(|(k, v)| format!("\"{}\":\"{}\"", k, v))
                                .collect();

                            let status_code = if is_ready {
                                "200 OK"
                            } else {
                                "503 Service Unavailable"
                            };
                            let status_field = if is_ready { "ok" } else { "starting" };

                            let json = format!(
                                "{{\"status\":\"{}\",\"uptime_secs\":0{}{}}}",
                                status_field,
                                if metrics_json.is_empty() { "" } else { "," },
                                metrics_json.join(",")
                            );

                            (status_code, json)
                        } else if first_line.starts_with("GET /metrics") {
                            let m = metrics.read().await;
                            let lines: Vec<String> =
                                m.iter().map(|(k, v)| format!("{} {}", k, v)).collect();
                            ("200 OK", lines.join("\n"))
                        } else {
                            ("404 Not Found", "{\"error\":\"not found\"}".to_string())
                        };

                        let response = format!(
                            "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            status,
                            body.len(),
                            body
                        );

                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.flush().await;
                    });
                }
                Err(e) => {
                    tracing::error!("Health server accept error: {}", e);
                }
            }
        }
    }
}
