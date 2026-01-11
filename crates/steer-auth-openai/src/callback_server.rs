use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::sync::Arc;
use steer_auth_plugin::{AuthError, Result};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// OAuth callback response
#[derive(Debug)]
pub struct CallbackResponse {
    pub code: String,
    pub state: String,
}

/// Handle for a spawned callback server.
#[derive(Debug)]
pub struct CallbackServerHandle {
    receiver: mpsc::Receiver<Result<CallbackResponse>>,
    task: tokio::task::JoinHandle<()>,
}

impl CallbackServerHandle {
    pub fn try_recv(&mut self) -> Option<Result<CallbackResponse>> {
        match self.receiver.try_recv() {
            Ok(value) => Some(value),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => None,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => Some(Err(
                AuthError::CallbackServer("Callback server channel closed".to_string()),
            )),
        }
    }
}

impl Drop for CallbackServerHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Spawn a callback server on a fixed address/path and return a handle.
pub async fn spawn_callback_server(
    expected_state: String,
    bind_addr: SocketAddr,
    expected_path: &str,
) -> Result<CallbackServerHandle> {
    let (tx, rx) = mpsc::channel::<Result<CallbackResponse>>(1);
    let tx = Arc::new(tx);

    let listener = TcpListener::bind(bind_addr)
        .await
        .map_err(|e| AuthError::CallbackServer(format!("Failed to bind {bind_addr}: {e}")))?;

    let expected_path = expected_path.to_string();
    let task = tokio::spawn(async move {
        let expected_state = Arc::new(expected_state);
        let expected_path = Arc::new(expected_path);

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
            let expected_path = expected_path.clone();

            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let tx = tx.clone();
                    let expected_state = expected_state.clone();
                    let expected_path = expected_path.clone();
                    async move { handle_request(req, tx, &expected_state, &expected_path).await }
                });

                if let Err(e) = http1::Builder::new().serve_connection(io, service).await {
                    tracing::error!("Failed to serve connection: {}", e);
                }
            });
        }
    });

    Ok(CallbackServerHandle { receiver: rx, task })
}

async fn handle_request(
    req: Request<Incoming>,
    tx: Arc<mpsc::Sender<Result<CallbackResponse>>>,
    expected_state: &str,
    expected_path: &str,
) -> std::result::Result<Response<String>, hyper::Error> {
    if req.method() != Method::GET {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body("Method not allowed".to_string())
            .unwrap());
    }

    let path = req.uri().path();
    if path != expected_path {
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body("Not found".to_string())
            .unwrap());
    }

    let query = req.uri().query().unwrap_or("");
    let params: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();

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
    <p>Error: {error}</p>
    <p>You can close this window and return to the terminal.</p>
</body>
</html>"#
            ))
            .unwrap());
    }

    let code = params.get("code");
    let state = params.get("state");

    if code.is_none() || state.is_none() {
        return Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body("Missing code or state".to_string())
            .unwrap());
    }

    if state.unwrap() != expected_state {
        let _ = tx.send(Err(AuthError::StateMismatch)).await;
        return Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body("State mismatch".to_string())
            .unwrap());
    }

    let response = CallbackResponse {
        code: code.unwrap().to_string(),
        state: state.unwrap().to_string(),
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
    <h1 class="success">Authorization Successful</h1>
    <p>You can close this window and return to the terminal.</p>
</body>
</html>"#
                .to_string(),
        )
        .unwrap())
}
