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
use tola_nu::{Host, InvocationContext, ScriptModule};
use tola_subprocess::Command;

#[derive(Serialize, Deserialize)]
struct Invocation {
    directory: PathBuf,
    environment: BTreeMap<String, Option<String>>,
    libraries: Vec<ScriptModule>,
    dependencies: Vec<PathBuf>,
    path: PathBuf,
    args: Vec<String>,
    site: Option<Value>,
    modules: BTreeMap<String, Value>,
    module: Option<String>,
    hook: Option<Value>,
    context: Option<Value>,
    fonts: tola_typst::FontOptions,
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
    RerunOnDigest,
    Assets {
        module: Option<String>,
        include: Vec<String>,
        exclude: Vec<String>,
    },
}

pub(super) fn run(
    hook: &NuHook<'_>,
    libraries: Vec<ScriptModule>,
    dependencies: Vec<PathBuf>,
    host: Host,
) -> Result<()> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .context("start the Nu hook request channel")?;
    listener.set_nonblocking(true)?;
    let mut key = [0; 32];
    getrandom::fill(&mut key).map_err(|_| anyhow!("create the Nu hook request key"))?;
    let mut environment = super::call_environment(hook.scope, &hook.context, hook.temp_dir)?;
    for (name, value) in crate::cli::color::captured_child_environment() {
        environment.insert(name.into(), Some(value));
    }
    let invocation = Invocation {
        directory: hook.config.root().to_owned(),
        environment,
        libraries,
        dependencies,
        path: hook.path.to_owned(),
        args: hook.args.to_vec(),
        site: host.site,
        modules: host.modules,
        module: host.module,
        hook: host.hook,
        context: host.context,
        fonts: hook.config.compiler_font_options(&hook.resources),
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
    let assets = host.get_assets;
    let rerun_on_digest = host.rerun_on_digest;
    let handle = move |request| match request {
        Request::Query {
            directory,
            target,
            config,
            label,
        } => query(&directory, &target, config.as_deref(), label.as_deref()),
        Request::Submit(issues) => submit(issues).map(|()| Value::Null),
        Request::RerunOnDigest => {
            let digest = rerun_on_digest
                .as_ref()
                .ok_or_else(|| "get-hook --rerun-on-digest needs a Nu build hook".to_owned())?;
            digest().map(|digest| digest.map_or(Value::Null, Value::String))
        }
        Request::Assets {
            module,
            include,
            exclude,
        } => {
            let assets = assets.as_ref().ok_or_else(|| {
                "get-assets needs a hook that runs after asset discovery".to_owned()
            })?;
            assets(module, include, exclude)
        }
    };
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
                span.in_scope(|| serve(listener, key, &stop, &active, handle))
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
    handle: impl Fn(Request) -> Result<Value, String>,
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
            let response = serde_json::from_reader(BufReader::new(&stream))
                .map_err(|error| error.to_string())
                .and_then(&handle);
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
        svg_fonts: Some(Ok(Arc::new(tola_typst::FontStore::with_options(
            invocation.fonts,
        )))),
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
        rerun_on_digest: Some(Arc::new(move || {
            match request(address, &key, Request::RerunOnDigest)? {
                Value::Null => Ok(None),
                Value::String(digest) => Ok(Some(digest)),
                _ => Err("could not read the hook rerun-on digest".into()),
            }
        })),
        get_assets: Some(Arc::new(move |module, include, exclude| {
            request(
                address,
                &key,
                Request::Assets {
                    module,
                    include,
                    exclude,
                },
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tola_typst::typst::text::FontVariant;
    use tola_typst::{BundleCancellation, FontOptions, FontStore};

    #[test]
    fn hook_worker_uses_supplied_svg_fonts() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let fonts = root.join("fonts");
        fs::create_dir(&fonts).unwrap();
        let bundled = FontStore::with_options(FontOptions::new().with_system_fonts(false));
        let bundled = bundled.load(&BundleCancellation::new()).unwrap();
        let font = bundled
            .book()
            .select("dejavu sans mono", FontVariant::default())
            .and_then(|index| bundled.font(index))
            .unwrap();
        fs::write(fonts.join("wordmark.ttf"), font.data().as_slice()).unwrap();
        fs::write(
            root.join("wordmark.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="24">
  <text x="1" y="20" font-family="DejaVu Sans Mono" font-size="18">Tola</text>
</svg>"#,
        )
        .unwrap();
        let path = root.join("hook.nu");
        fs::write(
            &path,
            "use tola image\nimage resize wordmark.svg --format png --output wordmark.png | ignore\n",
        )
        .unwrap();
        let mut invocation = Invocation {
            directory: root.clone(),
            environment: BTreeMap::new(),
            libraries: Vec::new(),
            dependencies: Vec::new(),
            path,
            args: Vec::new(),
            site: None,
            modules: BTreeMap::new(),
            module: None,
            hook: None,
            context: None,
            fonts: FontOptions::new()
                .with_system_fonts(false)
                .with_embedded_fonts(false),
            address: ([127, 0, 0, 1], 0).into(),
            key: [0; 32],
        };
        let snapshot = root.join("hook.json");
        let output = root.join("wordmark.png");
        fs::write(&snapshot, serde_json::to_vec(&invocation).unwrap()).unwrap();
        assert_eq!(run_worker(&snapshot).unwrap(), 0);
        let without_font = fs::read(&output).unwrap();
        fs::remove_file(&output).unwrap();

        invocation.fonts = invocation.fonts.add_path(fonts);
        fs::write(&snapshot, serde_json::to_vec(&invocation).unwrap()).unwrap();
        assert_eq!(run_worker(&snapshot).unwrap(), 0);
        assert_ne!(fs::read(output).unwrap(), without_font);
    }
}
