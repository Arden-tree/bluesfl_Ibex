use clap::Parser;
use log::debug;
use regex::Regex;
use serde::Serialize;
use std::fmt::Display;
use std::fs;
use std::path::Path;
use sv_analysis::{save_data_to_json, TimeAnnotation};

#[derive(Parser, Debug)]
#[command(name = "test-analysis")]
#[command(about = "Summarize test information from Ibex co-simulation result.")]
struct Args {
    #[clap(long)]
    info_file: String,
    #[clap(long)]
    inst_trace: String,
    #[clap(long, default_value_t = 2)]
    time_step: TimeAnnotation,
    #[clap(long)]
    output_file: String,
}

#[derive(Debug, Clone, Default)]
struct DutInfo {
    pc: u32,
    wen: bool,
    w_addr: u32,
    w_data: u32,
}

#[derive(Debug, PartialEq)]
enum MismatchType {
    PcMismatch(u32),
    WeMismatch(bool),
    WaddrMismatch(u32),
    WdataMismatch(u32),
}

#[derive(Debug, Default)]
struct CoSimResult {
    dut: DutInfo,
    mismatch: Option<MismatchType>,
    failure_time: Option<TimeAnnotation>,
    time_step: TimeAnnotation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstructionTrace {
    pub core: u32,
    pub pc: u64,
    pub encoding: u32,
    pub instruction: String,
}

#[derive(Debug, Serialize)]
struct TestMetaData {
    start_scope: String,
    start_sig: String,
    start_time: TimeAnnotation,
    test_info: String,
    time_bound: TimeAnnotation,
}

fn main() -> anyhow::Result<()> {
    /*
        --
        --info-file=mismatch_log.txt
        --inst-trace=ibex_simple_system.log
        --output-file=test_info.json
    */

    let args = Args::parse();
    debug!("args: {:?}", args);

    let input = fs::read_to_string(&args.info_file)?;

    let cosim_output = CoSimResult::parse(&input, args.time_step)?;
    debug!("cosim_output: {:?}", cosim_output);

    assert!(cosim_output.mismatch.is_some());
    assert!(cosim_output.failure_time.is_some());

    let inst_trace = InstructionTrace::parse_file(args.inst_trace)?;
    let ibex_cycle = 2 * cosim_output.time_step;
    let test_info_generator = TemplateTestInfoGenerator;

    let start_time = cosim_output.failure_time.unwrap() - 1 - cosim_output.time_step;
    let test_info = test_info_generator.generate(&cosim_output, &inst_trace)?;
    // include two instruction execution cycle
    let time_bound = start_time - 2 * ibex_cycle;
    let meta_data: TestMetaData = match cosim_output.mismatch.as_ref().unwrap() {
        MismatchType::PcMismatch(_) => {
            let start_scope = "TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core";
            let start_sig = "rvfi_pc_wdata";
            TestMetaData::new(start_scope, start_sig, start_time, &test_info, time_bound)
        }
        MismatchType::WeMismatch(_) | MismatchType::WaddrMismatch(_) => {
            let start_scope = "TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core";
            let start_sig = "rvfi_rd_addr_d";
            TestMetaData::new(start_scope, start_sig, start_time, &test_info, time_bound)
        }
        MismatchType::WdataMismatch(_) => {
            let start_scope = "TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core";
            let start_sig = "rvfi_rd_wdata_d";
            TestMetaData::new(start_scope, start_sig, start_time, &test_info, time_bound)
        }
    };

    println!("{}", test_info);

    save_data_to_json(&meta_data, &args.output_file)?;

    Ok(())
}

fn parse_hex_value(s: &str) -> anyhow::Result<u32, std::num::ParseIntError> {
    u32::from_str_radix(s, 16)
}

impl CoSimResult {
    fn parse(input: &str, time_step: TimeAnnotation) -> anyhow::Result<CoSimResult> {
        let mut result = CoSimResult::default();
        result.time_step = time_step;
        let lines: Vec<&str> = input.lines().collect();

        for (_, line) in lines.iter().enumerate() {
            let line = line.trim();

            let oracle_value = if let Some(pos) = line.find("ORACLE=") {
                let line = line[pos..].trim();
                let oracle_str = line.strip_prefix("ORACLE=").unwrap();
                parse_hex_value(oracle_str).ok()
            } else {
                None
            };

            // Accept both underscore format (PC_MISMATCH) and natural format (PC mismatch)
            if line.contains("PC_MISMATCH") || line.to_lowercase().contains("pc mismatch") {
                // For natural format "PC mismatch, DUT retired : f6148, ISS retired: 109fb8"
                if oracle_value.is_none() {
                    // Try parsing "DUT retired" and "ISS retired" from the same line
                    if let Some(dut_pos) = line.to_lowercase().find("dut retired") {
                        let after_dut = &line[dut_pos..];
                        if let Some(hex_start) = after_dut.find("0x").or_else(|| after_dut.find(':')) {
                            let hex_part = after_dut[hex_start..].trim_start_matches(':').trim();
                            let hex_str = hex_part.split_whitespace().next().unwrap_or("0");
                            result.dut.pc = parse_hex_value(hex_str).unwrap_or(0);
                        }
                    }
                    if let Some(iss_pos) = line.to_lowercase().find("iss retired") {
                        let after_iss = &line[iss_pos..];
                        if let Some(hex_start) = after_iss.find("0x").or_else(|| after_iss.find(':')) {
                            let hex_part = after_iss[hex_start..].trim_start_matches(':').trim();
                            let hex_str = hex_part.split_whitespace().next().unwrap_or("0");
                            let oracle = parse_hex_value(hex_str).unwrap_or(0);
                            result.mismatch = Some(MismatchType::PcMismatch(oracle));
                            continue;
                        }
                    }
                }
                result.mismatch = Some(MismatchType::PcMismatch(oracle_value.unwrap_or(0)));
            } else if line.contains("WE_MISMATCH") {
                result.mismatch = Some(MismatchType::WeMismatch(oracle_value.unwrap_or(0) != 0));
            } else if line.contains("WADDR_MISMATCH") {
                result.mismatch = Some(MismatchType::WaddrMismatch(oracle_value.unwrap_or(0)));
            } else if line.contains("WDATA_MISMATCH") {
                result.mismatch = Some(MismatchType::WdataMismatch(oracle_value.unwrap_or(0)));
            }

            // Parse DUT values — accept both "DUT PC:" and "DUT retired :" formats
            if line.starts_with("DUT PC:") {
                let pc_str = line.strip_prefix("DUT PC:").unwrap();
                result.dut.pc = parse_hex_value(pc_str)?;
            } else if line.starts_with("DUT WE:") {
                let we_str = line.strip_prefix("DUT WE:").unwrap();
                result.dut.wen = parse_hex_value(we_str)? != 0;
            } else if line.starts_with("DUT WADDR:") {
                let waddr_str = line.strip_prefix("DUT WADDR:").unwrap();
                result.dut.w_addr = parse_hex_value(waddr_str)?;
            } else if line.starts_with("DUT WDATA:") {
                let wdata_str = line.strip_prefix("DUT WDATA:").unwrap();
                result.dut.w_data = parse_hex_value(wdata_str)?;
            }

            // Parse failure time
            if line.contains("FAILURE: Co-simulation mismatch at time") {
                // Look for the time value in the line
                if let Some(time_part) = line.split("time").nth(1) {
                    if let Some(time_str) = time_part.trim().split_whitespace().next() {
                        result.failure_time = time_str.parse().ok();
                    }
                }
            }
        }

        Ok(result)
    }
}

impl TestMetaData {
    fn new(
        start_scope: &str,
        start_sig: &str,
        start_time: TimeAnnotation,
        test_info: &str,
        time_bound: TimeAnnotation,
    ) -> Self {
        Self {
            start_scope: start_scope.to_string(),
            start_sig: start_sig.to_string(),
            start_time,
            test_info: test_info.to_string(),
            time_bound,
        }
    }
}

impl InstructionTrace {
    pub fn new(core: u32, pc: u64, encoding: u32, instruction: String) -> Self {
        Self {
            core,
            pc,
            encoding,
            instruction,
        }
    }
}

impl Display for InstructionTrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Core {}: PC=0x{:08x}, Encoding=0x{:08x}, Instruction={}",
            self.core, self.pc, self.encoding, self.instruction
        )
    }
}

