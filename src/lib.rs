pub mod agents;
pub mod application;
pub mod binary;
pub mod config;
mod content;
mod context;
pub mod coordinator;
mod credentials;
pub mod desktop;
pub mod domain;
mod effects;
pub mod interface;
mod kernel;
pub mod llm;
pub mod mcp;
mod process_io;
mod prompts;
pub mod report;
pub mod runner;
pub mod runtime;
pub mod store;
mod supervisor;
pub mod tools;

mod bootstrap;
mod kernel_host;
mod model_catalog;
mod usage;

pub mod queries;

pub mod updates;
pub mod workbench;
