//! Native protobuf/HTTP-2 gRPC `call` transport.
//!
//! The bundled `grpc` function (`ows-runtime` `http` feature) targets gRPC
//! services exposed through a JSON transcoding/gateway endpoint. This module
//! adds a *native* gRPC transport behind the `grpc-native` feature: it compiles
//! the workflow's `.proto` (or an inline proto source) at runtime with `protox`
//! (pure Rust, no `protoc`), builds dynamic protobuf messages with
//! `prost-reflect`, and performs a unary gRPC call over HTTP/2 via `tonic`.
//!
//! No protobuf code generation is required, so arbitrary `.proto`-defined
//! services can be called directly from a workflow.
#![cfg(feature = "grpc-native")]

use std::path::Path;
use std::sync::Arc;

use ows_runtime_core::WorkflowError;
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, MethodDescriptor};
use serde_json::Value;

use crate::runtime::RuntimeInner;
use crate::service::{enforce_network_policy, FunctionInvoker, FunctionRequest};

/// Invokes gRPC services natively over HTTP/2 using dynamically-resolved
/// protobuf messages.
pub struct NativeGrpcInvoker {
    inner: Arc<RuntimeInner>,
}

impl NativeGrpcInvoker {
    /// Creates a new native gRPC invoker.
    pub fn new(inner: Arc<RuntimeInner>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl FunctionInvoker for NativeGrpcInvoker {
    async fn invoke(&self, req: FunctionRequest<'_>) -> Result<Value, WorkflowError> {
        let args = &req.args;

        let proto = args.get("proto").cloned().unwrap_or(Value::Null);
        let pool = build_pool(&proto)?;

        let service = args.get("service").cloned().unwrap_or(Value::Null);
        let service_name = service
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::error::semantic_error("grpc call requires service.name"))?;
        let method_name = args
            .get("method")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::error::semantic_error("grpc call requires method"))?;
        let method = resolve_method(&pool, service_name, method_name)?;

        let host = service
            .get("host")
            .and_then(|v| v.as_str())
            .unwrap_or("localhost");
        let port = service.get("port").and_then(|v| v.as_u64()).unwrap_or(80);

        let arguments = args
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));

        let uri = format!("http://{host}:{port}");
        enforce_network_policy(&self.inner, &uri)?;

        let channel = tonic::transport::Channel::from_shared(uri.clone())
            .map_err(|e| {
                crate::error::semantic_error(format!("invalid gRPC endpoint `{uri}`: {e}"))
            })?
            .connect()
            .await
            .map_err(|e| {
                crate::error::communication_error(
                    500,
                    format!("failed to connect to gRPC endpoint `{uri}`: {e}"),
                )
            })?;

        let request = encode_request(&method, arguments)?;
        let mut client = tonic::client::Grpc::new(channel);
        client
            .ready()
            .await
            .map_err(|e| crate::error::communication_error(500, format!("gRPC not ready: {e}")))?;

        let path = http::uri::PathAndQuery::try_from(format!(
            "/{}/{}",
            method.parent_service().full_name(),
            method.name()
        ))
        .map_err(|e| crate::error::runtime_error(format!("invalid gRPC method path: {e}")))?;

        let codec = DynamicCodec {
            output: method.output(),
        };
        let response = client
            .unary(tonic::Request::new(request), path, codec)
            .await
            .map_err(|e| {
                crate::error::communication_error(
                    500,
                    format!("gRPC call `{service_name}/{method_name}` failed: {e}"),
                )
            })?;

        let reply = response.into_inner();
        serde_json::to_value(&reply).map_err(|e| {
            crate::error::runtime_error(format!("failed to encode gRPC response: {e}"))
        })
    }
}

/// Dispatches the `grpc` function to the native protobuf/HTTP-2 transport when
/// the call supplies a `proto` descriptor, otherwise to the JSON/gateway
/// adapter (when the `http` feature is enabled).
pub struct GrpcDispatchInvoker {
    native: NativeGrpcInvoker,
    #[cfg(feature = "http")]
    gateway: crate::service::GrpcInvoker,
}

impl GrpcDispatchInvoker {
    /// Creates a new dispatching gRPC invoker.
    pub fn new(inner: Arc<RuntimeInner>) -> Self {
        Self {
            native: NativeGrpcInvoker::new(inner.clone()),
            #[cfg(feature = "http")]
            gateway: crate::service::GrpcInvoker::new(inner),
        }
    }
}

#[async_trait::async_trait]
impl FunctionInvoker for GrpcDispatchInvoker {
    async fn invoke(&self, req: FunctionRequest<'_>) -> Result<Value, WorkflowError> {
        if req.args.contains_key("proto") {
            return self.native.invoke(req).await;
        }
        #[cfg(feature = "http")]
        {
            self.gateway.invoke(req).await
        }
        #[cfg(not(feature = "http"))]
        {
            self.native.invoke(req).await
        }
    }
}

