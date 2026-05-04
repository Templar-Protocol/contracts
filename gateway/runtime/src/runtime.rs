use std::thread::JoinHandle;

use crate::ReadActor;
use actix::Addr;
use templar_gateway_core::{GatewayError, GatewayResult};

pub struct GatewayRuntime {
    system: actix::System,
    thread: JoinHandle<()>,
}

impl GatewayRuntime {
    pub fn shutdown(self) {
        self.system.stop();
        let _ = self.thread.join();
    }
}

pub fn spawn_runtime<ContextType>(
    context: ContextType,
) -> GatewayResult<(GatewayRuntime, Addr<ReadActor<ContextType>>)>
where
    ContextType: Send + Clone + std::marker::Unpin + 'static,
{
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        tracing::debug!("starting gateway actor runtime");
        let runner = actix::System::new();
        let system = actix::System::current();
        let arbiter = system.arbiter().clone();

        let read = ReadActor::spawn(&arbiter, context);

        if ready_tx.send((system, read)).is_err() {
            return;
        }

        if let Err(error) = runner.run() {
            tracing::warn!(error = %error, "gateway actor runtime stopped with an error");
        }
        tracing::debug!("gateway actor runtime stopped");
    });

    let (system, read) = ready_rx.recv().map_err(|error| {
        GatewayError::ExternalService(format!("gateway actor runtime failed to start: {error}"))
    })?;

    tracing::debug!("gateway actor runtime ready");
    Ok((GatewayRuntime { system, thread }, read))
}
