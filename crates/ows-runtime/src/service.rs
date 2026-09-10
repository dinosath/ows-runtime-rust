//! Service invocation: the `FunctionInvoker` abstraction, the built-in HTTP
//! function, and no-op adapters.

use std::collections::HashMap;
use std::sync::Arc;

#[cfg(feature = "http")]
use base64::Engine as _;
use ows_runtime_core::{
    ErrorKind, ExpressionContext, ProblemDetails, ProcessResult, ProcessRunner, ServiceInvoker,
    ServiceRequest, ServiceResponse, StandardErrorType, WorkflowError,
};
use serde_json::{json, Map, Value};

use crate::runtime::RuntimeInner;

/// A request to invoke a workflow function.
pub struct FunctionRequest<'a> {
    /// The name of the function being called.
    pub name: String,
    /// The function arguments (`with`).
    pub args: HashMap<String, Value>,
    /// The expression context.
    pub context: &'a ExpressionContext,
}

/// Invokes a workflow function (`call`).
#[async_trait::async_trait]
pub trait FunctionInvoker: Send + Sync {
    /// Invokes the function and returns its output.
    async fn invoke(&self, req: FunctionRequest<'_>) -> Result<Value, WorkflowError>;
}

/// The built-in OpenAPI function invoker.
///
/// It fetches the referenced OpenAPI document, resolves the operation by
/// `operationId`, and performs the HTTP call. Parameters are interpolated and
/// split into path, query and body parameters.
#[cfg(feature = "http")]
pub struct OpenApiInvoker {
    inner: Arc<RuntimeInner>,
    client: reqwest::Client,
}

#[cfg(feature = "http")]
impl OpenApiInvoker {
    /// Creates a new OpenAPI invoker.
    pub fn new(inner: Arc<RuntimeInner>) -> Self {
        let client = reqwest::Client::builder().build().unwrap_or_default();
        Self { inner, client }
    }
}

#[async_trait::async_trait]
impl FunctionInvoker for OpenApiInvoker {
    async fn invoke(&self, req: FunctionRequest<'_>) -> Result<Value, WorkflowError> {
        let args = &req.args;

        let document = args
            .get("document")
            .and_then(|d| d.get("endpoint"))
            .ok_or_else(|| {
                crate::error::semantic_error("openapi call requires a document endpoint")
            })?
            .clone();
        let doc_uri = match &document {
            Value::String(s) => s.clone(),
            Value::Object(m) => m
                .get("uri")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crate::error::semantic_error("openapi document endpoint requires a uri")
                })?
                .to_string(),
            _ => {
                return Err(crate::error::semantic_error(
                    "openapi document endpoint must be a string or object",
                ))
            }
        };

        let operation_id = args
            .get("operationId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::error::semantic_error("openapi call requires an operationId"))?
            .to_string();

        enforce_network_policy(&self.inner, &doc_uri)?;

        let spec: Value = self
            .client
            .get(&doc_uri)
            .send()
            .await
            .map_err(|e| {
                crate::error::communication_error(
                    500,
                    format!("failed to fetch openapi document: {e}"),
                )
            })?
            .json()
            .await
            .map_err(|e| {
                crate::error::communication_error(
                    500,
                    format!("failed to parse openapi document: {e}"),
                )
            })?;

        // Find the operation by operationId.
        let (method, path) = find_operation(&spec, &operation_id).ok_or_else(|| {
            crate::error::semantic_error(format!(
                "operationId `{operation_id}` not found in document"
            ))
        })?;

        // Build the base URL from the spec server or the document origin.
        let server_url = spec
            .get("servers")
            .and_then(|s| s.get(0))
            .and_then(|s| s.get("url"))
            .and_then(|u| u.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| doc_uri.clone());

        // Resolve parameters.
        let parameters = args
            .get("parameters")
            .cloned()
            .unwrap_or(Value::Object(Map::new()));
        let params_map = parameters.as_object().cloned().unwrap_or_default();
        let interpolated = interpolate_params(&self.inner, &params_map, req.context)?;

        // Split into path and query params.
        let mut path_params: HashMap<String, String> = HashMap::new();
        let mut query: Vec<(String, String)> = Vec::new();
        for (k, v) in &interpolated {
            let vstr = v
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| v.to_string());
            if path.contains(&format!("{{{k}}}")) {
                path_params.insert(k.clone(), vstr);
            } else {
                query.push((k.clone(), vstr));
            }
        }

        let mut url = server_url.trim_end_matches('/').to_string() + path.as_str();
        for (k, v) in &path_params {
            url = url.replace(&format!("{{{k}}}"), v);
        }

        let method = reqwest::Method::from_bytes(method.to_uppercase().as_bytes())
            .unwrap_or(reqwest::Method::GET);
        let method_str = method.clone();

        let mut rb = self.client.request(method, &url);
        if !query.is_empty() {
            rb = rb.query(&query);
        }
        let output = args
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("content");

        let response = rb.send().await.map_err(|e| {
            crate::error::communication_error(500, format!("openapi call failed: {e}"))
        })?;
        let status = response.status().as_u16();
        let headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let content_type = headers.get("content-type").cloned().unwrap_or_default();
        let bytes = response.bytes().await.map_err(|e| {
            crate::error::communication_error(500, format!("failed to read response: {e}"))
        })?;

        if !(200..300).contains(&status) {
            return Err(crate::error::communication_error(
                status,
                format!("openapi call returned status {status}"),
            ));
        }

        let content = deserialize_body(&bytes, &content_type);
        match output {
            "response" => Ok(json!({
                "request": { "method": method_str.as_str(), "uri": url, "headers": headers },
                "headers": headers,
                "statusCode": status,
                "content": content.unwrap_or(Value::Null),
            })),
            "raw" => {
                Ok(json!({ "content": base64::engine::general_purpose::STANDARD.encode(&bytes) }))
            }
            _ => Ok(content.unwrap_or(Value::Null)),
        }
    }
}

