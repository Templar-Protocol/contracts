use crate::{
    actor::{
        Actor, ActorGroup,
        read::{ReadActor, ReadHandle},
        write::{WriteActors, WriteHandle},
    },
    NearReadClient, NearWriteClient,
};

#[derive(Clone)]
pub struct GatewayService {
    read: ReadHandle,
    write: WriteHandle,
}

pub struct GatewayRuntime {
    actors: ActorGroup,
}

impl GatewayRuntime {
    pub async fn shutdown(self) {
        self.actors.shutdown_and_wait().await;
    }
}

impl GatewayService {
    pub fn spawn(near: NearReadClient, writer: NearWriteClient) -> (Self, GatewayRuntime) {
        let (read, read_task) = ReadActor::new(near).spawn();
        let (write, write_tasks) = WriteActors::new(writer).spawn();

        let mut actors = ActorGroup::new();
        actors.push(read_task);
        actors.extend_group(write_tasks);

        (
            Self { read, write },
            GatewayRuntime { actors },
        )
    }

    pub fn read(&self) -> &ReadHandle {
        &self.read
    }

    pub fn write(&self) -> &WriteHandle {
        &self.write
    }
}
