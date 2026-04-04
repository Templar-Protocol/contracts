mod rpc;

#[tokio::main]
async fn main() {
    let _ = rpc::rpc_module();
}
