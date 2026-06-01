//! Typed skill contracts — our key differentiator.
//!
//! Unlike Hermes' markdown-based skills, ARLI uses structured
//! TOML/JSON Schema contracts that are validated at the harness level,
//! not by the LLM. This means:
//!
//! - Parameters are validated before the tool runs
//! - Versioning is explicit
//! - Safety policies are per-skill, not global
//! - Auto-generated JSON Schema for function calling

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A typed skill contract.
///
/// This is what gets defined in TOML files under `skills/`.
/// The harness validates parameters against `parameters` before execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillContract {
    /// Unique skill name (e.g., "execute_trade")
    pub name: String,

    /// Semantic version
    #[serde(default = "default_version")]
    pub version: String,

    /// Human-readable description for the LLM
    pub description: String,

    /// Parameter definitions (name → schema)
    #[serde(default)]
    pub parameters: HashMap<String, ParameterDef>,

    /// What the skill returns
    #[serde(default)]
    pub returns: Option<serde_json::Value>,

    /// Error codes the skill can produce
    #[serde(default)]
    pub errors: HashMap<String, String>,

    /// Safety configuration
    #[serde(default)]
    pub safety: SafetyConfig,

    /// Which toolset this skill belongs to
    #[serde(default = "default_toolset")]
    pub toolset: String,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

fn default_toolset() -> String {
    "core".to_string()
}

/// A single parameter definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterDef {
    /// JSON type: string, integer, float, boolean, object, array
    #[serde(rename = "type")]
    pub param_type: String,

    /// Human-readable description
    pub description: String,

    /// Is this parameter required?
    #[serde(default)]
    pub required: bool,

    /// Default value (as JSON)
    #[serde(default)]
    pub default: Option<serde_json::Value>,

    /// For integer/float: minimum value
    #[serde(default)]
    pub min: Option<f64>,

    /// For integer/float: maximum value
    #[serde(default)]
    pub max: Option<f64>,

    /// For strings: allowed enum values
    #[serde(default)]
    pub values: Option<Vec<String>>,
}

/// Safety configuration for a skill.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SafetyConfig {
    /// When does this skill need human approval?
    /// "never" | "always" | "above_size_{N}"
    #[serde(default = "default_approval")]
    pub approval: String,

    /// Rate limit (e.g., "10/minute", "1/second")
    #[serde(default)]
    pub rate_limit: Option<String>,

    /// Maximum value for trades (USD)
    #[serde(default)]
    pub max_value_usd: Option<f64>,
}

fn default_approval() -> String {
    "never".to_string()
}

