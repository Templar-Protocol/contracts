use blockchain_gateway_core::MethodSpec;

use crate::GatewayResult;

pub struct RpcMessage<Spec: MethodSpec>(pub Spec::Input);

impl<Spec: MethodSpec> actix::Message for RpcMessage<Spec> {
    type Result = GatewayResult<Spec::Output>;
}