/// Finds the (method, path) for an operationId in an OpenAPI 3.x document.
/// Interpolates expressions in an OpenAPI parameters map.
#[cfg(feature = "http")]
fn interpolate_params(
    inner: &Arc<RuntimeInner>,
    params: &Map<String, Value>,
    ctx: &ExpressionContext,
) -> Result<HashMap<String, Value>, WorkflowError> {
    let vars = ctx.as_variable_map();
    let mut out = HashMap::new();
    for (k, v) in params {
        let resolved = match v {
            Value::String(s) => inner
                .expression
                .evaluate_interpolation(s, &ctx.input, &vars)
                .map_err(|e| crate::error::expression_error(e.to_string()))?,
            other => other.clone(),
        };
        out.insert(k.clone(), resolved);
    }
    Ok(out)
}

#[cfg(feature = "http")]
fn find_operation(spec: &Value, operation_id: &str) -> Option<(String, String)> {
    let paths = spec.get("paths")?.as_object()?;
    for (path, item) in paths {
        let item = item.as_object()?;
        for method in ["get", "post", "put", "patch", "delete", "options", "head"] {
            if let Some(op) = item.get(method) {
                if op.get("operationId").and_then(|v| v.as_str()) == Some(operation_id) {
                    return Some((method.to_string(), path.clone()));
                }
            }
        }
    }
    None
}

/// A generic service invoker that does nothing useful (used as a safe default).
#[derive(Debug, Clone, Default)]
pub struct NoopServiceInvoker;

#[async_trait::async_trait]
impl ServiceInvoker for NoopServiceInvoker {
    async fn invoke(&self, _req: &ServiceRequest) -> Result<ServiceResponse, WorkflowError> {
        Err(crate::error::runtime_error("no service invoker configured"))
    }
}

/// A process runner that always errors (used as a safe default; scripts and
/// containers are deny-by-default).
#[derive(Debug, Clone, Default)]
pub struct NoopProcessRunner;

#[async_trait::async_trait]
impl ProcessRunner for NoopProcessRunner {
    async fn run_shell(
        &self,
        _command: &str,
        _args: &[String],
        _env: &HashMap<String, String>,
        _stdin: Option<String>,
    ) -> Result<ProcessResult, WorkflowError> {
        Err(crate::error::policy_error(
            "script execution is disabled by policy",
        ))
    }
    async fn run_script(
        &self,
        _language: &str,
        _code: &str,
        _args: &[String],
        _env: &HashMap<String, String>,
        _stdin: Option<String>,
    ) -> Result<ProcessResult, WorkflowError> {
        Err(crate::error::policy_error(
            "script execution is disabled by policy",
        ))
    }
    async fn run_container(
        &self,
        _image: &str,
        _args: &[String],
        _env: &HashMap<String, String>,
        _stdin: Option<String>,
    ) -> Result<ProcessResult, WorkflowError> {
        Err(crate::error::policy_error(
            "container execution is disabled by policy",
        ))
    }
}

/// The built-in HTTP function invoker.
///
/// It enforces the runtime network policy before making any request. The
/// endpoint URI and its parameters support `${...}` expression interpolation and
/// `{var}` URI templates resolved against `$workflow.input`.
#[cfg(feature = "http")]
pub struct HttpServiceInvoker {
    inner: Arc<RuntimeInner>,
    client: reqwest::Client,
}

#[cfg(feature = "http")]
impl HttpServiceInvoker {
    /// Creates a new HTTP function invoker.
    pub fn new(inner: Arc<RuntimeInner>) -> Self {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        Self { inner, client }
    }
}

#[async_trait::async_trait]
impl FunctionInvoker for HttpServiceInvoker {
    async fn invoke(&self, req: FunctionRequest<'_>) -> Result<Value, WorkflowError> {
        let args = &req.args;

        let method = args
            .get("method")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::error::semantic_error("http call requires a `method`"))?
            .to_uppercase();

        let endpoint = args
            .get("endpoint")
            .ok_or_else(|| crate::error::semantic_error("http call requires an `endpoint`"))?;

        // Resolve endpoint URI.
        let (uri, auth) = resolve_endpoint(endpoint, req.context, &self.inner)?;

        // Enforce network policy.
        enforce_network_policy(&self.inner, &uri)?;

        let mut headers = resolve_headers(args.get("headers"), req.context, &self.inner)?;
        apply_auth(auth, &mut headers, req.context, &self.inner)?;

        let body = resolve_body(args.get("body"), req.context, &self.inner)?;
        let query = resolve_query(args.get("query"), req.context, &self.inner)?;
        let output = args
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("content");
        let redirect = args
            .get("redirect")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Build the request.
        let mut rb = self.client.request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET),
            &uri,
        );

        for (k, v) in &headers {
            rb = rb.header(k, v);
        }
        if let Some(q) = query {
            rb = rb.query(&q);
        }
        let req_builder = if let Some(b) = body {
            if is_json_content_type(&headers) {
                rb.json(&b)
            } else {
                rb.body(serde_json::to_string(&b).unwrap_or_default())
            }
        } else {
            rb
        };

        let response = match req_builder.send().await {
            Ok(r) => r,
            Err(e) => {
                return Err(crate::error::communication_error(
                    500,
                    format!("http request failed: {e}"),
                ));
            }
        };

        let status = response.status().as_u16();
        let response_headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let content_type = response_headers
            .get("content-type")
            .cloned()
            .unwrap_or_default();

        let body_bytes = response.bytes().await.map_err(|e| {
            crate::error::communication_error(500, format!("failed to read response: {e}"))
        })?;

        // Status handling.
        let ok = if redirect {
            (200..400).contains(&status)
        } else {
            (200..300).contains(&status)
        };
        if !ok {
            return Err(crate::error::communication_error(
                status,
                format!("http request returned status {status}"),
            ));
        }

        let content = deserialize_body(&body_bytes, &content_type);

        match output {
            "raw" => Ok(
                json!({ "content": base64::engine::general_purpose::STANDARD.encode(&body_bytes) }),
            ),
            "response" => Ok(json!({
                "request": {
                    "method": method,
                    "uri": uri,
                    "headers": headers,
                },
                "headers": response_headers,
                "statusCode": status,
                "content": content.unwrap_or(Value::Null),
            })),
            _ => Ok(content.unwrap_or(Value::Null)),
        }
    }
}

