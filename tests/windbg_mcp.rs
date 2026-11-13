//! Real, opt-in acceptance of the existing Rig/rmcp client against a WinDbg MCP
//! server in a dedicated VM. No debugger or target is started by default.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fs::OpenOptions, io::Write, path::PathBuf};
use wukong::mcp::{McpRegistry, McpServerConfig};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Acceptance {
    server: McpServerConfig,
    fixtures: Vec<Fixture>,
    transcript: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    architecture: String,
    target: String,
    dump: String,
    symbols: String,
}

struct Client {
    registry: McpRegistry,
    prefix: String,
    transcript: std::fs::File,
}

impl Client {
    async fn call(&mut self, tool: &str, arguments: Value) -> Result<Value> {
        let result = self
            .registry
            .call(&format!("{}{}", self.prefix, tool), arguments.clone())
            .await?;
        serde_json::to_writer(
            &mut self.transcript,
            &json!({"tool":tool,"arguments":arguments,"result":result}),
        )?;
        self.transcript.write_all(b"\n")?;
        self.transcript.flush()?;
        ensure!(result["isError"] != true, "{tool} failed: {result}");
        Ok(result["structuredContent"].clone())
    }

    async fn inspect(&mut self, fixture: &Fixture, mode: &str) -> Result<()> {
        let opened = if mode == "dump" {
            self.call("open_dump", json!({"path":fixture.dump})).await?
        } else {
            self.call(
                "launch",
                json!({"command_line":format!("\"{}\" --echo",fixture.target)}),
            )
            .await?
        };
        let session = opened["session_id"]
            .as_str()
            .context("opener did not return a session handle")?;
        let outcome = self.inspect_opened(fixture, mode, session).await;
        let closed = self
            .call("end_session", json!({"session_id":session}))
            .await?;
        ensure!(
            closed["released"] == true && closed["recovery_required"] == false,
            "session release was not confirmed: {closed}"
        );
        outcome
    }

    async fn inspect_opened(&mut self, fixture: &Fixture, mode: &str, session: &str) -> Result<()> {
        let route = json!({"session_id":session});
        self.call(
            "set_symbol_path",
            json!({"session_id":session,"path":fixture.symbols,"append":false}),
        )
        .await?;
        if mode == "live" {
            self.call(
                "set_breakpoint",
                json!({"session_id":session,"expression":"wukong_probe!wmain"}),
            )
            .await?;
            // WOW64 can stop at both its native and 32-bit loader breakpoints.
            let mut hit = false;
            for _ in 0..3 {
                self.call("go", route.clone()).await?;
                let trace = self.call("backtrace", route.clone()).await?;
                if trace["frames"].as_array().is_some_and(|frames| {
                    frames.first().is_some_and(|frame| {
                        frame["symbol"]
                            .as_str()
                            .is_some_and(|name| name == "wukong_probe!wmain")
                    })
                }) {
                    hit = true;
                    break;
                }
            }
            ensure!(hit, "fixture did not hit its wmain breakpoint");
        }
        let registers = self.call("registers", route.clone()).await?;
        let ip = registers["instruction_pointer"]
            .as_str()
            .context("instruction pointer is missing")?;
        let address = u64::from_str_radix(ip.trim_start_matches("0x"), 16)?;
        ensure!(
            address > 0 && (fixture.architecture != "x86" || address < 1 << 32),
            "program counter does not match the fixture architecture"
        );
        let memory = self
            .call(
                "read_memory",
                json!({"session_id":session,"address":ip,"size":16}),
            )
            .await?;
        ensure!(memory["read_size"] == 16, "instruction bytes unavailable");
        let modules = self.call("modules", route.clone()).await?;
        ensure!(
            modules["modules"].as_array().is_some_and(|modules| {
                modules
                    .iter()
                    .any(|module| module["name"] == "wukong_probe")
            }),
            "fixture module is absent"
        );
        let trace = self.call("backtrace", route.clone()).await?;
        ensure!(
            trace["frames"]
                .as_array()
                .is_some_and(|frames| !frames.is_empty()),
            "backtrace has no frames"
        );
        self.call("disassemble", route.clone()).await?;
        // The upstream threads tool currently returns text. Keep the raw MCP
        // result as evidence; neither parse it nor claim a structured result.
        self.call("threads", route.clone()).await?;
        if mode == "live" {
            self.call("step_into", route.clone()).await?;
            let stepped = self.call("registers", route.clone()).await?;
            ensure!(
                stepped["instruction_pointer"] != registers["instruction_pointer"],
                "single step did not advance the program counter"
            );
            self.call("step_over", route).await?;
        }
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires WUKONG_WINDBG_MCP_ACCEPTANCE pointing to a dedicated-VM fixture configuration"]
async fn existing_rig_client_drives_external_windbg_mcp() -> Result<()> {
    let path = std::env::var_os("WUKONG_WINDBG_MCP_ACCEPTANCE")
        .context("WUKONG_WINDBG_MCP_ACCEPTANCE is not configured")?;
    let config: Acceptance = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        config.fixtures.len() == 2
            && config.fixtures[0].architecture == "x64"
            && config.fixtures[1].architecture == "x86",
        "acceptance requires both sealed fixture architectures"
    );
    let transcript = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&config.transcript)?;
    let registry = McpRegistry::connect(std::slice::from_ref(&config.server)).await?;
    let mut client = Client {
        registry,
        prefix: format!("mcp_{}_", config.server.name),
        transcript,
    };
    let outcome = async {
        for tool in [
            "open_dump",
            "launch",
            "attach_process",
            "registers",
            "read_memory",
            "backtrace",
            "modules",
            "threads",
            "disassemble",
            "set_breakpoint",
            "go",
            "step_into",
            "step_over",
            "end_session",
            "session_status",
        ] {
            ensure!(
                client
                    .registry
                    .contains(&format!("{}{}", client.prefix, tool)),
                "required tool is absent: {tool}"
            );
        }
        for fixture in &config.fixtures {
            for mode in ["dump", "live"] {
                client.inspect(fixture, mode).await?;
            }
        }
        let status = client.call("session_status", json!({})).await?;
        ensure!(status["sessions"] == json!([]), "server retained sessions");
        Ok(())
    }
    .await;
    client.registry.close().await;
    outcome
}
