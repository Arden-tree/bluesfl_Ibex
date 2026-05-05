mod get_driven_block;
mod get_module_covered;
mod get_module_knowledge;
mod get_signal_values;
mod get_suspicious_ports;

pub use get_driven_block::*;
pub use get_module_covered::*;
pub use get_signal_values::*;
pub use get_suspicious_ports::*;

#[macro_export]
macro_rules! unwrap_serde_value {
    ($result:expr) => {
        if let Ok(wrapper_result) = $result {
            serde_json::to_value(wrapper_result).unwrap_or_else(|err| {
                error!("Serialization error: {}", err);
                Value::Null
            })
        } else {
            error!("Failed to parse input");
            Value::Null
        }
    };
}
