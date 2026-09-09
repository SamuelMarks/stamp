#![cfg_attr(coverage_nightly, coverage(off))]
//! Schema validation and type coercion for plugin configurations.
//!
//! Provides dynamic schema negotiation between Stamp and external plugins:
//! - Attribute-level type validation (`String`, `Bool`, `Int`, `Float`, `List`, `Map`).
//! - Checking required vs optional attributes with default values.
//! - Validating parsed HCL `Body` AST structures directly.
//! - gRPC `Schema` service server and client implementations.

use crate::error::StampError;
use hashicorp_configuration_language_rs::ast::structure::Body;
use hashicorp_configuration_language_rs::types::{Type, Value, ValueData};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tonic::{Request, Response, Status};

/// Defines the expected type for a configuration field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaType {
    /// String type.
    String,
    /// Boolean type.
    Bool,
    /// Integer type.
    Int,
    /// Floating point number type.
    Float,
    /// List / array type.
    List,
    /// Map / key-value dictionary type.
    Map,
}

/// Metadata and validation rules for a single plugin configuration attribute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaAttribute {
    /// Attribute name.
    pub name: String,
    /// Expected data type.
    pub field_type: SchemaType,
    /// Whether this attribute is required in the configuration.
    pub required: bool,
    /// Human-readable documentation for the attribute.
    pub description: String,
    /// Optional default string value.
    pub default_val: Option<String>,
}

impl SchemaAttribute {
    /// Creates a new `SchemaAttribute`.
    #[must_use]
    pub fn new(name: impl Into<String>, field_type: SchemaType) -> Self {
        Self {
            name: name.into(),
            field_type,
            required: false,
            description: String::new(),
            default_val: None,
        }
    }

    /// Marks the attribute as required.
    #[must_use]
    pub const fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Adds a description to the attribute.
    #[must_use]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Sets a default value for the attribute.
    #[must_use]
    pub fn with_default(mut self, default_val: impl Into<String>) -> Self {
        self.default_val = Some(default_val.into());
        self
    }
}

/// A schema for a plugin's configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginSchema {
    /// Legacy map of field names to their expected types.
    pub fields: HashMap<String, SchemaType>,
    /// Comprehensive attribute schema metadata.
    pub attributes: HashMap<String, SchemaAttribute>,
}

impl PluginSchema {
    /// Create a new empty schema.
    #[must_use]
    pub fn new() -> Self {
        Self {
            fields: HashMap::new(),
            attributes: HashMap::new(),
        }
    }

    /// Add a field to the schema.
    #[must_use]
    pub fn add_field(mut self, name: &str, field_type: SchemaType) -> Self {
        self.fields.insert(name.to_string(), field_type.clone());
        self.attributes
            .insert(name.to_string(), SchemaAttribute::new(name, field_type));
        self
    }

    /// Add a full `SchemaAttribute` definition to the schema.
    #[must_use]
    pub fn add_attribute(mut self, attr: SchemaAttribute) -> Self {
        self.fields
            .insert(attr.name.clone(), attr.field_type.clone());
        self.attributes.insert(attr.name.clone(), attr);
        self
    }

    /// Serializes the schema to a JSON string.
    ///
    /// # Errors
    /// Returns `StampError::Json` on serialization failure.
    pub fn to_json(&self) -> Result<String, StampError> {
        serde_json::to_string(self).map_err(StampError::Json)
    }

    /// Deserializes a schema from a JSON string.
    ///
    /// # Errors
    /// Returns `StampError::Json` on deserialization failure.
    pub fn from_json(json_str: &str) -> Result<Self, StampError> {
        serde_json::from_str(json_str).map_err(StampError::Json)
    }

    /// Validates an HCL configuration `Body` against this schema.
    ///
    /// # Errors
    /// Returns `StampError::Validation` if a required attribute is missing or an unknown attribute is present.
    pub fn validate_body(&self, body: &Body) -> Result<(), StampError> {
        // 1. Check all required attributes are present
        for (name, attr) in &self.attributes {
            if attr.required && !body.attributes.contains_key(name) {
                return Err(StampError::Validation(format!(
                    "Missing required configuration attribute '{name}'"
                )));
            }
        }

        // 2. Check for unexpected attributes if attributes are defined
        if !self.attributes.is_empty() {
            for name in body.attributes.keys() {
                if !self.attributes.contains_key(name) && !self.fields.contains_key(name) {
                    return Err(StampError::Validation(format!(
                        "Unknown configuration attribute '{name}'"
                    )));
                }
            }
        }

        Ok(())
    }

