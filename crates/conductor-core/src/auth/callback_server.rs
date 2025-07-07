use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;
use crate::auth::{AuthError, Result};

/// OAuth callback response
#[derive(Debug)]
pub struct CallbackResponse {
    pub code: String,
    pub state: String,
}

/// Start a local HTTP server to receive OAuth callback
pub async fn start_callback_server(
    expected_state: String,
    timeout_duration: Duration,
) -> Result<CallbackResponse> {
    let (tx, mut rx) = mpsc::channel::<Result<CallbackResponse>>(1);
    let tx = Arc::new(tx);
    
    // Try ports in range
    let listener = try_bind_listener().await?;
    let port = listener.local_addr()?.port();
    
    tracing::info!("OAuth callback server listening on port {}", port);
    
    // Spawn server task
    let server_task = tokio::spawn(async move {
        let expected_state = Arc::new(expected_state);
        
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::error!("Failed to accept connection: {}", e);
                    continue;
                }
            };
            
            let io = TokioIo::new(stream);
            let tx = tx.clone();
            let expected_state = expected_state.clone();
            
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let tx = tx.clone();
                    let expected_state = expected_state.clone();
                    async move {
                        handle_request(req, tx, &expected_state).await
                    }
                });
                
                if let Err(e) = http1::Builder::new()
                    .serve_connection(io, service)
                    .await
                {
                    tracing::error!("Failed to serve connection: {}", e);
                }
            });
        }
    });
    
    // Wait for callback with timeout
    let result = timeout(timeout_duration, rx.recv()).await
        .map_err(|_| AuthError::CallbackServer("Timeout waiting for OAuth callback".to_string()))?
        .ok_or_else(|| AuthError::CallbackServer("Callback server channel closed".to_string()))??;
    
    // Cleanup: abort the server task
    server_task.abort();
    
    Ok(result)
}

/// Try to bind to a port in the range 57845-57855
async fn try_bind_listener() -> Result<TcpListener> {
    let start_port = 57845;
    let end_port = 57855;
    
    for port in start_port..=end_port {
        match TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await {
            Ok(listener) => return Ok(listener),
            Err(_) => continue,
        }
    }
    
    Err(AuthError::CallbackServer(format!(
        "Failed to bind to any port in range {}-{}",
        start_port, end_port
    )))
}

/// Handle incoming HTTP request
async fn handle_request(
    req: Request<Incoming>,
    tx: Arc<mpsc::Sender<Result<CallbackResponse>>>,
    expected_state: &str,
) -> std::result::Result<Response<String>, hyper::Error> {
    if req.method() != Method::GET {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body("Method not allowed".to_string())
            .unwrap());
    }
    
    let path = req.uri().path();
    if !path.starts_with("/callback") {
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body("Not found".to_string())
            .unwrap());
    }
    
    // Parse query parameters
    let query = req.uri().query().unwrap_or("");
    let params: std::collections::HashMap<&str, &str> = query
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.split('=');
            match (parts.next(), parts.next()) {
                (Some(key), Some(value)) => Some((key, value)),
                _ => None,
            }
        })
        .collect();
    
    // Check for error parameter
    if let Some(error) = params.get("error") {
        let _ = tx.send(Err(AuthError::Cancelled)).await;
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/html")
            .body(format!(
                r#"<!DOCTYPE html>
<html>
<head>
    <title>Authorization Failed</title>
    <style>
        body {{ font-family: Arial, sans-serif; text-align: center; padding: 50px; }}
        .error {{ color: #d32f2f; }}
    </style>
</head>
<body>
    <h1 class="error">Authorization Failed</h1>
    <p>Error: {}</p>
    <p>You can close this window and return to the terminal.</p>
</body>
</html>"#,
                error
            ))
            .unwrap());
    }
    
    // Extract code and state
    let code = params.get("code");
    let state = params.get("state");
    
    match (code, state) {
        (Some(code), Some(state)) if *state == expected_state => {
            let response = CallbackResponse {
                code: code.to_string(),
                state: state.to_string(),
            };
            let _ = tx.send(Ok(response)).await;
            
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/html")
                .body(
                    r#"<!DOCTYPE html>
<html>
<head>
    <title>Authorization Successful</title>
    <style>
        body { font-family: Arial, sans-serif; text-align: center; padding: 50px; }
        .success { color: #4caf50; }
    </style>
</head>
<body>
    <h1 class="success">Authorization Successful!</h1>
    <p>You can close this window and return to the terminal.</p>
    <script>
        setTimeout(function() { window.close(); }, 3000);
    </script>
</body>
</html>"#
                    .to_string(),
                )
                .unwrap())
        }
        (Some(_), Some(state)) if *state != expected_state => {
            let _ = tx.send(Err(AuthError::StateMismatch)).await;
            Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body("Invalid state parameter".to_string())
                .unwrap())
        }
        _ => {
            let _ = tx
                .send(Err(AuthError::InvalidResponse(
                    "Missing code or state parameter".to_string(),
                )))
                .await;
            Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body("Missing required parameters".to_string())
                .unwrap())
        }
    }
}

/// Get the first available port in the range
pub async fn get_available_port() -> Result<u16> {
    let listener = try_bind_listener().await?;
    Ok(listener.local_addr()?.port())
}