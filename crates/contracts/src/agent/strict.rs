//! The Codex structured-output subset (spike S1a, ADR-009; OpenAI Structured Outputs "Supported schemas"):
//! every object closed with every property required, optional fields nullable, only the supported keywords,
//! local `$ref` only and resolvable, an object at the root of an output schema.

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value};

/// The only keywords a schema node may carry (allowlist: anything else, e.g. `oneOf`, `allOf`, `not`,
/// `if`/`then`/`else`, `dependentRequired`, `dependentSchemas`, `patternProperties`, is outside the subset).
const ALLOWED: [&str; 22] = [
    "type",
    "description",
    "title",
    "enum",
    "const",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "anyOf",
    "$defs",
    "$ref",
    "pattern",
    "format",
    "multipleOf",
    "maximum",
    "exclusiveMaximum",
    "minimum",
    "exclusiveMinimum",
    "minItems",
    "maxItems",
    "minLength",
];

/// Keywords whose value is one schema.
const SCHEMA_KEYWORDS: [&str; 2] = ["items", "additionalProperties"];
/// Keywords whose value is an array of schemas.
const SCHEMA_ARRAY_KEYWORDS: [&str; 2] = ["anyOf", "prefixItems"];
/// Keywords whose value is a map of name → schema.
const SCHEMA_MAP_KEYWORDS: [&str; 2] = ["properties", "$defs"];

/// A schema outside the Codex strict subset.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StrictSchemaError {
    /// The schema is not a JSON object.
    #[error("schema at {path} is not a JSON object")]
    NotASchema {
        /// JSON pointer of the offending node.
        path: String,
    },
    /// A keyword outside the subset, e.g. `oneOf` from a data-carrying enum.
    #[error("keyword `{keyword}` at {path} is outside the Codex strict subset")]
    ForbiddenKeyword {
        /// The keyword.
        keyword: String,
        /// JSON pointer of the offending node.
        path: String,
    },
    /// An object that allows additional properties.
    #[error("object at {path} does not set additionalProperties to false")]
    OpenObject {
        /// JSON pointer of the offending node.
        path: String,
    },
    /// A property missing from `required` (optional fields must be nullable instead).
    #[error("property `{property}` of the object at {path} is not required")]
    OptionalProperty {
        /// JSON pointer of the object.
        path: String,
        /// The property name.
        property: String,
    },
    /// A local `$ref` naming a definition that does not exist.
    #[error("reference `{reference}` at {path} names no definition in $defs")]
    DanglingRef {
        /// The reference.
        reference: String,
        /// JSON pointer of the offending node.
        path: String,
    },
    /// The root of an output schema is not a plain object schema (`type: object`, no `anyOf`).
    #[error("the root of an output schema must be an object without anyOf")]
    RootNotObject,
    /// A `$ref` outside the schema's own `$defs`.
    #[error("reference `{reference}` at {path} is not a local $defs reference")]
    NonLocalRef {
        /// The reference.
        reference: String,
        /// JSON pointer of the offending node.
        path: String,
    },
    /// A `format` other than `date-time`.
    #[error("format `{format}` at {path} is not supported")]
    UnsupportedFormat {
        /// The format.
        format: String,
        /// JSON pointer of the offending node.
        path: String,
    },
}

/// Generates the JSON Schema of `T`, every subschema inlined as in the shapes S7 validated, and brings it
/// into the Codex strict subset.
pub fn schema_for<T: JsonSchema>() -> Result<Value, StrictSchemaError> {
    let schema = SchemaSettings::draft2020_12()
        .with(|settings| settings.inline_subschemas = true)
        .into_generator()
        .into_root_schema_for::<T>();
    codex_strict(schema.to_value())
}

/// Brings a generated schema into the Codex strict subset, then checks it with [`check`].
///
/// Closes every object and requires every property, collapses schemars' `oneOf` of documented unit
/// variants into a string `enum`, drops formats other than `date-time` and the root `$schema` and `title`.
pub fn codex_strict(mut schema: Value) -> Result<Value, StrictSchemaError> {
    if let Some(root) = schema.as_object_mut() {
        root.remove("$schema");
        root.remove("title");
    }
    tighten(&mut schema);
    check(&schema)?;
    Ok(schema)
}

/// Generates the output schema of `T` with [`schema_for`] and checks it with [`check_output`].
pub fn output_schema_for<T: JsonSchema>() -> Result<Value, StrictSchemaError> {
    let schema = schema_for::<T>()?;
    check_output(&schema)?;
    Ok(schema)
}

/// Checks that a schema lies inside the Codex strict subset (any root, e.g. a lone enum).
pub fn check(schema: &Value) -> Result<(), StrictSchemaError> {
    let definitions = schema.get("$defs").and_then(Value::as_object);
    check_node(schema, "", definitions)
}

/// Checks an `--output-schema`: the subset of [`check`] plus an object root without `anyOf`.
pub fn check_output(schema: &Value) -> Result<(), StrictSchemaError> {
    let root_is_object = schema.get("type").and_then(Value::as_str) == Some("object")
        && schema.get("anyOf").is_none();
    if !root_is_object {
        return Err(StrictSchemaError::RootNotObject);
    }
    check(schema)
}

fn tighten(node: &mut Value) {
    let Some(map) = node.as_object_mut() else {
        return;
    };
    collapse_const_one_of(map);
    if map
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|format| format != "date-time")
    {
        map.remove("format");
    }
    if is_object_schema(map) {
        let names: Vec<Value> = map
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| properties.keys().cloned().map(Value::String).collect())
            .unwrap_or_default();
        map.insert("required".to_owned(), Value::Array(names));
        map.insert("additionalProperties".to_owned(), Value::Bool(false));
    }
    for_each_subschema_mut(map, tighten);
}