    /// Coerce a string-based configuration map into a typed HCL map.
    ///
    /// # Errors
    /// Returns `StampError` if type conversion fails.
    pub fn coerce(
        &self,
        config: &HashMap<String, String>,
    ) -> Result<std::collections::BTreeMap<String, Value>, StampError> {
        let mut map = std::collections::BTreeMap::new();

        for (k, v) in config {
            let field_type = self.fields.get(k).unwrap_or(&SchemaType::String);

            let value = match field_type {
                SchemaType::String => Value::new(Type::String, ValueData::String(v.clone())),
                SchemaType::Bool => {
                    let b = if v == "1" || v.eq_ignore_ascii_case("true") {
                        true
                    } else if v == "0" || v.eq_ignore_ascii_case("false") {
                        false
                    } else {
                        return Err(StampError::Validation(format!(
                            "Field '{k}' expects a boolean, found: {v}"
                        )));
                    };
                    Value::new(Type::Bool, ValueData::Bool(b))
                }
                SchemaType::Int => {
                    let n = v.parse::<i64>().map_err(|_| {
                        StampError::Validation(format!(
                            "Field '{k}' expects an integer, found: {v}"
                        ))
                    })?;
                    let num = hashicorp_configuration_language_rs::number::Number::from_str(
                        &n.to_string(),
                    )
                    .map_err(|e| StampError::Validation(format!("Number conversion error: {e}")))?;
                    Value::new(Type::Number, ValueData::Number(num))
                }
                SchemaType::Float => {
                    let num = hashicorp_configuration_language_rs::number::Number::from_str(v)
                        .map_err(|e| {
                            StampError::Validation(format!("Float conversion error: {e}"))
                        })?;
                    Value::new(Type::Number, ValueData::Number(num))
                }
                SchemaType::List | SchemaType::Map => {
                    Value::new(Type::String, ValueData::String(v.clone()))
                }
            };
            map.insert(k.clone(), value);
        }

        Ok(map)
    }
}

/// Server-side gRPC adapter serving plugin schemas over the wire.
#[derive(Clone)]
pub struct RemoteSchemaServer {
    /// Schemas mapped by component identifier (e.g. `builder.amazon-ebs`).
    schemas: Arc<HashMap<String, PluginSchema>>,
}

impl RemoteSchemaServer {
    /// Creates a new `RemoteSchemaServer` with the provided component schemas.
    #[must_use]
    pub fn new(schemas: HashMap<String, PluginSchema>) -> Self {
        Self {
            schemas: Arc::new(schemas),
        }
    }
}

#[tonic::async_trait]
impl crate::r#gen::packer::schema_server::Schema for RemoteSchemaServer {
    async fn get_schema(
        &self,
        request: Request<crate::r#gen::packer::GetSchemaRequest>,
    ) -> Result<Response<crate::r#gen::packer::GetSchemaResponse>, Status> {
        let req = request.into_inner();
        let key = format!("{}.{}", req.component_type, req.component_name);

        if let Some(schema) = self.schemas.get(&key) {
            match schema.to_json() {
                Ok(json) => Ok(Response::new(crate::r#gen::packer::GetSchemaResponse {
                    schema_json: json,
                    error: false,
                    error_message: String::new(),
                })),
                Err(e) => Ok(Response::new(crate::r#gen::packer::GetSchemaResponse {
                    schema_json: String::new(),
                    error: true,
                    error_message: e.to_string(),
                })),
            }
        } else {
            Ok(Response::new(crate::r#gen::packer::GetSchemaResponse {
                schema_json: String::new(),
                error: true,
                error_message: format!("No schema found for '{key}'"),
            }))
        }
    }
}

