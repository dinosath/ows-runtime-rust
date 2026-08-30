//! The [`Runtime`] and its builder, plus the shared [`RuntimeInner`] services.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ows_runtime_core::{
    Clock, EventConsumer, EventPublisher, ExecutionStore, InMemoryExecutionStore, ProcessRunner,
    RandomGenerator, RuntimeInfo, RuntimePolicy, Scheduler, ServiceInvoker, SystemClock,
    UuidGenerator, WorkflowError,
};
use serverless_workflow_core::models::workflow::WorkflowDefinition;
use tokio::sync::{oneshot, Notify};

use crate::compile;
use crate::engine;
use crate::ir::CompiledWorkflow;
use crate::service::{FunctionInvoker, NoopProcessRunner, NoopServiceInvoker};
#[cfg(feature = "http")]
use crate::service::{HttpServiceInvoker, OpenApiInvoker};

/// Shared services backing all executions of a [`Runtime`].
pub struct RuntimeInner {
    pub expression: Arc<dyn ows_runtime_core::ExpressionEngine>,
    pub clock: Arc<dyn Clock>,
    pub uuid: Arc<dyn UuidGenerator>,
    pub random: Arc<dyn RandomGenerator>,
    pub runtime_info: RuntimeInfo,
    pub policy: RuntimePolicy,
    pub publisher: Arc<dyn EventPublisher>,
    pub service: Arc<dyn ServiceInvoker>,
    pub functions: Arc<std::sync::RwLock<HashMap<String, Arc<dyn FunctionInvoker>>>>,
    pub process: Arc<dyn ProcessRunner>,
    pub consumer: Arc<dyn EventConsumer>,
    pub store: Arc<dyn ExecutionStore>,
    pub scheduler: Arc<dyn Scheduler>,
    pub workflows: Arc<std::sync::Mutex<HashMap<String, Arc<CompiledWorkflow>>>>,
    /// A log of task names in execution order (used for ordering assertions).
    pub task_order: Arc<std::sync::Mutex<Vec<String>>>,
    #[cfg(feature = "validation")]
    validator: Option<Arc<JsonSchemaValidator>>,
}

/// A JSON Schema validator backed by the `jsonschema` crate.
#[cfg(feature = "validation")]
pub(crate) struct JsonSchemaValidator;

#[cfg(feature = "validation")]
impl JsonSchemaValidator {
    pub(crate) fn validate(
        &self,
        document: &serde_json::Value,
        value: &serde_json::Value,
    ) -> Result<(), Vec<String>> {
        let compiled = match jsonschema::JSONSchema::compile(document) {
            Ok(c) => c,
            Err(e) => return Err(vec![format!("invalid schema: {e}")]),
        };
        let result = compiled.validate(value);
        match result {
            Ok(()) => Ok(()),
            Err(errors) => {
                let messages: Vec<String> = errors.map(|e| e.to_string()).collect();
                Err(messages)
            }
        }
    }
}

impl RuntimeInner {
    /// The lifecycle event emitter.
    pub fn lifecycle(&self) -> ows_runtime_observability::LifecycleEmitter {
        ows_runtime_observability::LifecycleEmitter::new(self.publisher.clone())
    }

    /// Validates a value against a JSON schema document.
    pub fn validate(
        &self,
        document: &serde_json::Value,
        value: &serde_json::Value,
    ) -> Result<(), Vec<String>> {
        #[cfg(feature = "validation")]
        {
            match &self.validator {
                Some(v) => v.validate(document, value),
                None => Ok(()),
            }
        }
        #[cfg(not(feature = "validation"))]
        {
            let _ = (document, value);
            Ok(())
        }
    }

    /// Looks up a workflow by identity key.
    pub fn workflow(&self, key: &str) -> Option<Arc<CompiledWorkflow>> {
        self.workflows.lock().unwrap().get(key).cloned()
    }

    /// Registers a compiled workflow.
    pub fn register(&self, workflow: CompiledWorkflow) -> Arc<CompiledWorkflow> {
        let arc = Arc::new(workflow);
        let key = arc.id.key();
        self.workflows.lock().unwrap().insert(key, arc.clone());
        arc
    }

    /// Records a task name in the execution order log.
    pub fn record_task(&self, name: &str) {
        self.task_order.lock().unwrap().push(name.to_string());
    }

    /// Clears and returns the recorded task order.
    pub fn take_task_order(&self) -> Vec<String> {
        std::mem::take(&mut *self.task_order.lock().unwrap())
    }
}

/// Cancellation token shared across an execution and its branches.
#[derive(Debug, Default)]
pub(crate) struct Cancellation {
    flag: AtomicBool,
    notify: Notify,
}

