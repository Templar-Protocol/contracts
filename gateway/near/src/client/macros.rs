macro_rules! contract_views {
    ($($vis:vis fn $fn_name:ident $([$method:literal])? ($args_ty:ty) -> $contract_return_type:ty;)+) => {
        $(
            $vis async fn $fn_name(&self, args: $args_ty) -> $crate::GatewayResult<$contract_return_type> {
                let client = $crate::client::ContractClient::client(self);
                let result: ::near_api::types::Data<$contract_return_type> = client
                    .view_json($crate::client::ContractClient::contract_id(self).to_owned(), contract_views!(@method $fn_name $(, $method)?), args)
                    .await?;

                Ok(result.data)
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
