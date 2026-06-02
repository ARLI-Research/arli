//! Sandbox policy types — OpenShell-inspired layered security model.
//!
//! Defines filesystem, process, network, and Landlock restrictions
//! that are enforced at sandbox startup and during execution.

use serde::{Deserialize, Serialize};

/// Complete sandbox security policy — combines all isolation layers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Filesystem access rules (Landlock-enforced)
    pub filesystem: FilesystemPolicy,

    /// Process identity and restrictions
    pub process: ProcessPolicy,

    /// Network access rules
    pub network: NetworkPolicy,

    /// Landlock ABI compatibility mode
    pub landlock: LandlockConfig,
}

/// Filesystem access policy — enforced via Landlock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemPolicy {
    /// Paths the sandbox can read (but not write)
    #[serde(default)]
    pub read_only: Vec<String>,

    /// Paths the sandbox can read and write
    #[serde(default)]
    pub read_write: Vec<String>,

    /// Automatically add the working directory as read-write
    #[serde(default)]
    pub include_workdir: bool,
}

/// Process identity and restriction policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessPolicy {
    /// Username to drop privileges to (must exist on system)
    #[serde(default = "default_user")]
    pub run_as_user: String,

    /// Group name to drop privileges to
    #[serde(default = "default_group")]
    pub run_as_group: String,

    /// Allow core dumps from sandboxed process
    #[serde(default)]
    pub allow_core_dumps: bool,
}

fn default_user() -> String {
    "nobody".into()
}

fn default_group() -> String {
    "nogroup".into()
}

/// Network access policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicy {
    /// Network isolation mode
    #[serde(default)]
    pub mode: NetworkMode,

    /// Allowed outbound endpoints (only relevant for Proxy mode)
    #[serde(default)]
    pub allowed_endpoints: Vec<NetworkEndpoint>,
}

/// Network isolation modes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    /// All network access blocked
    #[default]
    Block,

    /// Traffic routed through a policy-enforcing proxy
    Proxy,

    /// Full network access (no restrictions)
    Allow,
}

/// A single allowed network endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEndpoint {
    /// Host pattern (exact or wildcard: `*.example.com`, `**.example.com`)
    pub host: String,

    /// Optional port restriction
    pub port: Option<u16>,

    /// Require TLS for this endpoint
    #[serde(default)]
    pub tls: bool,
}

/// Landlock ABI compatibility configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LandlockConfig {
    /// Compatibility mode: `best_effort` (default) or `hard_requirement`
    #[serde(default)]
    pub compatibility: LandlockCompatibility,
}

/// Landlock enforcement strictness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LandlockCompatibility {
    /// Landlock is optional — proceed without it if unavailable
    #[default]
    BestEffort,

    /// Landlock is mandatory — fail if not fully enforced
    HardRequirement,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self::restrictive_default()
    }
}

impl SandboxPolicy {
    /// Restrictive default policy — blocks everything except essential system paths.
    ///
    /// Read-only: /usr, /lib, /lib64, /bin, /etc
    /// Read-write: /tmp, /dev/null, /dev/urandom
    /// Network: fully blocked
    /// Landlock: best-effort (works without it)
    pub fn restrictive_default() -> Self {
        Self {
            filesystem: FilesystemPolicy {
                read_only: vec![
                    "/usr".into(),
                    "/lib".into(),
                    "/lib64".into(),
                    "/bin".into(),
                    "/etc".into(),
                ],
                read_write: vec!["/tmp".into(), "/dev/null".into(), "/dev/urandom".into()],
                include_workdir: true,
            },
            process: ProcessPolicy {
                run_as_user: "nobody".into(),
                run_as_group: "nogroup".into(),
                allow_core_dumps: false,
            },
            network: NetworkPolicy {
                mode: NetworkMode::Block,
                allowed_endpoints: vec![],
            },
            landlock: LandlockConfig {
                compatibility: LandlockCompatibility::BestEffort,
            },
        }
    }

    /// Permissive policy — full filesystem and network access, no privilege drop.
    /// Suitable for trusted workloads where isolation isn't needed.
    pub fn permissive() -> Self {
        Self {
            filesystem: FilesystemPolicy {
                read_only: vec![],
                read_write: vec!["/".into()],
                include_workdir: true,
            },
            process: ProcessPolicy {
                run_as_user: "nobody".into(),
                run_as_group: "nogroup".into(),
                allow_core_dumps: true,
            },
            network: NetworkPolicy {
                mode: NetworkMode::Allow,
                allowed_endpoints: vec![],
            },
            landlock: LandlockConfig {
                compatibility: LandlockCompatibility::BestEffort,
            },
        }
    }

    /// Parse policy from YAML string.
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }

    /// Serialize policy to YAML string.
    pub fn to_yaml(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_restrictive_default_policy() {
        let policy = SandboxPolicy::restrictive_default();
        assert!(policy.filesystem.read_only.contains(&"/usr".to_string()));
        assert!(policy.filesystem.read_write.contains(&"/tmp".to_string()));
        assert_eq!(policy.process.run_as_user, "nobody");
        assert_eq!(policy.network.mode, NetworkMode::Block);
    }

    #[test]
    fn test_permissive_policy() {
        let policy = SandboxPolicy::permissive();
        assert_eq!(policy.network.mode, NetworkMode::Allow);
        assert!(policy.filesystem.read_write.contains(&"/".to_string()));
    }

    #[test]
    fn test_default_policy_is_restrictive() {
        let policy = SandboxPolicy::default();
        assert_eq!(policy.network.mode, NetworkMode::Block);
    }

    #[test]
    fn test_parse_policy_yaml() {
        let yaml = r#"
filesystem:
  read_only:
    - /usr
    - /etc
  read_write:
    - /tmp
    - /workspace
  include_workdir: true
process:
  run_as_user: nobody
  run_as_group: nogroup
  allow_core_dumps: false
network:
  mode: proxy
  allowed_endpoints:
    - host: api.github.com
      port: 443
      tls: true
    - host: "*.example.com"
      tls: false
landlock:
  compatibility: hard_requirement
"#;
        let policy: SandboxPolicy = SandboxPolicy::from_yaml(yaml).unwrap();
        assert_eq!(policy.network.mode, NetworkMode::Proxy);
        assert_eq!(policy.filesystem.read_only, vec!["/usr", "/etc"]);
        assert_eq!(
            policy.landlock.compatibility,
            LandlockCompatibility::HardRequirement
        );
        assert_eq!(policy.network.allowed_endpoints.len(), 2);
        assert_eq!(policy.network.allowed_endpoints[0].host, "api.github.com");
        assert!(policy.network.allowed_endpoints[0].tls);
        assert_eq!(policy.network.allowed_endpoints[1].host, "*.example.com");
        assert!(!policy.network.allowed_endpoints[1].tls);
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let policy = SandboxPolicy::restrictive_default();
        let yaml = policy.to_yaml().unwrap();
        let parsed: SandboxPolicy = SandboxPolicy::from_yaml(&yaml).unwrap();
        assert_eq!(parsed.network.mode, policy.network.mode);
        assert_eq!(parsed.filesystem.read_only, policy.filesystem.read_only);
    }
}
