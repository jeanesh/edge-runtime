use crate::utils::units::{bytes_to_display, human_elapsed, mib_to_bytes};

use anyhow::Error;
use deno_core::located_script_name;
use deno_core::url::Url;
use deno_core::JsRuntime;
use deno_core::ModuleSpecifier;
use deno_core::RuntimeOptions;
use import_map::{parse_from_json, ImportMap, ImportMapDiagnostic};
use log::{debug, error, warn};
use std::collections::HashMap;
use std::fs;
use std::panic;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::thread;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

pub mod env;
pub mod http_start;
pub mod module_loader;
pub mod net_override;
pub mod permissions;
pub mod runtime;
pub mod types;

use module_loader::DefaultModuleLoader;
use permissions::Permissions;

fn load_import_map(maybe_path: Option<String>) -> Result<Option<ImportMap>, Error> {
    if let Some(path_str) = maybe_path {
        let path = Path::new(&path_str);
        let json_str = fs::read_to_string(path)?;

        let abs_path = std::env::current_dir().map(|p| p.join(&path))?;
        let base_url = Url::from_directory_path(abs_path.parent().unwrap()).unwrap();
        let result = parse_from_json(&base_url, json_str.as_str())?;
        print_import_map_diagnostics(&result.diagnostics);
        Ok(Some(result.import_map))
    } else {
        Ok(None)
    }
}

fn print_import_map_diagnostics(diagnostics: &[ImportMapDiagnostic]) {
    if !diagnostics.is_empty() {
        warn!(
            "Import map diagnostics:\n{}",
            diagnostics
                .iter()
                .map(|d| format!("  - {d}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

pub struct JsWorker {
    js_runtime: JsRuntime,
    main_module_url: ModuleSpecifier,
    unix_stream_tx: mpsc::UnboundedSender<UnixStream>,
}

impl JsWorker {
    pub fn new(
        service_path: PathBuf,
        memory_limit_mb: u64,
        worker_timeout_ms: u64,
        no_module_cache: bool,
        import_map_path: Option<String>,
        env_vars: HashMap<String, String>,
    ) -> Result<Self, Error> {
        let user_agent = "supabase-edge-runtime".to_string();

        let base_url =
            Url::from_directory_path(std::env::current_dir().map(|p| p.join(&service_path))?)
                .unwrap();

        // TODO: check for other potential main paths (eg: index.js, index.tsx)
        let main_module_url = base_url.join("index.ts")?;

        // Note: this will load Mozilla's CAs (we may also need to support system certs)
        let root_cert_store = deno_tls::create_default_root_cert_store();

        let extensions_with_js = vec![
            deno_webidl::init(),
            deno_console::init(),
            deno_url::init(),
            deno_web::init::<Permissions>(deno_web::BlobStore::default(), None),
            deno_fetch::init::<Permissions>(deno_fetch::Options {
                user_agent: user_agent.clone(),
                root_cert_store: Some(root_cert_store.clone()),
                ..Default::default()
            }),
            // TODO: support providing a custom seed for crypto
            deno_crypto::init(None),
            deno_net::init::<Permissions>(Some(root_cert_store.clone()), false, None),
            deno_websocket::init::<Permissions>(
                user_agent.clone(),
                Some(root_cert_store.clone()),
                None,
            ),
            deno_http::init(),
            deno_tls::init(),
            env::init(),
        ];
        let extensions = vec![
            net_override::init(),
            http_start::init(),
            permissions::init(),
            runtime::init(main_module_url.clone()),
        ];

        let import_map = load_import_map(import_map_path)?;
        let module_loader = DefaultModuleLoader::new(import_map, no_module_cache)?;

        let mut js_runtime = JsRuntime::new(RuntimeOptions {
            extensions,
            extensions_with_js,
            module_loader: Some(Rc::new(module_loader)),
            is_main: true,
            create_params: Some(v8::CreateParams::default().heap_limits(
                mib_to_bytes(1) as usize,
                mib_to_bytes(memory_limit_mb) as usize,
            )),
            shared_array_buffer_store: None,
            compiled_wasm_module_store: None,
            ..Default::default()
        });

        let (memory_limit_tx, memory_limit_rx) = mpsc::unbounded_channel::<u64>();
        // add a callback when a worker reaches its memory limit
        let memory_limit_mb = memory_limit_mb;
        js_runtime.add_near_heap_limit_callback(move |cur, _init| {
            debug!(
                "low memory alert triggered {}",
                bytes_to_display(cur as u64)
            );
            let _ = memory_limit_tx.send(mib_to_bytes(memory_limit_mb));
            // add a 25% allowance to memory limit
            let cur = mib_to_bytes(memory_limit_mb + memory_limit_mb.div_euclid(4)) as usize;
            cur
        });

        // set bootstrap options
        let script = format!("globalThis.__build_target = \"{}\"", env!("TARGET"));
        js_runtime
            .execute_script(&located_script_name!(), &script)
            .expect("Failed to execute bootstrap script");

        // bootstrap the JS runtime
        let bootstrap_js = include_str!("./js_worker/js/bootstrap.js");
        js_runtime
            .execute_script("[js_worker]: bootstrap.js", bootstrap_js)
            .expect("Failed to execute bootstrap script");

        debug!("bootstrapped function");

        let (unix_stream_tx, unix_stream_rx) = mpsc::unbounded_channel::<UnixStream>();
        //run inside a closure, so op_state_rc is released
        {
            let op_state_rc = js_runtime.op_state();
            let mut op_state = op_state_rc.borrow_mut();
            op_state.put::<mpsc::UnboundedReceiver<UnixStream>>(unix_stream_rx);
            op_state.put::<types::EnvVars>(env_vars);
        }

        let mut worker = Self {
            js_runtime,
            main_module_url,
            unix_stream_tx,
        };

        worker.start_controller_thread(worker_timeout_ms, memory_limit_rx);
        Ok(worker)
    }

    pub fn snapshot() {
        unimplemented!();
    }

    pub fn run(self, shutdown_tx: oneshot::Sender<()>) -> Result<(), Error> {
        let mut js_runtime = self.js_runtime;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let future = async move {
            let mod_id = js_runtime
                .load_main_module(&self.main_module_url, None)
                .await?;
            let result = js_runtime.mod_evaluate(mod_id);
            js_runtime.run_event_loop(false).await?;

            result.await?
        };

        let local = tokio::task::LocalSet::new();
        let res = local.block_on(&runtime, future);

        // terminate the worker

        if res.is_err() {
            error!("worker thread panicked {:?}", res.as_ref().err().unwrap());
        }

        Ok(shutdown_tx.send(()).unwrap())
    }

    fn start_controller_thread(
        &mut self,
        worker_timeout_ms: u64,
        mut memory_limit_rx: mpsc::UnboundedReceiver<u64>,
    ) {
        let thread_safe_handle = self.js_runtime.v8_isolate().thread_safe_handle();

        thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            let future = async move {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(worker_timeout_ms)) => {
                        debug!("max duration reached for the worker. terminating the worker. (duration {})", human_elapsed(worker_timeout_ms))
                    }
                    Some(val) = memory_limit_rx.recv() => {
                        error!("memory limit reached for the worker. terminating the worker. (used: {})", bytes_to_display(val))
                    }
                }
            };
            rt.block_on(future);

            let ok = thread_safe_handle.terminate_execution();
            if ok {
                debug!("terminated execution");
            } else {
                debug!("worker is already destroyed");
            }
        });
    }

    pub fn accept(&self, stream: UnixStream) -> () {
        self.unix_stream_tx.send(stream);
    }
}