#[cfg(feature = "http")]
fn is_json_content_type(headers: &HashMap<String, String>) -> bool {
    headers
        .get("content-type")
        .map(|ct| ct.contains("json"))
        .unwrap_or(false)
}

#[cfg(feature = "http")]
fn deserialize_body(bytes: &[u8], content_type: &str) -> Option<Value> {
    if content_type.contains("json") {
        serde_json::from_slice(bytes).ok()
    } else {
        String::from_utf8(bytes.to_vec()).ok().map(Value::String)
    }
}

#[cfg(feature = "http")]
fn resolve_endpoint(
    endpoint: &Value,
    ctx: &ExpressionContext,
    inner: &Arc<RuntimeInner>,
) -> Result<(String, Option<Value>), WorkflowError> {
    let (uri_raw, auth) = match endpoint {
        Value::String(s) => (s.clone(), None),
        Value::Object(map) => {
            let uri = map
                .get("uri")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::error::semantic_error("endpoint object requires a `uri`"))?
                .to_string();
            let auth = map.get("authentication").cloned();
            (uri, auth)
        }
        _ => {
            return Err(crate::error::semantic_error(
                "http endpoint must be a string or an object",
            ))
        }
    };

    // Expand `${...}` interpolation and `{var}` URI templates.
    let vars = ctx.as_variable_map();
    let interpolated = inner
        .expression
        .evaluate_interpolation(&uri_raw, &ctx.input, &vars)
        .map_err(|e| crate::error::expression_error(e.to_string()))?;
    let uri = interpolated.as_str().unwrap_or(&uri_raw).to_string();
    let uri = expand_uri_template(&uri, &ctx.workflow.clone().unwrap_or(Value::Null));
    Ok((uri, auth))
}

#[cfg(feature = "http")]
fn expand_uri_template(uri: &str, workflow: &Value) -> String {
    // Replace {var} with workflow input values.
    let input = workflow.get("input");
    let mut out = uri.to_string();
    let chars: Vec<char> = uri.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            if let Some(close) = chars[i + 1..].iter().position(|c| *c == '}') {
                let name: String = chars[i + 1..i + 1 + close].iter().collect();
                let value = input.and_then(|inp| inp.get(&name));
                if let Some(v) = value {
                    let replacement = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    out = out.replace(&format!("{{{name}}}"), &replacement);
                }
                i += close + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[cfg(feature = "http")]
fn resolve_headers(
    headers: Option<&Value>,
    ctx: &ExpressionContext,
    inner: &Arc<RuntimeInner>,
) -> Result<HashMap<String, String>, WorkflowError> {
    let mut out = HashMap::new();
    if let Some(Value::Object(map)) = headers {
        for (k, v) in map {
            let resolved = crate::engine::eval_interpolation(inner, v, ctx)?;
            let value = resolved
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| resolved.to_string());
            out.insert(k.clone(), value);
        }
    }
    Ok(out)
}

#[cfg(feature = "http")]
fn resolve_query(
    query: Option<&Value>,
    ctx: &ExpressionContext,
    inner: &Arc<RuntimeInner>,
) -> Result<Option<Vec<(String, String)>>, WorkflowError> {
    let Some(Value::Object(map)) = query else {
        return Ok(None);
    };
    let mut out = Vec::new();
    for (k, v) in map {
        let resolved = crate::engine::eval_interpolation(inner, v, ctx)?;
        let value = resolved
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| resolved.to_string());
        out.push((k.clone(), value));
    }
    Ok(Some(out))
}

#[cfg(feature = "http")]
fn resolve_body(
    body: Option<&Value>,
    ctx: &ExpressionContext,
    inner: &Arc<RuntimeInner>,
) -> Result<Option<Value>, WorkflowError> {
    let Some(body) = body else {
        return Ok(None);
    };
    // Interpolate expressions recursively within the body.
    let resolved = interpolate_value(inner, body, ctx)?;
    Ok(Some(resolved))
}

#[cfg(feature = "http")]
fn interpolate_value(
    inner: &Arc<RuntimeInner>,
    value: &Value,
    ctx: &ExpressionContext,
) -> Result<Value, WorkflowError> {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), interpolate_value(inner, v, ctx)?);
            }
            Ok(Value::Object(out))
        }
        Value::Array(items) => {
            let mut out = Vec::new();
            for item in items {
                out.push(interpolate_value(inner, item, ctx)?);
            }
            Ok(Value::Array(out))
        }
        Value::String(_) => crate::engine::eval_interpolation(inner, value, ctx),
        other => Ok(other.clone()),
    }
}

#[cfg(feature = "http")]
fn apply_auth(
    auth: Option<Value>,
    headers: &mut HashMap<String, String>,
    ctx: &ExpressionContext,
    inner: &Arc<RuntimeInner>,
) -> Result<(), WorkflowError> {
    let Some(Value::Object(map)) = auth else {
        return Ok(());
    };
    if let Some(Value::Object(basic)) = map.get("basic") {
        let username = resolve_optional_str(basic.get("username"), ctx, inner)?;
        let password = resolve_optional_str(basic.get("password"), ctx, inner)?;
        let token =
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
        headers.insert("authorization".to_string(), format!("Basic {token}"));
    } else if let Some(Value::Object(bearer)) = map.get("bearer") {
        let token = resolve_optional_str(bearer.get("token"), ctx, inner)?;
        headers.insert("authorization".to_string(), format!("Bearer {token}"));
    }
    Ok(())
}

