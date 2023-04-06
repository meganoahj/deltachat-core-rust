use std::env;
///! Delta Chat core RPC server.
///!
///! It speaks JSON Lines over stdio.
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context as _, Result};
use deltachat::constants::DC_VERSION_STR;
use deltachat_jsonrpc::api::events::event_to_json_rpc_notification;
use deltachat_jsonrpc::api::{Accounts, CommandApi};
use futures_lite::stream::StreamExt;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use yerpc::{RpcClient, RpcSession};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let mut args = env::args_os();
    let _program_name = args.next().context("no command line arguments found")?;
    if let Some(first_arg) = args.next() {
        if first_arg.to_str() == Some("--version") {
            if let Some(arg) = args.next() {
                return Err(anyhow!("Unrecognized argument {:?}", arg));
            }
            eprintln!("{}", &*DC_VERSION_STR);
            return Ok(());
        } else {
            return Err(anyhow!("Unrecognized option {:?}", first_arg));
        }
    }
    if let Some(arg) = args.next() {
        return Err(anyhow!("Unrecognized argument {:?}", arg));
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let path = std::env::var("DC_ACCOUNTS_PATH").unwrap_or_else(|_| "accounts".to_string());
    log::info!("Starting with accounts directory `{}`.", path);
    let accounts = Accounts::new(PathBuf::from(&path)).await?;
    let events = accounts.get_event_emitter();

    log::info!("Creating JSON-RPC API.");
    let accounts = Arc::new(RwLock::new(accounts));
    let state = CommandApi::from_arc(accounts.clone());

    let (client, mut out_receiver) = RpcClient::new();
    let session = RpcSession::new(client.clone(), state.clone());
    let canceler = CancellationToken::new();

    // Events task converts core events to JSON-RPC notifications.
    let events_task: JoinHandle<Result<()>> = tokio::spawn(async move {
        let mut r = Ok(());
        while let Some(event) = events.recv().await {
            if r.is_err() {
                continue;
            }
            let event = event_to_json_rpc_notification(event);
            r = client.send_notification("event", Some(event)).await;
        }
        r?;
        Ok(())
    });

    // Send task prints JSON responses to stdout.
    let cancelable = canceler.clone();
    let send_task: JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
        loop {
            let message = tokio::select! {
                _ = cancelable.cancelled() => break,
                message = out_receiver.next() => match message {
                    None => break,
                    Some(message) => serde_json::to_string(&message)?,
                }
            };
            log::trace!("RPC send {}", message);
            println!("{message}");
        }
        Ok(())
    });

    // Receiver task reads JSON requests from stdin.
    let recv_task: JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
        let stdin = io::stdin();
        let mut lines = BufReader::new(stdin).lines();
        loop {
            let message = tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    log::info!("got ctrl-c event");
                    break;
                }
                message = lines.next_line() => match message? {
                    None => {
                        log::info!("EOF reached on stdin");
                        break;
                    }
                    Some(message) => message,
                }
            };
            log::trace!("RPC recv {}", message);
            let session = session.clone();
            tokio::spawn(async move {
                session.handle_incoming(&message).await;
            });
        }
        Ok(())
    });

    // Wait for the end of stdin / ctrl-c.
    recv_task.await?.ok();

    // See "Thread safety" section in deltachat-ffi/deltachat.h for explanation.
    // NB: Events are drained by events_task.
    canceler.cancel();
    accounts.read().await.stop_io().await;
    drop(state);
    let (r0, r1) = tokio::join!(events_task, send_task);
    for r in [r0, r1] {
        r??;
    }

    Ok(())
}
