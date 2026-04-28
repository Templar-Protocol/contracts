use std::thread::JoinHandle;

use crate::ReadActor;
use actix::Addr;
use templar_gateway_core::GatewayContext;

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

pub fn spawn_runtime(context: GatewayContext) -> (GatewayRuntime, Addr<ReadActor>) {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let runner = actix::System::new();
        let system = actix::System::current();
        let arbiter = system.arbiter().clone();

        let read = ReadActor::spawn(&arbiter, context);

        ready_tx
            .send((system, read))
            .expect("gateway actor runtime receiver should be available during startup");

        runner
            .run()
            .expect("gateway actor runtime should stop cleanly");
    });

    let (system, read) = ready_rx
        .recv()
        .expect("gateway actor runtime should initialize before use");

    (GatewayRuntime { system, thread }, read)
}
