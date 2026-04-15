use std::{collections::HashMap, sync::Arc, thread::JoinHandle};

use actix::Addr;
use blockchain_gateway_core::ManagedAccountId;
use tokio::sync::Mutex;

use crate::{
    actor::{
        map_mailbox_error,
        read::{ReadActor, ReadRpcRequest},
        rpc::RpcMessage,
        write::{WriteActors, WriteRpcRequest},
    },
    GatewayResult, ManagedSigner, NearClient,
};

#[derive(Clone)]
pub struct GatewayService {
    inner: Arc<GatewayInner>,
    runtime: Arc<Mutex<Option<GatewayRuntime>>>,
}

struct GatewayInner {
    read: Addr<ReadActor>,
    write: WriteActors,
}

struct GatewayRuntime {
    system: actix::System,
    thread: JoinHandle<()>,
}

impl GatewayService {
    pub async fn shutdown(self) {
        let runtime = self.runtime.lock().await.take();
        if let Some(runtime) = runtime {
            runtime.system.stop();
            let _ = runtime.thread.join();
        }
    }

    pub fn spawn(near: NearClient, signers: HashMap<ManagedAccountId, ManagedSigner>) -> Self {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let runner = actix::System::new();
            let system = actix::System::current();
            let arbiter = system.arbiter().clone();

            let write = WriteActors::spawn(&arbiter, &near, signers);
            let read = ReadActor::spawn(&arbiter, near);

            ready_tx
                .send((system, read, write))
                .expect("gateway actor runtime receiver should be available during startup");

            runner
                .run()
                .expect("gateway actor runtime should stop cleanly");
        });

        let (system, read, write) = ready_rx
            .recv()
            .expect("gateway actor runtime should initialize before use");

        Self {
            inner: Arc::new(GatewayInner { read, write }),
            runtime: Arc::new(Mutex::new(Some(GatewayRuntime { system, thread }))),
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
    {
        self.inner.write.request::<Request>(params).await
    }
}