#[cfg(feature = "http")]
fn resolve_optional_str(
    v: Option<&Value>,
    ctx: &ExpressionContext,
    inner: &Arc<RuntimeInner>,
) -> Result<String, WorkflowError> {
    match v {
        None => Ok(String::new()),
        Some(v) => {
            let resolved = crate::engine::eval_interpolation(inner, v, ctx)?;
            Ok(resolved
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| resolved.to_string()))
        }
    }
}

#[cfg(feature = "http")]
pub(crate) fn enforce_network_policy(
    inner: &Arc<RuntimeInner>,
    uri: &str,
) -> Result<(), WorkflowError> {
    let policy = &inner.policy;
    let parsed = url::Url::parse(uri)
        .map_err(|e| crate::error::semantic_error(format!("invalid endpoint uri `{uri}`: {e}")))?;
    if !policy.scheme_allowed(parsed.scheme()) {
        return Err(crate::error::policy_error(format!(
            "network access to scheme `{}` is denied by policy",
            parsed.scheme()
        )));
    }
    let host = parsed.host_str().unwrap_or("");
    if !policy.host_allowed(host) {
        return Err(crate::error::policy_error(format!(
            "network access to host `{host}` is denied by policy"
        )));
    }
    Ok(())
}

/// Performs an HTTP request and returns the raw JSON response body.
#[cfg(feature = "http")]
async fn send_json_request(
    client: &reqwest::Client,
    method: reqwest::Method,
    uri: &str,
    headers: &HashMap<String, String>,
    body: Option<&Value>,
) -> Result<(u16, HashMap<String, String>, String, Vec<u8>), WorkflowError> {
    let mut rb = client.request(method, uri);
    for (k, v) in headers {
        rb = rb.header(k, v);
    }
    if let Some(b) = body {
        rb = rb.json(b);
    }
    let response = rb.send().await.map_err(|e| {
        crate::error::communication_error(500, format!("request to `{uri}` failed: {e}"))
    })?;
    let status = response.status().as_u16();
    let response_headers: HashMap<String, String> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let content_type = response_headers
        .get("content-type")
        .cloned()
        .unwrap_or_default();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| {
            crate::error::communication_error(500, format!("failed to read response: {e}"))
        })?
        .to_vec();
    Ok((status, response_headers, content_type, bytes))
}

/// Parses an HTTP response into a structured JSON value following the same
/// `output` modes (`content` / `response`) used by the HTTP function.
#[cfg(feature = "http")]
#[allow(clippy::too_many_arguments)]
fn shape_response(
    uri: &str,
    method: &str,
    req_headers: &HashMap<String, String>,
    status: u16,
    resp_headers: HashMap<String, String>,
    content_type: String,
    bytes: &[u8],
    output: &str,
) -> Result<Value, WorkflowError> {
    let content = deserialize_body(bytes, &content_type);
    match output {
        "response" => Ok(json!({
            "request": { "method": method, "uri": uri, "headers": req_headers },
            "headers": resp_headers,
            "statusCode": status,
            "content": content.unwrap_or(Value::Null),
        })),
        _ => Ok(content.unwrap_or(Value::Null)),
    }
}

/// The MCP (Model Context Protocol) call function adapter.
///
/// It invokes a tool on an MCP server over JSON-RPC (streamable HTTP). Arguments
/// are `endpoint`, `tool`, `arguments` and an optional `output` mode. The MCP
/// server's structured content is returned.
#[cfg(feature = "http")]
pub struct McpInvoker {
    inner: Arc<RuntimeInner>,
    client: reqwest::Client,
}

#[cfg(feature = "http")]
impl McpInvoker {
    /// Creates a new MCP function invoker.
    pub fn new(inner: Arc<RuntimeInner>) -> Self {
        let client = reqwest::Client::builder().build().unwrap_or_default();
        Self { inner, client }
    }
}

#[cfg(feature = "http")]
#[async_trait::async_trait]
impl FunctionInvoker for McpInvoker {
    async fn invoke(&self, req: FunctionRequest<'_>) -> Result<Value, WorkflowError> {
        let args = &req.args;
        let tool = args
            .get("tool")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::error::semantic_error("mcp call requires a `tool`"))?
            .to_string();
        let arguments = args
            .get("arguments")
            .cloned()
            .unwrap_or(Value::Object(Map::new()));

        let endpoint = args
            .get("endpoint")
            .ok_or_else(|| crate::error::semantic_error("mcp call requires an `endpoint`"))?;
        let (uri, auth) = resolve_endpoint(endpoint, req.context, &self.inner)?;
        enforce_network_policy(&self.inner, &uri)?;

        let mut headers = resolve_headers(args.get("headers"), req.context, &self.inner)?;
        headers.insert("content-type".into(), "application/json".into());
        apply_auth(auth, &mut headers, req.context, &self.inner)?;

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments },
        });

        let (status, resp_headers, content_type, bytes) = send_json_request(
            &self.client,
            reqwest::Method::POST,
            &uri,
            &headers,
            Some(&request),
        )
        .await?;

        if !(200..300).contains(&status) {
            return Err(crate::error::communication_error(
                status,
                format!("mcp call returned status {status}"),
            ));
        }

        let envelope: Value = serde_json::from_slice(&bytes).map_err(|e| {
            crate::error::communication_error(500, format!("invalid mcp json-rpc response: {e}"))
        })?;
        // Surface JSON-RPC errors.
        if let Some(err) = envelope.get("error") {
            if !err.is_null() {
                return Err(crate::error::communication_error(
                    status,
                    format!("mcp json-rpc error: {err}"),
                ));
            }
        }
        let result = envelope.get("result").cloned().unwrap_or(Value::Null);

        let output = args
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("content");
        if output == "response" {
            return shape_response(
                &uri,
                "post",
                &headers,
                status,
                resp_headers,
                content_type,
                &bytes,
                "response",
            );
        }
        // By default return the joined text content of the tool result, falling
        // back to the full result object.
        Ok(result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|items| {
                let text: Vec<String> = items
                    .iter()
                    .filter_map(|i| {
                        if i.get("type").and_then(|t| t.as_str()) == Some("text") {
                            i.get("text").and_then(|t| t.as_str()).map(String::from)
                        } else {
                            None
                        }
                    })
                    .collect();
                if text.is_empty() {
                    result.clone()
                } else {
                    Value::String(text.join("\n"))
                }
            })
            .unwrap_or(result))
    }
}