/// Builds a descriptor pool from a `proto` argument (`endpoint` file or inline
/// `content`).
pub(crate) fn build_pool(proto: &Value) -> Result<DescriptorPool, WorkflowError> {
    use prost_types::FileDescriptorSet;

    let bytes = if let Some(content) = proto.get("content").and_then(|v| v.as_str()) {
        let name = proto
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("service.proto");
        let file = protox_parse::parse(name, content).map_err(|e| {
            crate::error::semantic_error(format!("failed to parse inline proto `{name}`: {e}"))
        })?;
        FileDescriptorSet { file: vec![file] }.encode_to_vec()
    } else {
        let endpoint = proto
            .get("endpoint")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::error::semantic_error("grpc call requires proto.endpoint or proto.content")
            })?;
        let path = endpoint.strip_prefix("file://").unwrap_or(endpoint);
        let path = Path::new(path);
        if !path.exists() {
            return Err(crate::error::semantic_error(format!(
                "proto file `{}` does not exist",
                path.display()
            )));
        }
        let include = path.parent().unwrap_or_else(|| Path::new("."));
        let set = protox::compile([path], [include]).map_err(|e| {
            crate::error::semantic_error(format!(
                "failed to compile proto `{}`: {e}",
                path.display()
            ))
        })?;
        set.encode_to_vec()
    };

    DescriptorPool::decode(bytes.as_slice())
        .map_err(|e| crate::error::semantic_error(format!("invalid proto descriptor set: {e}")))
}

/// Resolves a service/method descriptor by (possibly unqualified) name.
pub(crate) fn resolve_method(
    pool: &DescriptorPool,
    service_name: &str,
    method_name: &str,
) -> Result<MethodDescriptor, WorkflowError> {
    let service = pool
        .get_service_by_name(service_name)
        .or_else(|| {
            pool.services().find(|s| {
                s.full_name() == service_name
                    || s.full_name().ends_with(&format!(".{service_name}"))
            })
        })
        .ok_or_else(|| {
            crate::error::semantic_error(format!(
                "gRPC service `{service_name}` not found in proto"
            ))
        })?;
    let resolved = service
        .methods()
        .find(|m| m.name() == method_name || m.full_name() == method_name)
        .ok_or_else(|| {
            crate::error::semantic_error(format!(
                "gRPC method `{method_name}` not found on service `{service_name}`"
            ))
        });
    resolved
}

/// Builds a dynamic protobuf request message from JSON arguments.
pub(crate) fn encode_request(
    method: &MethodDescriptor,
    arguments: Value,
) -> Result<DynamicMessage, WorkflowError> {
    DynamicMessage::deserialize(method.input(), arguments).map_err(|e| {
        crate::error::semantic_error(format!(
            "gRPC arguments do not match `{}`: {e}",
            method.input().full_name()
        ))
    })
}

/// A tonic codec for dynamic protobuf messages (no generated types).
struct DynamicCodec {
    output: MessageDescriptor,
}

impl tonic::codec::Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynamicEncoder;
    type Decoder = DynamicDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        DynamicEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        DynamicDecoder {
            output: self.output.clone(),
        }
    }
}

struct DynamicEncoder;

impl tonic::codec::Encoder for DynamicEncoder {
    type Item = DynamicMessage;
    type Error = tonic::Status;

    fn encode(
        &mut self,
        item: DynamicMessage,
        dst: &mut tonic::codec::EncodeBuf<'_>,
    ) -> Result<(), Self::Error> {
        item.encode(dst)
            .map_err(|e| tonic::Status::internal(e.to_string()))
    }
}

struct DynamicDecoder {
    output: MessageDescriptor,
}

impl tonic::codec::Decoder for DynamicDecoder {
    type Item = DynamicMessage;
    type Error = tonic::Status;