/// Replaces `oneOf: [{const: "a", description: …}, …]` by `type: string, enum: [...]`, keeping the
/// variant descriptions in the parent description.
fn collapse_const_one_of(map: &mut Map<String, Value>) {
    let Some(branches) = map.get("oneOf").and_then(Value::as_array) else {
        return;
    };
    let mut values = Vec::with_capacity(branches.len());
    let mut notes = Vec::with_capacity(branches.len());
    for branch in branches {
        let Some(value) = branch.get("const").and_then(Value::as_str) else {
            return;
        };
        if let Some(description) = branch.get("description").and_then(Value::as_str) {
            notes.push(format!("`{value}`: {description}"));
        }
        values.push(Value::String(value.to_owned()));
    }
    map.remove("oneOf");
    map.insert("type".to_owned(), Value::String("string".to_owned()));
    map.insert("enum".to_owned(), Value::Array(values));
    if !notes.is_empty() {
        let joined = notes.join(" ");
        let description = match map.get("description").and_then(Value::as_str) {
            Some(existing) => format!("{existing} {joined}"),
            None => joined,
        };
        map.insert("description".to_owned(), Value::String(description));
    }
}

fn is_object_schema(map: &Map<String, Value>) -> bool {
    map.contains_key("properties")
        || match map.get("type") {
            Some(Value::String(kind)) => kind == "object",
            Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "object"),
            _ => false,
        }
}

fn for_each_subschema_mut(map: &mut Map<String, Value>, f: fn(&mut Value)) {
    for keyword in SCHEMA_KEYWORDS {
        if let Some(child) = map.get_mut(keyword) {
            f(child);
        }
    }
    for keyword in SCHEMA_ARRAY_KEYWORDS {
        if let Some(Value::Array(children)) = map.get_mut(keyword) {
            children.iter_mut().for_each(f);
        }
    }
    for keyword in SCHEMA_MAP_KEYWORDS {
        if let Some(Value::Object(children)) = map.get_mut(keyword) {
            children.values_mut().for_each(f);
        }
    }
}

fn check_node(
    node: &Value,
    path: &str,
    definitions: Option<&Map<String, Value>>,
) -> Result<(), StrictSchemaError> {
    let map = match node {
        Value::Object(map) => map,
        // `additionalProperties: false` is the only boolean schema the subset uses.
        Value::Bool(false) => return Ok(()),
        _ => {
            return Err(StrictSchemaError::NotASchema {
                path: path.to_owned(),
            });
        }
    };
    if let Some(keyword) = map
        .keys()
        .find(|keyword| !ALLOWED.contains(&keyword.as_str()))
    {
        return Err(StrictSchemaError::ForbiddenKeyword {
            keyword: keyword.clone(),
            path: path.to_owned(),
        });
    }
    if let Some(reference) = map.get("$ref") {
        check_ref(reference.as_str().unwrap_or_default(), path, definitions)?;
    }
    if let Some(format) = map.get("format") {
        let format = format.as_str().unwrap_or_default();
        if format != "date-time" {
            return Err(StrictSchemaError::UnsupportedFormat {
                format: format.to_owned(),
                path: path.to_owned(),
            });
        }
    }
    if is_object_schema(map) {
        check_object(map, path)?;
    }
    for keyword in SCHEMA_KEYWORDS {
        if let Some(child) = map.get(keyword) {
            check_node(child, &format!("{path}/{keyword}"), definitions)?;
        }
    }
    for keyword in SCHEMA_ARRAY_KEYWORDS {
        if let Some(Value::Array(children)) = map.get(keyword) {
            for (index, child) in children.iter().enumerate() {
                check_node(child, &format!("{path}/{keyword}/{index}"), definitions)?;
            }
        }
    }
    for keyword in SCHEMA_MAP_KEYWORDS {
        if let Some(Value::Object(children)) = map.get(keyword) {
            for (name, child) in children {
                check_node(child, &format!("{path}/{keyword}/{name}"), definitions)?;
            }
        }
    }
    Ok(())
}

/// `#` (root recursion) or `#/$defs/<name>` with `<name>` defined in the root `$defs`.
fn check_ref(
    reference: &str,
    path: &str,
    definitions: Option<&Map<String, Value>>,
) -> Result<(), StrictSchemaError> {
    if reference == "#" {
        return Ok(());
    }
    let Some(name) = reference.strip_prefix("#/$defs/") else {
        return Err(StrictSchemaError::NonLocalRef {
            reference: reference.to_owned(),
            path: path.to_owned(),
        });
    };
    if definitions.is_some_and(|definitions| definitions.contains_key(name)) {
        Ok(())
    } else {
        Err(StrictSchemaError::DanglingRef {
            reference: reference.to_owned(),
            path: path.to_owned(),
        })
    }
}

fn check_object(map: &Map<String, Value>, path: &str) -> Result<(), StrictSchemaError> {
    if map.get("additionalProperties") != Some(&Value::Bool(false)) {
        return Err(StrictSchemaError::OpenObject {
            path: path.to_owned(),
        });
    }
    let required: Vec<&str> = map
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let properties = map.get("properties").and_then(Value::as_object);
    for property in properties
        .into_iter()
        .flat_map(|properties| properties.keys())
    {
        if !required.contains(&property.as_str()) {
            return Err(StrictSchemaError::OptionalProperty {
                path: path.to_owned(),
                property: property.clone(),
            });
        }
    }
    Ok(())
}
