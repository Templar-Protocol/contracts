macro_rules! contract_views {
    ($($vis:vis fn $fn_name:ident $([$method:literal])? ($args_ty:ty) -> $contract_return_type:ty;)+) => {
        $(
            $vis async fn $fn_name(&self, args: $args_ty) -> $crate::GatewayResult<$contract_return_type> {
                let client = $crate::client::BoundContractClient::client(self);
                let result: $contract_return_type = client
                    .contract($crate::client::BoundContractClient::contract_id(self).to_owned())
                    .view_function(contract_views!(@method $fn_name $(, $method)?), ::serde_json::to_vec(&args)?)
                    .await?;

                Ok(result)
            }
        )+
    };

    (@method $fn_name:ident, $method:literal) => {
        $method
    };

    (@method $fn_name:ident) => {
        stringify!($fn_name)
    };
}

macro_rules! contract_writes {
    ($($vis:vis fn $fn_name:ident $([$method:literal])? ($args_ty:ty) ; )+) => {
        $(
            $vis async fn $fn_name(
                &self,
                options: $crate::client::ContractWriteOptions,
                args: $args_ty,
            ) -> $crate::GatewayResult<::near_api::types::transaction::result::TransactionResult> {
                let client = $crate::client::BoundContractClient::client(self);
                client
                    .tx(options.signer_account_id, options.signer.expect("signer should be present for immediate contract write"))
                    .function_call(
                        ::blockchain_gateway_core::tx::FunctionCallBody {
                            receiver_id: $crate::client::BoundContractClient::contract_id(self).to_owned(),
                            method_name: ::blockchain_gateway_core::ContractMethodName(
                                contract_writes!(@method $fn_name $(, $method)?).to_owned(),
                            ),
                            args: ::blockchain_gateway_core::common::ContractArgs::Json(::serde_json::to_value(&args)?),
                            gas: options.gas,
                            deposit: options.deposit,
                        },
                        options.wait_until,
                    )
                    .await
            }
        )+
    };

    (@method $fn_name:ident, $method:literal) => {
        $method
    };

    (@method $fn_name:ident) => {
        stringify!($fn_name)
    };
}

pub(crate) use contract_views;
pub(crate) use contract_writes;