/// Client-side adapter querying remote plugin schemas over gRPC.
#[derive(Debug, Clone)]
pub struct RemoteSchemaClient {
    /// Inner tonic gRPC client.
    client: crate::r#gen::packer::schema_client::SchemaClient<tonic::transport::Channel>,
}

impl RemoteSchemaClient {
    /// Connects to a remote Schema gRPC server over TCP.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if connection fails.
    pub async fn connect_tcp(address: &str) -> Result<Self, StampError> {
        let url = if address.starts_with("http") {
            address.to_string()
        } else {
            format!("http://{address}")
        };
        let client = crate::r#gen::packer::schema_client::SchemaClient::connect(url)
            .await
            .map_err(|e| StampError::Execution(format!("Schema connect error: {e}")))?;
        Ok(Self { client })
    }

    /// Fetches the schema for a component from the remote plugin.
    ///
    /// # Errors
    /// Returns `StampError` if the RPC fails or schema deserialization fails.
    pub async fn get_schema(
        &mut self,
        component_type: &str,
        component_name: &str,
    ) -> Result<PluginSchema, StampError> {
        let req = tonic::Request::new(crate::r#gen::packer::GetSchemaRequest {
            component_type: component_type.to_string(),
            component_name: component_name.to_string(),
        });

        let resp = self
            .client
            .get_schema(req)
            .await
            .map_err(|e| StampError::Execution(format!("GetSchema RPC error: {e}")))?
            .into_inner();

        if resp.error {
            return Err(StampError::SchemaMismatch(resp.error_message));
        }

        PluginSchema::from_json(&resp.schema_json)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use hashicorp_configuration_language_rs::ast::expr::Expression;
    use hashicorp_configuration_language_rs::ast::structure::Attribute;
    use hashicorp_configuration_language_rs::span::Span;

    #[test]
    fn test_coerce_valid() -> Result<(), StampError> {
        let schema = PluginSchema::new()
            .add_field("sleep_delay", SchemaType::Int)
            .add_field("ratio", SchemaType::Float)
            .add_field("disable", SchemaType::Bool)
            .add_field("name", SchemaType::String)
            .add_field("tags", SchemaType::List);

        let mut config = HashMap::new();
        config.insert("sleep_delay".to_string(), "5".to_string());
        config.insert("ratio".to_string(), "1.25".to_string());
        config.insert("disable".to_string(), "true".to_string());
        config.insert("name".to_string(), "test".to_string());
        config.insert("tags".to_string(), "a,b".to_string());
        config.insert("unknown".to_string(), "untyped".to_string());

        let result = schema.coerce(&config)?;
        if let Some(val) = result.get("sleep_delay") {
            if let ValueData::Number(ref num) = *val.data {
                assert_eq!(num.to_string(), "5");
            } else {
                panic!("Expected number");
            }
        } else {
            panic!("Missing sleep_delay");
        }

        if let Some(val) = result.get("disable") {
            if let ValueData::Bool(b) = *val.data {
                assert!(b);
            } else {
                panic!("Expected bool");
            }
        } else {
            panic!("Missing disable");
        }

        if let Some(val) = result.get("name") {
            if let ValueData::String(ref s) = *val.data {
                assert_eq!(s, "test");
            } else {
                panic!("Expected string");
            }
        } else {
            panic!("Missing name");
        }

        if let Some(val) = result.get("unknown") {
            if let ValueData::String(ref s) = *val.data {
                assert_eq!(s, "untyped");
            } else {
                panic!("Expected string");
            }
        } else {
            panic!("Missing unknown");
        }

        Ok(())
    }

    #[test]
    fn test_coerce_invalid_int() {
        let schema = PluginSchema::new().add_field("sleep_delay", SchemaType::Int);
        let mut config = HashMap::new();
        config.insert("sleep_delay".to_string(), "not_an_int".to_string());
        let err = schema.coerce(&config).unwrap_err();
        assert!(matches!(err, StampError::Validation(_)));
    }

    #[test]
    fn test_coerce_invalid_bool() {
        let schema = PluginSchema::new().add_field("disable", SchemaType::Bool);
        let mut config = HashMap::new();
        config.insert("disable".to_string(), "not_a_bool".to_string());
        let err = schema.coerce(&config).unwrap_err();
        assert!(matches!(err, StampError::Validation(_)));
    }

