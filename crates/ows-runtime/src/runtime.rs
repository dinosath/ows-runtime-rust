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
use crate::service::{
    FunctionInvoker, HttpServiceInvoker, NoopProcessRunner, NoopServiceInvoker, OpenApiInvoker,
};

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
    validator: Option<Arc<JsonSchemaValidator>>,
}

/// A JSON Schema validator backed by the `jsonschema` crate.
pub(crate) struct JsonSchemaValidator;

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
        match &self.validator {
            Some(v) => v.validate(document, value),
            None => Ok(()),
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
        let validator = if self.enable_schema_validation {
            Some(Arc::new(JsonSchemaValidator))
        } else {
            None
        };

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
            validator,
        });

        // Register the default HTTP function backed by the service invoker.
        // (The invoker itself enforces the network policy.)
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

        Ok(Runtime { inner })
    }
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// Re-export the HTTP invoker type for users who want to inspect it.
pub use crate::service::HttpServiceInvoker as _HttpServiceInvokerExport;
