pub mod json;
pub mod table;

use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputMode {
    Table,
    Json,
}

pub fn render<T: serde::Serialize + table::TableDisplay>(
    items: &[T],
    mode: OutputMode,
    quiet: bool,
) -> Result<(), AppError> {
    if quiet {
        return Ok(());
    }
    match mode {
        OutputMode::Json => {
            let output = serde_json::to_string_pretty(items)
                .map_err(|e| AppError::ServerError(format!("JSON serialization failed: {e}")))?;
            println!("{output}");
        }
        OutputMode::Table => {
            if items.is_empty() {
                println!("No results found.");
            } else {
                println!("{}", table::to_table(items));
            }
        }
    }
    Ok(())
}

pub fn render_single<T: serde::Serialize + table::TableDisplay>(
    item: &T,
    mode: OutputMode,
    quiet: bool,
) -> Result<(), AppError> {
    if quiet {
        return Ok(());
    }
    match mode {
        OutputMode::Json => {
            let output = serde_json::to_string_pretty(item)
                .map_err(|e| AppError::ServerError(format!("JSON serialization failed: {e}")))?;
            println!("{output}");
        }
        OutputMode::Table => {
            println!("{}", table::to_table(&[item]));
        }
    }
    Ok(())
}
