use clap::{Args, Subcommand};
use templar_gateway_methods_spec::op as spec;
use templar_gateway_types::OperationId;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum OpNs {
    Get(Get),
}

#[derive(Args, Debug)]
pub struct Get {
    #[arg(long, value_name = "OPERATION_ID")]
    operation_id: String,
}

impl Get {
    pub fn parse(self) -> spec::Get {
        spec::Get {
            operation_id: OperationId(self.operation_id),
        }
    }
}
