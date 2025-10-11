#![allow(async_fn_in_trait)]

use near_sdk::{
    serde::{de::DeserializeOwned, Serialize},
    serde_json, Gas, NearToken,
};
use near_workspaces::{result::ExecutionSuccess, Account, Contract};

pub mod ft;
pub mod lst_oracle;
pub mod market;
pub mod mt;
pub mod oracle;
pub mod registry;
pub mod storage_management;
pub mod token;
pub mod universal_account;

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

    async fn call_raw(
        &self,
        account: &Account,
        function_name: &str,
        args: Vec<u8>,
        deposit: NearToken,
        gas: Gas,
    ) -> ExecutionSuccess {
        eprintln!(
            "{} calls {}->{function_name}(...)",
            &account.id().as_str()[0..16],
            &self.contract().id().as_str()[0..16],
        );
        account
            .call(self.contract().id(), function_name)
            .args(args)
            .deposit(deposit)
            .gas(gas)
            .transact()
            .await
            .unwrap()
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
        let args_s = serde_json::to_string(&args).unwrap();
        const TARGET_LEN: usize = 128;
        let args_s = if args_s.len() > TARGET_LEN {
            format!("{}...", &args_s[0..(TARGET_LEN - 3)])
        } else {
            args_s
        };
        eprintln!(
            "{} calls {}->{function_name}({args_s})",
            &account.id().as_str()[0..16],
            &self.contract().id().as_str()[0..16],
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
            &{
                let mut a = serde_json::to_string(&args).unwrap();
                a.truncate(256);
                a
            },
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
    (#[call($($m:tt)*)] $v:vis fn $fn_name:ident ( $($args:tt)* ) $(-> $ret_t:ty)? ; $($tail:tt)* ) => {
        define! { @call($($m)*) $v fn $fn_name [stringify!($fn_name)] ( $($args)* ) $(-> $ret_t)? }
        define! { $($tail)* }
    };
    (#[call($($m:tt)*)] $v:vis fn $fn_name:ident [$call_name:literal] ( $($args:tt)* ) $(-> $ret_t:ty)? ; $($tail:tt)* ) => {
        define! { @call($($m)*) $v fn $fn_name [$call_name] ( $($args)* ) $(-> $ret_t)? }
        define! { $($tail)* }
    };
    (#[view] $v:vis fn $fn_name:ident ( $($args:tt)* ) -> $ret_t:ty ; $($tail:tt)* ) => {
        define! { @view $v fn $fn_name ( $($args)* ) -> $ret_t }
        define! { $($tail)* }
    };

    (@modifiers($d:ident,$g:ident)) => {};
    (@modifiers($d:ident,$g:ident) , $($tail:tt)*) => {
        define! { @modifiers($d,$g) $($tail)* }
    };
    (@modifiers($d:ident,$g:ident) near($v:expr) $($tail:tt)*) => {
        $d = ::near_sdk::NearToken::from_near($v);
        define! { @modifiers($d,$g) $($tail)* }
    };
    (@modifiers($d:ident,$g:ident) yocto($v:expr) $($tail:tt)*) => {
        $d = ::near_sdk::NearToken::from_yoctonear($v);
        define! { @modifiers($d,$g) $($tail)* }
    };
    (@modifiers($d:ident,$g:ident) deposit($v:expr) $($tail:tt)*) => {
        $d = $v;
        define! { @modifiers($d,$g) $($tail)* }
    };
    (@modifiers($d:ident,$g:ident) tgas($v:expr) $($tail:tt)*) => {
        $g = ::near_sdk::Gas::from_tgas($v);
        define! { @modifiers($d,$g) $($tail)* }
    };
    (@modifiers($d:ident,$g:ident) gas($v:expr) $($tail:tt)*) => {
        $g = $v;
        define! { @modifiers($d,$g) $($tail)* }
    };
    (@modifiers($d:ident,$g:ident) exec $($tail:tt)*) => {
        define! { @modifiers($d,$g) $($tail)* }
    };

    (@modifiers is_exec() then($($then:tt)*)) => { };
    (@modifiers is_exec() then($($then:tt)*) else($($tail:tt)*)) => {
        $($tail)*
    };
    (@modifiers is_exec(exec $($m:tt)*) then($($then:tt)*) $($tail:tt)*) => {
        $($then)*
    };
    (@modifiers is_exec($no:tt $($m:tt)*) $($tail:tt)*) => {
        define! { @modifiers is_exec($($m)*) $($tail)* }
    };

    // Calls
    (@call($($m:tt)*) $v:vis fn $fn_name:ident [$call_name:expr] ( $($arg:ident : $arg_t:ty),* ) $(-> $ret_t:ty)?) => {
        #[allow(unused_parens)]
        $v async fn $fn_name(
            &self,
            executor: &::near_workspaces::Account,
            $($arg : impl Into<$arg_t>),*
        ) -> define! { @modifiers is_exec($($m)*) then(::near_workspaces::result::ExecutionSuccess) else(($($ret_t)?)) } {
            #[allow(unused_assignments, unused_mut)]
            let mut deposit = ::near_sdk::NearToken::from_near(0);
            #[allow(unused_assignments, unused_mut)]
            let mut gas = ::near_sdk::Gas::from_tgas(10);

            define! { @modifiers(deposit, gas) $($m)* };

            let call = define! { @modifiers is_exec($($m)*) then($crate::controller::ContractController::call_exec) else($crate::controller::ContractController::call::<($($ret_t)?)>) };

            call(
                self,
                executor,
                $call_name,
                ::near_sdk::serde_json::json!({
                    $(stringify!($arg) : Into::<$arg_t>::into($arg)),*
                }),
                deposit,
                gas,
            )
            .await
        }
    };

    // Views
    (@view $v:vis fn $fn_name:ident ( $($arg:ident : $arg_t:ty),* ) -> $ret_t:ty) => {
        $v async fn $fn_name(
            &self,
            $($arg : impl Into<$arg_t>),*
        ) -> $ret_t {
            $crate::controller::ContractController::view::<$ret_t>(
                self,
                stringify!($fn_name),
                ::near_sdk::serde_json::json!({
                    $(stringify!($arg) : Into::<$arg_t>::into($arg)),*
                }),
            )
            .await
        }
    };

    // Empty
    () => {};
}