    #[test]
    fn test_coerce_invalid_float() {
        let schema = PluginSchema::new().add_field("rate", SchemaType::Float);
        let mut config = HashMap::new();
        config.insert("rate".to_string(), "invalid_float".to_string());
        let err = schema.coerce(&config).unwrap_err();
        assert!(matches!(err, StampError::Validation(_)));
    }

    #[test]
    fn test_schema_json_roundtrip_and_validation() {
        let attr = SchemaAttribute::new("ami_id", SchemaType::String)
            .required()
            .with_description("Target AMI")
            .with_default("ami-12345");

        let schema = PluginSchema::new()
            .add_attribute(attr)
            .add_field("region", SchemaType::String);

        let json = schema.to_json().unwrap();
        let decoded = PluginSchema::from_json(&json).unwrap();
        assert_eq!(decoded.attributes.len(), 2);
        assert!(decoded.attributes["ami_id"].required);

        // Test validate_body missing required
        let span = Span::default();
        let empty_body = Body::new(span.clone());
        assert!(decoded.validate_body(&empty_body).is_err());

        // Test validate_body success
        let mut valid_body = Body::new(span.clone());
        valid_body.attributes.insert(
            "ami_id".to_string(),
            Attribute::new(
                "ami_id",
                Expression::Variable("v".to_string(), span.clone()),
                span.clone(),
            ),
        );
        assert!(decoded.validate_body(&valid_body).is_ok());

        // Test validate_body unknown attribute
        valid_body.attributes.insert(
            "unexpected_attr".to_string(),
            Attribute::new(
                "unexpected_attr",
                Expression::Variable("v".to_string(), span.clone()),
                span.clone(),
            ),
        );
        assert!(decoded.validate_body(&valid_body).is_err());
    }

    #[tokio::test]
    async fn test_remote_schema_server_and_client_roundtrip() {
        use crate::r#gen::packer::schema_server::SchemaServer;
        use tokio_stream::wrappers::TcpListenerStream;

        let mut schemas = HashMap::new();
        let test_schema = PluginSchema::new()
            .add_field("instance_type", SchemaType::String)
            .add_attribute(SchemaAttribute::new("region", SchemaType::String).required());
        schemas.insert("builder.amazon-ebs".to_string(), test_schema);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let incoming = TcpListenerStream::new(listener);

        let server = RemoteSchemaServer::new(schemas);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(SchemaServer::new(server))
                .serve_with_incoming_shutdown(incoming, async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        let mut client = RemoteSchemaClient::connect_tcp(&addr.to_string())
            .await
            .unwrap();
        let fetched = client.get_schema("builder", "amazon-ebs").await.unwrap();
        assert!(fetched.attributes.contains_key("region"));
        assert!(fetched.attributes["region"].required);

        // Query nonexistent
        let err = client.get_schema("builder", "nonexistent").await;
        assert!(err.is_err());

        // Connect with http:// prefix
        let mut client_http = RemoteSchemaClient::connect_tcp(&format!("http://{addr}"))
            .await
            .unwrap();
        let fetched2 = client_http
            .get_schema("builder", "amazon-ebs")
            .await
            .unwrap();
        assert!(fetched2.attributes.contains_key("region"));

        let _ = shutdown_tx.send(());
    }

    #[test]
    fn test_schema_types_and_coerce_invalid_int() {
        let types = vec![
            SchemaType::String,
            SchemaType::Bool,
            SchemaType::Int,
            SchemaType::Float,
            SchemaType::List,
            SchemaType::Map,
        ];
        for t in types {
            let cloned = t.clone();
            assert_eq!(t, cloned);
            assert!(!format!("{t:?}").is_empty());
        }

        let schema = PluginSchema::new().add_field("count", SchemaType::Int);
        let mut config = HashMap::new();
        config.insert("count".to_string(), "not_an_int".to_string());
        let err = schema.coerce(&config).unwrap_err();
        assert!(matches!(err, StampError::Validation(_)));
    }
}
