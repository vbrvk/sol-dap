use serde::Deserialize;
use std::path::PathBuf;

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct LaunchConfig {
    pub project_root: PathBuf,
    pub test: Option<String>,
    pub contract: Option<String>,
    pub script: Option<String>,
    pub sig: Option<String>,
    pub profile: Option<String>,
    pub fork_url: Option<String>,
    pub fork_block_number: Option<u64>,
    pub verbosity: Option<u8>,
}

impl LaunchConfig {
    pub fn from_args(args: &serde_json::Value) -> eyre::Result<Self> {
        let config: Self = serde_json::from_value(args.clone())?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> eyre::Result<()> {
        if self.test.is_none() && self.script.is_none() {
            eyre::bail!("Launch config must specify either 'test' or 'script'");
        }
        Ok(())
    }
}
