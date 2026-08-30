//! Service invocation: the `FunctionInvoker` abstraction, the built-in HTTP
//! function, and no-op adapters.

use std::collections::HashMap;
use std::sync::Arc;

#[cfg(feature = "http")]
use base64::Engine as _;
use ows_runtime_core::{
    ExpressionContext, ProcessResult, ProcessRunner, ServiceInvoker, ServiceRequest,
    ServiceResponse, WorkflowError,
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
fn enforce_network_policy(inner: &Arc<RuntimeInner>, uri: &str) -> Result<(), WorkflowError> {
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
