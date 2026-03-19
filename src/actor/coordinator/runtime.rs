use anyhow::{Result, anyhow};
use crossbeam::channel::Receiver;
use tokio::sync::mpsc;
use tokio::task::JoinError;

use crate::actor::compiler::CompilerActor;
use crate::actor::fs::FsActor;
use crate::actor::messages::{CompilerMsg, VdomMsg, WsMsg};
use crate::actor::vdom::VdomActor;
use crate::actor::ws::WsActor;
use crate::reload::server::WsServerHandle;

/// Run all actors concurrently.
pub(super) async fn run_actors(
    fs: FsActor,
    compiler: CompilerActor,
    vdom: VdomActor,
    ws: WsActor,
    ws_server: Option<WsServerHandle>,
    compiler_tx: mpsc::Sender<CompilerMsg>,
    vdom_tx: mpsc::Sender<VdomMsg>,
    ws_tx: mpsc::Sender<WsMsg>,
    shutdown_rx: Option<Receiver<()>>,
) -> Result<()> {
    let mut vdom_handle = tokio::spawn(async move { vdom.run().await });
    let mut fs_handle = tokio::spawn(async move { fs.run().await });
    let mut compiler_handle = tokio::spawn(async move { compiler.run().await });
    let mut ws_handle = tokio::spawn(async move { ws.run().await });

    let mut result = Ok(());
    let mut finished_actor = None;

    match shutdown_rx {
        Some(rx) => {
            tokio::select! {
                join = &mut vdom_handle => {
                    finished_actor = Some("vdom");
                    result = actor_join_result("vdom", join);
                }
                join = &mut fs_handle => {
                    finished_actor = Some("fs");
                    result = actor_join_result("fs", join);
                }
                join = &mut compiler_handle => {
                    finished_actor = Some("compiler");
                    result = actor_join_result("compiler", join);
                }
                join = &mut ws_handle => {
                    finished_actor = Some("ws");
                    result = actor_join_result("ws", join);
                }
                () = wait_for_shutdown(rx) => {
                    crate::debug!("actor"; "shutdown signal received");
                }
            }
        }
        None => {
            tokio::select! {
                join = &mut vdom_handle => {
                    finished_actor = Some("vdom");
                    result = actor_join_result("vdom", join);
                }
                join = &mut fs_handle => {
                    finished_actor = Some("fs");
                    result = actor_join_result("fs", join);
                }
                join = &mut compiler_handle => {
                    finished_actor = Some("compiler");
                    result = actor_join_result("compiler", join);
                }
                join = &mut ws_handle => {
                    finished_actor = Some("ws");
                    result = actor_join_result("ws", join);
                }
            }
        }
    }

    if let Some(actor) = finished_actor {
        crate::debug!("actor"; "{} actor stopped first", actor);
    }

    crate::debug!("actor"; "sending shutdown to compiler/ws/vdom");
    if let Some(ws_server) = ws_server {
        ws_server.request_stop();
    }
    let _ = compiler_tx.send(CompilerMsg::Shutdown).await;
    let _ = ws_tx.send(WsMsg::Shutdown).await;
    let _ = vdom_tx.send(VdomMsg::Shutdown).await;

    let timeout = std::time::Duration::from_millis(500);
    if finished_actor != Some("compiler") {
        record_shutdown_result(
            &mut result,
            wait_for_actor("compiler", compiler_handle, timeout).await,
        );
    }
    if finished_actor != Some("ws") {
        record_shutdown_result(&mut result, wait_for_actor("ws", ws_handle, timeout).await);
    }
    if finished_actor != Some("vdom") {
        record_shutdown_result(
            &mut result,
            wait_for_actor("vdom", vdom_handle, timeout).await,
        );
    }
    if finished_actor != Some("fs") {
        record_shutdown_result(&mut result, abort_fs_actor(fs_handle, timeout).await);
    }

    result
}

async fn wait_for_shutdown(rx: Receiver<()>) {
    loop {
        if rx.try_recv().is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

fn actor_join_result(actor: &str, result: std::result::Result<(), JoinError>) -> Result<()> {
    result.map_err(|err| anyhow!("{actor} actor task failed: {err}"))
}

async fn wait_for_actor(
    actor: &str,
    handle: tokio::task::JoinHandle<()>,
    timeout: std::time::Duration,
) -> Result<()> {
    match tokio::time::timeout(timeout, handle).await {
        Ok(result) => actor_join_result(actor, result),
        Err(_) => Err(anyhow!("{actor} actor did not stop within {timeout:?}")),
    }
}

async fn abort_fs_actor(
    handle: tokio::task::JoinHandle<()>,
    timeout: std::time::Duration,
) -> Result<()> {
    handle.abort();
    match tokio::time::timeout(timeout, handle).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) if err.is_cancelled() => Ok(()),
        Ok(Err(err)) => actor_join_result("fs", Err(err)),
        Err(_) => Err(anyhow!("fs actor did not stop within {timeout:?}")),
    }
}

fn record_shutdown_result(result: &mut Result<()>, shutdown_result: Result<()>) {
    if let Err(err) = shutdown_result {
        if result.is_ok() {
            *result = Err(err);
        } else {
            crate::debug!("actor"; "additional shutdown error: {:#}", err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{abort_fs_actor, actor_join_result};

    #[tokio::test]
    async fn actor_join_error_becomes_error() {
        let handle = tokio::spawn(std::future::pending::<()>());
        handle.abort();
        let result = handle.await;

        let error = actor_join_result("vdom", result).unwrap_err();

        assert!(
            error.to_string().contains("vdom actor task failed"),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn aborted_fs_actor_cancellation_is_graceful() {
        let handle = tokio::spawn(std::future::pending::<()>());

        let result = abort_fs_actor(handle, std::time::Duration::from_millis(100)).await;

        assert!(result.is_ok(), "{result:#?}");
    }
}