impl Cancellation {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
    pub fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }
}

/// A running workflow execution.
pub struct ExecutionHandle {
    /// The stable execution id.
    pub execution_id: String,
    rx: tokio::sync::Mutex<Option<oneshot::Receiver<Result<serde_json::Value, WorkflowError>>>>,
    cancel: Arc<Cancellation>,
}

impl ExecutionHandle {
    /// Awaits the workflow completion, returning the output or error.
    pub async fn wait(self) -> Result<serde_json::Value, WorkflowError> {
        let rx = self.rx.lock().await.take();
        match rx {
            Some(rx) => rx.await.map_err(|_| WorkflowError::cancelled())?,
            None => Err(WorkflowError::cancelled()),
        }
    }

    /// Requests cancellation of the workflow.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

/// A `Runtime` executes compiled OWS workflows.
#[derive(Clone)]
pub struct Runtime {
    pub(crate) inner: Arc<RuntimeInner>,
}

impl Runtime {
    /// Starts building a new runtime.
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::new()
    }

    /// Compiles and validates a workflow definition.
    pub fn compile(&self, def: &WorkflowDefinition) -> Result<CompiledWorkflow, WorkflowError> {
        compile::compile(def)
    }

    /// Compiles and registers a workflow definition.
    pub fn register_definition(
        &self,
        def: &WorkflowDefinition,
    ) -> Result<Arc<CompiledWorkflow>, WorkflowError> {
        let compiled = self.compile(def)?;
        Ok(self.inner.register(compiled))
    }

    /// Executes a compiled workflow with the given input.
    pub async fn execute(
        &self,
        workflow: Arc<CompiledWorkflow>,
        input: serde_json::Value,
    ) -> Result<ExecutionHandle, WorkflowError> {
        let inner = self.inner.clone();
        let execution_id = inner.uuid.new_v4().to_string();
        let cancel = Arc::new(Cancellation::new());
        let (tx, rx) = oneshot::channel();

        let inner_exec = inner.clone();
        let workflow_exec = workflow.clone();
        let cancel_exec = cancel.clone();

        tokio::spawn(async move {
            let result = run_with_cancel(inner_exec, workflow_exec, input, cancel_exec).await;
            let _ = tx.send(result);
        });

        Ok(ExecutionHandle {
            execution_id,
            rx: tokio::sync::Mutex::new(Some(rx)),
            cancel,
        })
    }

    /// Runs a compiled workflow to completion and returns its output.
    pub async fn run(
        &self,
        workflow: Arc<CompiledWorkflow>,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, WorkflowError> {
        self.execute(workflow, input).await?.wait().await
    }

    /// Clears and returns the recorded task execution order.
    ///
    /// Used by conformance and integration tests to assert task ordering.
    pub fn take_task_order(&self) -> Vec<String> {
        self.inner.take_task_order()
    }
}

async fn run_with_cancel(
    inner: Arc<RuntimeInner>,
    workflow: Arc<CompiledWorkflow>,
    input: serde_json::Value,
    cancel: Arc<Cancellation>,
) -> Result<serde_json::Value, WorkflowError> {
    let workflow_timeout = workflow.timeout;
    let cancel_for_exec = cancel.clone();
    let mut result_fut = Box::pin(engine::execute(inner, workflow, input, cancel_for_exec));

    let execution = async {
        tokio::select! {
            res = &mut result_fut => res,
            _ = cancel.notified() => Err(WorkflowError::cancelled()),
        }
    };

    match workflow_timeout {
        Some(dur) => match tokio::time::timeout(dur, execution).await {
            Ok(res) => res,
            Err(_) => Err(WorkflowError::timeout(None)),
        },
        None => execution.await,
    }
}

/// Builds a [`Runtime`] with configurable services.
pub struct RuntimeBuilder {
    expression: Option<Arc<dyn ows_runtime_core::ExpressionEngine>>,
    clock: Option<Arc<dyn Clock>>,
    uuid: Option<Arc<dyn UuidGenerator>>,
    random: Option<Arc<dyn RandomGenerator>>,
    runtime_info: RuntimeInfo,
    policy: RuntimePolicy,
    publisher: Option<Arc<dyn EventPublisher>>,
    service: Option<Arc<dyn ServiceInvoker>>,
    functions: HashMap<String, Arc<dyn FunctionInvoker>>,
    process: Option<Arc<dyn ProcessRunner>>,
    consumer: Option<Arc<dyn EventConsumer>>,
    store: Option<Arc<dyn ExecutionStore>>,
    scheduler: Option<Arc<dyn Scheduler>>,
    enable_schema_validation: bool,
}

impl RuntimeBuilder {
    /// Creates a new builder with safe defaults (deny-by-default network).
    pub fn new() -> Self {
        Self {
            expression: None,
            clock: None,
            uuid: None,
            random: None,
            runtime_info: RuntimeInfo::default(),
            policy: RuntimePolicy::default(),
            publisher: None,
            service: None,
            functions: HashMap::new(),
            process: None,
            consumer: None,
            store: None,
            scheduler: None,
            enable_schema_validation: true,
        }
    }

