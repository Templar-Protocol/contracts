use std::{collections::HashMap, sync::Arc};

use actix::Addr;
use blockchain_gateway_core::ManagedAccountId;
use tokio::sync::Mutex;

use crate::{
    actor::{
        map_mailbox_error, DispatchRead, DispatchWrite, ManagedSigner, ReadActor, RpcMessage,
        WriteActors,
    },
    GatewayResult,
};

use super::runtime::{spawn_runtime, GatewayRuntime};

#[derive(Clone)]
pub struct GatewayService {
    inner: Arc<GatewayInner>,
    runtime: Arc<Mutex<Option<GatewayRuntime>>>,
}

struct GatewayInner {
    read: Addr<ReadActor>,
    write: WriteActors,
}

impl GatewayService {
    pub fn spawn(
        near: crate::NearClient,
        signers: HashMap<ManagedAccountId, ManagedSigner>,
    ) -> Self {
        let (runtime, read, write) = spawn_runtime(near, signers);

        Self {
            inner: Arc::new(GatewayInner { read, write }),
            runtime: Arc::new(Mutex::new(Some(runtime))),
        }
    }

    pub async fn shutdown(self) {
        if let Some(runtime) = self.runtime.lock().await.take() {
            runtime.shutdown();
        }
    }

    pub async fn request_read<Request>(
        &self,
        params: Request::Input,
    ) -> GatewayResult<Request::Output>
    where
        Request: DispatchRead,
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
        Request: DispatchWrite,
    {
        self.inner.write.request::<Request>(params).await
    }
}
