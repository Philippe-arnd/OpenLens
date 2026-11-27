//! Capacity as a command (#606, D-088): the recordings a day the disk
//! budget gives, the budget behind them and why, with the fixed replay
//! and event retention (D-015) for the record. The rules are in [`crate::capacity`]; the host measures.

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::capacity::{self, Capacity};

/// The input of `capacity.get`: none, the capacity is the instance's.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetInput {}

/// `capacity.get`: the values in force and their reasons.
pub struct Get;

impl Command for Get {
    const NAME: &'static str = "capacity.get";
    // The whole instance's disk and every project's cost.
    const ACCESS: Access = Access::Admin;
    type Input = GetInput;
    type Output = Capacity;

    async fn run<H: Host>(host: &H, _: &Caller, _: GetInput) -> Result<Capacity, Error> {
        Ok(capacity::derive(&host.capacity_inputs()))
    }
}
