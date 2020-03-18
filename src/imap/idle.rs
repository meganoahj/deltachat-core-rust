use super::Imap;

use async_imap::extensions::idle::IdleResponse;
use async_std::prelude::*;
use std::time::{Duration, SystemTime};

use crate::context::Context;

use super::select_folder;
use super::session::Session;

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Fail)]
pub enum Error {
    #[fail(display = "IMAP IDLE protocol failed to init/complete")]
    IdleProtocolFailed(#[cause] async_imap::error::Error),

    #[fail(display = "IMAP IDLE protocol timed out")]
    IdleTimeout(#[cause] async_std::future::TimeoutError),

    #[fail(display = "IMAP server does not have IDLE capability")]
    IdleAbilityMissing,

    #[fail(display = "IMAP select folder error")]
    SelectFolderError(#[cause] select_folder::Error),

    #[fail(display = "Setup handle error")]
    SetupHandleError(#[cause] super::Error),
}

impl From<select_folder::Error> for Error {
    fn from(err: select_folder::Error) -> Error {
        Error::SelectFolderError(err)
    }
}

impl Imap {
    pub fn can_idle(&self) -> bool {
        self.config.can_idle
    }

    pub async fn idle(&mut self, context: &Context, watch_folder: Option<String>) -> Result<()> {
        use futures::future::FutureExt;

        if !self.can_idle() {
            return Err(Error::IdleAbilityMissing);
        }

        self.setup_handle_if_needed(context)
            .await
            .map_err(Error::SetupHandleError)?;

        self.select_folder(context, watch_folder.clone()).await?;

        let session = self.session.take();
        let timeout = Duration::from_secs(23 * 60);

        if let Some(session) = session {
            let mut handle = session.idle();
            if let Err(err) = handle.init().await {
                return Err(Error::IdleProtocolFailed(err));
            }

            let (idle_wait, interrupt) = handle.wait_with_timeout(timeout);

            if self.skip_next_idle_wait {
                // interrupt_idle has happened before we
                // provided self.interrupt
                self.skip_next_idle_wait = false;
                drop(idle_wait);
                drop(interrupt);

                info!(context, "Idle wait was skipped");
            } else {
                info!(context, "Idle entering wait-on-remote state");
                let fut = idle_wait.race(
                    self.idle_interrupt
                        .recv()
                        .map(|_| Ok(IdleResponse::ManualInterrupt)),
                );

                match fut.await {
                    Ok(IdleResponse::NewData(_)) => {
                        info!(context, "Idle has NewData");
                    }
                    // TODO: idle_wait does not distinguish manual interrupts
                    // from Timeouts if we would know it's a Timeout we could bail
                    // directly and reconnect .
                    Ok(IdleResponse::Timeout) => {
                        info!(context, "Idle-wait timeout or interruption");
                    }
                    Ok(IdleResponse::ManualInterrupt) => {
                        info!(context, "Idle wait was interrupted");
                    }
                    Err(err) => {
                        warn!(context, "Idle wait errored: {:?}", err);
                    }
                }
            }

            // if we can't properly terminate the idle
            // protocol let's break the connection.
            let res = handle
                .done()
                .timeout(Duration::from_secs(15))
                .await
                .map_err(|err| {
                    self.trigger_reconnect();
                    Error::IdleTimeout(err)
                })?;

            match res {
                Ok(session) => {
                    self.session = Some(Session { inner: session });
                }
                Err(err) => {
                    // if we cannot terminate IDLE it probably
                    // means that we waited long (with idle_wait)
                    // but the network went away/changed
                    self.trigger_reconnect();
                    return Err(Error::IdleProtocolFailed(err));
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn fake_idle(&mut self, context: &Context, watch_folder: Option<String>) {
        // Idle using polling. This is also needed if we're not yet configured -
        // in this case, we're waiting for a configure job (and an interrupt).

        let fake_idle_start_time = SystemTime::now();
        info!(context, "IMAP-fake-IDLEing...");

        if self.skip_next_idle_wait {
            // interrupt_idle has happened before we
            // provided self.interrupt
            self.skip_next_idle_wait = false;
            info!(context, "fake-idle wait was skipped");
        } else {
            // check every minute if there are new messages
            // TODO: grow sleep durations / make them more flexible
            let mut interval = async_std::stream::interval(Duration::from_secs(60));

            // loop until we are interrupted or if we fetched something
            loop {
                use futures::future::FutureExt;
                match interval
                    .next()
                    .race(self.idle_interrupt.recv().map(|_| None))
                    .await
                {
                    Some(_) => {
                        // try to connect with proper login params
                        // (setup_handle_if_needed might not know about them if we
                        // never successfully connected)
                        if let Err(err) = self.connect_configured(context).await {
                            warn!(context, "fake_idle: could not connect: {}", err);
                            continue;
                        }
                        if self.config.can_idle {
                            // we only fake-idled because network was gone during IDLE, probably
                            break;
                        }
                        info!(context, "fake_idle is connected");
                        // we are connected, let's see if fetching messages results
                        // in anything.  If so, we behave as if IDLE had data but
                        // will have already fetched the messages so perform_*_fetch
                        // will not find any new.

                        if let Some(ref watch_folder) = watch_folder {
                            match self.fetch_new_messages(context, watch_folder).await {
                                Ok(res) => {
                                    info!(context, "fetch_new_messages returned {:?}", res);
                                    if res {
                                        break;
                                    }
                                }
                                Err(err) => {
                                    error!(context, "could not fetch from folder: {}", err);
                                    self.trigger_reconnect()
                                }
                            }
                        }
                    }
                    None => {
                        // Interrupt
                        break;
                    }
                }
            }
        }

        info!(
            context,
            "IMAP-fake-IDLE done after {:.4}s",
            SystemTime::now()
                .duration_since(fake_idle_start_time)
                .unwrap_or_default()
                .as_millis() as f64
                / 1000.,
        );
    }
}
