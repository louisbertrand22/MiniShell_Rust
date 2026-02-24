use crate::parser::ast::{Command, LogicalOp};
use std::process::Command as SysCommand;

pub struct Executor;

impl Executor {
    pub fn execute(command: Command) -> Result<(), String> {
        match command {
            Command::Simple(simple_cmd) => {
                let mut child = SysCommand::new(&simple_cmd.name)
                    .args(&simple_cmd.args)
                    .spawn()
                    .map_err(|e| format!("Failed to start command '{}': {}", simple_cmd.name, e))?;

                let status = child
                    .wait()
                    .map_err(|e| format!("Failed to wait for command: {}", e))?;

                if status.success() {
                    Ok(())
                } else {
                    Err(format!("Command exited with non-zero status: {:?}", status.code()))
                }
            }
            Command::BinaryOp { left, op, right } => {
                let left_res = Self::execute(*left);

                match op {
                    LogicalOp::And => {
                        if left_res.is_ok() {
                            Self::execute(*right)
                        } else {
                            left_res
                        }
                    }
                    LogicalOp::Or => {
                        if left_res.is_err() {
                            Self::execute(*right)
                        } else {
                            left_res
                        }
                    }
                }
            }
            Command::If { condition, then_part, else_part } => {
                if Self::execute(*condition).is_ok() {
                    Self::execute(*then_part)
                } else if let Some(else_cmd) = else_part {
                    Self::execute(*else_cmd)
                } else {
                    Ok(())
                }
            }
            Command::Pipeline(_) => {
                Err("Pipelines are not yet implemented in the executor".to_string())
            }
        }
    }
}