/// The A2A (Agent2Agent) call function adapter.
///
/// It sends a JSON-RPC request to an agent over HTTP. Arguments are `endpoint`,
/// `method` (defaults to `message/send`), `params` and an optional `output`.
#[cfg(feature = "http")]
pub struct A2aInvoker {
    inner: Arc<RuntimeInner>,
    client: reqwest::Client,
}

#[cfg(feature = "http")]
impl A2aInvoker {
    /// Creates a new A2A function invoker.
    pub fn new(inner: Arc<RuntimeInner>) -> Self {
        let client = reqwest::Client::builder().build().unwrap_or_default();
        Self { inner, client }
    }
}

#[cfg(feature = "http")]
#[async_trait::async_trait]
impl FunctionInvoker for A2aInvoker {
    async fn invoke(&self, req: FunctionRequest<'_>) -> Result<Value, WorkflowError> {
        let args = &req.args;
        let endpoint = args
            .get("endpoint")
            .ok_or_else(|| crate::error::semantic_error("a2a call requires an `endpoint`"))?;
        let (uri, auth) = resolve_endpoint(endpoint, req.context, &self.inner)?;
        enforce_network_policy(&self.inner, &uri)?;

        let method = args
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("message/send");
        let params = args
            .get("params")
            .cloned()
            .or_else(|| args.get("message").cloned())
            .unwrap_or(Value::Object(Map::new()));

        let mut headers = resolve_headers(args.get("headers"), req.context, &self.inner)?;
        headers.insert("content-type".into(), "application/json".into());
        apply_auth(auth, &mut headers, req.context, &self.inner)?;

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let (status, resp_headers, content_type, bytes) = send_json_request(
            &self.client,
            reqwest::Method::POST,
            &uri,
            &headers,
            Some(&request),
        )
        .await?;

        if !(200..300).contains(&status) {
            return Err(crate::error::communication_error(
                status,
                format!("a2a call returned status {status}"),
            ));
        }

        let envelope: Value = serde_json::from_slice(&bytes).map_err(|e| {
            crate::error::communication_error(500, format!("invalid a2a json-rpc response: {e}"))
        })?;
        if let Some(err) = envelope.get("error") {
            if !err.is_null() {
                return Err(crate::error::communication_error(
                    status,
                    format!("a2a json-rpc error: {err}"),
                ));
            }
        }
        let result = envelope.get("result").cloned().unwrap_or(Value::Null);

        let output = args
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("content");
        if output == "response" {
            return shape_response(
                &uri,
                "post",
                &headers,
                status,
                resp_headers,
                content_type,
                &bytes,
                "response",
            );
        }
        Ok(result)
    }
}

/// The AsyncAPI call function adapter.
///
/// AsyncAPI describes message-driven channels and their operations. This adapter
/// resolves a `publish`/`subscribe` operation on a channel from an AsyncAPI
/// document and publishes a message. Arguments are `document` (an endpoint URI
/// or inline object), an `operationId` or `channel` name, `message`, and an
/// optional HTTP `endpoint` override for the target server.
#[cfg(feature = "http")]
pub struct AsyncApiInvoker {
    inner: Arc<RuntimeInner>,
    client: reqwest::Client,
}

#[cfg(feature = "http")]
impl AsyncApiInvoker {
    /// Creates a new AsyncAPI function invoker.
    pub fn new(inner: Arc<RuntimeInner>) -> Self {
        let client = reqwest::Client::builder().build().unwrap_or_default();
        Self { inner, client }
    }

    /// Fetches or returns the inline AsyncAPI document value.
    async fn document(&self, endpoint: &Value) -> Result<Value, WorkflowError> {
        match endpoint {
            Value::String(uri) => {
                enforce_network_policy(&self.inner, uri)?;
                self.client
                    .get(uri)
                    .send()
                    .await
                    .map_err(|e| {
                        crate::error::communication_error(
                            500,
                            format!("failed to fetch asyncapi document: {e}"),
                        )
                    })?
                    .json()
                    .await
                    .map_err(|e| {
                        crate::error::communication_error(
                            500,
                            format!("failed to parse asyncapi document: {e}"),
                        )
                    })
            }
            other => Ok(other.clone()),
        }
    }

