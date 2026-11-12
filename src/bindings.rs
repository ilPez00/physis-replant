//! Deliberately small input binding language: request data, never expressions.
use anyhow::{ensure, Context, Result};
use serde_json::Value;

use crate::workflow::{Recipe, Request};

/// Resolve the complete template before any intent or effect. Selected request
/// values are copied as data and are never interpreted as further bindings.
pub(crate) fn resolve(recipe: &Recipe, request: &Request) -> Result<Recipe> {
    let mut resolved = recipe.clone();
    for call in resolved
        .preconditions
        .iter_mut()
        .chain(&mut resolved.steps)
        .chain(&mut resolved.checks)
        .chain(&mut resolved.undo)
    {
        if recipe.bind_inputs {
            call.input = value(&call.input, &request.input)?;
        }
        ensure!(
            serde_json::to_vec(&call.input)?.len() <= crate::runner::MAX_TOOL_BYTES,
            "resolved tool input exceeds 64 KiB"
        );
    }
    Ok(resolved)
}

fn value(template: &Value, input: &Value) -> Result<Value> {
    match template {
        Value::Object(object) if object.contains_key("$input") => {
            ensure!(
                object.len() == 1,
                "$input binding must be a singleton object"
            );
            let selector = object["$input"]
                .as_str()
                .context("$input selector must be a string")?;
            ensure!(!selector.is_empty(), "$input selector must not be empty");
            let selected = if selector.starts_with('/') {
                // serde_json's pointer accepts non-RFC escapes. Validate first.
                let mut chars = selector.chars();
                while let Some(c) = chars.next() {
                    if c == '~' {
                        ensure!(
                            matches!(chars.next(), Some('0' | '1')),
                            "invalid RFC6901 escape in $input selector"
                        );
                    }
                }
                input.pointer(selector)
            } else {
                input.as_object().and_then(|object| object.get(selector))
            };
            selected
                .cloned()
                .with_context(|| format!("missing $input selector: {selector}"))
        }
        Value::Object(object) => object
            .iter()
            .map(|(key, template)| Ok((key.clone(), value(template, input)?)))
            .collect::<Result<_>>()
            .map(Value::Object),
        Value::Array(array) => array
            .iter()
            .map(|template| value(template, input))
            .collect::<Result<_>>()
            .map(Value::Array),
        _ => Ok(template.clone()),
    }
}