impl InstructionTrace {
    pub fn parse_line(line: &str) -> Option<InstructionTrace> {
        // Format 1: "core 0: 0x00100080 (0x7390906f) j pc + 0x9f38"
        let instruction_regex =
            Regex::new(r"^core\s+(\d+):\s+0x([0-9a-fA-F]+)\s+\(0x([0-9a-fA-F]+)\)\s+(.+)$").ok()?;

        if let Some(captures) = instruction_regex.captures(line.trim()) {
            let core = captures[1].parse::<u32>().ok()?;
            let pc = u64::from_str_radix(&captures[2], 16).ok()?;
            let encoding = u32::from_str_radix(&captures[3], 16).ok()?;
            let instruction = captures[4].to_string();
            return Some(InstructionTrace::new(core, pc, encoding, instruction));
        }

        // Format 2: tabular "  20  6  00100080  7390906f  jal x0,f6148  x0=..."
        let tabular_regex =
            Regex::new(r"^\s*\d+\s+\d+\s+([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+(.+?)\s{2,}.*$")
                .ok()?;

        if let Some(captures) = tabular_regex.captures(line.trim()) {
            let pc = u64::from_str_radix(&captures[1], 16).ok()?;
            let encoding = u32::from_str_radix(&captures[2], 16).ok()?;
            let instruction = captures[3].trim().to_string();
            return Some(InstructionTrace::new(0, pc, encoding, instruction));
        }

        // Format 2b: tabular without trailing register info
        let tabular_regex2 =
            Regex::new(r"^\s*\d+\s+\d+\s+([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+(\S+)\s*$").ok()?;

        if let Some(captures) = tabular_regex2.captures(line.trim()) {
            let pc = u64::from_str_radix(&captures[1], 16).ok()?;
            let encoding = u32::from_str_radix(&captures[2], 16).ok()?;
            let instruction = captures[3].trim().to_string();
            return Some(InstructionTrace::new(0, pc, encoding, instruction));
        }

        None
    }