    /// Publishes to or consumes from a message broker using the runtime's
    /// `EventPublisher`/`EventConsumer` (broker-based AsyncAPI transport).
    async fn invoke_broker(
        &self,
        target: &AsyncApiTarget<'_>,
        args: &HashMap<String, Value>,
        ctx: &ExpressionContext,
    ) -> Result<Value, WorkflowError> {
        let type_ = args
            .get("type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| target.channel.clone());
        let message = args
            .get("message")
            .cloned()
            .unwrap_or(Value::Object(Map::new()));

        if target.kind == "publish" {
            let mut event = ows_runtime_core::EventMessage::new(
                self.inner.uuid.new_v4().to_string(),
                target.channel.clone(),
                type_,
            );
            event.data = Some(message);
            self.inner.publisher.publish(&event).await?;
            return Ok(json!({
                "channel": target.channel,
                "type": event.type_,
                "published": true,
            }));
        }

        // Subscribe: optionally filter events with the `subscription.filter`
        // runtime expression, then consume `count` events (default 1).
        let filter_source = args
            .get("subscription")
            .and_then(|s| s.get("filter"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let compiled = filter_source
            .map(|src| self.inner.expression.compile(&src))
            .transpose()
            .map_err(|e| {
                WorkflowError::new(
                    ErrorKind::Expression,
                    ProblemDetails::standard(StandardErrorType::Expression)
                        .with_detail(e.to_string()),
                )
            })?;
        let vars = ctx.as_variable_map();
        let expression = self.inner.expression.clone();
        let vars_for_filter = vars.clone();

        let subscription = self
            .inner
            .consumer
            .subscribe(Box::new(
                move |event: &ows_runtime_core::EventMessage| match &compiled {
                    None => true,
                    Some(compiled) => {
                        let input = event.data.clone().unwrap_or(Value::Null);
                        expression
                            .evaluate(compiled, &input, &vars_for_filter)
                            .map(|v| !matches!(v, Value::Bool(false) | Value::Null))
                            .unwrap_or(false)
                    }
                },
            ))
            .await?;

        let count = args
            .get("subscription")
            .and_then(|s| s.get("consume"))
            .and_then(|c| c.get("count"))
            .and_then(|v| v.as_u64())
            .or_else(|| args.get("count").and_then(|v| v.as_u64()))
            .unwrap_or(1)
            .max(1) as usize;

        let read = args.get("read").and_then(|v| v.as_str()).unwrap_or("data");

        let mut consumed = Vec::new();
        let mut envelopes = Vec::new();
        for _ in 0..count {
            match subscription.recv().await {
                Some(event) => {
                    envelopes.push(
                        serde_json::to_value(ows_runtime_events::CloudEvent::from_message(&event))
                            .unwrap_or(Value::Null),
                    );
                    consumed.push(event.data.clone().unwrap_or(Value::Null));
                }
                None => break,
            }
        }

        let values = if read == "envelope" {
            envelopes
        } else {
            consumed
        };
        if count == 1 {
            Ok(values.into_iter().next().unwrap_or(Value::Null))
        } else {
            Ok(Value::Array(values))
        }
    }
}

#[cfg(feature = "http")]
#[async_trait::async_trait]
impl FunctionInvoker for AsyncApiInvoker {
    async fn invoke(&self, req: FunctionRequest<'_>) -> Result<Value, WorkflowError> {
        let args = &req.args;

        let message = args
            .get("message")
            .cloned()
            .unwrap_or(Value::Object(Map::new()));

        let doc_value = args
            .get("document")
            .or_else(|| args.get("spec"))
            .ok_or_else(|| crate::error::semantic_error("asyncapi call requires a `document`"))?;
        let document = match doc_value {
            Value::Object(m) if m.contains_key("endpoint") => self.document(&m["endpoint"]).await?,
            other => self.document(other).await?,
        };

        // Locate the channel operation by operationId or channel name.
        let operation_id = args
            .get("operationId")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let channel_name = args.get("channel").and_then(|v| v.as_str()).unwrap_or("");

        let channels = document
            .get("channels")
            .and_then(|c| c.as_object())
            .ok_or_else(|| crate::error::semantic_error("asyncapi document has no channels"))?;

        let target = resolve_asyncapi_operation(channels, operation_id, channel_name)?;

        // Broker-based transport: when the call declares a broker transport (or
        // no HTTP server is available), publish/consume through the runtime's
        // `EventPublisher`/`EventConsumer` instead of an HTTP channel binding.
        let wants_broker = args
            .get("transport")
            .and_then(|t| t.get("broker"))
            .is_some()
            || args.get("broker").is_some();
        if wants_broker {
            return self.invoke_broker(&target, args, req.context).await;
        }

        // Determine the endpoint: explicit override, the channel's server url
        // (from the document origin) or the operation's HTTP server binding.
        let endpoint_override = args.get("endpoint").cloned();
        let (uri, _auth) = match endpoint_override {
            Some(Value::Object(m)) if m.contains_key("uri") => (
                m["uri"].as_str().unwrap_or_default().to_string(),
                None::<Value>,
            ),
            Some(Value::String(s)) => (s, None::<Value>),
            _ => (String::new(), None::<Value>),
        };
        let mut uri = uri;
        if uri.is_empty() {
            if let Some(base) = document.get("servers").and_then(|s| s.as_object()) {
                // Prefer the first http/https server url for the channel.
                let url = base.values().find_map(|v| {
                    v.get("url")
                        .and_then(|u| u.as_str())
                        .filter(|u| u.starts_with("http"))
                        .map(|u| u.to_string())
                });
                if let Some(u) = url {
                    uri = u.trim_end_matches('/').to_string() + target.path;
                }
            }
        }
        if uri.is_empty() {
            // Fall back to a relative publish path derived from the channel.
            uri = format!("{}{}", target.channel.trim_start_matches('/'), "/");
        }

        enforce_network_policy(&self.inner, &uri)?;
        let mut headers = resolve_headers(args.get("headers"), req.context, &self.inner)?;
        headers.insert("content-type".into(), "application/json".into());

        let (status, resp_headers, content_type, bytes) = send_json_request(
            &self.client,
            reqwest::Method::POST,
            &uri,
            &headers,
            Some(&message),
        )
        .await?;

        if !(200..300).contains(&status) {
            return Err(crate::error::communication_error(
                status,
                format!("asyncapi publish returned status {status}"),
            ));
        }

        let output = args
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("content");
        shape_response(
            &uri,
            "post",
            &headers,
            status,
            resp_headers,
            content_type,
            &bytes,
            output,
        )
    }
}

/// A resolved AsyncAPI operation.
#[cfg(feature = "http")]
struct AsyncApiTarget<'a> {
    channel: String,
    path: &'a str,
    kind: &'a str,
}

/// Resolves a channel operation (by operationId or by channel name) within an
/// AsyncAPI channels map.
#[cfg(feature = "http")]
fn resolve_asyncapi_operation<'a>(
    channels: &'a Map<String, Value>,
    operation_id: &str,
    channel_name: &str,
) -> Result<AsyncApiTarget<'a>, WorkflowError> {
    for (name, chan) in channels {
        if !channel_name.is_empty() && name != channel_name {
            continue;
        }
        for op_kind in ["publish", "subscribe"] {
            if let Some(op) = chan.get(op_kind) {
                let op_id = op.get("operationId").and_then(|v| v.as_str()).unwrap_or("");
                if !operation_id.is_empty() && op_id != operation_id {
                    continue;
                }
                let path = chan
                    .get("bindings")
                    .and_then(|b| b.get("http"))
                    .and_then(|h| h.get("path"))
                    .and_then(|p| p.as_str())
                    .unwrap_or_default();
                return Ok(AsyncApiTarget {
                    channel: name.clone(),
                    path,
                    kind: op_kind,
                });
            }
        }
    }
    Err(crate::error::semantic_error(
        "asyncapi operation not found in document",
    ))
}

