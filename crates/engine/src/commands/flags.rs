//! Feature flags as commands (#508): the list Ask reads to propose a
//! change, and the switch it applies once an admin confirms. The flag
//! rules themselves are in `openlens_flags`.

use openlens_flags::Flag;
use openlens_meta::Meta;
use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};

/// The project's flags, by key; unreadable rows are left out and a store
/// error reads as no flags.
#[must_use]
pub fn load(meta: &Meta, project: &str) -> Vec<Flag> {
    meta.flags(project)
        .map(|rows| {
            rows.iter()
                .filter_map(|j| serde_json::from_str(j).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Check and store one flag: why it was refused or not stored.
pub fn save(meta: &Meta, project: &str, f: &Flag) -> Result<(), String> {
    f.validate()?;
    let json = serde_json::to_string(f).map_err(|e| e.to_string())?;
    meta.save_flag(project, &f.key, &json)
        .map_err(|e| e.to_string())
}

/// The input of `flags.list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListInput {
    /// Project id.
    pub project: String,
}

/// `flags.list`: the project's flags.
pub struct List;

impl Command for List {
    const NAME: &'static str = "flags.list";
    const ACCESS: Access = Access::Read;
    type Input = ListInput;
    type Output = Vec<Flag>;

    async fn run<H: Host>(host: &H, _: &Caller, input: ListInput) -> Result<Vec<Flag>, Error> {
        Ok(load(host.meta(), &input.project))
    }
}

/// The input of `flags.update`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateInput {
    /// Project id.
    pub project: String,
    /// The flag's key.
    pub key: String,
    /// On or off; unchanged when absent.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Rollout percentage; unchanged when absent.
    #[serde(default)]
    pub rollout: Option<u8>,
}

/// `flags.update`: switch an existing flag on or off and/or set its
/// rollout.
pub struct Update;

impl Command for Update {
    const NAME: &'static str = "flags.update";
    const ACCESS: Access = Access::Write;
    type Input = UpdateInput;
    type Output = Flag;

    async fn run<H: Host>(host: &H, _: &Caller, input: UpdateInput) -> Result<Flag, Error> {
        let mut f = load(host.meta(), &input.project)
            .into_iter()
            .find(|x| x.key == input.key)
            .ok_or_else(|| Error::Invalid("The flag is gone.".into()))?;
        f.enabled = input.enabled.unwrap_or(f.enabled);
        f.rollout = input.rollout.unwrap_or(f.rollout);
        save(host.meta(), &input.project, &f).map_err(Error::Invalid)?;
        Ok(f)
    }
}