    pub fn parse_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Vec<InstructionTrace>> {
        let contents = std::fs::read_to_string(path)?;
        Ok(Self::parse_string(&contents))
    }

    pub fn parse_string(content: &str) -> Vec<InstructionTrace> {
        content
            .lines()
            .filter_map(Self::parse_line)
            .collect::<Vec<_>>()
    }
}

trait TestInfoGenerator {
    fn generate(
        &self,
        report: &CoSimResult,
        trace: &Vec<InstructionTrace>,
    ) -> anyhow::Result<String>;
}

struct TemplateTestInfoGenerator;

impl TestInfoGenerator for TemplateTestInfoGenerator {
    fn generate(
        &self,
        report: &CoSimResult,
        trace: &Vec<InstructionTrace>,
    ) -> anyhow::Result<String> {
        let mismatch = report.mismatch.as_ref().unwrap();

        // Find the instruction that likely caused the mismatch
        let current_instruction = self.find_relevant_instruction(trace, report);
        let context_instructions = self.get_instruction_context(trace, 3);
        let last_instruction = if context_instructions.len() > 1 {
            context_instructions
                .get(context_instructions.len() - 2)
                .cloned()
        } else {
            current_instruction
        };

        let mut output = String::new();

        // Generate contextual mismatch message based on type
        match mismatch {
            MismatchType::PcMismatch(oracle_pc) => {
                if let Some(instr) = last_instruction {
                    output.push_str(&format!(
                        "Now this core design is buggy when executing the instruction:\n {} \n\
                        After executing this instruction, this core jumps to a wrong address 0x{:08x} which should jump to address 0x{:08x}\n\n",
                        instr.instruction,
                        report.dut.pc,
                        oracle_pc
                    ));
                } else {
                    output.push_str(&format!(
                        "PC mismatch detected: Expected PC 0x{:08x}, but got 0x{:08x}\n\n",
                        oracle_pc, report.dut.pc
                    ));
                }
            }
            MismatchType::WeMismatch(oracle_we) => {
                if let Some(instr) = current_instruction {
                    let expected_behavior = if *oracle_we {
                        "should write to register"
                    } else {
                        "should not write to register"
                    };
                    let actual_behavior = if report.dut.wen {
                        "is writing to register"
                    } else {
                        "is not writing to register"
                    };

                    output.push_str(&format!(
                        "Now this core design is buggy when executing a instruction. {} this instruction {} but {}.\n\n",
                        instr.instruction,
                        expected_behavior,
                        actual_behavior
                    ));
                } else {
                    output.push_str(&format!(
                        "Write enable mismatch detected: Expected WE {}, but got {}\n\n",
                        oracle_we, report.dut.wen
                    ));
                }
            }
            MismatchType::WaddrMismatch(oracle_waddr) => {
                if let Some(instr) = current_instruction {
                    output.push_str(&format!(
                        "Now this core design is buggy when executing a instruction. {} this instruction writes to wrong register address 0x{:08x} which should write to register address 0x{:08x}.\n\n",
                        instr.instruction,
                        report.dut.w_addr,
                        oracle_waddr
                    ));
                } else {
                    output.push_str(&format!(
                        "Write address mismatch detected: Expected WADDR 0x{:08x}, but got 0x{:08x}\n\n",
                        oracle_waddr, report.dut.w_addr
                    ));
                }
            }
            MismatchType::WdataMismatch(oracle_wdata) => {
                if let Some(instr) = current_instruction {
                    output.push_str(&format!(
                        "Now this core design is buggy when executing a instruction. {} this instruction writes wrong data 0x{:08x} to register which should write data 0x{:08x}.\n\n",
                        instr.instruction,
                        report.dut.w_data,
                        oracle_wdata
                    ));
                } else {
                    output.push_str(&format!(
                        "Write data mismatch detected: Expected WDATA 0x{:08x}, but got 0x{:08x}\n\n",
                        oracle_wdata, report.dut.w_data
                    ));
                }
            }
        }

        output.push_str("\n");

        // Instruction trace context
        output.push_str("INSTRUCTION TRACE CONTEXT:\n");
        if context_instructions.is_empty() {
            output.push_str("  No instruction trace available\n");
        } else {
            for (i, instr) in context_instructions.iter().enumerate() {
                let marker = if Some(instr) == current_instruction.as_ref() {
                    ">>>"
                } else {
                    "   "
                };
                output.push_str(&format!(
                    "{} [{:2}] Core {}: PC=0x{:08x} (0x{:08x}) {}\n",
                    marker, i, instr.core, instr.pc, instr.encoding, instr.instruction
                ));
            }
        }

        output.push_str("\n");

        Ok(output)
    }
}

impl TemplateTestInfoGenerator {
    fn find_relevant_instruction<'a>(
        &self,
        trace: &'a Vec<InstructionTrace>,
        report: &CoSimResult,
    ) -> Option<&'a InstructionTrace> {
        // For PC mismatch, try to find the instruction that should have jumped to the oracle PC
        if let Some(MismatchType::PcMismatch(_oracle_pc)) = &report.mismatch {
            // Look for the last instruction in the trace, which is likely the one that caused the PC mismatch
            return trace.last();
        }

        // For other mismatches (WE/WADDR/WDATA), find the last instruction
        trace.last()
    }

    fn get_instruction_context<'a>(
        &self,
        trace: &'a Vec<InstructionTrace>,
        context_size: usize,
    ) -> Vec<&'a InstructionTrace> {
        let len = trace.len();
        if len == 0 {
            return vec![];
        }

        let start_idx = if len >= context_size {
            len - context_size
        } else {
            0
        };

        trace[start_idx..].iter().collect()
    }
}
