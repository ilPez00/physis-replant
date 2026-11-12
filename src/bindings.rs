//! Deliberately small input binding language: request data, never expressions.
use std::io::{self, Write};

use anyhow::{ensure, Context, Result};
use serde_json::Value;

use crate::workflow::{Recipe, Request, MAX_RECIPE_CALLS};

const LIMIT_ERROR: &str = "resolved tool input exceeds 64 KiB";

/// Count actual JSON encoding bytes without allocating a serialized copy.
/// Every call has one budget, shared by all recursive containers/selectors.
struct Budget(usize);

impl Budget {
    fn charge(&mut self, bytes: usize) -> io::Result<()> {
        self.0 = self
            .0
            .checked_sub(bytes)
            .ok_or_else(|| io::Error::other(LIMIT_ERROR))?;
        Ok(())
    }

    fn json(&mut self, value: &(impl serde::Serialize + ?Sized)) -> Result<()> {
        serde_json::to_writer(self, value)?;
        Ok(())
    }
}

impl Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.charge(bytes.len())?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Resolve the complete template before any intent or effect. Selected request
/// values are charged before cloning and never interpreted as further bindings.
pub(crate) fn resolve(recipe: &Recipe, request: &Request) -> Result<Recipe> {
    // Bound aggregate expansion before cloning templates or resolving any call.
    let count =
        recipe.preconditions.len() + recipe.steps.len() + recipe.checks.len() + recipe.undo.len();
    ensure!(
        count <= MAX_RECIPE_CALLS,
        "recipe exceeds {MAX_RECIPE_CALLS} calls"
    );
    let mut resolved = recipe.clone();
    for call in resolved
        .preconditions
        .iter_mut()
        .chain(&mut resolved.steps)
        .chain(&mut resolved.checks)
        .chain(&mut resolved.undo)
    {
        let mut budget = Budget(crate::runner::MAX_TOOL_BYTES);
        if recipe.bind_inputs {
            call.input = value(&call.input, &request.input, &mut budget)?;
        } else {
            budget.json(&call.input)?;
        }
    }
    Ok(resolved)
}

fn value(template: &Value, input: &Value, budget: &mut Budget) -> Result<Value> {
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
            }
            .with_context(|| format!("missing $input selector: {selector}"))?;
            budget.json(selected)?;
            Ok(selected.clone())
        }
        Value::Object(object) => {
            budget.charge(2 + object.len().saturating_sub(1))?; // braces + commas
            let mut resolved = serde_json::Map::new();
            for (key, template) in object {
                budget.json(key)?; // encoded key, including quotes and escapes
                budget.charge(1)?; // colon
                let value = value(template, input, budget)?;
                resolved.insert(key.clone(), value);
            }
            Ok(Value::Object(resolved))
        }
        Value::Array(array) => {
            budget.charge(2 + array.len().saturating_sub(1))?; // brackets + commas
            let mut resolved = Vec::new();
            for template in array {
                resolved.push(value(template, input, budget)?);
            }
            Ok(Value::Array(resolved))
        }
        _ => {
            budget.json(template)?;
            Ok(template.clone())
        }
    }
}
