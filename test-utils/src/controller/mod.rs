#![allow(async_fn_in_trait)]

use near_sdk::{
    serde::{de::DeserializeOwned, Serialize},
    serde_json, Gas, NearToken,
};
use near_workspaces::{result::ExecutionSuccess, Account, Contract};

pub mod ft;
pub mod market;
pub mod oracle;
pub mod registry;
pub mod storage_management;

pub trait ContractController {
    fn contract(&self) -> &Contract;

    async fn view<T: DeserializeOwned>(&self, function_name: &str, args: impl Serialize) -> T {
        self.contract()
            .view(function_name)
            .args_json(args)
            .await
            .unwrap()
            .json::<T>()
            .unwrap()
    }

    async fn call_exec(
        &self,
        account: &Account,
        function_name: &str,
        args: impl Serialize,
        deposit: NearToken,
        gas: Gas,
    ) -> ExecutionSuccess {
        eprintln!(
            "{} calls {}->{function_name}({})",
            &account.id().as_str()[0..16],
            &self.contract().id().as_str()[0..16],
            serde_json::to_string(&args).unwrap(),
        );
        account
            .call(self.contract().id(), function_name)
            .args_json(args)
            .deposit(deposit)
            .gas(gas)
            .transact()
            .await
            .unwrap()
            .unwrap()
    }

    async fn call<T: DeserializeOwned>(
        &self,
        account: &Account,
        function_name: &str,
        args: impl Serialize,
        deposit: NearToken,
        gas: Gas,
    ) -> T {
        eprintln!(
            "{} calls {}->{function_name}({})",
            &account.id().as_str()[0..16],
            &self.contract().id().as_str()[0..16],
            serde_json::to_string(&args).unwrap(),
        );
        account
            .call(self.contract().id(), function_name)
            .args_json(args)
            .deposit(deposit)
            .gas(gas)
            .transact()
            .await
            .unwrap()
            .json::<T>()
            .unwrap()
    }
}

#[macro_export]
macro_rules! define {
    (#[call] $($tail:tt)*) => {
        define! { #[call()] $($tail)* }
    };
    (#[call($($modifier:ident ($modifier_args:expr) ),*)] $v:vis fn $fn_name:ident ( $($args:tt)* ) ; $($tail:tt)* ) => {
        define! { @call $(#[$modifier ( $modifier_args )])* $v fn $fn_name ( $($args)* ) }
        define! { $($tail)* }
    };
    (#[call($($modifier:ident ($modifier_args:expr) ),*)] $v:vis fn $fn_name:ident ( $($args:tt)* ) -> $ret_t:ty ; $($tail:tt)* ) => {
        define! { @call $(#[$modifier ( $modifier_args )])* $v fn $fn_name ( $($args)* ) -> $ret_t }
        define! { $($tail)* }
    };
    (#[view] $v:vis fn $fn_name:ident ( $($args:tt)* ) -> $ret_t:ty ; $($tail:tt)* ) => {
        define! { @view $v fn $fn_name ( $($args)* ) -> $ret_t }
        define! { $($tail)* }
    };

    // Calls
    (@call #[deposit($d:expr)] #[gas($g:expr)] $v:vis fn $fn_name:ident ( $($arg:ident : $arg_t:ty),* ) -> $ret_t:ty) => {
        $v async fn $fn_name(
            &self,
            executor: &::near_workspaces::Account,
            $($arg : $arg_t),*
        ) -> $ret_t {
            $crate::controller::ContractController::call::<$ret_t>(
                self,
                executor,
                stringify!($fn_name),
                ::near_sdk::serde_json::json!({
                    $(stringify!($arg) : $arg),*
                }),
                $d,
                $g,
            )
            .await
        }
    };
    (@call #[deposit($d:expr)] #[gas($g:expr)] $v:vis fn $fn_name:ident ( $($arg:ident : $arg_t:ty),* )) => {
        $v async fn $fn_name(
            &self,
            executor: &::near_workspaces::Account,
            $($arg : $arg_t),*
        ) -> ::near_workspaces::result::ExecutionSuccess {
            $crate::controller::ContractController::call_exec(
                self,
                executor,
                stringify!($fn_name),
                ::near_sdk::serde_json::json!({
                    $(stringify!($arg) : $arg),*
                }),
                $d,
                $g,
            )
            .await
        }
    };
    (@call #[deposit($d:expr)] #[tgas($g:expr)] $($tail:tt)*) => {
        define! { @call #[deposit($d)] #[gas(::near_sdk::Gas::from_tgas($g))] $($tail)* }
    };
    (@call #[deposit($d:expr)] $($tail:tt)*) => {
        define! { @call #[deposit($d)] #[gas(::near_sdk::Gas::from_tgas(10))] $($tail)* }
    };
    (@call #[near($d:expr)] $($tail:tt)*) => {
        define! { @call #[deposit(::near_sdk::NearToken::from_near($d))] $($tail)* }
    };
    (@call #[yocto($d:expr)] $($tail:tt)*) => {
        define! { @call #[deposit(::near_sdk::NearToken::from_yoctonear($d))] $($tail)* }
    };
    (@call $($tail:tt)*) => {
        define! { @call #[deposit(::near_sdk::NearToken::from_near(0))] $($tail)* }
    };

    // Views
    (@view $v:vis fn $fn_name:ident ( $($arg:ident : $arg_t:ty),* ) -> $ret_t:ty) => {
        $v async fn $fn_name(
            &self,
            $($arg : $arg_t),*
        ) -> $ret_t {
            $crate::controller::ContractController::view::<$ret_t>(
                self,
                stringify!($fn_name),
                ::near_sdk::serde_json::json!({
                    $(stringify!($arg) : $arg),*
                }),
            )
            .await
        }
    };

    // Empty
    () => {};
}