    /// Sets the expression engine.
    pub fn with_expression(mut self, e: Arc<dyn ows_runtime_core::ExpressionEngine>) -> Self {
        self.expression = Some(e);
        self
    }

    /// Sets the clock.
    pub fn with_clock(mut self, c: Arc<dyn Clock>) -> Self {
        self.clock = Some(c);
        self
    }

    /// Sets the UUID generator.
    pub fn with_uuid(mut self, u: Arc<dyn UuidGenerator>) -> Self {
        self.uuid = Some(u);
        self
    }

    /// Sets the random generator.
    pub fn with_random(mut self, r: Arc<dyn RandomGenerator>) -> Self {
        self.random = Some(r);
        self
    }

    /// Sets the runtime info.
    pub fn with_runtime_info(mut self, info: RuntimeInfo) -> Self {
        self.runtime_info = info;
        self
    }

    /// Sets the runtime policy.
    pub fn with_policy(mut self, policy: RuntimePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Sets the event publisher.
    pub fn with_event_publisher(mut self, p: Arc<dyn EventPublisher>) -> Self {
        self.publisher = Some(p);
        self
    }

    /// Sets the generic service invoker (used by the built-in `http` function).
    pub fn with_service(mut self, s: Arc<dyn ServiceInvoker>) -> Self {
        self.service = Some(s);
        self
    }

    /// Registers a custom function invoker for `call` tasks.
    pub fn register_function(mut self, name: &str, f: Arc<dyn FunctionInvoker>) -> Self {
        self.functions.insert(name.to_string(), f);
        self
    }

    /// Sets the process runner (for `run` tasks).
    pub fn with_process(mut self, p: Arc<dyn ProcessRunner>) -> Self {
        self.process = Some(p);
        self
    }

    /// Sets the event consumer (for `listen` tasks and event-driven scheduling).
    pub fn with_event_consumer(mut self, c: Arc<dyn EventConsumer>) -> Self {
        self.consumer = Some(c);
        self
    }

    /// Sets the execution store.
    pub fn with_store(mut self, s: Arc<dyn ExecutionStore>) -> Self {
        self.store = Some(s);
        self
    }

    /// Sets the scheduler.
    pub fn with_scheduler(mut self, s: Arc<dyn Scheduler>) -> Self {
        self.scheduler = Some(s);
        self
    }

    /// Builds the runtime.
    pub fn build(self) -> Result<Runtime, WorkflowError> {
        #[cfg(feature = "validation")]
        let validator = if self.enable_schema_validation {
            Some(Arc::new(JsonSchemaValidator))
        } else {
            None
        };
        #[cfg(not(feature = "validation"))]
        let validator = ();

        let inner = Arc::new(RuntimeInner {
            expression: self
                .expression
                .unwrap_or_else(|| Arc::new(ows_runtime_expressions::JqEngine::new())),
            clock: self.clock.unwrap_or_else(|| Arc::new(SystemClock::new())),
            uuid: self
                .uuid
                .unwrap_or_else(|| Arc::new(ows_runtime_core::DefaultUuidGenerator)),
            random: self
                .random
                .unwrap_or_else(|| Arc::new(ows_runtime_core::DefaultRandomGenerator::default())),
            runtime_info: self.runtime_info,
            policy: self.policy,
            publisher: self
                .publisher
                .unwrap_or_else(|| Arc::new(ows_runtime_core::NullEventPublisher)),
            service: self.service.unwrap_or_else(|| Arc::new(NoopServiceInvoker)),
            functions: Arc::new(std::sync::RwLock::new(self.functions)),
            process: self.process.unwrap_or_else(|| Arc::new(NoopProcessRunner)),
            consumer: self
                .consumer
                .unwrap_or_else(|| Arc::new(ows_runtime_core::NoopEventConsumer)),
            store: self
                .store
                .unwrap_or_else(|| Arc::new(InMemoryExecutionStore::new())),
            scheduler: self
                .scheduler
                .unwrap_or_else(|| Arc::new(ows_runtime_scheduler::NoopScheduler)),
            workflows: Arc::new(std::sync::Mutex::new(HashMap::new())),
            task_order: Arc::new(std::sync::Mutex::new(Vec::new())),
            #[cfg(feature = "validation")]
            validator,
        });

        // Register the default HTTP function backed by the service invoker.
        // (The invoker itself enforces the network policy.)
        #[cfg(feature = "http")]
        {
            let mut functions = inner.functions.write().unwrap();
            if !functions.contains_key("http") {
                functions.insert(
                    "http".to_string(),
                    Arc::new(HttpServiceInvoker::new(inner.clone())),
                );
            }
            if !functions.contains_key("openapi") {
                functions.insert(
                    "openapi".to_string(),
                    Arc::new(OpenApiInvoker::new(inner.clone())),
                );
            }
            drop(functions);
        }

        Ok(Runtime { inner })
    }
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// Re-export the HTTP invoker type for users who want to inspect it.
#[cfg(feature = "http")]
pub use crate::service::HttpServiceInvoker as _HttpServiceInvokerExport;

#[cfg(test)]
mod tests {
    use super::*;
    use ows_runtime_core::{DeterministicClock, SeededRandomGenerator};

    #[test]
    fn builder_defaults_and_overrides() {
        let rt = Runtime::builder().build().unwrap();
        assert!(!rt.inner.runtime_info.name.is_empty());

        let rt = Runtime::builder()
            .with_clock(Arc::new(DeterministicClock::new()))
            .with_random(Arc::new(SeededRandomGenerator::new(1)))
            .with_runtime_info(RuntimeInfo {
                name: "test".into(),
                ..Default::default()
            })
            .build()
            .unwrap();
        assert_eq!(rt.inner.runtime_info.name, "test");
        assert!(rt.inner.clock.is_deterministic());
    }

    #[tokio::test]
    async fn register_and_run_definition() {
        let rt = Runtime::builder().build().unwrap();
        let def = ows_runtime_dsl::from_yaml(
            r#"
document: { dsl: '1.0.3', namespace: n, name: w, version: '1.0.0' }
do:
  - a: { set: { x: 1 } }
"#,
        )
        .unwrap();
        let wf = rt.register_definition(&def).unwrap();
        let out = rt.run(wf, serde_json::Value::Null).await.unwrap();
        assert_eq!(out["x"], 1);
    }

    #[test]
    fn builder_register_function_and_policy() {
        let b = Runtime::builder();
        let b = b.with_policy(ows_runtime_core::RuntimePolicy {
            allow_network: true,
            ..Default::default()
        });
        assert!(!b.policy.allow_network || b.policy.allow_network);
    }

    #[test]
    fn runtime_inner_workflow_lookup() {
        let rt = Runtime::builder().build().unwrap();
        let def = ows_runtime_dsl::from_yaml(
            r#"
document: { dsl: '1.0.3', namespace: n, name: w, version: '1.0.0' }
do:
  - a: { set: { x: 1 } }
"#,
        )
        .unwrap();
        let wf = rt.register_definition(&def).unwrap();
        let key = format!("{}/{}/{}", wf.id.namespace, wf.id.name, wf.id.version);
        assert!(rt.inner.workflow(&key).is_some());
        assert!(rt.inner.workflow("nope").is_none());
    }

    #[test]
    fn cancellation_token_behavior() {
        let c = Cancellation::new();
        assert!(!c.is_cancelled());
        c.cancel();
        assert!(c.is_cancelled());
    }

    #[tokio::test]
    async fn cancellation_notifies() {
        let c = Arc::new(Cancellation::new());
        let c2 = c.clone();
        let handle = tokio::spawn(async move {
            tokio::select! {
                _ = c2.notified() => true,
                _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => false,
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        c.cancel();
        assert!(handle.await.unwrap());
    }

    #[test]
    fn builder_all_with_methods() {
        use ows_runtime_core::{
            DefaultUuidGenerator, NoopEventConsumer, NullEventPublisher, RuntimePolicy,
        };
        let rt = Runtime::builder()
            .with_expression(Arc::new(ows_runtime_expressions::JqEngine::new()))
            .with_uuid(Arc::new(DefaultUuidGenerator))
            .with_event_publisher(Arc::new(NullEventPublisher))
            .with_event_consumer(Arc::new(NoopEventConsumer))
            .with_service(Arc::new(NoopServiceInvoker))
            .with_process(Arc::new(NoopProcessRunner))
            .with_store(Arc::new(InMemoryExecutionStore::new()))
            .with_scheduler(Arc::new(ows_runtime_scheduler::NoopScheduler))
            .with_policy(RuntimePolicy::default())
            .build()
            .unwrap();
        assert!(rt.inner.functions.read().unwrap().contains_key("http"));
    }
}
