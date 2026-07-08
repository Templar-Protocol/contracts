use clap::Args;
use templar_gateway_types::common::Pagination;

/// The shared `--offset`/`--count` window for list commands, flattened into each
/// so pagination looks like any other pair of flags.
#[derive(Args, Debug)]
pub struct PaginationArgs {
    /// Number of items to skip before the first returned item.
    #[arg(long)]
    pub offset: Option<u32>,
    /// Maximum number of items to return.
    #[arg(long)]
    pub count: Option<u32>,
}

impl PaginationArgs {
    /// The gateway `Pagination` window (whose `limit` is this `count`).
    pub fn into_pagination(self) -> Pagination {
        Pagination {
            offset: self.offset,
            limit: self.count,
        }
    }
}
