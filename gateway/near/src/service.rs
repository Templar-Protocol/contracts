use std::thread::JoinHandle;

use actix::Addr;

use blockchain_gateway_core::ManagedAccountId;

use crate::{
    actor::{
        map_mailbox_error,
        read::{ReadActor, ReadRpcRequest},
        rpc::RpcMessage,
        write::{sender_for, AccountWriteActor, WriteActors, WriteRpcRequest},
    },
    GatewayResult, NearReadClient, NearWriteClient,
};

#[derive(Clone)]
pub struct GatewayService {
    inner: std::sync::Arc<GatewayInner>,
    runtime: std::sync::Arc<std::sync::Mutex<Option<GatewayRuntime>>>,
}

struct GatewayInner {
    read: Addr<ReadActor>,
    write: std::collections::HashMap<ManagedAccountId, Addr<AccountWriteActor>>,
}

struct GatewayRuntime {
    system: actix::System,
    thread: JoinHandle<()>,
}

impl GatewayService {
    pub async fn shutdown(self) {
        let runtime = self
            .runtime
            .lock()
            .expect("gateway runtime lock poisoned")
            .take();
        if let Some(runtime) = runtime {
            runtime.system.stop();
            let _ = runtime.thread.join();
        }
    }

    pub fn spawn(near: NearReadClient, writer: NearWriteClient) -> Self {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let runner = actix::System::new();
            let system = actix::System::current();
            let arbiter = system.arbiter().clone();

            let read_addr = ReadActor::spawn(&arbiter, near);
            let write = WriteActors::new(writer).spawn(&arbiter);

            ready_tx
                .send((system, read_addr, write))
                .expect("gateway actor runtime receiver should be available during startup");

            runner
                .run()
                .expect("gateway actor runtime should stop cleanly");
        });

        let (system, read, write) = ready_rx
            .recv()
            .expect("gateway actor runtime should initialize before use");

        Self {
            inner: std::sync::Arc::new(GatewayInner { read, write }),
            runtime: std::sync::Arc::new(std::sync::Mutex::new(Some(GatewayRuntime {
                system,
                thread,
            }))),
        }
    }

    pub async fn request_read<Request>(
        &self,
        params: Request::Input,
    ) -> GatewayResult<Request::Output>
    where
        Request: ReadRpcRequest,
        ReadActor: actix::Handler<RpcMessage<Request>>,
    {
        self.inner
            .read
            .send(RpcMessage(params))
            .await
            .map_err(|error| map_mailbox_error(error, "read-actor"))?
    }

    pub async fn request_write<Request>(
        &self,
        params: Request::Input,
    ) -> GatewayResult<Request::Output>
    where
        Request: WriteRpcRequest,
        AccountWriteActor: actix::Handler<RpcMessage<Request>>,
    {
        let signer_id = Request::signer_account_id(&params);
        let sender = sender_for(&self.inner.write, signer_id)?;
        sender
            .send(RpcMessage(params))
            .await
            .map_err(|error| map_mailbox_error(error, "write-actor"))?
    }
}