/// The gRPC call function adapter.
///
/// The runtime does not embed protobuf code generation, so this adapter targets
/// gRPC services that expose a JSON transcoding/HTTP gateway endpoint (the
/// `grpc-gateway` pattern). Arguments are an `endpoint`, the fully-qualified
/// `service`/`method` used to build the transcoding path, and the JSON `message`.
#[cfg(feature = "http")]
pub struct GrpcInvoker {
    inner: Arc<RuntimeInner>,
    client: reqwest::Client,
}

#[cfg(feature = "http")]
impl GrpcInvoker {
    /// Creates a new gRPC function invoker.
    pub fn new(inner: Arc<RuntimeInner>) -> Self {
        let client = reqwest::Client::builder().build().unwrap_or_default();
        Self { inner, client }
    }
}

#[cfg(feature = "http")]
#[async_trait::async_trait]
impl FunctionInvoker for GrpcInvoker {
    async fn invoke(&self, req: FunctionRequest<'_>) -> Result<Value, WorkflowError> {
        let args = &req.args;
        let endpoint = args
            .get("endpoint")
            .ok_or_else(|| crate::error::semantic_error("grpc call requires an `endpoint`"))?;
        let (uri, auth) = resolve_endpoint(endpoint, req.context, &self.inner)?;
        enforce_network_policy(&self.inner, &uri)?;

        let message = args.get("message").cloned().unwrap_or(Value::Null);

        let mut headers = resolve_headers(args.get("headers"), req.context, &self.inner)?;
        headers.insert("content-type".into(), "application/json".into());
        apply_auth(auth, &mut headers, req.context, &self.inner)?;

        let (status, resp_headers, content_type, bytes) = send_json_request(
            &self.client,
            reqwest::Method::POST,
            &uri,
            &headers,
            Some(&message),
        )
        .await?;

        if !(200..300).contains(&status) {
            return Err(crate::error::communication_error(
                status,
                format!("grpc call returned status {status}"),
            ));
        }

        let output = args
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("content");
        shape_response(
            &uri,
            "post",
            &headers,
            status,
            resp_headers,
            content_type,
            &bytes,
            output,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ows_runtime_core::{ExpressionContext, RuntimePolicy};

    #[test]
    fn expand_uri_template_resolves_input() {
        let wf = json!({"input": {"petId": 7}});
        assert_eq!(
            expand_uri_template("https://x/pet/{petId}", &wf),
            "https://x/pet/7"
        );
        assert_eq!(
            expand_uri_template("https://x/no/{missing}", &wf),
            "https://x/no/{missing}"
        );
    }

    #[test]
    fn deserialize_body_json_and_text() {
        let json = deserialize_body(b"{\"a\":1}", "application/json");
        assert_eq!(json, Some(json!({"a":1})));
        let text = deserialize_body(b"hello", "text/plain");
        assert_eq!(text, Some(json!("hello")));
    }

    #[test]
    fn is_json_content_type_check() {
        let mut h = HashMap::new();
        h.insert("content-type".to_string(), "application/json".to_string());
        assert!(is_json_content_type(&h));
        let mut h2 = HashMap::new();
        h2.insert("content-type".to_string(), "text/plain".to_string());
        assert!(!is_json_content_type(&h2));
    }

    #[test]
    fn find_operation_resolves() {
        let spec = json!({
            "paths": {
                "/pet/{id}": {
                    "get": { "operationId": "getPet" },
                    "post": { "operationId": "addPet" }
                }
            }
        });
        assert_eq!(
            find_operation(&spec, "getPet"),
            Some(("get".into(), "/pet/{id}".into()))
        );
        assert_eq!(
            find_operation(&spec, "addPet"),
            Some(("post".into(), "/pet/{id}".into()))
        );
        assert_eq!(find_operation(&spec, "missing"), None);
    }

    #[test]
    fn interpolate_params_strings() {
        let rt = crate::Runtime::builder().build().unwrap();
        let inner = rt.inner.clone();
        let ctx = ExpressionContext {
            input: json!({"status": "available"}),
            ..Default::default()
        };
        let mut params = Map::new();
        params.insert("status".to_string(), json!("${ .status }"));
        let out = interpolate_params(&inner, &params, &ctx).unwrap();
        assert_eq!(out["status"], json!("available"));
        params.insert("n".to_string(), json!(42));
        let out = interpolate_params(&inner, &params, &ctx).unwrap();
        assert_eq!(out["n"], json!(42));
    }

    #[test]
    fn network_policy_deny_by_default() {
        let rt = crate::Runtime::builder().build().unwrap();
        let inner = rt.inner.clone();
        let err = enforce_network_policy(&inner, "https://example.com").unwrap_err();
        assert_eq!(err.kind, ows_runtime_core::ErrorKind::Policy);
    }

    #[test]
    fn network_policy_allows_when_enabled() {
        let rt = crate::Runtime::builder()
            .with_policy(RuntimePolicy {
                allow_network: true,
                ..Default::default()
            })
            .build()
            .unwrap();
        enforce_network_policy(&rt.inner, "https://example.com").unwrap();
    }

    #[test]
    fn apply_auth_basic_and_bearer() {
        let rt = crate::Runtime::builder().build().unwrap();
        let inner = rt.inner.clone();
        let ctx = ExpressionContext {
            input: json!({"u":"user","p":"pass"}),
            ..Default::default()
        };
        let mut headers = HashMap::new();
        apply_auth(
            Some(json!({"basic": {"username": "${ .u }", "password": "${ .p }"}})),
            &mut headers,
            &ctx,
            &inner,
        )
        .unwrap();
        assert!(headers["authorization"].starts_with("Basic "));
        let mut headers = HashMap::new();
        apply_auth(
            Some(json!({"bearer": {"token": "tok"}})),
            &mut headers,
            &ctx,
            &inner,
        )
        .unwrap();
        assert_eq!(headers["authorization"], "Bearer tok");
    }

    #[test]
    fn resolve_endpoint_uri_and_auth() {
        let rt = crate::Runtime::builder().build().unwrap();
        let inner = rt.inner.clone();
        let ctx = ExpressionContext {
            input: json!({}),
            workflow: Some(json!({"input": {"id": 5}})),
            ..Default::default()
        };
        // String endpoint.
        let (uri, auth) = resolve_endpoint(&json!("https://x/{id}"), &ctx, &inner).unwrap();
        assert_eq!(uri, "https://x/5");
        assert!(auth.is_none());
        // Object endpoint with uri + authentication.
        let obj = json!({"uri": "https://x/y", "authentication": {"basic": {"username":"u","password":"p"}}});
        let (uri, auth) = resolve_endpoint(&obj, &ctx, &inner).unwrap();
        assert_eq!(uri, "https://x/y");
        assert!(auth.is_some());
    }

    #[test]
    fn resolve_headers_query_body() {
        let rt = crate::Runtime::builder().build().unwrap();
        let inner = rt.inner.clone();
        let ctx = ExpressionContext {
            input: json!({"name":"Bob"}),
            ..Default::default()
        };
        let headers =
            resolve_headers(Some(&json!({"X-Name": "${ .name }"})), &ctx, &inner).unwrap();
        assert_eq!(headers["X-Name"], "Bob");
        let q = resolve_query(Some(&json!({"q": "${ .name }"})), &ctx, &inner).unwrap();
        assert_eq!(q.unwrap()[0], ("q".to_string(), "Bob".to_string()));
        let body = resolve_body(
            Some(&json!({"greeting": "${ \"Hi \" + .name }"})),
            &ctx,
            &inner,
        )
        .unwrap();
        assert_eq!(body.unwrap(), json!({"greeting": "Hi Bob"}));
    }

    #[test]
    fn resolve_optional_str_and_missing() {
        let rt = crate::Runtime::builder().build().unwrap();
        let inner = rt.inner.clone();
        let ctx = ExpressionContext {
            input: json!({"u":"u1"}),
            ..Default::default()
        };
        let s = resolve_optional_str(Some(&json!("${ .u }")), &ctx, &inner).unwrap();
        assert_eq!(s, "u1");
        let s = resolve_optional_str(None, &ctx, &inner).unwrap();
        assert_eq!(s, "");
    }

    #[test]
    fn resolve_endpoint_invalid_type_errors() {
        let rt = crate::Runtime::builder().build().unwrap();
        let inner = rt.inner.clone();
        let ctx = ExpressionContext::default();
        assert!(resolve_endpoint(&json!(42), &ctx, &inner).is_err());
    }

    #[test]
    fn is_json_content_type_missing_header() {
        let h: HashMap<String, String> = HashMap::new();
        assert!(!is_json_content_type(&h));
    }

    #[test]
    fn enforce_network_policy_invalid_uri() {
        let rt = crate::Runtime::builder().build().unwrap();
        let inner = rt.inner.clone();
        assert!(enforce_network_policy(&inner, "not a url").is_err());
    }

    #[cfg(feature = "http")]
    #[test]
    fn shape_response_modes() {
        let mut resp = HashMap::new();
        resp.insert("content-type".into(), "application/json".into());
        // "response" output mode returns the full envelope.
        let out = shape_response(
            "http://x/y",
            "post",
            &HashMap::new(),
            200,
            resp.clone(),
            "application/json".into(),
            br#"{"a":1}"#,
            "response",
        )
        .unwrap();
        assert_eq!(out["statusCode"], 200);
        assert_eq!(out["content"], json!({"a": 1}));
        assert_eq!(out["request"]["uri"], "http://x/y");
        // Default "content" mode returns just the parsed body.
        let content = shape_response(
            "http://x/y",
            "post",
            &HashMap::new(),
            200,
            resp,
            "application/json".into(),
            br#"{"a":1}"#,
            "content",
        )
        .unwrap();
        assert_eq!(content, json!({"a": 1}));
    }

    #[cfg(feature = "http")]
    #[test]
    fn resolve_asyncapi_operation_found_and_missing() {
        let doc = json!({
            "user/signedup": { "publish": { "operationId": "onUserSignedUp" } }
        });
        let channels = doc.as_object().unwrap();
        let found = resolve_asyncapi_operation(channels, "onUserSignedUp", "").unwrap();
        assert_eq!(found.channel, "user/signedup");
        // By channel name.
        let found = resolve_asyncapi_operation(channels, "", "user/signedup").unwrap();
        assert_eq!(found.channel, "user/signedup");
        // Unknown operation -> error.
        assert!(resolve_asyncapi_operation(channels, "nope", "").is_err());
        // Unknown channel -> error.
        assert!(resolve_asyncapi_operation(channels, "", "missing").is_err());
    }
}
