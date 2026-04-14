pub mod chain;
pub mod market;
pub mod registry;
pub mod storage;
pub mod tx;
pub mod universal_account;

use crate::{
    actor::{read::ReadHandle, write::WriteHandle},
    NearReadClient, NearWriteClient,
};

#[derive(Clone)]
pub struct GatewayService {
    read: ReadHandle,
    write: WriteHandle,
}

impl GatewayService {
    pub fn new(near: NearReadClient, writer: NearWriteClient) -> Self {
        Self {
            read: crate::actor::read::spawn(near),
            write: crate::actor::write::spawn(writer),
        }
    }

    pub fn read(&self) -> &ReadHandle {
        &self.read
    }

    pub fn write(&self) -> &WriteHandle {
        &self.write
    }
}
