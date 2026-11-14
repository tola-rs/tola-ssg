//! Run a Nu hook with captured standard streams and access to the build that started it.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tola_build::hooks::NuHook;
use tola_nu::{ConfiguredModule, Host, InvocationContext};
use tola_subprocess::Command;

#[derive(Serialize, Deserialize)]
struct Invocation {
    directory: PathBuf,
    environment: BTreeMap<String, Option<String>>,
    libraries: Vec<ConfiguredModule>,
    dependencies: Vec<PathBuf>,
    path: PathBuf,
    args: Vec<String>,
    site: Option<Value>,
    modules: BTreeMap<String, Value>,
    module: Option<String>,
    hook: Option<Value>,
    context: Option<Value>,
    address: SocketAddr,
    key: [u8; 32],
}

#[derive(Serialize, Deserialize)]
enum Request {
    Query {
        directory: PathBuf,
        target: String,
        config: Option<PathBuf>,
        label: Option<String>,
    },
    Submit(Value),
}

pub(super) fn run(
    hook: &NuHook<'_>,
    libraries: Vec<ConfiguredModule>,
    dependencies: Vec<PathBuf>,
    host: Host,
) -> Result<()> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .context("start the Nu hook request channel")?;
    listener.set_nonblocking(true)?;
    let mut key = [0; 32];
    getrandom::fill(&mut key).map_err(|_| anyhow!("create the Nu hook request key"))?;
    let invocation = Invocation {
        directory: hook.config.get_root().to_owned(),
        environment: super::call_environment(hook.scope, &hook.context, hook.temp_dir)?,
        libraries,
        dependencies,
        path: hook.path.to_owned(),
        args: hook.args.to_vec(),
        site: host.site,
        modules: host.modules,
        module: host.module,
        hook: host.hook,
        context: host.context,
        address: listener.local_addr()?,
        key,
    };
    // The invocation contains the request key. Create it with private file permissions.
    let mut snapshot = tempfile::NamedTempFile::new_in(hook.temp_dir)
        .context("create the Nu hook invocation file")?;
    serde_json::to_writer(&mut snapshot, &invocation).context("write the Nu hook invocation")?;
    let command = Command::new(crate::cli::external::current_executable()?)
        .args(["nu", "--hook-worker"])
        .arg(snapshot.path())
        .cwd(&invocation.directory);
    let query = host
        .query
        .context("Nu hook requires a site query handler")?;
    let submit = host
        .submit_diagnostics
        .context("Nu hook requires a report handler")?;
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let connection = Arc::new(Mutex::new(None::<TcpStream>));
    let active = connection.clone();
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    let span = tracing::Span::current();
    let server = thread::Builder::new()
        .name("tola-nu-hook".into())
        .spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || {
                span.in_scope(|| serve(listener, key, &stop, &active, query, submit))
            })
        })
        .context("start the Nu hook request handler")?;
    let completion = hook.run_command(&command);
    stopped.store(true, Ordering::Relaxed);
    if let Some(stream) = connection.lock().ok().and_then(|mut active| active.take()) {
        let _ = stream.shutdown(Shutdown::Both);
    }
    server.thread().unpark();
    let served = server
        .join()
        .map_err(|_| anyhow!("Nu hook request handler stopped"));
    completion?;
    served?.context("serve Nu hook requests")
}

fn serve(
    listener: TcpListener,
    key: [u8; 32],
    stopped: &AtomicBool,
    connection: &Mutex<Option<TcpStream>>,
    query: tola_nu::Query,
    submit: tola_nu::SubmitDiagnostics,
) -> std::io::Result<()> {
    while !stopped.load(Ordering::Relaxed) {
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::park_timeout(Duration::from_millis(10));
                continue;
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        {
            let mut active = connection.lock().expect("Nu hook connection");
            if stopped.load(Ordering::Relaxed) {
                break;
            }
            *active = Some(stream.try_clone()?);
        }
        stream.set_nonblocking(false)?;
        // Authenticate before reading a request or invoking the build's handlers.
        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
        let mut received = [0; 32];
        if stream.read_exact(&mut received).is_ok() && received == key {
            stream.set_read_timeout(None)?;
            let response = match serde_json::from_reader(BufReader::new(&stream)) {
                Ok(Request::Query {
                    directory,
                    target,
                    config,
                    label,
                }) => query(&directory, &target, config.as_deref(), label.as_deref()),
                Ok(Request::Submit(issues)) => submit(issues).map(|()| Value::Null),
                Err(error) => Err(error.to_string()),
            };
            let mut writer = BufWriter::new(&mut stream);
            if serde_json::to_writer(&mut writer, &response).is_ok() {
                // A cancelled hook can close its socket before the query returns.
                let _ = writer.flush();
            }
        }
        connection.lock().expect("Nu hook connection").take();
    }
    Ok(())
}

pub(super) fn run_worker(path: &Path) -> Result<i32> {
    let invocation: Invocation = serde_json::from_reader(BufReader::new(File::open(path)?))
        .context("read the Nu hook invocation")?;
    ensure!(
        invocation.address.ip().is_loopback(),
        "invalid Nu hook request address"
    );
    let address = invocation.address;
    let key = invocation.key;
    let runtime = super::prepare_runtime(
        invocation.libraries,
        invocation.dependencies,
        InvocationContext {
            directory: invocation.directory,
            environment: invocation.environment,
        },
        Vec::new(),
    )?
    .with_host(Host {
        site: invocation.site,
        modules: invocation.modules,
        module: invocation.module,
        hook: invocation.hook,
        context: invocation.context,
        query: Some(Arc::new(move |directory, target, config, label| {
            request(
                address,
                &key,
                Request::Query {
                    directory: directory.to_owned(),
                    target: target.to_owned(),
                    config: config.map(Path::to_owned),
                    label: label.map(str::to_owned),
                },
            )
        })),
        submit_diagnostics: Some(Arc::new(move |issues| {
            request(address, &key, Request::Submit(issues)).map(|_| ())
        })),
        ..super::host()
    })?;
    Ok(runtime.run_embedded(&invocation.path, &invocation.args)?)
}

fn request(address: SocketAddr, key: &[u8; 32], request: Request) -> Result<Value, String> {
    let exchange = || -> Result<Result<Value, String>> {
        let mut stream = TcpStream::connect(address)?;
        let mut writer = BufWriter::new(&mut stream);
        writer.write_all(key)?;
        serde_json::to_writer(&mut writer, &request)?;
        writer.flush()?;
        drop(writer);
        stream.shutdown(Shutdown::Write)?;
        Ok(serde_json::from_reader(BufReader::new(stream))?)
    };
    exchange().map_err(|_| "could not reach the Tola build from the Nu hook".to_owned())?
}
