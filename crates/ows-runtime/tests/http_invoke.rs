//! Tests the real HTTP function invoker against a local in-process server,
//! covering content/response output modes and status handling without external
//! network dependencies.
#![cfg(feature = "http")]

use ows_runtime::Runtime;
use ows_runtime_core::RuntimePolicy;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serves one HTTP response for each accepted connection.
async fn serve_one(addr: std::net::SocketAddr, body: &'static str, status: &'static str) {
    let listener = TcpListener::bind(addr).await.unwrap();
    for _ in 0..4 {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = socket.read(&mut buf).await;
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(resp.as_bytes()).await;
        let _ = socket.shutdown().await;
    }
}

fn network_policy() -> RuntimePolicy {
    RuntimePolicy {
        allow_network: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn http_invoke_content_output() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    drop(listener);
    tokio::spawn(serve_one(addr, r#"{"id":1,"name":"rex"}"#, "200 OK"));

    let runtime = Runtime::builder()
        .with_policy(network_policy())
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(&format!(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: http
  version: '1.0.0'
do:
  - get:
      call: http
      with:
        method: get
        endpoint: http://127.0.0.1:{port}/pet/{{petId}}
"#
    ))
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, json!({ "petId": 1 })).await.unwrap();
    assert_eq!(out, json!({"id":1,"name":"rex"}));
}

#[tokio::test]
async fn http_invoke_response_output() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    drop(listener);
    tokio::spawn(serve_one(addr, r#"{"id":1}"#, "200 OK"));

    let runtime = Runtime::builder()
        .with_policy(network_policy())
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(&format!(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: http
  version: '1.0.0'
do:
  - get:
      call: http
      with:
        method: get
        endpoint: http://127.0.0.1:{port}/x
        output: response
"#
    ))
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out["statusCode"], 200);
    assert_eq!(out["content"]["id"], 1);
    assert_eq!(out["request"]["method"], "GET");
}

#[tokio::test]
async fn http_invoke_non_2xx_errors() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    drop(listener);
    tokio::spawn(serve_one(addr, r#"{"error":"nope"}"#, "404 Not Found"));

    let runtime = Runtime::builder()
        .with_policy(network_policy())
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(&format!(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: http
  version: '1.0.0'
do:
  - get:
      call: http
      with:
        method: get
        endpoint: http://127.0.0.1:{port}/missing
"#
    ))
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let err = runtime.run(wf, Value::Null).await.unwrap_err();
    assert_eq!(err.problem.status, 404);
}

#[tokio::test]
async fn http_invoke_query_and_basic_auth() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    drop(listener);
    tokio::spawn(serve_one(addr, r#"{"ok":true}"#, "200 OK"));

    let runtime = Runtime::builder()
        .with_policy(network_policy())
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(&format!(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: http
  version: '1.0.0'
do:
  - get:
      call: http
      with:
        method: get
        endpoint:
          uri: http://127.0.0.1:{port}/auth
          authentication:
            basic:
              username: user
              password: pass
        query:
          status: active
"#
    ))
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!({"ok": true}));
}

async fn serve_status(addr: std::net::SocketAddr, status: &'static str, body: &'static str) {
    let listener = TcpListener::bind(addr).await.unwrap();
    for _ in 0..4 {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = socket.read(&mut buf).await;
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(resp.as_bytes()).await;
        let _ = socket.shutdown().await;
    }
}

#[tokio::test]
async fn http_invoke_redirect_true_accepts_3xx() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    drop(listener);
    tokio::spawn(serve_status(addr, "302 Found", r#"{"moved":true}"#));

    let runtime = Runtime::builder()
        .with_policy(network_policy())
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(&format!(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: http
  version: '1.0.0'
do:
  - get:
      call: http
      with:
        method: get
        endpoint: http://127.0.0.1:{port}/r
        redirect: true
"#
    ))
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    assert_eq!(out, json!({"moved": true}));
}

#[tokio::test]
async fn http_invoke_redirect_false_errors_on_3xx() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    drop(listener);
    tokio::spawn(serve_status(addr, "302 Found", r#"{"moved":true}"#));

    let runtime = Runtime::builder()
        .with_policy(network_policy())
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(&format!(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: http
  version: '1.0.0'
do:
  - get:
      call: http
      with:
        method: get
        endpoint: http://127.0.0.1:{port}/r
"#
    ))
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let err = runtime.run(wf, Value::Null).await.unwrap_err();
    assert_eq!(err.problem.status, 302);
}

#[tokio::test]
async fn http_invoke_raw_output() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    drop(listener);
    tokio::spawn(serve_one(addr, r#"{"x":1}"#, "200 OK"));

    let runtime = Runtime::builder()
        .with_policy(network_policy())
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(&format!(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: http
  version: '1.0.0'
do:
  - get:
      call: http
      with:
        method: get
        endpoint: http://127.0.0.1:{port}/x
        output: raw
"#
    ))
    .unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let out = runtime.run(wf, Value::Null).await.unwrap();
    let content = out["content"].as_str().unwrap();
    // base64 of `{"x":1}`.
    assert!(!content.is_empty());
}
