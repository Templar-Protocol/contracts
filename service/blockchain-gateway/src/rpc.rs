use jsonrpsee::RpcModule;

pub fn rpc_module() -> RpcModule<()> {
    RpcModule::new(())
}

pub fn attach_gateway() -> RpcModule<()> {
    rpc_module()
}