impl SkillContract {
    /// Convert to a JSON Schema for LLM function calling.
    pub fn to_function_schema(&self) -> serde_json::Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for (name, param) in &self.parameters {
            let mut prop = serde_json::Map::new();
            prop.insert(
                "type".to_string(),
                serde_json::Value::String(param.param_type.clone()),
            );
            prop.insert(
                "description".to_string(),
                serde_json::Value::String(param.description.clone()),
            );

            if let Some(ref default) = param.default {
                prop.insert("default".to_string(), default.clone());
            }
            if let Some(min) = param.min {
                prop.insert("minimum".to_string(), serde_json::json!(min));
            }
            if let Some(max) = param.max {
                prop.insert("maximum".to_string(), serde_json::json!(max));
            }
            if let Some(ref values) = param.values {
                prop.insert("enum".to_string(), serde_json::json!(values));
            }

            properties.insert(name.clone(), serde_json::Value::Object(prop));

            if param.required {
                required.push(name.clone());
            }
        }

        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
        })
    }

    /// Validate arguments against the contract.
    pub fn validate(&self, args: &serde_json::Value) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        for (name, param) in &self.parameters {
            if param.required
                && !args
                    .as_object()
                    .map(|o| o.contains_key(name))
                    .unwrap_or(false)
            {
                errors.push(format!("Missing required parameter: {}", name));
                continue;
            }

            if let Some(value) = args.get(name) {
                // Type check
                match param.param_type.as_str() {
                    "string" => {
                        if !value.is_string() {
                            errors.push(format!("{}: expected string, got {}", name, value));
                        }
                        if let Some(ref allowed) = param.values {
                            if let Some(s) = value.as_str() {
                                if !allowed.contains(&s.to_string()) {
                                    errors.push(format!(
                                        "{}: '{}' not in allowed values: {:?}",
                                        name, s, allowed
                                    ));
                                }
                            }
                        }
                    }
                    "integer" | "float" => {
                        if !value.is_number() {
                            errors.push(format!("{}: expected number, got {}", name, value));
                        }
                        if let Some(n) = value.as_f64() {
                            if let Some(min) = param.min {
                                if n < min {
                                    errors.push(format!("{}: {} < minimum {}", name, n, min));
                                }
                            }
                            if let Some(max) = param.max {
                                if n > max {
                                    errors.push(format!("{}: {} > maximum {}", name, n, max));
                                }
                            }
                        }
                    }
                    "boolean" if !value.is_boolean() => {
                        errors.push(format!("{}: expected boolean, got {}", name, value));
                    }
                    _ => {} // object, array — defer to JSON Schema
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Registry of loaded skill contracts.
pub struct SkillRegistry {
    contracts: HashMap<String, SkillContract>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            contracts: HashMap::new(),
        }
    }

    /// Register a skill contract.
    pub fn register(&mut self, contract: SkillContract) {
        tracing::info!("Registered skill: {} v{}", contract.name, contract.version);
        self.contracts.insert(contract.name.clone(), contract);
    }

    /// Get a contract by name.
    pub fn get(&self, name: &str) -> Option<&SkillContract> {
        self.contracts.get(name)
    }

    /// Load contracts from a TOML string.
    pub fn load_toml(&mut self, toml_str: &str) -> Result<usize, toml::de::Error> {
        let contracts: Vec<SkillContract> = toml::from_str(toml_str)?;
        let count = contracts.len();
        for c in contracts {
            self.register(c);
        }
        Ok(count)
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_contract_validation() {
        let contract = SkillContract {
            name: "test_skill".into(),
            version: "1.0.0".into(),
            description: "test".into(),
            parameters: {
                let mut p = HashMap::new();
                p.insert(
                    "amount".into(),
                    ParameterDef {
                        param_type: "float".into(),
                        description: "amount".into(),
                        required: true,
                        default: None,
                        min: Some(0.0),
                        max: Some(1000.0),
                        values: None,
                    },
                );
                p.insert(
                    "side".into(),
                    ParameterDef {
                        param_type: "string".into(),
                        description: "side".into(),
                        required: true,
                        default: None,
                        min: None,
                        max: None,
                        values: Some(vec!["long".into(), "short".into()]),
                    },
                );
                p
            },
            returns: None,
            errors: HashMap::new(),
            safety: SafetyConfig::default(),
            toolset: "core".into(),
        };

        // Valid
        let valid = serde_json::json!({"amount": 500.0, "side": "long"});
        assert!(contract.validate(&valid).is_ok());

        // Missing required
        let missing = serde_json::json!({"amount": 500.0});
        assert!(contract.validate(&missing).is_err());

        // Invalid enum
        let invalid_side = serde_json::json!({"amount": 500.0, "side": "diagonal"});
        assert!(contract.validate(&invalid_side).is_err());

        // Out of range
        let out_of_range = serde_json::json!({"amount": 5000.0, "side": "long"});
        assert!(contract.validate(&out_of_range).is_err());
    }

    #[test]
    fn test_to_function_schema() {
        let contract = SkillContract {
            name: "test".into(),
            version: "1.0.0".into(),
            description: "test".into(),
            parameters: {
                let mut p = HashMap::new();
                p.insert(
                    "x".into(),
                    ParameterDef {
                        param_type: "integer".into(),
                        description: "x coord".into(),
                        required: true,
                        default: None,
                        min: None,
                        max: None,
                        values: None,
                    },
                );
                p
            },
            returns: None,
            errors: HashMap::new(),
            safety: SafetyConfig::default(),
            toolset: "core".into(),
        };

        let schema = contract.to_function_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("x")));
    }
}
