use anyhow::Result;
use crossbeam::channel::Receiver;
use tokio::sync::mpsc;

use crate::actor::compiler::CompilerActor;
use crate::actor::fs::FsActor;
use crate::actor::messages::VdomMsg;
use crate::actor::vdom::VdomActor;
use crate::actor::ws::WsActor;

/// Run all actors concurrently.
pub(super) async fn run_actors(
    fs: FsActor,
    compiler: CompilerActor,
    vdom: VdomActor,
    ws: WsActor,
    vdom_tx: mpsc::Sender<VdomMsg>,
    shutdown_rx: Option<Receiver<()>>,
) -> Result<()> {
    let vdom_handle = tokio::spawn(async move { vdom.run().await });

    let fs_handle = tokio::spawn(async move { fs.run().await });
    let compiler_handle = tokio::spawn(async move { compiler.run().await });
    let ws_handle = tokio::spawn(async move { ws.run().await });

    if let Some(rx) = shutdown_rx {
        loop {
            if rx.try_recv().is_ok() {
                crate::debug!("actor"; "shutdown signal received");
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    } else {
        tokio::select! {
            _ = fs_handle => {}
            _ = compiler_handle => {}
            _ = ws_handle => {}
        }
    }

    crate::debug!("actor"; "sending shutdown to vdom");
    let _ = vdom_tx.send(VdomMsg::Shutdown).await;

    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), vdom_handle).await;

    Ok(())
}
