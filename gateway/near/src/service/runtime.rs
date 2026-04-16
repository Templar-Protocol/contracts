use std::{collections::HashMap, thread::JoinHandle};

use actix::Addr;
use blockchain_gateway_core::ManagedAccountId;

use crate::{
    actor::{ManagedSigner, ReadActor, WriteActors},
    NearClient,
};

pub(super) struct GatewayRuntime {
    system: actix::System,
    thread: JoinHandle<()>,
}

impl GatewayRuntime {
    pub fn shutdown(self) {
        self.system.stop();
        let _ = self.thread.join();
    }
}

pub(super) fn spawn_runtime(
    near: NearClient,
    signers: HashMap<ManagedAccountId, ManagedSigner>,
) -> (GatewayRuntime, Addr<ReadActor>, WriteActors) {
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

    (GatewayRuntime { system, thread }, read, write)
}
