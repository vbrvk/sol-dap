use serde::Deserialize;
use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_test_config_parses() {
        let json = serde_json::json!({
            "project_root": "/tmp/proj",
            "test": "testFoo",
            "contract": "MyTest"
        });
        let config = LaunchConfig::from_args(&json).unwrap();
        assert_eq!(config.project_root, PathBuf::from("/tmp/proj"));
        assert_eq!(config.test.as_deref(), Some("testFoo"));
        assert_eq!(config.contract.as_deref(), Some("MyTest"));
        assert!(config.script.is_none());
    }

    #[test]
    fn valid_script_config_parses() {
        let json = serde_json::json!({
            "project_root": "/tmp/proj",
            "script": "script/Deploy.s.sol"
        });
        let config = LaunchConfig::from_args(&json).unwrap();
        assert_eq!(config.script.as_deref(), Some("script/Deploy.s.sol"));
        assert!(config.test.is_none());
    }

    #[test]
    fn rejects_missing_test_and_script() {
        let json = serde_json::json!({
            "project_root": "/tmp/proj"
        });
        let result = LaunchConfig::from_args(&json);
        assert!(result.is_err());
    }

    #[test]
    fn optional_fields_default_to_none() {
        let json = serde_json::json!({
            "project_root": "/tmp/proj",
            "test": "testBar"
        });
        let config = LaunchConfig::from_args(&json).unwrap();
        assert!(config.sig.is_none());
        assert!(config.profile.is_none());
        assert!(config.fork_url.is_none());
        assert!(config.fork_block_number.is_none());
        assert!(config.verbosity.is_none());
    }

    #[test]
    fn all_optional_fields_populated() {
        let json = serde_json::json!({
            "project_root": "/tmp/proj",
            "test": "testBaz",
            "contract": "BazTest",
            "sig": "run()",
            "profile": "ci",
            "fork_url": "https://rpc.example.com",
            "fork_block_number": 12345678,
            "verbosity": 3
        });
        let config = LaunchConfig::from_args(&json).unwrap();
        assert_eq!(config.sig.as_deref(), Some("run()"));
        assert_eq!(config.profile.as_deref(), Some("ci"));
        assert_eq!(config.fork_url.as_deref(), Some("https://rpc.example.com"));
        assert_eq!(config.fork_block_number, Some(12345678));
        assert_eq!(config.verbosity, Some(3));
    }
}