    fn decode(
        &mut self,
        src: &mut tonic::codec::DecodeBuf<'_>,
    ) -> Result<Option<DynamicMessage>, Self::Error> {
        DynamicMessage::decode(self.output.clone(), src)
            .map(Some)
            .map_err(|e| tonic::Status::internal(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use serde_json::json;
    use tower::service_fn;

    const PROTO: &str = r#"
syntax = "proto3";
package greet;
message HelloRequest { string name = 1; }
message HelloReply { string message = 1; }
service Greeter { rpc SayHello (HelloRequest) returns (HelloReply); }
"#;

    #[test]
    fn builds_pool_from_inline_content() {
        let pool = build_pool(&json!({ "content": PROTO, "name": "greet.proto" })).unwrap();
        assert!(pool.get_service_by_name("greet.Greeter").is_some());
    }

    #[test]
    fn builds_pool_from_file_endpoint() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("ows-grpc-{}.proto", std::process::id()));
        std::fs::write(&path, PROTO).unwrap();
        let endpoint = format!("file://{}", path.display());
        let pool = build_pool(&json!({ "endpoint": endpoint })).unwrap();
        assert!(pool.get_service_by_name("greet.Greeter").is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn build_pool_reports_missing_and_invalid_proto() {
        // No endpoint and no content.
        let err = build_pool(&json!({})).unwrap_err();
        assert_eq!(err.kind, ows_runtime_core::ErrorKind::Semantic);
        // Endpoint that does not exist.
        let err = build_pool(&json!({ "endpoint": "file:///nope/missing.proto" })).unwrap_err();
        assert!(err
            .problem
            .detail
            .unwrap_or_default()
            .contains("does not exist"));
        // Endpoint with a proto compile error.
        let dir = std::env::temp_dir();
        let path = dir.join(format!("ows-grpc-bad-{}.proto", std::process::id()));
        std::fs::write(&path, "syntax = \"proto3\";\nmessage {").unwrap();
        let endpoint = format!("file://{}", path.display());
        assert!(build_pool(&json!({ "endpoint": endpoint })).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resolves_fully_qualified_names() {
        let pool = build_pool(&json!({ "content": PROTO })).unwrap();
        let method = resolve_method(&pool, "greet.Greeter", "greet.Greeter.SayHello").unwrap();
        assert_eq!(method.name(), "SayHello");
    }

    #[test]
    fn resolves_unqualified_service_and_method() {
        let pool = build_pool(&json!({ "content": PROTO })).unwrap();
        let method = resolve_method(&pool, "Greeter", "SayHello").unwrap();
        assert_eq!(method.input().full_name(), "greet.HelloRequest");
        assert_eq!(method.output().full_name(), "greet.HelloReply");
        assert!(resolve_method(&pool, "Greeter", "Nope").is_err());
        assert!(resolve_method(&pool, "Nope", "SayHello").is_err());
    }

    #[test]
    fn encodes_request_and_rejects_bad_arguments() {
        let pool = build_pool(&json!({ "content": PROTO })).unwrap();
        let method = resolve_method(&pool, "Greeter", "SayHello").unwrap();
        let msg = encode_request(&method, json!({ "name": "Ada" })).unwrap();
        assert_eq!(
            msg.get_field_by_name("name").unwrap().as_str().unwrap(),
            "Ada"
        );
        assert!(encode_request(&method, json!({ "unknown": 1 })).is_err());
    }

    /// Exercises the full dynamic call (path, framing, encode and decode)
    /// against an in-memory service, with no network.
    #[tokio::test]
    async fn dynamic_unary_call_roundtrips() {
        let pool = build_pool(&json!({ "content": PROTO })).unwrap();
        let method = resolve_method(&pool, "greet.Greeter", "SayHello").unwrap();
        let input = method.input();
        let output = method.output();

        let mock_output = output.clone();
        let mock_input = input.clone();
        let service = service_fn(move |req: http::Request<tonic::body::BoxBody>| {
            let mock_output = mock_output.clone();
            let mock_input = mock_input.clone();
            async move {
                assert_eq!(req.uri().path(), "/greet.Greeter/SayHello");
                assert_eq!(
                    req.headers().get("content-type").unwrap(),
                    "application/grpc"
                );
                let bytes = req.into_body().collect().await.unwrap().to_bytes();
                // Strip the 5-byte gRPC frame header.
                let request = DynamicMessage::decode(mock_input.clone(), &bytes[5..]).unwrap();
                let name = request
                    .get_field_by_name("name")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string();
                let mut reply = DynamicMessage::new(mock_output.clone());
                reply.set_field_by_name(
                    "message",
                    prost_reflect::Value::String(format!("Hello, {name}!")),
                );
                let mut body = Vec::new();
                reply.encode(&mut body).unwrap();
                let mut frame = vec![0u8];
                frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
                frame.extend_from_slice(&body);
                Ok::<_, std::convert::Infallible>(http::Response::new(tonic::body::boxed(
                    Full::new(Bytes::from(frame)),
                )))
            }
        });

        let request = encode_request(&method, json!({ "name": "Ada" })).unwrap();
        let mut client = tonic::client::Grpc::new(service);
        client.ready().await.unwrap();
        let path =
            http::uri::PathAndQuery::try_from("/greet.Greeter/SayHello".to_string()).unwrap();
        let response = client
            .unary(tonic::Request::new(request), path, DynamicCodec { output })
            .await
            .unwrap();
        let value = serde_json::to_value(response.into_inner()).unwrap();
        assert_eq!(value, json!({ "message": "Hello, Ada!" }));
    }
}
